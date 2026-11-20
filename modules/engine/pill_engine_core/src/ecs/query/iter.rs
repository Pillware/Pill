//! Sequential and parallel iterators produced by [`Query`](super::Query).
//!
//! # Responsibilities
//!
//! - Implements [`QueryIterMut`] for sequential, single-threaded iteration.
//! - Implements [`ParQueryIter`] for parallel iteration on Rayon's thread pool.
//! - Provides [`BatchStats`] for inspecting how a parallel run spread its work
//!   across batches.
//!
//! # Design
//!
//! Both iterators work from the query's cached list of matching archetypes. The
//! sequential iterator visits the archetypes in order; the parallel one cuts the
//! rows into slices and gives groups of them to Rayon. The component pointers of
//! the current archetype are cached as raw pointers, so the inner loop does not
//! look them up in a `HashMap` again.

// Standard library
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// External crates
use pill_core::rayon;

// Current crate
use super::filter::QueryFilter;
use super::target::QueryTarget;
use super::FilteredArchetypeRange;
use crate::archetype::ArchetypeId;
use crate::component::Tick;
use crate::config;
use crate::world::World;

// =============================================================================
// BatchStats
// =============================================================================

/// How one parallel iteration spread its work over batches.
///
/// [`ParQueryIter::for_each`] returns this when tracking is on, so the caller
/// can see how Rayon split the rows between its tasks. A run small enough to
/// stay on the calling thread reports a single batch.
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    /// Number of threads in the Rayon thread pool.
    pub num_threads: usize,
    /// Number of batches the iteration was split into.
    pub batch_count: usize,
    /// Rows visited in total, counted before the row filter runs.
    pub total_entities: usize,
    /// Size of the smallest batch, in rows.
    pub min_batch_size: usize,
    /// Size of the largest batch, in rows.
    pub max_batch_size: usize,
    /// Average batch size, `total_entities / batch_count`.
    pub avg_batch_size: f64,
}

impl std::fmt::Display for BatchStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "BatchStats {{ threads: {}, batches: {}, entities: {}, min: {}, max: {}, avg: {:.1} }}",
            self.num_threads,
            self.batch_count,
            self.total_entities,
            self.min_batch_size,
            self.max_batch_size,
            self.avg_batch_size
        )
    }
}

// =============================================================================
// QueryIterMut (Sequential)
// =============================================================================

/// Sequential iterator for mutable queries.
///
/// It walks the query's cached list of matching archetypes in order and visits
/// every row the filter accepts. For each archetype it keeps the component
/// pointers and the filter state, so the hot loop below runs no further lookups.
pub struct QueryIterMut<'w, Q: QueryTarget, F: QueryFilter = ()> {
    world_ptr: *mut World,
    matching_archetypes: Vec<ArchetypeId>,
    current_archetype_index: usize,
    current_entity_index: usize,
    // Row count of the current archetype, cached so the loop does not look it up
    // on every row.
    current_archetype_len: usize,
    // Component storage pointers of the current archetype; `Some` whenever the
    // loop is inside an archetype.
    current_state: Option<Q::State>,
    // Filter state for the current archetype.
    current_filter_state: Option<F::State>,
    /// World tick captured at iterator construction; passed into each
    /// `Mut<T>` produced during iteration so mutations can be detected.
    this_run: Tick,
    /// Baseline tick used by the filter to detect changes since the
    /// owning system last ran.
    last_run: Tick,
    _phantom: std::marker::PhantomData<&'w mut (Q, F)>,
}

impl<'w, Q: QueryTarget, F: QueryFilter> QueryIterMut<'w, Q, F> {
    /// Constructs a new sequential iterator for [`Query::iter_mut`].
    ///
    /// # Safety
    ///
    /// `world_ptr` must come from a `&mut World` that stays valid and
    /// exclusively borrowed for the whole lifetime `'w`, because the iterator
    /// reads and hands out its component storage through that pointer.
    pub(crate) fn new(
        world_ptr: *mut World,
        matching_archetypes: Vec<ArchetypeId>,
        this_run: Tick,
        last_run: Tick,
    ) -> Self {
        let _zone = crate::profile_scope!(
            "create mutable query iterator",
            [(
                "Matching archetype ranges to iterate: {}",
                matching_archetypes.len()
            )]
        );
        Self {
            world_ptr,
            matching_archetypes,
            current_archetype_index: 0,
            current_entity_index: 0,
            current_archetype_len: 0,
            current_state: None,
            current_filter_state: None,
            this_run,
            last_run,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Moves to the next matching archetype and caches its component pointers
    /// and filter state.
    ///
    /// Returns `None` once the list is used up, and also when the world does not
    /// hold the next id. The call is kept out of line (`#[inline(never)]`) so
    /// the hot loop in [`Iterator::next`] stays small and predicts its branches
    /// well.
    #[inline(never)]
    fn advance_archetype(&mut self) -> Option<()> {
        if self.current_archetype_index >= self.matching_archetypes.len() {
            return None;
        }

        let _zone = crate::profile_scope!(
            "advance to next archetype",
            [(
                "archetype {}/{}",
                self.current_archetype_index + 1,
                self.matching_archetypes.len()
            )]
        );

        // SAFETY: `world_ptr` came from a valid `&mut World` in
        // [`Query::iter_mut`], and this iterator borrows that world exclusively
        // for its lifetime `'w`, so the reference built here is valid and
        // nothing else can alias it. The bounds check above returns before the
        // index is used when the list is exhausted, so the indexing below is in
        // bounds; the `HashMap` lookup may still fail, and `?` then ends the
        // iteration. `init_state` caches raw pointers into component storage,
        // and those stay valid because the world is borrowed exclusively and
        // neither the archetypes nor their storage vectors move or reallocate
        // while the iterator lives. References handed out from those cached
        // pointers never outlive the iterator.
        unsafe {
            let _zone_pointers = crate::profile_scope!(
                "get world and archetype id",
                [(
                    "Advancing to archetype: {}/{}",
                    self.current_archetype_index,
                    self.matching_archetypes.len()
                )]
            );
            let world = &mut *self.world_ptr;
            let archetype_id = self.matching_archetypes[self.current_archetype_index];
            drop(_zone_pointers);

            let _zone_archetype = crate::profile_scope!("get archetype");
            let archetype = world.storage.archetypes.get_mut(&archetype_id)?;
            _zone_archetype.text_lazy(|| archetype.get_archetype_info(&world.component_registry));
            drop(_zone_archetype);

            let _zone_cache = crate::profile_scope!(
                "cache storage pointers",
                [("Entities in this archetype: {}", archetype.len())]
            );

            // Cache the row count and both states. The target takes the
            // archetype mutably, because it may look up a mutable column, and the
            // filter takes it shared. The two calls run one after the other, so
            // one reborrow of `archetype` covers both.
            self.current_archetype_len = archetype.len();
            self.current_state = Some(Q::init_state(archetype, self.this_run));
            self.current_filter_state =
                Some(F::init_state(archetype, self.last_run, self.this_run));
            self.current_entity_index = 0;
            self.current_archetype_index += 1;
        }

        Some(())
    }
}

impl<'w, Q: QueryTarget, F: QueryFilter> Iterator for QueryIterMut<'w, Q, F> {
    type Item = Q::Item<'w>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Step 1: the hot path - walk the rows of the archetype already
            // loaded.
            while self.current_entity_index < self.current_archetype_len {
                let index = self.current_entity_index;
                self.current_entity_index += 1;

                // SAFETY: `current_state` is `Some` whenever this loop runs.
                // The constructor leaves it `None` and the row count at zero, so
                // the first pass skips the loop, and `advance_archetype` sets
                // both before it returns.
                let target_state = unsafe { self.current_state.as_ref().unwrap_unchecked() };

                if !F::ACCEPTS_ALL {
                    // SAFETY: `current_filter_state` is `Some` whenever
                    // `current_state` is, because `advance_archetype` sets both
                    // together and nothing sets either one back to `None`.
                    let filter_state =
                        unsafe { self.current_filter_state.as_ref().unwrap_unchecked() };
                    if !F::matches(filter_state, index) {
                        continue;
                    }
                }

                return Some(Q::fetch_with_state(target_state, index));
            }

            // Step 2: the archetype is done - move to the next one. This is the
            // cold path, which also refills both cached states.
            self.advance_archetype()?;
        }
    }
}

// =============================================================================
// ParQueryIter (Parallel)
// =============================================================================

/// Parallel iterator that runs a query's rows on Rayon's thread pool.
///
/// Call these before running it:
/// - `.label("system_name")` names the Tracy zones it creates.
/// - `.with_batch_size(n)` sets the most rows one work slice holds.
/// - `.tracked()` makes `for_each` return [`BatchStats`].
///
/// `.for_each` then runs the closure once for every matching row.
pub struct ParQueryIter<'w, Q: QueryTarget, F: QueryFilter = ()> {
    archetype_ranges: Vec<FilteredArchetypeRange<Q::State, F::State>>,
    min_batch_size: Option<usize>,
    tracked: bool,
    /// Sum of all queried component sizes - the per-entity byte cost the
    /// default slice size is derived from.
    total_components_size: usize,
    /// Optional label for Tracy zones - set via `.label("system_name")`.
    label: Option<&'static str>,
    /// How long past runs of each label took. Shared through an `Arc<Mutex<_>>`
    /// with the world, so the scheduler and every iterator read one map.
    iterator_timings: std::sync::Arc<std::sync::Mutex<crate::world::IteratorTimings>>,
    _phantom: std::marker::PhantomData<&'w mut (Q, F)>,
}

// SAFETY: `ParQueryIter` may move between threads. It owns its archetype ranges,
// and the raw pointers they hold point into component storage that stays valid
// and unaliased for the query's lifetime, because the iterator keeps the world
// borrowed exclusively. Every thread is given its own range of rows, and the
// `Send` bounds below require the pointed-to data to move with it, so two
// threads never touch the same row.
//
// That the *systems* sharing a batch are themselves disjoint is established by
// `SystemAccess::conflicts_with`, not here; see its contract for the
// mask-completeness rule that makes it trustworthy.
unsafe impl<'w, Q: QueryTarget, F: QueryFilter> Send for ParQueryIter<'w, Q, F>
where
    Q::State: Send,
    F::State: Send,
{
}
// SAFETY: `ParQueryIter` is `Sync` because the raw pointers in `Q::State` and
// `F::State` point into component storage that the iterator borrows exclusively
// from the `World` for its whole lifetime. The `Sync` bounds below require the
// pointed-to data to be safe to share, and each thread reads its own row range,
// so access through the shared state never races.
unsafe impl<'w, Q: QueryTarget, F: QueryFilter> Sync for ParQueryIter<'w, Q, F>
where
    Q::State: Sync,
    F::State: Sync,
{
}

impl<'w, Q: QueryTarget, F: QueryFilter> ParQueryIter<'w, Q, F> {
    /// Constructs a new parallel iterator with default settings: no label, no
    /// tracking, and the default slice size. Used by [`Query::par_iter_mut`].
    pub(crate) fn new(
        archetype_ranges: Vec<FilteredArchetypeRange<Q::State, F::State>>,
        iterator_timings: std::sync::Arc<std::sync::Mutex<crate::world::IteratorTimings>>,
        total_components_size: usize,
    ) -> Self {
        let _zone = crate::profile_scope!(
            "create parallel query iterator",
            [(
                "Matching archetype ranges for parallel iteration: {}",
                archetype_ranges.len()
            )]
        );
        Self {
            archetype_ranges,
            min_batch_size: None,
            tracked: false,
            total_components_size,
            label: None,
            iterator_timings,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Attaches a label, usually the system name, used to name this iterator's
    /// Tracy zones.
    pub fn label(mut self, name: &'static str) -> Self {
        self.label = Some(name);
        self
    }
}

impl<'w, Q: QueryTarget, F: QueryFilter> ParQueryIter<'w, Q, F>
where
    Q::State: Send + Sync,
    F::State: Send + Sync,
    for<'a> Q::Item<'a>: Send,
{
    /// Returns the number of threads in Rayon's thread pool.
    pub fn num_threads() -> usize {
        rayon::current_num_threads()
    }

    /// Returns an upper bound on how many rows this iterator will visit.
    ///
    /// It adds up every row of every matching archetype, before the row filter
    /// runs, so a filter such as `Changed<T>` can make the real count smaller.
    pub fn entity_count(&self) -> usize {
        self.archetype_ranges
            .iter()
            .map(|range| range.entity_count)
            .sum()
    }

    /// Sets the largest number of rows one work slice holds.
    ///
    /// Larger slices mean less scheduling overhead; smaller slices balance the
    /// work more evenly. Light work per row (simple arithmetic) suits slices of
    /// 1,000 to 10,000 rows, heavy work suits 10 to 100.
    ///
    /// The value divides each archetype into slices, so a zero is raised to one:
    /// a size computed from a frame's entity count may come out as zero, and a
    /// division by zero would panic in the middle of that frame.
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.min_batch_size = Some(size.max(1));
        self
    }

    /// Turns on batch statistics collection. `for_each` then returns
    /// [`BatchStats`].
    pub fn tracked(mut self) -> Self {
        self.tracked = true;
        self
    }

    /// Runs the closure once for every matching row, in parallel.
    ///
    /// Returns [`BatchStats`] when `.tracked()` was called, and `None`
    /// otherwise.
    pub fn for_each<Func>(self, f: Func) -> Option<BatchStats>
    where
        Func: Fn(Q::Item<'_>) + Send + Sync,
    {
        // Step 1: look up the splitting hint that earlier runs recorded for this
        // label, if there is one.
        let hint_nanoseconds = self
            .label
            .and_then(|label| {
                self.iterator_timings.lock().ok().and_then(|timings| {
                    timings
                        .per_iterator_label_average_duration
                        .get(label)
                        .copied()
                })
            })
            .unwrap_or(0);

        // Step 2: copy out what is still needed after `self` is consumed by the
        // execute methods.
        let label = self.label;
        let iterator_timings = std::sync::Arc::clone(&self.iterator_timings);

        // Step 3: plan the dispatch once, then run it - on the calling thread
        // below the parallel threshold, or as the plan's groups on the pool.
        let started_at = pill_core::platform::Instant::now();
        let stats = match self.plan(hint_nanoseconds) {
            WorkPlan::Sequential {
                total_entities,
                threshold,
            } => {
                let processed_row_count = self.run_sequential(&f, total_entities, threshold);
                self.tracked.then(|| BatchStats {
                    num_threads: rayon::current_num_threads(),
                    batch_count: 1,
                    total_entities,
                    min_batch_size: processed_row_count,
                    max_batch_size: processed_row_count,
                    avg_batch_size: processed_row_count as f64,
                })
            }
            WorkPlan::Parallel(plan) => {
                if self.tracked {
                    Some(self.execute_tracked(&f, plan))
                } else {
                    self.execute_untracked(&f, plan);
                    None
                }
            }
        };
        let elapsed_nanoseconds = started_at.elapsed().as_nanos() as u64;

        // Step 4: store the measured duration as the splitting hint for this
        // label, and record labels that show up twice in one frame.
        if let Some(label) = label {
            if let Ok(mut timings) = iterator_timings.lock() {
                // The label is looked up by its borrowed text and stored as an
                // owned copy: the label lives in the calling module's DLL, while
                // the timings live in the world, which outlives that DLL (see
                // `IteratorTimings`).
                let durations = &mut timings.per_iterator_label_average_duration;
                let entry = match durations.get_mut(label) {
                    Some(entry) => entry,
                    None => durations
                        .entry(label.to_owned())
                        .or_insert(elapsed_nanoseconds),
                };
                let delta = elapsed_nanoseconds as i64 - *entry as i64;
                *entry = (*entry as i64
                    + delta / config::ParallelProcessingConfig::SPLITTING_HINT_WINDOW)
                    as u64;
                if timings
                    .visited_iterator_labels
                    .iter()
                    .any(|visited_label| visited_label == label)
                {
                    timings
                        .visited_duplicated_iterator_labels
                        .push(label.to_owned());
                } else {
                    timings.visited_iterator_labels.push(label.to_owned());
                }
            }
        }

        stats
    }

    /// Decides how one dispatch runs.
    ///
    /// The parallel threshold, the slice size and the work-group count are all
    /// decided here and nowhere else. Below `pool threads × MINIMUM_SLICE_SIZE`
    /// rows the work runs on the calling thread. Above it, the rows become flat
    /// slices that are grouped into contiguous ranges, and [`rayon::scope`]
    /// starts every group at once. Both executors run the plan this returns, so
    /// they cannot disagree about the split.
    fn plan(&self, hint_nanoseconds: u64) -> WorkPlan {
        let pool_threads = rayon::current_num_threads();
        let total_entities: usize = self
            .archetype_ranges
            .iter()
            .map(|range| range.entity_count)
            .sum();
        let threshold = pool_threads * config::ParallelProcessingConfig::MINIMUM_SLICE_SIZE;

        // Adaptive fallback: run tiny workloads on the calling thread, where
        // starting Rayon tasks costs more than the work itself. This matters
        // most when there are many small archetypes.
        if total_entities < threshold {
            return WorkPlan::Sequential {
                total_entities,
                threshold,
            };
        }

        let _zone = crate::profile_scope!(
            "prepare batches",
            [
                ("Total entities to process: {}", total_entities),
                ("Rayon thread pool size: {}", pool_threads)
            ]
        );

        // Step 1: cut every archetype's rows into flat slices of a fixed size.
        //
        // The slices are built first so the next step can group them into
        // contiguous ranges. Because the scope starts every group at once, all
        // threads pull work from the shared queue at the same moment instead of
        // picking up the leftovers later.
        let entities_per_slice = self
            .min_batch_size
            .unwrap_or(config::default_entities_per_slice(
                self.total_components_size,
            ));

        // Each slice is an `(archetype index, start row, end row)` range. The
        // capacity is exact, so pushing never reallocates the vector.
        let slice_count: usize = self
            .archetype_ranges
            .iter()
            .map(|range| range.entity_count.div_ceil(entities_per_slice))
            .sum();
        let mut slices: Vec<(usize, usize, usize)> = Vec::with_capacity(slice_count);
        for (archetype_index, range) in self.archetype_ranges.iter().enumerate() {
            let mut slice_start_row = 0;
            while slice_start_row < range.entity_count {
                let slice_end_row = range.entity_count.min(slice_start_row + entities_per_slice);
                slices.push((archetype_index, slice_start_row, slice_end_row));
                slice_start_row = slice_end_row;
            }
        }

        // Step 2: pick the group count from the timing feedback when there is
        // one, then give every group a contiguous run of slices so one thread
        // does all its work back-to-back. A group is stored as an `(offset,
        // count)` pair into the slice list, so no slice is copied.
        let group_count = iterator_work_group_count(hint_nanoseconds, pool_threads, slices.len());
        let slices_per_group = slices.len() / group_count;
        let extra_slice_count = slices.len() % group_count;
        let mut groups: Vec<(usize, usize)> = Vec::with_capacity(group_count);
        let mut offset = 0;
        for group_index in 0..group_count {
            let count = if group_index < extra_slice_count {
                slices_per_group + 1
            } else {
                slices_per_group
            };
            if count == 0 {
                break;
            }
            groups.push((offset, count));
            offset += count;
        }

        crate::profile_message!(
            "rayon parallel iteration prepared: {} pool threads, {} archetype ranges totalling {} entities, sliced into {} work items, grouped into {} thread assignments (system splitting hint {} ns)",
            pool_threads,
            self.archetype_ranges.len(),
            total_entities,
            slices.len(),
            groups.len(),
            hint_nanoseconds,
        );

        WorkPlan::Parallel(ParallelPlan {
            slices,
            groups,
            total_entities,
            pool_threads,
        })
    }

    /// Runs the rows on the calling thread and returns how many rows it visited.
    ///
    /// This is the fallback below the parallel threshold. Tracked and untracked
    /// calls both land here, so the rows run the same way whichever result the
    /// caller asked for.
    #[cfg_attr(not(feature = "profiling"), allow(unused_variables))]
    fn run_sequential<Func>(&self, f: &Func, total_entities: usize, threshold: usize) -> usize
    where
        Func: Fn(Q::Item<'_>),
    {
        let _zone = crate::profile_scope!("sequential iteration below the parallel threshold");
        crate::profile_message!(
            "rayon workload below parallel threshold: {} entities is less than {} ({} threads x {}), falling back to sequential iteration on the calling thread",
            total_entities,
            threshold,
            rayon::current_num_threads(),
            crate::config::ParallelProcessingConfig::MINIMUM_SLICE_SIZE,
        );

        // The same row walk the parallel groups use, one whole-archetype slice
        // at a time, so the two paths cannot diverge on filtering or fetching.
        let mut visited_row_count = 0;
        for (archetype_index, range) in self.archetype_ranges.iter().enumerate() {
            visited_row_count += run_group::<Q, F, _>(
                &self.archetype_ranges,
                &[(archetype_index, 0, range.entity_count)],
                f,
            );
        }
        visited_row_count
    }

    /// Runs the plan [`plan`](Self::plan) produced, without collecting
    /// statistics.
    ///
    /// Each slice holds at most `with_batch_size` rows, or else the count
    /// `default_entities_per_slice` returns: `ECS_SLICE_SIZE` overrides it, and
    /// otherwise it stays within 256 to 4096 rows. The slices are grouped into
    /// chunks that [`rayon::scope`] starts all at once, so every thread pulls
    /// work from the shared queue.
    fn execute_untracked<Func>(&self, f: &Func, plan: ParallelPlan)
    where
        Func: Fn(Q::Item<'_>) + Send + Sync,
    {
        let ranges_ref = &self.archetype_ranges;
        let func_ref = f;
        let system_label = self.label;
        let iterator_slices_ref = &plan.slices;

        // Wrap the whole scope in one zone, so the entire parallel section shows
        // up as a single named block in Tracy.
        let _zone_scope = if let Some(system_name) = system_label {
            crate::profile_scope!(
                "{} parallel scope",
                system_name;
                [("Total entities to process: {}", plan.total_entities), ("Rayon thread pool size: {}", plan.pool_threads)]
            )
        } else {
            crate::profile_scope!(
                "parallel scope",
                [
                    ("Total entities to process: {}", plan.total_entities),
                    ("Rayon thread pool size: {}", plan.pool_threads)
                ]
            )
        };

        rayon::scope(|scope| {
            let iterator_work_group_total = plan.groups.len();
            for (iterator_work_group_index, &(start_index, count)) in plan.groups.iter().enumerate()
            {
                scope.spawn(move |_| {
                    let iterator_work_group_slice =
                        &iterator_slices_ref[start_index..start_index + count];
                    // One zone per work group, on the thread that runs it, so the
                    // work shows up on the Rayon workers in Tracy instead of only
                    // as the caller waiting inside the parallel scope. The name is
                    // only built while a profiler is connected.
                    let _zone = if let Some(system_name) = system_label {
                        crate::profile_scope!(
                            "{} group {}/{}",
                            system_name,
                            iterator_work_group_index + 1,
                            iterator_work_group_total;
                            [("{} entities in this group", iterator_work_group_slice.iter().map(|(_, start, end)| end - start).sum::<usize>())]
                        )
                    } else {
                        crate::profile_scope!(
                            "thread group {}/{}",
                            iterator_work_group_index + 1,
                            iterator_work_group_total;
                            [("{} entities in this group", iterator_work_group_slice.iter().map(|(_, start, end)| end - start).sum::<usize>())]
                        )
                    };
                    run_group::<Q, F, _>(ranges_ref, iterator_work_group_slice, func_ref);
                });
            }
        });

        crate::profile_message!(
            "rayon parallel iteration finished: {} entities processed across all work groups",
            plan.total_entities,
        );
    }

    /// Runs the same plan as
    /// [`execute_untracked`](Self::execute_untracked) and returns
    /// [`BatchStats`] describing how the work was spread.
    ///
    /// Tracking adds three counters that every work group updates: how many
    /// groups ran, and the smallest and largest group. The caller can use them to
    /// judge how evenly the rows were shared out.
    fn execute_tracked<Func>(&self, f: &Func, plan: ParallelPlan) -> BatchStats
    where
        Func: Fn(Q::Item<'_>) + Send + Sync,
    {
        let batch_count = Arc::new(AtomicUsize::new(0));
        let smallest_batch_size = Arc::new(AtomicUsize::new(usize::MAX));
        let largest_batch_size = Arc::new(AtomicUsize::new(0));
        let _zone_dist = crate::profile_scope!(
            "start distribution",
            [
                ("Work slices prepared: {}", plan.slices.len()),
                ("Thread groups assigned: {}", plan.groups.len())
            ]
        );

        let ranges_ref = &self.archetype_ranges;
        let func_ref = f;
        let system_label = self.label;
        let iterator_slices_ref = &plan.slices;

        // Wrap the whole scope in one zone, so the entire parallel section shows
        // up as a single named block in Tracy.
        let _zone_scope = if let Some(system_name) = system_label {
            crate::profile_scope!(
                "{} parallel scope",
                system_name;
                [("Total entities to process: {}", plan.total_entities), ("Rayon thread pool size: {}", plan.pool_threads)]
            )
        } else {
            crate::profile_scope!(
                "parallel scope",
                [
                    ("Total entities to process: {}", plan.total_entities),
                    ("Rayon thread pool size: {}", plan.pool_threads)
                ]
            )
        };

        rayon::scope(|scope| {
            let iterator_work_group_total = plan.groups.len();
            for (iterator_work_group_index, &(start_index, count)) in plan.groups.iter().enumerate()
            {
                let batch_count = &batch_count;
                let smallest_batch_size = &smallest_batch_size;
                let largest_batch_size = &largest_batch_size;
                let iterator_work_group_slice =
                    &iterator_slices_ref[start_index..start_index + count];
                scope.spawn(move |_| {
                    // One zone per thread group - all its slices run contiguously,
                    // with no gap for another task on the same thread.
                    let zone = if let Some(system_name) = system_label {
                        crate::profile_scope!(
                            "{} group {}/{}",
                            system_name,
                            iterator_work_group_index + 1,
                            iterator_work_group_total;
                            [("{} entities in this group", iterator_work_group_slice.iter().map(|(_, start, end)| end - start).sum::<usize>())]
                        )
                    } else {
                        crate::profile_scope!(
                            "thread group {}/{}",
                            iterator_work_group_index + 1,
                            iterator_work_group_total;
                            [("{} entities in this group", iterator_work_group_slice.iter().map(|(_, start, end)| end - start).sum::<usize>())]
                        )
                    };
                    zone.text(format_args!("thread {:?}", std::thread::current().id()));

                    let processed_row_count =
                        run_group::<Q, F, _>(ranges_ref, iterator_work_group_slice, func_ref);

                    zone.text(format_args!("{} processed", processed_row_count));

                    batch_count.fetch_add(1, Ordering::Relaxed);
                    smallest_batch_size.fetch_min(processed_row_count, Ordering::Relaxed);
                    largest_batch_size.fetch_max(processed_row_count, Ordering::Relaxed);
                });
            }
        });

        let batch_count = batch_count.load(Ordering::Relaxed);
        let min_batch_size = smallest_batch_size.load(Ordering::Relaxed);
        let max_batch_size = largest_batch_size.load(Ordering::Relaxed);

        crate::profile_message!(
            "rayon tracked parallel iteration completed: {} total entities distributed across {} rayon tasks (pool has {} threads), batch sizes min {} max {} avg {:.1} entities per task",
            plan.total_entities,
            batch_count,
            plan.pool_threads,
            min_batch_size,
            max_batch_size,
            if batch_count > 0 {
                plan.total_entities as f64 / batch_count as f64
            } else {
                0.0
            }
        );

        BatchStats {
            num_threads: plan.pool_threads,
            batch_count,
            total_entities: plan.total_entities,
            min_batch_size: if batch_count > 0 { min_batch_size } else { 0 },
            max_batch_size,
            avg_batch_size: if batch_count > 0 {
                plan.total_entities as f64 / batch_count as f64
            } else {
                0.0
            },
        }
    }
}

// =============================================================================
// Work planning
// =============================================================================

/// How one parallel dispatch runs, decided once per `for_each`.
enum WorkPlan {
    /// Below the parallel threshold: walk the rows on the calling thread.
    Sequential {
        /// Every row in every matching archetype.
        total_entities: usize,
        /// `pool threads × MINIMUM_SLICE_SIZE`, the parallel threshold.
        threshold: usize,
    },
    /// Split the rows into flat slices and run the groups on the pool.
    Parallel(ParallelPlan),
}

/// One parallel dispatch's slices and the contiguous groups of them.
struct ParallelPlan {
    /// Each slice is an `(archetype index, start row, end row)` range.
    slices: Vec<(usize, usize, usize)>,
    /// Each group is `(offset into `slices`, slice count)`; all its slices run
    /// back-to-back on one Rayon task.
    groups: Vec<(usize, usize)>,
    /// Every row in every matching archetype, before the row filter.
    total_entities: usize,
    /// Pool size the plan was sized for.
    pool_threads: usize,
}

/// Walks one work group's slices and returns how many rows it visited.
///
/// Every dispatch path runs rows through this loop: it takes the archetype's
/// cached states by index, then fetches each row the filter accepts.
fn run_group<Q, F, Func>(
    archetype_ranges: &[FilteredArchetypeRange<Q::State, F::State>],
    slices: &[(usize, usize, usize)],
    f: &Func,
) -> usize
where
    Q: QueryTarget,
    F: QueryFilter,
    Func: Fn(Q::Item<'_>),
{
    let mut visited_row_count = 0;
    for &(archetype_index, start, end) in slices {
        let archetype_range = &archetype_ranges[archetype_index];
        for index in start..end {
            if F::matches(&archetype_range.filter_state, index) {
                f(Q::fetch_with_state(&archetype_range.target_state, index));
            }
            visited_row_count += 1;
        }
    }
    visited_row_count
}

/// Returns how many Rayon tasks one parallel iteration is split into.
///
/// The count comes from the measured time of earlier runs: one task per
/// `TARGET_ITERATOR_WORK_GROUP_DURATION` of that time, so a short loop is not
/// cut into tasks that cost more to schedule than they save. The count never
/// passes `WORK_GROUPS_PER_THREAD` per pool thread, and never passes the number
/// of slices. With no measurement yet, it is one task per thread.
///
/// More tasks than threads keeps a long loop balanced on a CPU whose threads do
/// not all run equally fast, such as a hybrid CPU with efficiency cores or two
/// hyperthreads sharing one core. With exactly one task per thread, the loop
/// waits for whichever task landed on the slowest thread. With several tasks per
/// thread, the faster threads take the tasks the slower ones have not reached.
fn iterator_work_group_count(
    hint_nanoseconds: u64,
    pool_threads: usize,
    slice_count: usize,
) -> usize {
    let most_work_group_count =
        pool_threads * config::ParallelProcessingConfig::WORK_GROUPS_PER_THREAD;
    let target_work_group_count = if hint_nanoseconds > 0 {
        (hint_nanoseconds / config::ParallelProcessingConfig::TARGET_ITERATOR_WORK_GROUP_DURATION)
            as usize
    } else {
        pool_threads
    };
    target_work_group_count
        .clamp(1, most_work_group_count.max(1))
        .min(slice_count)
        .max(1)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod work_group_count_tests {
    use super::*;

    #[test]
    fn a_long_loop_splits_into_several_tasks_per_thread() {
        // 30 ms of work on 20 threads: as many tasks as allowed, so the fast
        // threads can take over the slow ones' share.
        let most_work_group_count = 20 * config::ParallelProcessingConfig::WORK_GROUPS_PER_THREAD;
        assert_eq!(
            iterator_work_group_count(30_000_000, 20, 1_000),
            most_work_group_count
        );
    }

    #[test]
    fn a_short_loop_keeps_its_tasks_near_the_target_duration() {
        // 200 us of work is four 50 us tasks, however many threads there are.
        assert_eq!(iterator_work_group_count(200_000, 20, 1_000), 4);
        assert_eq!(iterator_work_group_count(10_000, 20, 1_000), 1);
    }

    #[test]
    fn tasks_never_outnumber_slices_and_start_at_one_per_thread() {
        assert_eq!(iterator_work_group_count(30_000_000, 20, 50), 50);
        assert_eq!(iterator_work_group_count(0, 20, 1_000), 20);
        assert_eq!(iterator_work_group_count(0, 20, 0), 1);
    }
}
