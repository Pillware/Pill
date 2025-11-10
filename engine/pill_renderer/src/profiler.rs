use std::{cell::Cell, num::NonZeroU32};
use wgpu::{ 
    core::device::queue, 
    Adapter, 
    Buffer, 
    BufferDescriptor, 
    BufferUsages, 
    CommandEncoder, 
    Device, 
    Features, 
    Maintain, 
    PipelineStatisticsTypes, 
    QuerySet, 
    QuerySetDescriptor, 
    QueryType, 
    Queue
};

/* 
This module provides GPU-side profiling capabilities using wgpu.

- Timestamp queries:
  Measures: Time taken by sections of code (eg. shadow pass, main pass, post-processing, etc).
  This is true GPU time, showing stalls and bottlenecks on GPU side
  Result: Vector of u64 ticks, which can be converted to milliseconds using the provided conversion function.
  Not available on the web (needs TIMESTAMP_QUERY feature)
  Learnings:
    - GPU bottlenecks: If a section takes too long, it indicates a GPU-side bottleneck.
    - Frame time analysis: Helps understand how much time each rendering stage takes.

- Occlusion queries:
  Measures: How many fragments(pixels) passed depth/stencil during the draw region maked with begin_occlusion_tracking/end_occlusion_tracking.
  Result: Vector of u64 counts, one per query. Will be 0 if fully occluded.
  Learnings:
    - Overdraw insights: If you see high occlusion counts, it means many fragments were discarded by depth test.
    - Visibility culling: Can be used to skip rendering objects that are fully occluded.
    - Bound analysis: Can help understand how many pixels are actually visible in the scene.

- Pipeline statistics:
  Collects various pipeline statistics like:
  - VERTEX_SHADER_INVOCATIONS — how many times the vertex shader ran (accounts for the vertex cache with indexed draws). 
  - CLIPPER_INVOCATIONS — number of times the clipper stage was invoked (equals triangles output by the vertex stage). 
  - CLIPPER_PRIMITIVES_OUT — primitives that survived clipping (triangles that actually proceed to rasterization). 
  - FRAGMENT_SHADER_INVOCATIONS — how many fragments executed the fragment shader (per-sample with MSAA; GPUs often execute in 2×2 quads for derivatives). 
  - COMPUTE_SHADER_INVOCATIONS — total compute shader invocations (dispatch count × workgroup size). 
  Result: Vector of u64 values, one per statistic type requested.
  Not available on the web (needs PIPELINE_STATISTICS_QUERY feature)
*/

/// How many frames we pipeline readbacks to avoid stalls.
/// You can set this to 2–4 depending on your swapchain latency.
const FRAMES_IN_FLIGHT: usize = 3;

/// GPU-side profiling using wgpu
pub struct Profiler {
    // Capabilities
    has_timestamps: bool,
    has_pipeline_stats: bool,

    // Query sets (optional if feature unsupported)
    timestamp_query_set: Option<QuerySet>,
    timestamp_query_names: Vec<String>, // To store names associated with each timestamp
    occlusion_query_set: Option<QuerySet>,
    pipeline_statistics_query_set: Option<QuerySet>,
    pipeline_statistics_types: PipelineStatisticsTypes,

    // Maximum queries we allow per frame for each kind
    max_timestamp_queries: u32,
    max_occlusion_queries: u32,
    max_pipeline_statistics_queries: u32,

    // Rolling per-frame indices
    current_timestamp_query: Cell<u32>,
    current_occlusion_query: Cell<u32>,
    current_pipeline_statistics_query: Cell<u32>,

    // Resolve buffers (ring) for readback, one per in-flight frame
    timestamp_buffers: Vec<Option<Buffer>>,
    occlusion_buffers: Vec<Option<Buffer>>,
    pipeline_buffers: Vec<Option<Buffer>>,

    // Bytes per query result set
    timestamp_queries_result_bytes: u64,
    occlusion_queries_result_bytes: u64,
    pipeline_statistics_queries_result_bytes: u64,

    // Frame index for the ring
    frame_index: usize,

    // Conversion period (nanoseconds per timestamp tick)
    timestamp_period_ns: f32,
}

impl Profiler {
    pub fn new(
        device: &Device, 
        queue: &Queue,
        adapter: &Adapter, 
        max_timestamp_queries: u32, // Number of timestamp writes planned to record per frame (start/end of sections)
        max_occlusion_queries: u32, // Number of occlusion queries per frame
        max_pipeline_statistics_queries: u32, // Number of pipeline statistics queries per frame
        pipeline_statistics_types: PipelineStatisticsTypes, // Type of pipeline statistics to collect
    ) -> Self {
        let features = adapter.features();
        let has_timestamps = features.contains(Features::TIMESTAMP_QUERY);
        let has_pipeline_statistics = features.contains(Features::PIPELINE_STATISTICS_QUERY);

        // Timestamp query set
        let timestamp_query_set = if has_timestamps && max_timestamp_queries > 0 {
            Some(device.create_query_set(&QuerySetDescriptor {
                label: Some("gpu_profiler.timestamp_query_set"),
                ty: QueryType::Timestamp,
                count: max_timestamp_queries,
            }))
        } else {
            None
        };

        // Occlusion query set
        let occlusion_query_set = if max_occlusion_queries > 0 {
            Some(device.create_query_set(&QuerySetDescriptor {
                label: Some("gpu_profiler.occlusion_query_set"),
                ty: QueryType::Occlusion,
                count: max_occlusion_queries,
            }))
        } else {
            None
        };

        // Pipeline statistics query set
        let pipeline_statistics_query_set = if has_pipeline_statistics && !pipeline_statistics_types.is_empty() {
            Some(device.create_query_set(&QuerySetDescriptor {
                label: Some("gpu_profiler.pipeline_statistics_query_set"),
                ty: QueryType::PipelineStatistics(pipeline_statistics_types),
                count: max_pipeline_statistics_queries,
            }))
        } else {
            None
        };

        // Bytes per query result entry
        let timestamp_queries_result_bytes = std::mem::size_of::<u64>() as u64;
        let occlusion_queries_result_bytes = std::mem::size_of::<u64>() as u64;
        let pipeline_statistics_fields = pipeline_statistics_types.bits().count_ones() as u64;
        let pipeline_statistics_queries_result_bytes = if pipeline_statistics_fields == 0 { 0 } else { pipeline_statistics_fields * std::mem::size_of::<u64>() as u64 };

        // Resolve buffers ring (created lazily on first use)
        let timestamp_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();
        let occlusion_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();
        let pipeline_buffers = (0..FRAMES_IN_FLIGHT).map(|_| None).collect();

        // Timestamp period
        let timestamp_period_ns = if has_timestamps { queue.get_timestamp_period() } else { 0.0 };

        Self {
            has_timestamps,
            has_pipeline_stats: has_pipeline_statistics,

            timestamp_query_set,
            timestamp_query_names: Vec::new(),
            occlusion_query_set,
            pipeline_statistics_query_set,
            pipeline_statistics_types,

            max_timestamp_queries,
            max_occlusion_queries,
            max_pipeline_statistics_queries: max_occlusion_queries.max(1), // we bound ps queries with the same cap for simplicity

            current_timestamp_query: Cell::new(0),
            current_occlusion_query: Cell::new(0),
            current_pipeline_statistics_query: Cell::new(0),

            timestamp_buffers,
            occlusion_buffers,
            pipeline_buffers,

            timestamp_queries_result_bytes,
            occlusion_queries_result_bytes,
            pipeline_statistics_queries_result_bytes,

            frame_index: 0,
            timestamp_period_ns,
        }
    }

    /// Call once at the start of frame.
    pub fn begin_frame(&mut self) {
        self.current_timestamp_query.set(0);
        self.current_occlusion_query.set(0);
        self.current_pipeline_statistics_query.set(0);
        self.timestamp_query_names.clear();
    }

    /// Call once at the end of frame, after all resolves were scheduled into encoder
    /// This advances the ring index so next frame resolves into a different buffer
    pub fn end_frame(&mut self) {
        self.frame_index = (self.frame_index + 1) % FRAMES_IN_FLIGHT;
    }

    // --- Timestamps ---

    pub fn get_timestamp_query_set(&self) -> Option<&QuerySet> {
        self.timestamp_query_set.as_ref()
    }

    /// Write a timestamp (returns its query index)
    /// Called before and after a region to time
    pub fn write_timestamp(&mut self, encoder: &mut CommandEncoder, name: &str) -> Option<u32> {
        if let Some(query_set) = &self.timestamp_query_set {
            // Check if there is space for another timestamp
            if self.current_timestamp_query.get() >= self.max_timestamp_queries {
                println!("Profiler: Max timestamps reached for this frame");
                return None;
            }

            let index = self.current_timestamp_query.get();
            self.current_timestamp_query.set(index + 1);
            encoder.write_timestamp(query_set, index);

            // Store the name in parallel with the timestamp index
            self.timestamp_query_names.push(name.to_string());

            Some(index)
        } else {
            None
        }
    }

    /// Resolve all timestamps recorded so far this frame into the ring buffer
    pub fn resolve_timestamp_queries(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        if let Some(query_set) = &self.timestamp_query_set {
            let count = self.current_timestamp_query.get();
            if count == 0 { return; }
            let byte_len = self.timestamp_queries_result_bytes * count as u64;

            let index = self.frame_index;
            let slot = &mut self.timestamp_buffers[index]; 
            let buffer = ensure_buffer_slot(device, slot, byte_len, "gpu_profiler.timestamp_queries.resolve");

            encoder.resolve_query_set(query_set, 0..count, buffer, 0);
        }
    }

    /// Blocking readback of all timestamps for the frame that was resolved into the previous ring slot
    /// Returns the raw u64 ticks and also a helper to convert ticks to ms
    pub fn read_timestamp_queries_blocking(&self, device: &Device) -> Option<Vec<u64>> {
        let index = (self.frame_index + FRAMES_IN_FLIGHT - 1) % FRAMES_IN_FLIGHT;
        let buffer = self.timestamp_buffers[index].as_ref()?;
        let slice = buffer.slice(..);

        // Map and wait
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(Maintain::Wait);
        let data = slice.get_mapped_range();
        let values: Vec<u64> = bytemuck::cast_slice(&data).to_vec();

        drop(data);
        buffer.unmap();
        Some(values)
    }

    /// Convert a delta of timestamp ticks to milliseconds.
    pub fn timestamp_ticks_to_ms(&self, delta_ticks: u64) -> f32 {
        (delta_ticks as f32 * self.timestamp_period_ns) / 1_000_000.0
    }

    pub fn summarize_timestamp_queries(&self, ticks: &[u64]) {
        if ticks.len() < 2 {
            println!("[GPU] no timestamp sections recorded");
            return;
        }
        let want = ticks.len() - 1;
        let use_default = self.timestamp_query_names.len() != want;

        for (i, window) in ticks.windows(2).enumerate() {
            let delta_ticks = window[1] - window[0];
            let ms = self.timestamp_ticks_to_ms(delta_ticks);
            let label = if use_default {
                format!("Section {}", i)
            } else {
                self.timestamp_query_names[i].to_string()
            };
            println!("[GPU] {:<24}: {:6.3} ms", label, ms);
        }
    }

    // --- Occlusion ---

    /// Expose the occlusion query set for putting into `RenderPassDescriptor.occlusion_query_set`.
    pub fn get_occlusion_query_set(&self) -> Option<&QuerySet> {
        self.occlusion_query_set.as_ref()
    }

    /// Begin an occlusion query within a render pass. Returns query index.
    pub fn begin_occlusion_query(&self, render_pass: &mut wgpu::RenderPass<'_>) -> Option<u32> {
        if let Some(_query_set) = &self.occlusion_query_set {
            // Check if there is space for another occlusion query
            if self.current_occlusion_query.get() >= self.max_occlusion_queries {
                println!("Profiler: Max occlusion queries reached for this frame");
                return None;
            }

            let index = self.current_occlusion_query.get();
            self.current_occlusion_query.set(index + 1);
            render_pass.begin_occlusion_query(index);
            Some(index)
        } else {
            None
        }
    }

    pub fn end_occlusion_query(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.end_occlusion_query();
    }

    /// Resolve occlusion queries recorded this frame.
    pub fn resolve_occlusion_queries(&mut self, device: &Device, encoder: &mut CommandEncoder) {
        if let Some(query_set) = &self.occlusion_query_set {
            let count = self.current_occlusion_query.get();
            if count == 0 { return; }
            let byte_len = self.occlusion_queries_result_bytes * count as u64;

            let index = self.frame_index;
            let slot = &mut self.occlusion_buffers[index];
            let buffer = ensure_buffer_slot(device, slot, byte_len, "gpu_profiler.occlusion_queries.resolve");

            encoder.resolve_query_set(query_set, 0..count, buffer, 0);
        }
    }

    pub fn read_occlusion_queries_blocking(&self, device: &Device) -> Option<Vec<u64>> {
        let index = (self.frame_index + FRAMES_IN_FLIGHT - 1) % FRAMES_IN_FLIGHT;
        let buffer = self.occlusion_buffers[index].as_ref()?;
        let slice = buffer.slice(..);

        // Map and wait
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(Maintain::Wait);
        let data = slice.get_mapped_range();
        let values: Vec<u64> = bytemuck::cast_slice(&data).to_vec();

        drop(data);
        buffer.unmap();
        Some(values)
    }

    pub fn summarize_occlusion_queries(&self, samples: &[u64]) {
        if samples.is_empty() {
            println!("[GPU] no occlusion queries recorded");
            return;
        }
        let mut visible = 0usize;
        for (i, &sample) in samples.iter().enumerate() {
            let is_visible = sample > 0;
            if is_visible { visible += 1; }
            println!("[GPU] occlusion[{:02}] = {:>12}  {}", i, sample, if is_visible { "(visible)" } else { "(occluded)" });
        }
        println!("[GPU] occlusion visible: {}/{}", visible, samples.len());
    }

    // --- Pipeline statistics ---

    /// Expose the pipeline stats query set (and mask) so you can begin/end in passes
    pub fn pipeline_statistics_query_set(&self) -> Option<(&QuerySet, PipelineStatisticsTypes)> {
        self.pipeline_statistics_query_set
            .as_ref()
            .map(|query_set| (query_set, self.pipeline_statistics_types))
    }

    /// Begin a pipeline statistics query in the pass; returns query index
    pub fn begin_pipeline_statistics_query(&self, render_pass: &mut wgpu::RenderPass<'_>) -> Option<u32> {
        if let Some(query_set) = &self.pipeline_statistics_query_set {
            // Check if there is space for another pipeline statistics query
            if self.current_pipeline_statistics_query.get() >= self.max_pipeline_statistics_queries {
                println!("Profiler: Max pipeline statistics queries reached for this frame");
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

    pub fn summarize_pipeline_statistics_queries(&self, raw: &[u64]) {
        use wgpu::PipelineStatisticsTypes as PST;

        let mask = self.pipeline_statistics_types;
        if raw.is_empty() || mask.is_empty() {
            println!("[GPU] no pipeline statistics recorded");
            return;
        }

        let mut layout: Vec<(&'static str, wgpu::PipelineStatisticsTypes)> = Vec::new();
        let push = |v: &mut Vec<_>, name, flag, mask: wgpu::PipelineStatisticsTypes| { if mask.contains(flag) { v.push((name, flag)); } };
        push(&mut layout, "VS invocations",          wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS,    mask);
        push(&mut layout, "Clipper invocations",     wgpu::PipelineStatisticsTypes::CLIPPER_INVOCATIONS,          mask);
        push(&mut layout, "Clipper primitives out",  wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT,       mask);
        push(&mut layout, "FS invocations",          wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,  mask);
        push(&mut layout, "CS invocations",          wgpu::PipelineStatisticsTypes::COMPUTE_SHADER_INVOCATIONS,   mask);

        let stride = layout.len();
        if stride == 0 {
            println!("[GPU] pipeline statistics mask is empty");
            return;
        }

        for (query, chunk) in raw.chunks(stride).enumerate() {
            if chunk.len() < stride { break; }
            println!("[GPU] pipeline stats query {}:", query);
            for ((name, _flag), &value) in layout.iter().zip(chunk.iter()) {
                println!("       {:>24}: {}", name, value);
            }
        }
    }

    pub fn end_pipeline_statistics_query(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        render_pass.end_pipeline_statistics_query();
    }

    pub fn resolve_pipeline_statistics_queries(&mut self, device: &Device, encoder: &mut CommandEncoder) {
        if let Some(query_set) = &self.pipeline_statistics_query_set {
            let count = self.current_pipeline_statistics_query.get();
            if count == 0 { return; }
            let byte_len = self.pipeline_statistics_queries_result_bytes * count as u64;

            let index = self.frame_index;
            let slot = &mut self.pipeline_buffers[index];
            let buffer = ensure_buffer_slot(device, slot, byte_len, "gpu_profiler.pipeline_statistics_queries.resolve");

            encoder.resolve_query_set(query_set, 0..count, buffer, 0);
        }
    }

    pub fn read_pipeline_statistics_queries_blocking(&self, device: &Device) -> Option<Vec<u64>> {
        let index = (self.frame_index + FRAMES_IN_FLIGHT - 1) % FRAMES_IN_FLIGHT;
        let buffer = self.pipeline_buffers[index].as_ref()?;
        let slice = buffer.slice(..);

        // Map and wait
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(Maintain::Wait);
        let data = slice.get_mapped_range();
        let values: Vec<u64> = bytemuck::cast_slice(&data).to_vec();

        drop(data);
        buffer.unmap();
        Some(values)
    }

    // --- Misc ---

    pub fn summarize_all_blocking(&self, device: &wgpu::Device) {
        if let Some(timestamp_queries) = self.read_timestamp_queries_blocking(device) {
            self.summarize_timestamp_queries(&timestamp_queries);
        }
        if let Some(occlusion_queries) = self.read_occlusion_queries_blocking(device) {
            self.summarize_occlusion_queries(&occlusion_queries);
        }
        if let Some(pipeline_statistics_queries) = self.read_pipeline_statistics_queries_blocking(device) {
            self.summarize_pipeline_statistics_queries(&pipeline_statistics_queries);
        }
    }
}

#[inline]
fn ensure_buffer_slot<'a>(
    device: &wgpu::Device,
    slot: &'a mut Option<wgpu::Buffer>,
    size: u64,
    label: &str,
) -> &'a wgpu::Buffer {
    let need_new = slot.as_ref().map(|b| b.size() < size).unwrap_or(true);
    if need_new {
        *slot = Some(device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size,
            usage: BufferUsages::COPY_SRC | wgpu::BufferUsages::MAP_READ  | BufferUsages::QUERY_RESOLVE,
            mapped_at_creation: false,
        }));
        // *slot = Some(device.create_buffer(&wgpu::BufferDescriptor {
        //     label: Some(label),
        //     size,
        //     usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::QUERY_RESOLVE,
        //     mapped_at_creation: false,
        // }));
    }
    slot.as_ref().unwrap()
}