//! Configuration constants for the engine, and the startup report of the
//! parallel-iteration settings.
//!
//! # Responsibilities
//!
//! - Defines the tunable constants for parallel iteration, entity builders,
//!   queries and allocation profiling.
//! - Provides `default_entities_per_slice` to size parallel work slices.
//! - Reports the parallel-execution configuration when the engine starts.
//!
//! # Design
//!
//! Keeping the tuning numbers in one file means changing one does not require
//! hunting through the source. The [`ParallelProcessingConfig`] struct groups
//! the parallel settings; free functions compute the slice size and write the
//! startup report.
//!
//! The startup report ([`print_parallel_config`]) goes through the tracing
//! stack under `telemetry_target::SYSTEM`, so it can be filtered, redirected
//! to the file lane or silenced like any other engine output. It is one
//! multi-line event (`log_block`), which the terminal formatter prints as a
//! block below its time, level and target. The hardware report lives in
//! `pill_player` (its `system_specs` module), which keeps `sysinfo` out of the
//! shared engine dylib.

// External crates
use pill_core::info;
use pill_core::rayon;
use pill_core::telemetry::log_block;
use pill_core::telemetry::telemetry_target;

// =============================================================================
// Profiling
// =============================================================================

/// Callstack depth recorded for each allocation by
/// `tracy_client::ProfiledAllocator`.
///
/// `0` records no callstack. A non-zero value records up to that many call
/// frames, which is what shows where an allocation came from in Tracy's
/// memory view; deeper stacks cost more, and Windows keeps at most 62 frames.
/// Every allocation is recorded either way - this value decides only how much
/// of the callstack is kept.
pub const PROFILING_MEMORY_ALLOCATIONS_SAMPLING_FREQUENCY: u16 = 10;

// =============================================================================
// ParallelProcessingConfig
// =============================================================================

// Frame
//  └─ Scheduler BATCH ("run systems batch 1/2")
//      ├─ System: movement   ──┐
//      ├─ System: health_decay ─┤ these run concurrently in the batch
//      └─ System: cleanup    ──┘
//           │
//           └─ par_iter_mut().for_each()
//                │
//                ├─ iterator_slices (up to 4096 entities each)  ← DEFAULT_ITERATOR_SLICE_SIZE
//                │   iterator_slice 0: entities 0..4096
//                │   iterator_slice 1: entities 4096..8192
//                │   ...
//                │
//                └─ iterator_work_groups (1 per rayon task)  ← TARGET_ITERATOR_WORK_GROUP_DURATION
//                    iterator_work_group 0: iterator_slices [0,1,2,3]  → rayon task 0
//                    iterator_work_group 1: iterator_slices [4,5,6,7]  → rayon task 1

/// Zero-sized struct grouping the parallel-iterator constants.
///
/// Read a constant as `crate::config::ParallelProcessingConfig::<CONSTANT>`.
pub struct ParallelProcessingConfig;

impl ParallelProcessingConfig {
    /// Smoothing factor for the moving average of how long a system run or an
    /// iterator loop takes. Each new sample moves the average by `1/32 ≈ 0.031`
    /// of the difference, so the value follows roughly the last 32 samples
    /// instead of copying one slow run.
    pub const SPLITTING_HINT_WINDOW: i64 = 32;

    /// Target wall-clock duration of one parallel work group, in nanoseconds.
    ///
    /// The iterator chooses its task count by dividing the system's measured
    /// run time by this value, so each task gets roughly this much work. A
    /// larger value means fewer, larger tasks: threads wake up less often, but
    /// the work is split less finely. 50 µs works well because a sleeping
    /// thread needs about 10 µs to wake, so the wake-up cost stays small
    /// beside the work.
    pub const TARGET_ITERATOR_WORK_GROUP_DURATION: u64 = 50_000;

    /// Most parallel work groups one pool thread gets.
    ///
    /// Giving a long loop more groups than threads keeps it balanced when
    /// threads do not run equally fast (efficiency cores on a hybrid CPU, or
    /// two hyperthreads sharing one core): the faster threads take the groups
    /// the slower ones have not reached yet.
    pub const WORK_GROUPS_PER_THREAD: usize = 4;

    /// Default number of entities in one parallel work slice.
    ///
    /// A starting value chosen for sequential (streaming) access, not tuned
    /// by measurement: the access pattern and the CPU's prefetching make the
    /// exact number matter little. [`default_entities_per_slice`] scales it
    /// down for a query whose components have unusually large rows.
    pub const DEFAULT_ITERATOR_SLICE_SIZE: usize = 4096;

    /// Minimum entities per thread for a query to run in parallel.
    ///
    /// When the total is below `pool threads × MINIMUM_SLICE_SIZE` entities,
    /// the iterator runs the rows on the calling thread, because spawning
    /// Rayon tasks would cost more than the work itself.
    pub const MINIMUM_SLICE_SIZE: usize = 256;
}

// =============================================================================
// EntityBuilderConfig
// =============================================================================

/// Zero-sized struct grouping the entity-builder constants.
pub struct EntityBuilderConfig;

impl EntityBuilderConfig {
    /// Starting capacity of the component list in `EntityBuilder` and
    /// `DeferredEntityBuilder`.
    ///
    /// Most entities carry 3 to 8 components, so pre-allocating 8 avoids a
    /// reallocation while components are added one by one with `.with()`.
    pub const DEFAULT_COMPONENTS_CAPACITY: usize = 8;
}

// =============================================================================
// QueryConfig
// =============================================================================

/// Zero-sized struct grouping the query constants.
pub struct QueryConfig;

impl QueryConfig {
    /// Starting capacity for the component-id and filter-pair lists that the
    /// query-target and filter-tuple macros build.
    ///
    /// A typical query names 1 to 4 components or filters, so 4 avoids a
    /// reallocation in the common case.
    pub const DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY: usize = 4;

    /// Starting capacity for the filter-pair list built when an `Or` filter is
    /// expanded.
    pub const DEFAULT_FILTER_PAIRS_CAPACITY: usize = 4;
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the number of entities in one parallel work slice, scaled down for
/// components with large rows.
///
/// Without an override the result is between
/// `ParallelProcessingConfig::MINIMUM_SLICE_SIZE` (256) and
/// `ParallelProcessingConfig::DEFAULT_ITERATOR_SLICE_SIZE` (4096), and it
/// falls as `bytes_per_entity` grows, which keeps the per-slice overhead
/// small. A positive `ECS_SLICE_SIZE` environment variable overrides the
/// computed value.
///
/// # Examples
///
/// ```
/// use pill_engine::config::default_entities_per_slice;
///
/// // Larger components yield smaller slices, keeping per-slice byte volume bounded.
/// let small_components = default_entities_per_slice(8);
/// let large_components = default_entities_per_slice(64);
/// assert!(large_components <= small_components);
/// ```
pub fn default_entities_per_slice(bytes_per_entity: usize) -> usize {
    // Step 1: Use the `ECS_SLICE_SIZE` override when it is set to a positive
    // number.
    if let Ok(slice_size_override_text) = std::env::var("ECS_SLICE_SIZE") {
        if let Ok(override_slice_entity_count) = slice_size_override_text.parse::<usize>() {
            if override_slice_entity_count > 0 {
                return override_slice_entity_count;
            }
        }
    }

    // Step 2: Otherwise scale the default by the bytes per entity and clamp it
    // to the configured range.
    let default_slice_entity_count = ParallelProcessingConfig::DEFAULT_ITERATOR_SLICE_SIZE;
    let minimum_slice_entity_count = ParallelProcessingConfig::MINIMUM_SLICE_SIZE;
    // Keep the data per slice roughly constant: 4096 entities of 8 bytes is
    // 32 KiB. The query path clamps bytes_per_entity to at least 8, so this
    // cannot divide by zero.
    (default_slice_entity_count * 8 / bytes_per_entity)
        .clamp(minimum_slice_entity_count, default_slice_entity_count)
}

/// Reports the active parallel-iterator settings through the telemetry stack.
///
/// The report lists the Rayon pool size, the work-group duration target, the
/// averaging window, the default slice size and the minimum slice size. It
/// goes to the `telemetry_target::SYSTEM` target, the same one the runtime's
/// hardware report uses.
///
/// Called during [`Engine::new`](crate::Engine::new).
pub fn print_parallel_config() {
    let rayon_thread_count = rayon::current_num_threads();
    let lines = [
        format!("├─ Rayon threads: {rayon_thread_count}"),
        format!(
            "├─ Target work-group duration: {} µs",
            ParallelProcessingConfig::TARGET_ITERATOR_WORK_GROUP_DURATION / 1000
        ),
        format!(
            "├─ Splitting-hint averaging window: {} frames",
            ParallelProcessingConfig::SPLITTING_HINT_WINDOW
        ),
        format!(
            "├─ Default entities per slice: {}",
            default_entities_per_slice(8)
        ),
        format!(
            "└─ Minimum slice size: {}",
            ParallelProcessingConfig::MINIMUM_SLICE_SIZE
        ),
    ];
    info!(
        target: telemetry_target::SYSTEM,
        "{}",
        log_block("Parallel execution config", lines)
    );
}
