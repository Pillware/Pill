//! Tracy profiling: zones, plots and messages behind compile-time feature
//! gating.
//!
//! # Responsibilities
//!
//! - Provide the macros call sites use: `profile_scope!`,
//!   `profile_frame_mark!`, `profile_plot!` and the rest.
//! - Compute the parallel-utilization metrics of a frame.
//! - Compile every entry point away to a no-op when no profiling feature is
//!   enabled.
//!
//! # Design
//!
//! | Capability                                   | `profiling` | `profiling-minimal` |
//! |----------------------------------------------|-------------|---------------------|
//! | CPU zones, frame marks, secondary frames     | +           | +                   |
//! | Call stack sampling, on-demand collection    | +           | +                   |
//! | Zone text, native plots, messages, threads   | +           | -                   |
//! | Context switches, system info, code transfer | +           | -                   |
//!
//! A zone is recorded only when the telemetry builder installed its Tracy
//! lane (`TelemetryBuilder::with_tracy`) and a profiler is connected; until
//! then every call returns without recording anything. `profile_init!()`
//! creates the shared client handle. With no profiling feature at all, every
//! entry point is a no-op stub the compiler drops.

// =============================================================================
// Profiling Targets
// =============================================================================

/// The `tracing` target that filters the coarse profiling lane
/// (`profile::coarse`).
///
/// With the `tracy` feature, [`TelemetryBuilder::with_tracy`] adds a layer
/// that turns this target on at `TRACE`, whatever the terminal's own
/// verbosity is. A zone checks that filter to learn whether the profiler
/// wants it.
///
/// [`TelemetryBuilder::with_tracy`]: crate::telemetry::TelemetryBuilder::with_tracy
pub const PROFILE_COARSE_TARGET: &str = crate::telemetry::telemetry_target::PROFILE_COARSE;

/// The `tracing` target that filters the fine investigative lane
/// (`profile::fine`).
///
/// It is turned on only when the telemetry builder was asked for fine
/// profiling. Fine zones change the timing they measure, so they stay off by
/// default.
pub const PROFILE_FINE_TARGET: &str = crate::telemetry::telemetry_target::PROFILE_FINE;

// =============================================================================
// Compile-Time Safeguards
// =============================================================================

#[cfg(all(feature = "profiling", feature = "profiling-minimal"))]
compile_error!("`profiling` and `profiling-minimal` are mutually exclusive. Enable only one.");

// =============================================================================
// Shared Profiling Data
// =============================================================================

// Neither type names anything from Tracy, so both are declared once for every
// build and re-exported from whichever `enabled` module is compiled. A field
// added here is compiled by every feature set, including a shipping build's.

/// The zone color that leaves the choice to the Tracy viewer, which by default
/// colors zones by the thread they ran on.
pub const DEFAULT_ZONE_COLOR: u32 = 0;

/// The zone color of an ECS system the project registered, `0xRRGGBB`.
///
/// Tracy colors every uncolored zone by its thread, so a project's ECS systems
/// would look like the engine's own work around them. A fixed blue lets a
/// reader find the project's code on the timeline at a glance.
pub const PROJECT_SYSTEM_ZONE_COLOR: u32 = 0x2F6FDF;

/// The graphics API a GPU timeline's timestamps come from, so Tracy can label
/// the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuApi {
    /// Vulkan.
    Vulkan,
    /// Direct3D 12.
    Direct3D12,
    /// OpenGL.
    OpenGL,
    /// Anything Tracy has no label for.
    Other,
}

/// The pipeline statistics of one geometry pass.
///
/// A frame's totals go to [`plot_gpu_statistics`].
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuStatistics {
    /// Vertex shader invocations.
    pub vertex_invocations: u64,
    /// Primitives that entered clipping.
    pub clipper_invocations: u64,
    /// Primitives that passed clipping.
    pub clipper_primitives_out: u64,
    /// Fragment shader invocations.
    pub fragment_invocations: u64,
}

// =============================================================================
// Tracy-Enabled Implementation (Either Profiling Feature)
// =============================================================================

#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
mod enabled {
    // Standard library
    use std::fmt::Arguments;
    use std::mem::ManuallyDrop;
    use std::sync::OnceLock;

    // External crates
    use tracy_client::Client;

    // Current crate
    pub use super::{GpuApi, GpuStatistics};

    /// The shared Tracy client handle, created the first time it is asked for.
    pub(crate) fn client() -> Client {
        static CLIENT: OnceLock<Client> = OnceLock::new();
        CLIENT.get_or_init(Client::start).clone()
    }

    /// Creates the shared Tracy client handle.
    ///
    /// Safe to call more than once. Later calls reuse the handle made by the
    /// first.
    #[inline]
    pub fn init() {
        let _ = client();
    }

    /// A Tracy zone opened through the client itself.
    ///
    /// Every call into Tracy's C API (open, add text, close) is made from a
    /// function in this crate marked `#[inline(never)]`. The C functions exist
    /// only inside `pill_core.dll`, which exports Rust symbols alone. A call
    /// the compiler inlined into an extension or project DLL would therefore
    /// fail to link there with "undefined symbol ___tracy_emit_zone_*". The
    /// span is wrapped in `ManuallyDrop` for the same reason. Its drop has to
    /// run in [`Drop::drop`] below, not in drop code generated wherever the
    /// zone ends.
    struct DirectZone {
        span: ManuallyDrop<tracy_client::Span>,
    }

    impl DirectZone {
        /// Opens a zone called `name` drawn in `color` (`0xRRGGBB`, or `0` for
        /// the viewer's own choice), or `None` when Tracy is not running.
        #[inline(never)]
        fn open(name: &str, color: u32, function: &str, file: &str, line: u32) -> Option<Self> {
            Client::running().map(|client| {
                // The last argument is the callstack depth to capture, none.
                let span = client.span_alloc(Some(name), function, file, line, 0);
                // Tracy sets a zone's color after it opens. Color zero is
                // left unsent, so the viewer keeps its own choice.
                if color != super::DEFAULT_ZONE_COLOR {
                    span.emit_color(color);
                }
                Self {
                    span: ManuallyDrop::new(span),
                }
            })
        }

        /// Appends `text` to the zone, shown in Tracy's zone detail view.
        #[cfg(feature = "profiling")]
        #[inline(never)]
        fn text(&self, text: &str) {
            self.span.emit_text(text);
        }
    }

    impl Drop for DirectZone {
        #[inline(never)]
        fn drop(&mut self) {
            // SAFETY: `span` is dropped exactly once, here, and never used
            // again: `self` is being dropped.
            unsafe { ManuallyDrop::drop(&mut self.span) }
        }
    }

    /// Whether a zone on the fine (`profile::fine`) or coarse
    /// (`profile::coarse`) lane would reach a connected profiler.
    ///
    /// The lanes are switched by the telemetry stack's `tracing` filter. The
    /// builder's Tracy layer turns `profile::coarse` on at `TRACE`, and fine
    /// profiling does the same for `profile::fine`. Both happen whatever the
    /// terminal's own verbosity is. The answer is checked before a dynamic
    /// name is formatted, so a run with no profiler connected allocates
    /// nothing.
    #[inline(never)]
    fn lane_wanted(fine_lane: bool) -> bool {
        if !Client::is_running() || !Client::is_connected() {
            return false;
        }
        if fine_lane {
            tracing::enabled!(target: crate::profiling::PROFILE_FINE_TARGET, tracing::Level::TRACE)
        } else {
            tracing::enabled!(target: crate::profiling::PROFILE_COARSE_TARGET, tracing::Level::TRACE)
        }
    }

    /// Keeps a CPU zone open until it is dropped.
    ///
    /// `profile_scope!` and `profile_scope_fine!` create one, and the zone
    /// closes when the guard drops. The zone is opened through the Tracy
    /// client itself, not as a `tracing` span. A span gets its name when it is
    /// created, from its fixed name and the fields known then, so a name or
    /// text recorded later could never reach Tracy.
    #[must_use = "zone closes on drop - bind to a variable"]
    pub struct TracyZone {
        // `None` when the lane is off or no profiler is connected.
        #[cfg_attr(not(feature = "profiling"), allow(dead_code))]
        zone: Option<DirectZone>,
    }

    impl TracyZone {
        /// Opens a zone called `name` on the given lane.
        #[inline]
        fn open(fine_lane: bool, name: &str, function: &str, file: &str, line: u32) -> Self {
            if !lane_wanted(fine_lane) {
                return Self { zone: None };
            }
            Self {
                zone: DirectZone::open(name, super::DEFAULT_ZONE_COLOR, function, file, line),
            }
        }

        /// Opens a zone whose name is built only when the lane is wanted.
        #[inline]
        fn open_lazy(
            fine_lane: bool,
            name: impl FnOnce() -> String,
            color: u32,
            function: &str,
            file: &str,
            line: u32,
        ) -> Self {
            if !lane_wanted(fine_lane) {
                return Self { zone: None };
            }
            Self {
                zone: DirectZone::open(&name(), color, function, file, line),
            }
        }

        /// Static-name zone on the coarse lane.
        #[doc(hidden)]
        #[inline]
        pub fn new_static(name: &'static str, function: &str, file: &str, line: u32) -> Self {
            Self::open(false, name, function, file, line)
        }

        /// Static-name zone on the fine lane.
        #[doc(hidden)]
        #[inline]
        pub fn new_static_fine(name: &'static str, function: &str, file: &str, line: u32) -> Self {
            Self::open(true, name, function, file, line)
        }

        /// Dynamic-name zone on the coarse lane, named by a borrowed string.
        #[doc(hidden)]
        #[inline]
        pub fn new_dynamic(name: &str, function: &str, file: &str, line: u32) -> Self {
            Self::open(false, name, function, file, line)
        }

        /// Dynamic-name zone on the coarse lane. The closure building the name
        /// only runs when the zone will be recorded.
        #[doc(hidden)]
        #[inline]
        pub fn new_dynamic_lazy(
            name: impl FnOnce() -> String,
            function: &str,
            file: &str,
            line: u32,
        ) -> Self {
            Self::open_lazy(false, name, super::DEFAULT_ZONE_COLOR, function, file, line)
        }

        /// Same as [`new_dynamic_lazy`](Self::new_dynamic_lazy), drawn in
        /// `color` (`0xRRGGBB`). [`DEFAULT_ZONE_COLOR`](super::DEFAULT_ZONE_COLOR)
        /// leaves the color to the viewer.
        #[doc(hidden)]
        #[inline]
        pub fn new_dynamic_lazy_colored(
            name: impl FnOnce() -> String,
            color: u32,
            function: &str,
            file: &str,
            line: u32,
        ) -> Self {
            Self::open_lazy(false, name, color, function, file, line)
        }

        /// Same as [`new_dynamic_lazy`](Self::new_dynamic_lazy) but on the
        /// `profile::fine` lane for investigative fine-grained zones.
        #[doc(hidden)]
        #[inline]
        pub fn new_dynamic_lazy_fine(
            name: impl FnOnce() -> String,
            function: &str,
            file: &str,
            line: u32,
        ) -> Self {
            Self::open_lazy(true, name, super::DEFAULT_ZONE_COLOR, function, file, line)
        }

        /// Adds a diagnostic message to this zone. Tracy shows it in the
        /// zone's tooltip and detail view.
        #[cfg(feature = "profiling")]
        #[inline]
        pub fn text(&self, message_text: Arguments<'_>) {
            if let Some(zone) = &self.zone {
                zone.text(&message_text.to_string());
            }
        }

        /// No-op detail recording under `profiling-minimal`.
        #[cfg(not(feature = "profiling"))]
        #[inline]
        pub fn text(&self, _message_text: Arguments<'_>) {}

        /// Adds a diagnostic message that is built only when the zone is
        /// recorded.
        ///
        /// Because the closure runs only then, work such as allocating a
        /// `String` or formatting archetype details is skipped while the game
        /// runs without a profiler.
        #[cfg(feature = "profiling")]
        #[inline]
        pub fn text_lazy(&self, build_message_text: impl FnOnce() -> String) {
            if let Some(zone) = &self.zone {
                zone.text(&build_message_text());
            }
        }

        /// No-op lazy detail recording under `profiling-minimal`.
        #[cfg(not(feature = "profiling"))]
        #[inline]
        pub fn text_lazy(&self, _build_message_text: impl FnOnce() -> String) {}
    }

    /// Shows GPU work as zones on a track of its own in Tracy, beside the CPU
    /// threads.
    ///
    /// Every call into Tracy's C API happens in a function of this crate that
    /// is never inlined, for the reason given on `DirectZone`: those functions
    /// exist only inside `pill_core.dll`.
    pub struct GpuTimeline {
        context: ManuallyDrop<tracy_client::GpuContext>,
    }

    impl GpuTimeline {
        /// Creates a timeline whose clock reads `gpu_timestamp` now.
        ///
        /// One tick of that clock lasts `period_nanoseconds`. Returns `None`
        /// when Tracy is not running, or when the process has already created
        /// Tracy's 255 GPU contexts.
        #[inline(never)]
        pub fn new(
            name: &str,
            api: GpuApi,
            gpu_timestamp: i64,
            period_nanoseconds: f32,
        ) -> Option<Self> {
            let context_type = match api {
                GpuApi::Vulkan => tracy_client::GpuContextType::Vulkan,
                GpuApi::Direct3D12 => tracy_client::GpuContextType::Direct3D12,
                GpuApi::OpenGL => tracy_client::GpuContextType::OpenGL,
                GpuApi::Other => tracy_client::GpuContextType::Invalid,
            };
            let context = Client::running()?
                .new_gpu_context(Some(name), context_type, gpu_timestamp, period_nanoseconds)
                .ok()?;
            Some(Self {
                context: ManuallyDrop::new(context),
            })
        }

        /// Opens a GPU zone called `name`, recorded now on the calling thread.
        /// Its GPU start and end arrive later through
        /// [`GpuTimelineSpan::upload`]. `None` when Tracy has too many spans
        /// waiting for their timestamps.
        #[inline(never)]
        pub fn begin_span(&self, name: &str, file: &str, line: u32) -> Option<GpuTimelineSpan> {
            let span = self.context.span_alloc(name, "", file, line).ok()?;
            Some(GpuTimelineSpan {
                span: ManuallyDrop::new(span),
            })
        }
    }

    impl Drop for GpuTimeline {
        #[inline(never)]
        fn drop(&mut self) {
            // SAFETY: `context` is dropped exactly once, here, and never used
            // again: `self` is being dropped.
            unsafe { ManuallyDrop::drop(&mut self.context) }
        }
    }

    /// One GPU zone of a [`GpuTimeline`].
    ///
    /// It is begun and ended as its commands are recorded, and it is timed
    /// once the GPU's timestamps are read back. A span dropped before its
    /// timestamps are uploaded is placed at the start of the timeline, so it
    /// does not overlap the real zones.
    pub struct GpuTimelineSpan {
        span: ManuallyDrop<tracy_client::GpuSpan>,
    }

    impl GpuTimelineSpan {
        /// Marks the end of the zone's commands, recorded now.
        #[inline(never)]
        pub fn end(&mut self) {
            self.span.end_zone();
        }

        /// Gives the zone the GPU timestamps, in ticks, of its start and end.
        #[inline(never)]
        pub fn upload(&self, start_timestamp: i64, end_timestamp: i64) {
            self.span.upload_timestamp_start(start_timestamp);
            self.span.upload_timestamp_end(end_timestamp);
        }

        /// Drops the zone without reporting anything to Tracy.
        ///
        /// It is for a zone whose timeline the profiler no longer knows: a
        /// normal drop would send its end there.
        pub fn discard(self) {
            std::mem::forget(self);
        }
    }

    impl Drop for GpuTimelineSpan {
        #[inline(never)]
        fn drop(&mut self) {
            // SAFETY: `span` is dropped exactly once, here, and never used
            // again: `self` is being dropped.
            unsafe { ManuallyDrop::drop(&mut self.span) }
        }
    }

    /// Whether a Tracy profiler is connected right now.
    ///
    /// Tracy records on demand: until a profiler connects, it drops every
    /// event. So a one-time setup event, such as a GPU timeline's context,
    /// has to wait for a connection and be sent again after a reconnect.
    /// Otherwise later events refer to something the profiler never saw, and
    /// `tracy-capture` crashes on GPU zones of an unknown context.
    #[inline(never)]
    pub fn profiler_connected() -> bool {
        Client::is_running() && Client::is_connected()
    }

    /// Adds one frame's pipeline statistics to Tracy's plots.
    ///
    /// The plot names are this crate's statics. Tracy reads a plot's name
    /// through the pointer it is given, long after the call. A name in a DLL
    /// the host reloads would therefore leave Tracy reading unmapped memory.
    #[inline(never)]
    pub fn plot_gpu_statistics(statistics: GpuStatistics) {
        let Some(client) = Client::running() else {
            return;
        };
        client.plot(
            tracy_client::plot_name!("GPU vertex shader invocations"),
            statistics.vertex_invocations as f64,
        );
        client.plot(
            tracy_client::plot_name!("GPU primitives in"),
            statistics.clipper_invocations as f64,
        );
        client.plot(
            tracy_client::plot_name!("GPU primitives rasterized"),
            statistics.clipper_primitives_out as f64,
        );
        client.plot(
            tracy_client::plot_name!("GPU fragment shader invocations"),
            statistics.fragment_invocations as f64,
        );
    }

    /// Marks the end of a frame for Tracy's frame-time graphs.
    #[inline]
    pub fn frame_mark() {
        if Client::is_running() {
            tracy_client::frame_mark();
        }
    }

    /// Marks the end of a secondary (named) continuous frame.
    #[inline]
    pub fn secondary_frame_mark(name: tracy_client::FrameName) {
        if Client::is_running() {
            client().secondary_frame_mark(name);
        }
    }

    /// RAII guard for a non-continuous frame. The frame ends on drop.
    #[must_use = "non-continuous frame ends on drop - bind to a variable"]
    pub struct NonContinuousFrame {
        #[allow(dead_code)]
        tracy_frame: Option<tracy_client::Frame>,
    }

    /// Begins a non-continuous frame (a one-shot operation, not in a loop).
    #[inline]
    pub fn non_continuous_frame_begin(name: tracy_client::FrameName) -> NonContinuousFrame {
        let tracy_frame = Client::running().map(|client| client.non_continuous_frame(name));
        NonContinuousFrame { tracy_frame }
    }
}

// =============================================================================
// Full `profiling` Feature - Plots, Messages, Thread Names
// =============================================================================

/// Helpers that need the full `profiling` feature: plots, messages and thread
/// names.
#[cfg(feature = "profiling")]
pub mod profiling_extras {
    // Standard library
    use std::fmt::Arguments;

    // External crates
    use tracy_client::Client;

    // Current crate
    use super::enabled::client;

    /// Adds one value to a named time-series plot.
    ///
    /// The plots are Tracy's own, so the profiler draws them as a graph. The
    /// call does nothing while no profiler is connected.
    #[inline]
    pub fn plot(name: tracy_client::PlotName, value: f64) {
        if Client::is_running() && Client::is_connected() {
            client().plot(name, value);
        }
    }

    /// Configures how a plot appears in the Tracy profiler UI.
    #[inline]
    pub fn plot_config(
        name: tracy_client::PlotName,
        configuration: tracy_client::PlotConfiguration,
    ) {
        if Client::is_running() {
            client().plot_config(name, configuration);
        }
    }

    /// Emits a diagnostic message, shown in white.
    ///
    /// The text is built only when a profiler is connected.
    #[inline]
    pub fn message(message_text: Arguments<'_>) {
        if Client::is_running() && Client::is_connected() {
            let text = format!("{}", message_text);
            client().message(&text, 0);
        }
    }

    /// Emits a warning message, shown in orange (RGBA 0xFF8800FF).
    #[inline]
    pub fn warn(message_text: Arguments<'_>) {
        if Client::is_running() && Client::is_connected() {
            let text = format!("{}", message_text);
            client().color_message(&text, 0xFF8800FF, 0);
        }
    }

    /// Emits an error message, shown in red (RGBA 0xFF0000FF).
    #[inline]
    pub fn error(message_text: Arguments<'_>) {
        if Client::is_running() && Client::is_connected() {
            let text = format!("{}", message_text);
            client().color_message(&text, 0xFF0000FF, 0);
        }
    }

    /// Sets the display name of the current thread in Tracy.
    #[inline]
    pub fn set_thread_name(name: &str) {
        if Client::is_running() {
            client().set_thread_name(name);
        }
    }
}

// =============================================================================
// Disabled (No-Op) Implementation - Neither Feature Enabled
// =============================================================================

#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
mod enabled {
    // Standard library
    use std::fmt::Arguments;

    // Current crate
    pub use super::{GpuApi, GpuStatistics};

    /// Compile-time no-op CPU zone guard.
    ///
    /// Compiled when no profiling feature is enabled; every method is an
    /// empty stub so instrumentation compiles away to zero cost.
    #[must_use = "zone closes on drop - bind to a variable"]
    pub struct TracyZone;

    impl TracyZone {
        /// Compile-time no-op: creates an empty static-name zone guard.
        #[doc(hidden)]
        #[inline(always)]
        pub fn new_static(_name: &'static str, _function: &str, _file: &str, _line: u32) -> Self {
            Self
        }

        /// Compile-time no-op: creates an empty dynamic-name zone guard.
        #[doc(hidden)]
        #[inline(always)]
        pub fn new_dynamic(_name: &str, _function: &str, _file: &str, _line: u32) -> Self {
            Self
        }

        /// Compile-time no-op: creates an empty lazily-named zone guard.
        #[inline(always)]
        pub fn new_dynamic_lazy(
            _name: impl FnOnce() -> String,
            _function: &str,
            _file: &str,
            _line: u32,
        ) -> Self {
            Self
        }

        /// Compile-time no-op: creates an empty colored zone guard.
        #[doc(hidden)]
        #[inline(always)]
        pub fn new_dynamic_lazy_colored(
            _name: impl FnOnce() -> String,
            _color: u32,
            _function: &str,
            _file: &str,
            _line: u32,
        ) -> Self {
            Self
        }

        /// Compile-time no-op: creates an empty fine-lane zone guard.
        #[inline(always)]
        pub fn new_dynamic_lazy_fine(
            _name: impl FnOnce() -> String,
            _function: &str,
            _file: &str,
            _line: u32,
        ) -> Self {
            Self
        }

        /// Compile-time no-op: records nothing on this zone.
        #[inline(always)]
        pub fn text(&self, _message_text: Arguments<'_>) {}

        /// Compile-time no-op: never evaluates the lazy message closure.
        #[inline(always)]
        pub fn text_lazy(&self, _build_message_text: impl FnOnce() -> String) {}
    }

    /// Compile-time no-op guard for a non-continuous frame.
    ///
    /// Dropping it does nothing; the real Tracy frame lifecycle does not
    /// exist when no profiling feature is enabled.
    #[must_use = "non-continuous frame ends on drop - bind to a variable"]
    pub struct NonContinuousFrame;

    /// Opaque stand-in for `tracy_client::FrameName` when the crate isn't linked.
    #[doc(hidden)]
    pub struct OpaqueFrameName;

    /// Compile-time no-op GPU timeline: never created without Tracy.
    pub struct GpuTimeline;

    impl GpuTimeline {
        /// Compile-time no-op: there is no Tracy to show a timeline in.
        #[inline(always)]
        pub fn new(
            _name: &str,
            _api: GpuApi,
            _gpu_timestamp: i64,
            _period_nanoseconds: f32,
        ) -> Option<Self> {
            None
        }

        /// Compile-time no-op: opens no zone.
        #[inline(always)]
        pub fn begin_span(&self, _name: &str, _file: &str, _line: u32) -> Option<GpuTimelineSpan> {
            None
        }
    }

    /// Compile-time no-op GPU zone.
    pub struct GpuTimelineSpan;

    impl GpuTimelineSpan {
        /// Compile-time no-op: ends nothing.
        #[inline(always)]
        pub fn end(&mut self) {}

        /// Compile-time no-op: uploads nothing.
        #[inline(always)]
        pub fn upload(&self, _start_timestamp: i64, _end_timestamp: i64) {}

        /// Compile-time no-op: there is nothing to let go of.
        #[inline(always)]
        pub fn discard(self) {}
    }

    /// Compile-time no-op: no profiler can connect without Tracy.
    #[inline(always)]
    pub fn profiler_connected() -> bool {
        false
    }

    /// Compile-time no-op: there are no plots without Tracy.
    #[inline(always)]
    pub fn plot_gpu_statistics(_statistics: GpuStatistics) {}

    /// Compile-time no-op: Tracy is never initialized.
    #[inline(always)]
    pub fn init() {}

    /// Compile-time no-op: no frame is ever marked.
    #[inline(always)]
    pub fn frame_mark() {}

    /// Compile-time no-op: no secondary frame is ever marked.
    #[inline(always)]
    pub fn secondary_frame_mark(_name: OpaqueFrameName) {}

    /// Compile-time no-op: returns a guard that ends nothing.
    #[inline(always)]
    pub fn non_continuous_frame_begin(_name: OpaqueFrameName) -> NonContinuousFrame {
        NonContinuousFrame
    }
}

pub use enabled::*;

// `extras` exists only under the `profiling` feature. The macros reach the
// module through `$crate::profiling::extras::*`, so the alias is cfg-gated the
// same way.
#[cfg(feature = "profiling")]
pub use profiling_extras as extras;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns how much of a frame's thread capacity was busy, as a percentage.
///
/// The total of `system_durations_ns` is divided by the frame's full thread
/// capacity (`frame_wall_time_ns` times `thread_count`). 100% means every
/// thread was busy for the whole frame; anything below that left some
/// capacity idle.
///
/// This is a lower-bound estimate because each system's wall-clock
/// duration already includes internal Rayon parallelism - a system that
/// takes 10 ms on 12 threads may represent up to 120 ms of CPU work.
/// The metric treats each system as using one thread, so actual thread
/// saturation is always at least this number.
///
/// # Examples
///
/// ```
/// use pill_core::profiling::compute_parallel_utilization;
///
/// let system_durations_ns = [10_000_000, 20_000_000];
/// let utilization = compute_parallel_utilization(&system_durations_ns, 20_000_000, 2);
/// assert_eq!(utilization, 75.0);
/// ```
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
pub fn compute_parallel_utilization(
    system_durations_ns: &[u64],
    frame_wall_time_ns: u64,
    thread_count: usize,
) -> f64 {
    // Step 1: Add up the per-system durations to get the total CPU work done.
    let total_work: u128 = system_durations_ns
        .iter()
        .map(|&duration_nanoseconds| duration_nanoseconds as u128)
        .sum();
    // Step 2: Divide the total work by the frame's full thread capacity.
    let capacity = frame_wall_time_ns as u128 * thread_count as u128;
    if capacity == 0 {
        return 0.0;
    }
    (total_work as f64 / capacity as f64) * 100.0
}

/// Returns how evenly the systems of each parallel batch share their work,
/// as a percentage.
///
/// For one batch the score is its total duration divided by the product of its
/// longest system's duration and the batch size. The frame score averages
/// those batch scores, each weighted by the batch's ideal work (its longest
/// duration times its size). 100% means every system of every batch takes the
/// same time. Below that, the threads of the shorter systems sit idle while
/// the longest one runs, because Rayon steals work inside a system, not
/// between systems.
///
/// # Examples
///
/// ```
/// use pill_core::profiling::compute_batch_packing_efficiency;
///
/// // Two systems batched together: one takes 10 ms, the other 20 ms.
/// let system_durations_ns = [10_000_000, 20_000_000];
/// let execution_graph = [vec![0, 1]];
/// let efficiency = compute_batch_packing_efficiency(&system_durations_ns, &execution_graph);
/// assert_eq!(efficiency, 75.0);
/// ```
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
pub fn compute_batch_packing_efficiency(
    system_durations_ns: &[u64],
    execution_graph: &[Vec<usize>],
) -> f64 {
    let mut total_ideal: u128 = 0;
    let mut total_actual: u128 = 0;

    // Step 1: Add up the ideal work (longest duration × batch size) and the
    // actual work (sum of durations) of every parallel batch.
    for batch in execution_graph {
        if batch.is_empty() {
            continue;
        }
        let max_duration = batch
            .iter()
            .filter_map(|&system_index| system_durations_ns.get(system_index).copied())
            .max()
            .unwrap_or(0);
        let sum_durations: u64 = batch
            .iter()
            .filter_map(|&system_index| system_durations_ns.get(system_index).copied())
            .sum();

        total_ideal += max_duration as u128 * batch.len() as u128;
        total_actual += sum_durations as u128;
    }

    // Step 2: Turn the two totals into a percentage.
    if total_ideal == 0 {
        return 100.0; // No systems at all: nothing can be uneven.
    }
    (total_actual as f64 / total_ideal as f64) * 100.0
}

/// Computes the frame's parallel-utilization and batch-packing percentages and
/// emits them as native Tracy plots.
///
/// Compiled only under `profiling`: `profiling-minimal` has no native plots.
///
/// # Examples
///
/// ```
/// use pill_core::profiling::emit_parallel_utilization_plots;
///
/// emit_parallel_utilization_plots(
///     &[10_000_000, 20_000_000],
///     30_000_000,
///     2,
///     &[vec![0], vec![1]],
/// );
/// ```
#[cfg(feature = "profiling")]
pub fn emit_parallel_utilization_plots(
    system_durations_ns: &[u64],
    frame_wall_time_ns: u64,
    thread_count: usize,
    execution_graph: &[Vec<usize>],
) {
    // Step 1: Compute the frame's overall thread utilization.
    let utilization =
        compute_parallel_utilization(system_durations_ns, frame_wall_time_ns, thread_count);
    // Step 2: Compute how evenly systems within each batch balance out.
    let batch_packing = compute_batch_packing_efficiency(system_durations_ns, execution_graph);
    // Step 3: Emit both metrics as native Tracy plot points.
    crate::profile_plot!(parallel_utilization_pct, utilization);
    crate::profile_plot!(batch_packing_pct, batch_packing);
}

// =============================================================================
// Public Macros - the Only API Call Sites Use
// =============================================================================

/// Creates the shared Tracy client handle. Safe to call more than once.
/// Active under both `profiling` and `profiling-minimal`.
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
#[macro_export]
macro_rules! profile_init {
    () => {
        $crate::profiling::init();
    };
}

/// Compile-time no-op of [`profile_init!`] when no profiling feature is on.
#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
#[macro_export]
macro_rules! profile_init {
    () => {};
}

/// Creates a scoped CPU profiling zone on the `profile::coarse` lane.
///
/// Active under both `profiling` and `profiling-minimal`. Zone details (the
/// `[...]` form) are recorded only under `profiling`.
///
/// The zone opens through the Tracy client directly, on the lane the telemetry
/// builder enables for `profile::coarse`. A dynamic name is formatted only when
/// the profiler is connected and wants the zone.
///
/// ```ignore
/// let _zone = profile_scope!("movement");                        // static name
/// let _zone = profile_scope!("movement", [("entities: {}", n)]); // with details
/// let _zone = profile_scope!("system: {}", name);                // dynamic name
/// let _zone = profile_scope!(color = 0x2F6FDF, "system: {}", name; [("tick: {}", t)]);
/// ```
///
/// The `color = ...` form draws the zone in that color (`0xRRGGBB`) instead of
/// the viewer's default, which colors zones by thread. `DEFAULT_ZONE_COLOR`
/// keeps the viewer's choice. The color form always takes a dynamic name, so
/// the name is still formatted only when the zone is recorded.
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
#[macro_export]
macro_rules! profile_scope {
    (color = $color:expr, $fmt:literal $(, $fmt_arg:expr)* ; [ $( $detail:tt ),* $(,)? ]) => {{
        let zone = $crate::profiling::TracyZone::new_dynamic_lazy_colored(
            || format!($fmt, $($fmt_arg),*),
            $color,
            module_path!(),
            file!(),
            line!(),
        );
        $( $crate::profile_scope_detail!(zone, $detail); )*
        zone
    }};
    (color = $color:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::profiling::TracyZone::new_dynamic_lazy_colored(
            || format!($fmt, $($arg),*),
            $color,
            module_path!(),
            file!(),
            line!(),
        )
    };
    ($name:literal) => {
        $crate::profiling::TracyZone::new_static($name, module_path!(), file!(), line!())
    };
    ($name:literal, [ $( $detail:tt ),* $(,)? ]) => {{
        let zone = $crate::profiling::TracyZone::new_static($name, module_path!(), file!(), line!());
        $( $crate::profile_scope_detail!(zone, $detail); )*
        zone
    }};
    ($fmt:literal $(, $fmt_arg:expr)* ; [ $( $detail:tt ),* $(,)? ]) => {{
        let zone = $crate::profiling::TracyZone::new_dynamic_lazy(
            || format!($fmt, $($fmt_arg),*),
            module_path!(),
            file!(),
            line!(),
        );
        $( $crate::profile_scope_detail!(zone, $detail); )*
        zone
    }};
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::profiling::TracyZone::new_dynamic_lazy(
            || format!($fmt, $($arg),*),
            module_path!(),
            file!(),
            line!(),
        )
    };
}

/// Compile-time no-op of [`profile_scope!`] when no profiling feature is on.
#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
#[macro_export]
macro_rules! profile_scope {
    (color = $color:expr, $fmt:literal $(, $fmt_arg:expr)* ; [ $( $detail:tt ),* $(,)? ]) => {
        $crate::profiling::TracyZone::new_dynamic_lazy_colored(
            || format!($fmt, $($fmt_arg),*),
            $color,
            module_path!(),
            file!(),
            line!(),
        )
    };
    (color = $color:expr, $fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::profiling::TracyZone::new_dynamic_lazy_colored(
            || format!($fmt, $($arg),*),
            $color,
            module_path!(),
            file!(),
            line!(),
        )
    };
    ($name:literal) => {
        $crate::profiling::TracyZone::new_static($name, module_path!(), file!(), line!())
    };
    ($name:literal, [ $( $detail:tt ),* $(,)? ]) => {
        $crate::profiling::TracyZone::new_static($name, module_path!(), file!(), line!())
    };
    ($fmt:literal $(, $fmt_arg:expr)* ; [ $( $detail:tt ),* $(,)? ]) => {
        $crate::profiling::TracyZone::new_dynamic_lazy(
            || format!($fmt, $($fmt_arg),*),
            module_path!(),
            file!(),
            line!(),
        )
    };
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::profiling::TracyZone::new_dynamic_lazy(
            || format!($fmt, $($arg),*),
            module_path!(),
            file!(),
            line!(),
        )
    };
}

/// Creates a fine-grained investigative CPU zone on the `profile::fine` lane.
///
/// Only active under `profiling-fine` (which implies `profiling`). Fine zones
/// perturb the timing they measure and are meant for temporary deep
/// investigation. They compile away completely when `profiling-fine` is
/// disabled.
///
/// A `profiling-fine` build records nothing here unless the telemetry builder
/// also turned the fine lane on (`with_fine_profiling`), because the zone asks
/// that filter first.
#[cfg(feature = "profiling-fine")]
#[macro_export]
macro_rules! profile_scope_fine {
    ($name:literal) => {
        $crate::profiling::TracyZone::new_static_fine($name, module_path!(), file!(), line!())
    };
    ($name:literal, [ $( $detail:tt ),* $(,)? ]) => {{
        let zone = $crate::profiling::TracyZone::new_static_fine($name, module_path!(), file!(), line!());
        $( $crate::profile_scope_detail!(zone, $detail); )*
        zone
    }};
    ($fmt:literal $(, $arg:expr)* $(,)?) => {
        $crate::profiling::TracyZone::new_dynamic_lazy_fine(
            || format!($fmt, $($arg),*),
            module_path!(),
            file!(),
            line!(),
        )
    };
}

/// Compile-time no-op of [`profile_scope_fine!`].
#[cfg(not(feature = "profiling-fine"))]
#[macro_export]
macro_rules! profile_scope_fine {
    ($($any:tt)*) => {};
}

/// Internal helper: dispatches a single detail element to `zone.text()`.
/// Only active under `profiling` (no-op under `profiling-minimal`).
#[cfg(feature = "profiling")]
#[doc(hidden)]
#[macro_export]
macro_rules! profile_scope_detail {
    ($zone:ident, ($fmt:literal $(, $arg:expr)* $(,)?)) => {
        $zone.text(format_args!($fmt, $($arg),*));
    };
    ($zone:ident, $text:literal) => {
        $zone.text(format_args!($text));
    };
}

/// Compile-time no-op of the detail helper in [`profile_scope!`] when the
/// `profiling` feature is off.
#[cfg(not(feature = "profiling"))]
#[doc(hidden)]
#[macro_export]
macro_rules! profile_scope_detail {
    ($zone:ident, ($fmt:literal $(, $arg:expr)* $(,)?)) => {};
    ($zone:ident, $text:literal) => {};
}

/// Marks the end of a frame. Call once per frame loop iteration.
/// Active under both `profiling` and `profiling-minimal`.
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
#[macro_export]
macro_rules! profile_frame_mark {
    () => {
        $crate::profiling::frame_mark();
    };
}

/// Compile-time no-op of [`profile_frame_mark!`] when no profiling feature is
/// on.
#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
#[macro_export]
macro_rules! profile_frame_mark {
    () => {};
}

/// Marks the end of a secondary (named) continuous frame.
/// Active under both `profiling` and `profiling-minimal`.
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
#[macro_export]
macro_rules! profile_secondary_frame_mark {
    ($name:literal) => {
        $crate::profiling::secondary_frame_mark($crate::tracy_client::frame_name!($name));
    };
}

/// Compile-time no-op of [`profile_secondary_frame_mark!`] when no profiling
/// feature is on.
#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
#[macro_export]
macro_rules! profile_secondary_frame_mark {
    ($name:literal) => {};
}

/// Begins a non-continuous frame (a one-shot operation, not in a loop).
/// Returns an RAII guard that ends the frame on drop.
/// Active under both `profiling` and `profiling-minimal`.
#[cfg(any(feature = "profiling", feature = "profiling-minimal"))]
#[macro_export]
macro_rules! profile_non_continuous_frame {
    ($name:literal) => {
        $crate::profiling::non_continuous_frame_begin($crate::tracy_client::frame_name!($name))
    };
}

/// Compile-time no-op of [`profile_non_continuous_frame!`] when no profiling
/// feature is on: the guard it returns ends nothing.
#[cfg(not(any(feature = "profiling", feature = "profiling-minimal")))]
#[macro_export]
macro_rules! profile_non_continuous_frame {
    ($name:literal) => {
        $crate::profiling::NonContinuousFrame
    };
}

/// Emits a data point on a named time-series plot.
/// Only active under `profiling` (not `profiling-minimal`).
///
/// The plot name is a Rust identifier (not a string literal). Tracy draws each
/// plot as a graph in its UI.
///
/// ```ignore
/// profile_plot!(entity_count, world.entity_count() as f64);
/// profile_plot!(frame_time_us, elapsed.as_micros() as f64);
/// ```
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_plot {
    ($name:ident, $value:expr) => {
        $crate::profiling::extras::plot(
            $crate::tracy_client::plot_name!(stringify!($name)),
            $value as f64,
        );
    };
}

/// Compile-time no-op of [`profile_plot!`] when the `profiling` feature is
/// off: no plot point is emitted.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_plot {
    ($name:ident, $value:expr) => {};
}

/// Configures how a plot appears in the Tracy profiler UI.
/// Only active under `profiling`.
///
/// ```ignore
/// use tracy_client::{PlotConfiguration, PlotFormat};
/// profile_plot_config!(entity_count, PlotConfiguration::default()
///     .format(PlotFormat::Number));
/// ```
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_plot_config {
    ($name:ident, $config:expr) => {
        $crate::profiling::extras::plot_config(
            $crate::tracy_client::plot_name!(stringify!($name)),
            $config,
        );
    };
}

/// Compile-time no-op of [`profile_plot_config!`] when the `profiling`
/// feature is off: no plot is configured.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_plot_config {
    ($name:ident, $config:expr) => {};
}

/// Emits a diagnostic message (white) at the current point in the trace.
/// Only active under `profiling` (not `profiling-minimal`).
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_message {
    ($($arg:tt)*) => {
        $crate::profiling::extras::message(format_args!($($arg)*));
    };
}

/// Compile-time no-op of [`profile_message!`] when the `profiling` feature is
/// off: no message is emitted.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_message {
    ($($arg:tt)*) => {};
}

/// Emits a warning message (orange) at the current point in the trace.
/// Only active under `profiling`.
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_warn {
    ($($arg:tt)*) => {
        $crate::profiling::extras::warn(format_args!($($arg)*));
    };
}

/// Compile-time no-op of [`profile_warn!`] when the `profiling` feature is
/// off: no warning is emitted.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_warn {
    ($($arg:tt)*) => {};
}

/// Emits an error message (red) at the current point in the trace.
/// Only active under `profiling`.
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_error {
    ($($arg:tt)*) => {
        $crate::profiling::extras::error(format_args!($($arg)*));
    };
}

/// Compile-time no-op of [`profile_error!`] when the `profiling` feature is
/// off: no error message is emitted.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_error {
    ($($arg:tt)*) => {};
}

/// Sets the display name of the current thread in Tracy.
/// Only active under `profiling`.
#[cfg(feature = "profiling")]
#[macro_export]
macro_rules! profile_thread {
    ($name:expr) => {
        $crate::profiling::extras::set_thread_name($name);
    };
}

/// Compile-time no-op of [`profile_thread!`] when the `profiling` feature is
/// off: the thread keeps its default name.
#[cfg(not(feature = "profiling"))]
#[macro_export]
macro_rules! profile_thread {
    ($name:expr) => {};
}
