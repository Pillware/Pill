//! GPU-side profiling: timestamps, occlusion queries and pipeline statistics.
//!
//! # Responsibilities
//!
//! - Own the query sets, the resolve buffers and the readbacks that turn them
//!   into numbers, for a renderer built on wgpu.
//!
//! # Design
//!
//! Each query set is created once with a per-frame cap. A frame records its
//! queries into one slot of an eight-slot ring (`FRAMES_IN_FLIGHT`), and a
//! later frame reads that slot back once the GPU has finished with it, so no
//! frame waits for a readback. A slot that is still waiting makes its frame
//! record no queries, rather than overwrite results nobody has read yet.
//!
//! Every capability is optional. A device without the timestamp or pipeline
//! statistics features gets `None` where a query set would be, and occlusion
//! queries need no feature at all.

// Standard library
use std::cell::Cell;

// External crates
use pill_core::profiling::{GpuApi, GpuStatistics, GpuTimeline, GpuTimelineSpan};
use pill_core::telemetry::log_block;
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use wgpu::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoder, Device, Features,
    PipelineStatisticsTypes, PollType, QuerySet, QuerySetDescriptor, QueryType, Queue,
};

// Current crate
#[cfg(test)]
use crate::readback::MapOutcome;
use crate::readback::{MapState, Readback};

/*
The three query kinds, and what each number answers:

- Timestamps time a section of GPU work - a shadow pass, the main pass, a
  post-processing step. The ticks convert to milliseconds with
  `timestamp_ticks_to_ms`; a section that takes too long is a GPU-side
  bottleneck.
- Occlusion queries count the samples that passed the depth and stencil tests
  while one query is open, so the count over a pass is its visible coverage,
  not its drawn area: fragments that fail the depth test are never counted. A
  pass with a steady 0 is fully occluded by what was drawn before it.
- Pipeline statistics count what the stages did: vertex shader invocations
  (already reduced by the vertex cache), primitives out of the clipper, and
  fragments that ran the fragment shader (per sample with MSAA).
*/

// =============================================================================
// Constants
// =============================================================================

/// The device features GPU profiling needs: timestamps written between
/// passes, and pipeline statistics queried inside a pass.
pub(crate) const GPU_PROFILE_FEATURES: Features = Features::TIMESTAMP_QUERY
    .union(Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
    .union(Features::PIPELINE_STATISTICS_QUERY);

/// Environment variable that turns GPU profiling on (`PILL_GPU_PROFILE=1`).
const GPU_PROFILE_ENVIRONMENT_VARIABLE: &str = "PILL_GPU_PROFILE";

/// How many frames can be waiting for their results at once.
///
/// The ring must be deeper than the GPU can fall behind the CPU. A frame whose
/// slot is still waiting records no queries, so too shallow a ring loses that
/// frame's measurements whenever the GPU is a few frames late. A slot holds
/// only resolved values, so the ring stays small.
const FRAMES_IN_FLIGHT: usize = 8;

// =============================================================================
// Profiler
// =============================================================================

/// GPU profiling through wgpu query sets.
///
/// Holds one query set per category (timestamps, occlusion and pipeline
/// statistics), plus a ring of resolve buffers per category. That lets a
/// readback map one slot while a later frame still records into another.
pub struct Profiler {
    timestamp_query_set: Option<QuerySet>,
    // One name per timestamp recorded this frame, in query index order.
    timestamp_query_names: Vec<String>,
    pipeline_statistics_query_set: Option<QuerySet>,
    pipeline_statistics_types: PipelineStatisticsTypes,
    occlusion_query_set: Option<QuerySet>,

    // Per-frame caps, as passed to `Profiler::new`.
    max_timestamp_queries: u32,
    max_pipeline_statistics_queries: u32,
    max_occlusion_queries: u32,

    // Query counters, reset at the start of each frame.
    current_timestamp_query: Cell<u32>,
    current_pipeline_statistics_query: Cell<u32>,
    current_occlusion_query: Cell<u32>,

    // One resolve slot per in-flight frame, used for readback.
    timestamp_buffers: Vec<Option<ResolveSlot>>,
    pipeline_statistics_buffers: Vec<Option<ResolveSlot>>,
    occlusion_buffers: Vec<Option<ResolveSlot>>,

    // Bytes per query result entry
    timestamp_query_result_bytes: u64,
    pipeline_statistics_query_result_bytes: u64,

    // Frame index for the ring
    frame_index: usize,

    // Conversion period (nanoseconds per timestamp tick)
    timestamp_period_nanoseconds: f32,

    // Tracy's GPU timeline, which exists while a profiler is connected (see
    // `attach_timeline`), and the API label it was created with.
    timeline: Option<GpuTimeline>,
    timeline_api: Option<GpuApi>,
    // Counts the timelines created, one per profiler connection, so a
    // frame's zones are only ever uploaded to the timeline they began on.
    timeline_generation: u64,
    // What each ring slot's frame recorded, until it is read back.
    frames: Vec<FrameRecord>,
    // Whether this frame records no queries, because its ring slot still
    // waits for an earlier frame's results.
    skipping_frame: bool,
    // The most recent results read back, for `log_latest`.
    latest_timings: Vec<(String, f32)>,
    latest_statistics: Vec<GpuStatistics>,
    // One sample count per occlusion query read back, in pass order.
    latest_occlusion: Vec<u64>,
}

impl Profiler {
    /// Creates a profiler for `device` and `queue`.
    ///
    /// Each `max_*` argument is a per-frame cap. Recording past a cap is
    /// refused with a warning in the log instead of overwriting an earlier
    /// query of the same kind.
    ///
    /// A query set for a feature-gated category is created only when the
    /// device was created with that feature. An adapter that offers a feature
    /// is not enough, because the device has to request it (see
    /// [`GPU_PROFILE_FEATURES`]). On a device without the timestamp or
    /// pipeline statistics features, the matching readers always answer
    /// `None`. Occlusion queries need no feature in wgpu, so only the cap
    /// decides whether that set exists.
    pub fn new(
        device: &Device,
        queue: &Queue,
        max_timestamp_queries: u32, // Number of timestamp writes planned to record per frame (start/end of sections)
        max_pipeline_statistics_queries: u32, // Number of pipeline statistics queries per frame
        pipeline_statistics_types: PipelineStatisticsTypes, // Type of pipeline statistics to collect
        max_occlusion_queries: u32, // Number of occlusion queries per frame (one per geometry pass)
    ) -> Self {
        // Step 1: Check which profiling features the device was created with.
        // `write_timestamp` on an encoder is what this profiler calls, so the
        // inside-encoders bit is required, not just the query feature.
        let features = device.features();
        let has_timestamps = features
            .contains(Features::TIMESTAMP_QUERY | Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let has_pipeline_statistics = features.contains(Features::PIPELINE_STATISTICS_QUERY);

        // Step 2: Create one query set per category the device supports.
        let timestamp_query_set = if has_timestamps && max_timestamp_queries > 0 {
            Some(device.create_query_set(&QuerySetDescriptor {
                label: Some("gpu_profiler.timestamp_query_set"),
                ty: QueryType::Timestamp,
                count: max_timestamp_queries,
            }))
        } else {
            None
        };

        let pipeline_statistics_query_set =
            if has_pipeline_statistics && !pipeline_statistics_types.is_empty() {
                Some(device.create_query_set(&QuerySetDescriptor {
                    label: Some("gpu_profiler.pipeline_statistics_query_set"),
                    ty: QueryType::PipelineStatistics(pipeline_statistics_types),
                    count: max_pipeline_statistics_queries,
                }))
            } else {
                None
            };

        // Occlusion queries are core in wgpu, so they need no feature. One
        // query per geometry pass counts the samples that survived its depth
        // and stencil tests, and only the cap can leave the set out.
        let occlusion_query_set = if max_occlusion_queries > 0 {
            Some(device.create_query_set(&QuerySetDescriptor {
                label: Some("gpu_profiler.occlusion_query_set"),
                ty: QueryType::Occlusion,
                count: max_occlusion_queries,
            }))
        } else {
            None
        };

        // Step 3: Size one result entry for each category.
        let timestamp_query_result_bytes = std::mem::size_of::<u64>() as u64;
        let pipeline_statistics_fields = pipeline_statistics_types.bits().count_ones() as u64;
        let pipeline_statistics_query_result_bytes = if pipeline_statistics_fields == 0 {
            0
        } else {
            pipeline_statistics_fields * std::mem::size_of::<u64>() as u64
        };

        // Step 4: Start every ring slot empty and read the timestamp period.
        let timestamp_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();
        let pipeline_statistics_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();
        let occlusion_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();

        let timestamp_period_nanoseconds = if has_timestamps {
            queue.get_timestamp_period()
        } else {
            0.0
        };

        Self {
            timestamp_query_set,
            timestamp_query_names: Vec::new(),
            pipeline_statistics_query_set,
            pipeline_statistics_types,
            occlusion_query_set,

            max_timestamp_queries,
            max_pipeline_statistics_queries: max_pipeline_statistics_queries.max(1),
            max_occlusion_queries: max_occlusion_queries.max(1),

            current_timestamp_query: Cell::new(0),
            current_pipeline_statistics_query: Cell::new(0),
            current_occlusion_query: Cell::new(0),

            timestamp_buffers,
            pipeline_statistics_buffers,
            occlusion_buffers,

            timestamp_query_result_bytes,
            pipeline_statistics_query_result_bytes,

            frame_index: 0,
            timestamp_period_nanoseconds,

            timeline: None,
            timeline_api: None,
            timeline_generation: 0,
            frames: (0..FRAMES_IN_FLIGHT)
                .map(|_| FrameRecord::default())
                .collect(),
            skipping_frame: false,
            latest_timings: Vec::new(),
            latest_statistics: Vec::new(),
            latest_occlusion: Vec::new(),
        }
    }

    /// Sets the API that Tracy labels the GPU timeline with.
    ///
    /// The timeline is created later, when a profiler connects. Tracy drops
    /// every event until one connects, so a timeline created earlier would
    /// never be announced, and Tracy would receive zones for a timeline it
    /// does not know. [`Self::follow_profiler_connection`] creates it.
    ///
    /// Does nothing when the device has no timestamp queries.
    pub fn attach_timeline(&mut self, api: GpuApi) {
        if self.timestamp_query_set.is_some() {
            self.timeline_api = Some(api);
        }
    }

    /// Creates the GPU timeline when a profiler connects, and drops it when
    /// the profiler disconnects, so every connection gets its own timeline.
    ///
    /// Tracy aligns GPU time with CPU time using one timestamp taken when the
    /// timeline is created. Creating it therefore records one timestamp,
    /// submits it and waits for it, which stalls once per connection.
    fn follow_profiler_connection(&mut self, device: &Device, queue: &Queue) {
        // Step 1: Drop the timeline once no profiler is connected any more.
        let connected = pill_core::profiling::profiler_connected();
        if !connected && self.timeline.is_some() {
            self.forget_pending_spans();
            self.timeline = None;
        }
        // Step 2: Create a timeline when a profiler connects and none exists.
        let (true, None, Some(api), Some(query_set)) = (
            connected,
            &self.timeline,
            self.timeline_api,
            &self.timestamp_query_set,
        ) else {
            return;
        };
        let Some(now_ticks) = read_current_timestamp(device, queue, query_set) else {
            warn!(target: telemetry_target::RENDERING, "GPU profiler: could not read a starting timestamp, so Tracy shows no GPU timeline");
            self.timeline_api = None;
            return;
        };
        // Zones still waiting belong to the previous connection.
        self.forget_pending_spans();
        self.timeline = GpuTimeline::new(
            "GPU",
            api,
            now_ticks as i64,
            self.timestamp_period_nanoseconds,
        );
        self.timeline_generation += 1;
    }

    /// Releases every zone that is still waiting for its timestamps, without
    /// telling Tracy. These zones belong to a timeline the profiler no longer
    /// knows, so sending their end there would point at nothing.
    fn forget_pending_spans(&mut self) {
        for record in &mut self.frames {
            for span in record.spans.drain(..).flatten() {
                span.discard();
            }
        }
    }

    /// Starts a frame: collects the results of earlier frames that are ready,
    /// then resets this frame's query counters.
    ///
    /// When this frame's ring slot is still waiting for its readback, the GPU
    /// is further behind than the ring is deep. The frame then records no
    /// queries, rather than overwrite results nobody has read yet.
    pub fn begin_frame(&mut self, device: &Device, queue: &Queue) {
        // Step 1: Run the map callbacks for whatever the GPU finished, without
        // waiting, and collect the results that are ready.
        let _ = device.poll(PollType::Poll);
        self.collect_finished_frames();
        // Step 2: Create or drop the Tracy timeline to match the connection.
        self.follow_profiler_connection(device, queue);
        // Step 3: Reset this frame's counters, and skip the frame when its ring
        // slot is still waiting.
        self.skipping_frame = self.frames[self.frame_index].is_waiting();
        self.current_timestamp_query.set(0);
        self.current_pipeline_statistics_query.set(0);
        self.current_occlusion_query.set(0);
        self.timestamp_query_names.clear();
    }

    /// Ends the frame once it was submitted, and moves the ring on.
    ///
    /// When the frame recorded queries, this starts their readbacks without
    /// waiting for them. A later [`Self::begin_frame`] collects the results
    /// once the GPU is done. The ring then advances, so the next frame
    /// resolves into another slot.
    pub fn end_frame(&mut self) {
        // Step 1: Start a readback for each category the frame recorded.
        if !self.skipping_frame {
            let slot_index = self.frame_index;
            let timestamp_count = self.current_timestamp_query.get();
            let statistics_count = self.current_pipeline_statistics_query.get();
            let occlusion_count = self.current_occlusion_query.get();
            let timestamp_buffer = self.timestamp_buffers[slot_index]
                .as_ref()
                .map(|resolve_slot| &resolve_slot.readback);
            let statistics_buffer = self.pipeline_statistics_buffers[slot_index]
                .as_ref()
                .map(|resolve_slot| &resolve_slot.readback);
            let occlusion_buffer = self.occlusion_buffers[slot_index]
                .as_ref()
                .map(|resolve_slot| &resolve_slot.readback);
            let record = &mut self.frames[slot_index];
            record.timestamp_names = self.timestamp_query_names.clone();
            record.timestamp_count = timestamp_count;
            record.statistics_count = statistics_count;
            record.occlusion_count = occlusion_count;
            record.timestamp_map = timestamp_buffer
                .filter(|_| timestamp_count > 0)
                .map(MapState::start);
            record.statistics_map = statistics_buffer
                .filter(|_| statistics_count > 0)
                .map(MapState::start);
            record.occlusion_map = occlusion_buffer
                .filter(|_| occlusion_count > 0)
                .map(MapState::start);
        }
        // Step 2: Move the ring on, so the next frame uses another slot.
        self.frame_index = (self.frame_index + 1) % FRAMES_IN_FLIGHT;
    }

    /// Collects every ring slot whose readbacks are done. It uploads the
    /// slot's timestamps to their Tracy zones, plots its statistics, and keeps
    /// the timings, statistics and occlusion counts for [`Self::log_latest`].
    fn collect_finished_frames(&mut self) {
        for slot_index in 0..FRAMES_IN_FLIGHT {
            if !self.frames[slot_index].is_ready() {
                continue;
            }
            let mut record = std::mem::take(&mut self.frames[slot_index]);
            if record.timeline_generation != self.timeline_generation || self.timeline.is_none() {
                for span in record.spans.drain(..).flatten() {
                    span.discard();
                }
            }

            if let (Some(map_state), Some(resolve_slot)) =
                (&record.timestamp_map, &self.timestamp_buffers[slot_index])
            {
                let ticks = match map_state.poll(&resolve_slot.readback, |bytes| {
                    first_u64_values(bytes, record.timestamp_count as usize)
                }) {
                    Readback::Done(ticks) => ticks,
                    Readback::Pending | Readback::Failed => Vec::new(),
                };
                if ticks.is_empty() && record.timestamp_count > 0 {
                    warn_readback("timestamps");
                }
                let mut timings = Vec::new();
                for (pair_index, pair) in ticks.chunks_exact(2).enumerate() {
                    if let Some(Some(span)) = record.spans.get(pair_index) {
                        span.upload(pair[0] as i64, pair[1] as i64);
                    }
                    let name = record
                        .timestamp_names
                        .get(pair_index * 2)
                        .cloned()
                        .unwrap_or_else(|| format!("Section {pair_index}"));
                    timings.push((
                        name,
                        self.timestamp_ticks_to_ms(pair[1].saturating_sub(pair[0])),
                    ));
                }
                if !timings.is_empty() {
                    self.latest_timings = timings;
                }
            }

            if let (Some(map_state), Some(resolve_slot)) = (
                &record.statistics_map,
                &self.pipeline_statistics_buffers[slot_index],
            ) {
                let stride = self.pipeline_statistics_types.bits().count_ones() as usize;
                let statistic_values = match map_state.poll(&resolve_slot.readback, |bytes| {
                    first_u64_values(bytes, record.statistics_count as usize * stride)
                }) {
                    Readback::Done(statistic_values) => statistic_values,
                    Readback::Pending | Readback::Failed => Vec::new(),
                };
                if statistic_values.is_empty() && record.statistics_count > 0 {
                    warn_readback("pipeline statistics");
                }
                let statistics: Vec<GpuStatistics> = statistic_values
                    .chunks_exact(stride.max(1))
                    .map(|query_values| statistics_of(self.pipeline_statistics_types, query_values))
                    .collect();
                if !statistics.is_empty() {
                    // Plot the whole frame as one sample: every geometry pass
                    // added up.
                    let mut frame_total = GpuStatistics::default();
                    for query_statistics in &statistics {
                        frame_total.vertex_invocations += query_statistics.vertex_invocations;
                        frame_total.clipper_invocations += query_statistics.clipper_invocations;
                        frame_total.clipper_primitives_out +=
                            query_statistics.clipper_primitives_out;
                        frame_total.fragment_invocations += query_statistics.fragment_invocations;
                    }
                    pill_core::profiling::plot_gpu_statistics(frame_total);
                    self.latest_statistics = statistics;
                }
            }

            if let (Some(map_state), Some(resolve_slot)) =
                (&record.occlusion_map, &self.occlusion_buffers[slot_index])
            {
                let samples = match map_state.poll(&resolve_slot.readback, |bytes| {
                    first_u64_values(bytes, record.occlusion_count as usize)
                }) {
                    Readback::Done(samples) => samples,
                    Readback::Pending | Readback::Failed => Vec::new(),
                };
                if samples.is_empty() && record.occlusion_count > 0 {
                    warn_readback("occlusion samples");
                }
                if !samples.is_empty() {
                    self.latest_occlusion = samples;
                }
            }
            // The record is dropped at the end of this step. Dropping a span
            // hands its zone to Tracy.
        }
    }

    /// Logs the latest timings, occlusion samples and pipeline statistics that
    /// were read back, without waiting for the GPU.
    ///
    /// `viewport_pixels` is the drawable area of the frame in pixels. The
    /// occlusion block reports its total visible samples as a share of it.
    /// Pass `0` to report raw counts without a percentage.
    pub fn log_latest(&self, viewport_pixels: u64) {
        if !self.latest_timings.is_empty() {
            let lines = self
                .latest_timings
                .iter()
                .map(|(label, milliseconds)| format!("{label:<24}: {milliseconds:6.3} ms"));
            info!(target: telemetry_target::RENDERING, "{}", log_block("GPU timestamps", lines));
        }
        if !self.latest_occlusion.is_empty() {
            let total: u64 = self.latest_occlusion.iter().sum();
            let coverage = if viewport_pixels > 0 {
                format!(
                    "{:.1}% of {viewport_pixels} px",
                    total as f64 * 100.0 / viewport_pixels as f64
                )
            } else {
                "viewport unknown".to_string()
            };
            let mut lines: Vec<String> = self
                .latest_occlusion
                .iter()
                .enumerate()
                .map(|(pass_index, samples)| {
                    format!("{:<24}: {samples} samples", format!("pass {pass_index}"))
                })
                .collect();
            lines.push(format!("{:<24}: {total} samples ({coverage})", "visible"));
            info!(target: telemetry_target::RENDERING, "{}", log_block("GPU occlusion", lines));
        }
        if !self.latest_statistics.is_empty() {
            let lines = self.latest_statistics.iter().enumerate().flat_map(
                |(query_index, query_statistics)| {
                    [
                        format!("query {query_index}:"),
                        format!(
                            "  {:>24}: {}",
                            "VS invocations", query_statistics.vertex_invocations
                        ),
                        format!(
                            "  {:>24}: {}",
                            "Clipper invocations", query_statistics.clipper_invocations
                        ),
                        format!(
                            "  {:>24}: {}",
                            "Clipper primitives out", query_statistics.clipper_primitives_out
                        ),
                        format!(
                            "  {:>24}: {}",
                            "FS invocations", query_statistics.fragment_invocations
                        ),
                    ]
                },
            );
            info!(target: telemetry_target::RENDERING, "{}", log_block("GPU pipeline statistics", lines));
        }
    }

    // --- Timestamps ---

    /// Returns the timestamp query set, or `None` when the device was created
    /// without the timestamp features or the cap was zero.
    ///
    /// For a caller that writes timestamps itself. Prefer
    /// [`Self::write_timestamp`], which also keeps the query indices and names
    /// that the summaries use.
    pub fn get_timestamp_query_set(&self) -> Option<&QuerySet> {
        self.timestamp_query_set.as_ref()
    }

    /// Writes a timestamp for the region called `name` and returns its query
    /// index.
    ///
    /// Call it twice around the region to time. The pair's ticks are the
    /// region's GPU time, and the pair is reported under the name given to the
    /// first call.
    pub fn write_timestamp(&mut self, encoder: &mut CommandEncoder, name: &str) -> Option<u32> {
        if self.skipping_frame {
            return None;
        }
        if let Some(query_set) = &self.timestamp_query_set {
            // Refuse the timestamp once this frame's cap is reached.
            if self.current_timestamp_query.get() >= self.max_timestamp_queries {
                warn!(target: telemetry_target::RENDERING, "GPU profiler: timestamp queries are full for this frame");
                return None;
            }

            let index = self.current_timestamp_query.get();
            self.current_timestamp_query.set(index + 1);
            encoder.write_timestamp(query_set, index);

            // Keep the name at the same index as the timestamp.
            self.timestamp_query_names.push(name.to_string());

            // Timestamps come in pairs around a region. The first opens the
            // pair's Tracy zone now, on this thread; the second closes it.
            let record = &mut self.frames[self.frame_index];
            record.timeline_generation = self.timeline_generation;
            if index.is_multiple_of(2) {
                let span = self
                    .timeline
                    .as_ref()
                    .and_then(|timeline| timeline.begin_span(name, file!(), line!()));
                record.spans.push(span);
            } else if let Some(Some(span)) = record.spans.last_mut() {
                span.end();
            }

            Some(index)
        } else {
            None
        }
    }

    /// Resolves the timestamps recorded this frame into the current ring slot.
    pub fn resolve_timestamp_queries(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        if let Some(query_set) = &self.timestamp_query_set {
            let count = self.current_timestamp_query.get();
            if count == 0 {
                return;
            }
            let byte_len = self.timestamp_query_result_bytes * count as u64;

            let slot_index = self.frame_index;
            let slot = &mut self.timestamp_buffers[slot_index];
            let buffers = ensure_buffer_slot(
                device,
                slot,
                byte_len,
                "gpu_profiler.timestamp_queries.resolve",
            );

            encoder.resolve_query_set(query_set, 0..count, &buffers.resolve, 0);

            // Resolved results cannot be mapped, so they are copied into the
            // readback buffer.

            encoder.copy_buffer_to_buffer(&buffers.resolve, 0, &buffers.readback, 0, byte_len);
        }
    }

    /// Converts a delta of timestamp ticks to milliseconds.
    pub fn timestamp_ticks_to_ms(&self, delta_ticks: u64) -> f32 {
        (delta_ticks as f32 * self.timestamp_period_nanoseconds) / 1_000_000.0
    }

    // --- Pipeline statistics ---

    /// Returns the pipeline statistics query set together with the statistics
    /// types it was created for, for a caller that begins and ends the queries
    /// itself.
    pub fn pipeline_statistics_query_set(&self) -> Option<(&QuerySet, PipelineStatisticsTypes)> {
        self.pipeline_statistics_query_set
            .as_ref()
            .map(|query_set| (query_set, self.pipeline_statistics_types))
    }

    /// Begins a pipeline statistics query in `render_pass` and returns its
    /// index.
    pub fn begin_pipeline_statistics_query(
        &self,
        render_pass: &mut wgpu::RenderPass<'_>,
    ) -> Option<u32> {
        if self.skipping_frame {
            return None;
        }
        if let Some(query_set) = &self.pipeline_statistics_query_set {
            // Refuse the query once this frame's cap is reached.
            if self.current_pipeline_statistics_query.get() >= self.max_pipeline_statistics_queries
            {
                warn!(target: telemetry_target::RENDERING, "GPU profiler: pipeline statistics queries are full for this frame");
                return None;
            }

            let index = self.current_pipeline_statistics_query.get();
            self.current_pipeline_statistics_query.set(index + 1);
            render_pass.begin_pipeline_statistics_query(query_set, index);
            Some(index)
        } else {
            None
        }
    }

    /// Ends the pipeline statistics query most recently begun in
    /// `render_pass`.
    ///
    /// Pairs with [`Self::begin_pipeline_statistics_query`]. The counters can
    /// be read once the frame's queries are resolved.
    pub fn end_pipeline_statistics_query(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.end_pipeline_statistics_query();
    }

    /// Resolves this frame's pipeline statistics queries into the current ring
    /// slot.
    ///
    /// Does nothing when the feature is unsupported or no query was begun this
    /// frame. In that case the slot keeps whatever the previous resolve put
    /// there.
    pub fn resolve_pipeline_statistics_queries(
        &mut self,
        device: &Device,
        encoder: &mut CommandEncoder,
    ) {
        if let Some(query_set) = &self.pipeline_statistics_query_set {
            let count = self.current_pipeline_statistics_query.get();
            if count == 0 {
                return;
            }
            let byte_len = self.pipeline_statistics_query_result_bytes * count as u64;

            let slot_index = self.frame_index;
            let slot = &mut self.pipeline_statistics_buffers[slot_index];
            let buffers = ensure_buffer_slot(
                device,
                slot,
                byte_len,
                "gpu_profiler.pipeline_statistics_queries.resolve",
            );

            encoder.resolve_query_set(query_set, 0..count, &buffers.resolve, 0);

            // Resolved results cannot be mapped, so they are copied into the
            // readback buffer.

            encoder.copy_buffer_to_buffer(&buffers.resolve, 0, &buffers.readback, 0, byte_len);
        }
    }

    // --- Occlusion ---

    /// Exposes the occlusion query set for a geometry pass's
    /// `RenderPassDescriptor.occlusion_query_set`.
    ///
    /// `None` when the cap was zero. A pass begins its queries through
    /// [`Self::begin_occlusion_query`], which also refuses during a skipped
    /// frame.
    pub fn get_occlusion_query_set(&self) -> Option<&QuerySet> {
        self.occlusion_query_set.as_ref()
    }

    /// Begins the next occlusion query in `render_pass` and returns its index.
    ///
    /// One query per geometry pass counts the samples that passed the depth
    /// and stencil tests while it was open, so the pass's visible coverage is
    /// read back as one number.
    pub fn begin_occlusion_query(&self, render_pass: &mut wgpu::RenderPass<'_>) -> Option<u32> {
        if self.skipping_frame || self.occlusion_query_set.is_none() {
            return None;
        }
        if self.current_occlusion_query.get() >= self.max_occlusion_queries {
            warn!(target: telemetry_target::RENDERING, "GPU profiler: occlusion queries are full for this frame");
            return None;
        }
        let index = self.current_occlusion_query.get();
        self.current_occlusion_query.set(index + 1);
        render_pass.begin_occlusion_query(index);
        Some(index)
    }

    /// Ends the occlusion query most recently begun in `render_pass`.
    ///
    /// Pairs with [`Self::begin_occlusion_query`]. The sample counts can be
    /// read once the frame's queries are resolved.
    pub fn end_occlusion_query(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.end_occlusion_query();
    }

    /// Resolves this frame's occlusion queries into the current ring slot.
    ///
    /// Does nothing when no query set exists or no query was begun this frame,
    /// like [`Self::resolve_pipeline_statistics_queries`].
    pub fn resolve_occlusion_queries(&mut self, device: &Device, encoder: &mut CommandEncoder) {
        if let Some(query_set) = &self.occlusion_query_set {
            let count = self.current_occlusion_query.get();
            if count == 0 {
                return;
            }
            // One u64 per occlusion query.
            let byte_len = std::mem::size_of::<u64>() as u64 * count as u64;

            let slot_index = self.frame_index;
            let slot = &mut self.occlusion_buffers[slot_index];
            let buffers = ensure_buffer_slot(
                device,
                slot,
                byte_len,
                "gpu_profiler.occlusion_queries.resolve",
            );

            encoder.resolve_query_set(query_set, 0..count, &buffers.resolve, 0);

            // Resolved results cannot be mapped, so they are copied into the
            // readback buffer.

            encoder.copy_buffer_to_buffer(&buffers.resolve, 0, &buffers.readback, 0, byte_len);
        }
    }
}

// =============================================================================
// Frame Record
// =============================================================================

/// What one ring slot's frame recorded. It is kept until its results are
/// collected without blocking the frame.
#[derive(Default)]
struct FrameRecord {
    /// The Tracy zone for each timestamp pair, or `None` when no timeline was
    /// attached when the pair began.
    spans: Vec<Option<GpuTimelineSpan>>,
    /// The timeline generation the zones began on.
    timeline_generation: u64,
    timestamp_names: Vec<String>,
    timestamp_count: u32,
    statistics_count: u32,
    occlusion_count: u32,
    /// The timestamp, statistics and occlusion readbacks in flight.
    timestamp_map: Option<MapState>,
    statistics_map: Option<MapState>,
    occlusion_map: Option<MapState>,
}

impl FrameRecord {
    /// Whether at least one readback was asked for, and all of them finished.
    fn is_ready(&self) -> bool {
        let is_finished =
            |map_state: &Option<MapState>| map_state.as_ref().is_none_or(MapState::finished);
        self.is_waiting()
            && is_finished(&self.timestamp_map)
            && is_finished(&self.statistics_map)
            && is_finished(&self.occlusion_map)
    }

    /// Whether this slot still waits for a readback.
    fn is_waiting(&self) -> bool {
        self.timestamp_map.is_some()
            || self.statistics_map.is_some()
            || self.occlusion_map.is_some()
    }
}

// =============================================================================
// Resolve Slot
// =============================================================================

/// One ring slot's buffers. Queries resolve into `resolve`, which is then
/// copied into `readback` for the CPU to map. wgpu only allows a mappable
/// buffer as a copy destination, so a query cannot resolve into one directly.
struct ResolveSlot {
    resolve: Buffer,
    readback: Buffer,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Whether `PILL_GPU_PROFILE` asks for GPU profiling. Off by default: the
/// queries cost GPU time on every frame that records them.
pub(crate) fn gpu_profiling_requested() -> bool {
    std::env::var(GPU_PROFILE_ENVIRONMENT_VARIABLE)
        .is_ok_and(|value| value != "0" && !value.is_empty())
}

/// The first `count` 64-bit values of a mapped readback's bytes.
fn first_u64_values(bytes: &[u8], count: usize) -> Vec<u64> {
    let values: &[u64] = bytemuck::cast_slice(bytes);
    values[..count.min(values.len())].to_vec()
}

/// Reports that one kind of measurement never arrived, so a profiling run
/// explains why it has no numbers instead of staying silent.
fn warn_readback(measurement: &str) {
    warn!(
        target: telemetry_target::RENDERING,
        "PILL_GPU_PROFILE: the {measurement} readback could not be read, so this frame reports none"
    );
}

/// Returns the slot's buffers, creating them when they cannot hold `size`
/// bytes.
///
/// A slot only grows. Once a frame with many queries has asked for a large
/// buffer, later smaller frames reuse that allocation instead of shrinking it.
#[inline]
fn ensure_buffer_slot<'a>(
    device: &wgpu::Device,
    slot: &'a mut Option<ResolveSlot>,
    size: u64,
    label: &str,
) -> &'a ResolveSlot {
    let needs_new_buffer = slot
        .as_ref()
        .map(|resolve_slot| resolve_slot.resolve.size() < size)
        .unwrap_or(true);
    if needs_new_buffer {
        let create_buffer = |usage: BufferUsages| {
            device.create_buffer(&BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        *slot = Some(ResolveSlot {
            resolve: create_buffer(BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC),
            readback: create_buffer(BufferUsages::MAP_READ | BufferUsages::COPY_DST),
        });
    }
    slot.as_ref().unwrap()
}

/// Reads one query's pipeline statistics in the order wgpu writes them: one
/// value per statistic in `types`, lowest flag first.
fn statistics_of(types: PipelineStatisticsTypes, query_values: &[u64]) -> GpuStatistics {
    let mut statistics = GpuStatistics::default();
    let mut remaining_values = query_values.iter().copied();
    for flag in types.iter() {
        let value = remaining_values.next().unwrap_or(0);
        if flag == PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS {
            statistics.vertex_invocations = value;
        } else if flag == PipelineStatisticsTypes::CLIPPER_INVOCATIONS {
            statistics.clipper_invocations = value;
        } else if flag == PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT {
            statistics.clipper_primitives_out = value;
        } else if flag == PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS {
            statistics.fragment_invocations = value;
        }
    }
    statistics
}

/// Reads the GPU's clock now, in timestamp ticks.
///
/// This writes one timestamp, submits it and waits for its readback, so the
/// caller stalls. Returns `None` when the device poll or the readback fails.
fn read_current_timestamp(device: &Device, queue: &Queue, query_set: &QuerySet) -> Option<u64> {
    let size = std::mem::size_of::<u64>() as u64;
    let create_buffer = |usage: BufferUsages| {
        device.create_buffer(&BufferDescriptor {
            label: Some("gpu_profiler.calibration"),
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let resolve = create_buffer(BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC);
    let readback = create_buffer(BufferUsages::MAP_READ | BufferUsages::COPY_DST);

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gpu_profiler.calibration"),
    });
    encoder.write_timestamp(query_set, 0);
    encoder.resolve_query_set(query_set, 0..1, &resolve, 0);
    encoder.copy_buffer_to_buffer(&resolve, 0, &readback, 0, size);
    queue.submit(std::iter::once(encoder.finish()));

    let map_state = MapState::start(&readback);
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .ok()?;
    match map_state.poll(&readback, |bytes| first_u64_values(bytes, 1)) {
        Readback::Done(values) => values.first().copied(),
        Readback::Pending | Readback::Failed => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statistics_are_read_in_flag_order() {
        let types = PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
            | PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT
            | PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS;

        let statistics = statistics_of(types, &[10, 20, 30]);

        assert_eq!(statistics.vertex_invocations, 10);
        assert_eq!(statistics.clipper_invocations, 0);
        assert_eq!(statistics.clipper_primitives_out, 20);
        assert_eq!(statistics.fragment_invocations, 30);
    }

    #[test]
    fn a_slot_is_ready_only_once_every_readback_it_asked_for_finished() {
        let mut record = FrameRecord {
            timestamp_map: Some(MapState::at_outcome(MapOutcome::Pending)),
            statistics_map: Some(MapState::at_outcome(MapOutcome::Mapped)),
            ..FrameRecord::default()
        };
        assert!(record.is_waiting() && !record.is_ready());

        record.timestamp_map = Some(MapState::at_outcome(MapOutcome::Failed));
        assert!(record.is_ready(), "a failed map still finishes the slot");

        record.timestamp_map = None;
        record.statistics_map = None;
        assert!(!record.is_waiting() && !record.is_ready());
    }
}
