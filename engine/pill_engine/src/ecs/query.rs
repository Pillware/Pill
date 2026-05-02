//! # Query System - Component Access and Iteration
//!
//! Queries provide efficient iteration over entities with specific components.
//! The query system uses the [`QueryTarget`] trait to support flexible component
//! access patterns, including mutable and immutable references.
//!
//! ## How it works:
//! - The `Query` struct is parameterized by a `QueryTarget`, which defines the components
//!   being accessed and their mutability.
//! - The `QueryTarget` trait has associated types for the item being iterated over and
//!   any cached state needed for efficient access.
//! - The query system builds a component mask from the `QueryTarget`'s required components
//!   and matches it against archetypes to find relevant entities.
//! - For parallel iteration, the query caches raw pointers to component storage in
//!   the `QueryTarget::State` to allow worker threads to access components without needing to look up storages repeatedly.
//! - The `BatchStats` struct provides insights into how work is distributed across
//!   threads during parallel iteration, which can help with performance tuning.
//!
//! ## Usage Examples
//!
//! ```ignore
//! // Sequential iteration
//! fn movement_system(mut query: Query<(&mut Transform, &Velocity)>) {
//!     for (transform, velocity) in query.iter_mut() {
//!         transform.x += velocity.x * 0.016;
//!     }
//! }
//!
//! // Parallel iteration
//! fn physics_system(mut query: Query<(&mut Transform, &Velocity)>) {
//!     query.par_iter_mut().for_each(|(transform, velocity)| {
//!         transform.x += velocity.x * 0.016;
//!     });
//! }
//!
//! // Parallel with batch size control and tracking
//! fn tracked_system(mut query: Query<(&mut Transform, &Velocity)>) {
//!     let stats = query.par_iter_mut()
//!         .with_batch_size(500)
//!         .tracked()
//!         .for_each(|(transform, velocity)| {
//!             transform.x += velocity.x * 0.016;
//!         });
//!     println!("{}", stats);
//! }
//! ```

use crate::ecs::archetype::{Archetype, ArchetypeId};
use crate::ecs::component::{Component, ComponentId, ComponentMask};
use crate::ecs::entity::Entity;
use crate::ecs::world::World;
use crate::ecs::Resource;

use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use trait_type_map::VecStorage;

// ============================================================================
// Types
// ============================================================================

/// Cached archetype state for parallel iteration: (archetype_id, state, entity_count)
type ArchetypeRange<S> = (ArchetypeId, S, usize);

// ============================================================================
// Batch Statistics
// ============================================================================

/// Statistics about batch distribution during parallel iteration.
///
/// Returned by tracked parallel iteration to provide insight into
/// how Rayon distributed work across threads.
#[derive(Debug, Clone, Default)]
pub struct BatchStats {
    /// Number of threads in the Rayon thread pool
    pub num_threads: usize,
    /// Total number of batches that were executed
    pub batch_count: usize,
    /// Total number of entities that were processed
    pub total_entities: usize,
    /// Size of the smallest batch
    pub min_batch_size: usize,
    /// Size of the largest batch
    pub max_batch_size: usize,
    /// Average batch size (total_entities / batch_count)
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

// ============================================================================
// Thread-Safe Pointer Wrappers
// ============================================================================

/// A wrapper for `*const T` that implements `Send` and `Sync`.
///
/// Used internally by the query system to enable parallel iteration over
/// archetype data. The pointer is cached during query setup and used by
/// worker threads during parallel for_each operations.
///
/// SAFETY:
///
/// This type is safe to use when:
/// 1. The pointer points to valid data for the lifetime of the query
/// 2. Different threads access different indices (no aliasing)
/// 3. The World has exclusive access during iteration
///
/// # Example (internal usage)
///
/// ```ignore
/// // During query setup, cache a pointer to the entity vector
/// let ptr = SendPtr::new(&archetype.entities as *const Vec<Entity>);
///
/// // Later, in parallel iteration, access via the pointer
/// let entities = unsafe { &*ptr.as_ptr() };
/// let entity = entities[index];
/// ```
#[derive(Clone, Copy)]
pub struct SendPtr<T>(*const T);

unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> SendPtr<T> {
    pub fn new(ptr: *const T) -> Self {
        Self(ptr)
    }

    pub fn as_ptr(&self) -> *const T {
        self.0
    }
}

/// A wrapper for `*mut T` that implements `Send` and `Sync`.
///
/// Used internally by the query system to enable parallel mutable iteration
/// over archetype data. Each worker thread accesses a disjoint set of indices,
/// ensuring no data races occur.
///
/// SAFETY:
///
/// This type is safe to use when:
/// 1. The pointer points to valid data for the lifetime of the query
/// 2. Different threads access different indices (no aliasing)
/// 3. The World has exclusive access during iteration
///
/// # Example (internal usage)
///
/// ```ignore
/// // During query setup, cache a mutable pointer to component storage
/// let ptr = SendPtrMut::new(storage.as_mut_ptr());
///
/// // Later, in parallel iteration, mutate via the pointer
/// let component = unsafe { &mut *ptr.as_ptr().add(index) };
/// component.value += 1;
/// ```
#[derive(Clone, Copy)]
pub struct SendPtrMut<T>(*mut T);

unsafe impl<T> Send for SendPtrMut<T> {}
unsafe impl<T> Sync for SendPtrMut<T> {}

impl<T> SendPtrMut<T> {
    pub fn new(ptr: *mut T) -> Self {
        Self(ptr)
    }

    pub fn as_ptr(&self) -> *mut T {
        self.0
    }
}

// ============================================================================
// QueryTarget Trait
// ============================================================================

/// Trait for fetching components from archetypes.
///
/// Implemented for:
/// - `Entity`: Access to entity IDs
/// - `&T`: Immutable component reference
/// - `&mut T`: Mutable component reference
/// - Tuples: Multiple components at once (up to 4 elements)
///
/// State is used to cache archetype-specific data (e.g. storage pointers) for efficient access during parallel iteration.
/// E.g. `Query<(&Transform, &mut Velocity)>` will cache pointers to the Transform and Velocity storages for each archetype.
/// State defines these two storage pointers, which are then used by worker threads to access components without needing to look up storages repeatedly.
pub trait QueryTarget {
    type Item<'a>;
    type State;

    /// Get the component IDs required by this query.
    fn component_ids() -> Vec<ComponentId>;

    /// Report component access for system dependency analysis.
    /// Returns (reads, writes) as vectors of ComponentIds.
    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>);

    /// Initialize state for fetching from an archetype (caches storage pointers).
    fn init_state(archetype: &mut Archetype) -> Self::State;

    /// Fetch components using cached state (for parallel iteration).
    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a>;

    /// Fetch components from an archetype (immutable access).
    fn fetch<'a>(archetype: &'a Archetype, index: usize) -> Self::Item<'a>;

    /// Fetch components from an archetype (mutable access).
    fn fetch_mut<'a>(archetype: &'a mut Archetype, index: usize) -> Self::Item<'a>;
}

// ----------------------------------------------------------------------------
// QueryTarget Implementations
// ----------------------------------------------------------------------------

/// Entity query target for accessing entity IDs in a query.
/// This allows queries to include Entity in a query. e.g. Query<(Entity, &Transform)>
/// to get the entity along with its components.
impl QueryTarget for Entity {
    type Item<'a> = Entity;
    type State = SendPtr<Vec<Entity>>;

    fn component_ids() -> Vec<ComponentId> {
        Vec::new()
    }

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (Vec::new(), Vec::new())
    }

    fn init_state(archetype: &mut Archetype) -> Self::State {
        SendPtr::new(&archetype.entities as *const Vec<Entity>)
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        unsafe { *(&*state.as_ptr()).get_unchecked(index) }
    }

    fn fetch<'a>(archetype: &'a Archetype, index: usize) -> Self::Item<'a> {
        archetype.entities[index]
    }

    fn fetch_mut<'a>(archetype: &'a mut Archetype, index: usize) -> Self::Item<'a> {
        archetype.entities[index]
    }
}

/// Implementations for &T and &mut T are generated by the "impl_query_target_tuple" macro below,
/// which handles both immutable and mutable component access patterns.
/// This allows queries to specify components with the desired mutability, e.g. Query<(&Transform, &mut Velocity)>
impl<T: Component> QueryTarget for &T {
    type Item<'a> = &'a T;
    type State = SendPtr<VecStorage<T, dyn Component>>;

    fn component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (vec![ComponentId::of::<T>()], Vec::new())
    }

    fn init_state(archetype: &mut Archetype) -> Self::State {
        SendPtr::new(
            archetype.component_storages.get_storage::<T>() as *const VecStorage<T, dyn Component>
        )
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        unsafe { (*state.as_ptr()).get(index) }
    }

    fn fetch<'a>(archetype: &'a Archetype, index: usize) -> Self::Item<'a> {
        archetype.component_storages.get_storage::<T>().get(index)
    }

    fn fetch_mut<'a>(archetype: &'a mut Archetype, index: usize) -> Self::Item<'a> {
        archetype
            .component_storages
            .get_storage_mut::<T>()
            .get_mut(index)
    }
}

impl<T: Component> QueryTarget for &mut T {
    type Item<'a> = &'a mut T;
    type State = SendPtrMut<VecStorage<T, dyn Component>>;

    fn component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (Vec::new(), vec![ComponentId::of::<T>()])
    }

    fn init_state(archetype: &mut Archetype) -> Self::State {
        SendPtrMut::new(archetype.component_storages.get_storage_mut::<T>()
            as *mut VecStorage<T, dyn Component>)
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        unsafe { (*state.as_ptr()).get_mut(index) }
    }

    fn fetch<'a>(_archetype: &'a Archetype, _index: usize) -> Self::Item<'a> {
        // SAFETY: This path should never be reached in correct code.
        // The query system ensures mutable queries use fetch_mut.
        // If we get here, it indicates a bug in the query infrastructure.
        unreachable!(
            "BUG: fetch() called for &mut {} - mutable queries must use fetch_mut(). \
             This indicates a bug in the query infrastructure.",
            std::any::type_name::<T>()
        )
    }

    fn fetch_mut<'a>(archetype: &'a mut Archetype, index: usize) -> Self::Item<'a> {
        archetype
            .component_storages
            .get_storage_mut::<T>()
            .get_mut(index)
    }
}

// ----------------------------------------------------------------------------
// Tuple Implementations (via macro)
// ----------------------------------------------------------------------------
/// Macro to implement QueryTarget for tuples of different sizes
/// This allows queries like Query<(Entity, &Transform, &mut Velocity)>
macro_rules! impl_query_target_tuple {
    ($($T:ident),*) => {
        impl<$($T: QueryTarget),*> QueryTarget for ($($T,)*) {
            type Item<'a> = ($($T::Item<'a>,)*);
            type State = ($($T::State,)*);

            fn component_ids() -> Vec<ComponentId> {
                let mut ids = Vec::new();
                $(ids.extend($T::component_ids());)*
                ids
            }

            fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
                let mut reads = Vec::new();
                let mut writes = Vec::new();
                $(
                    let (r, w) = $T::report_component_access();
                    reads.extend(r);
                    writes.extend(w);
                )*
                (reads, writes)
            }

            #[allow(non_snake_case)]
            fn init_state(archetype: &mut Archetype) -> Self::State {
                let arch_ptr = archetype as *mut Archetype;
                unsafe { ($($T::init_state(&mut *arch_ptr),)*) }
            }

            #[allow(non_snake_case)]
            fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
                let ($($T,)*) = state;
                ($($T::fetch_with_state($T, index),)*)
            }

            #[allow(non_snake_case)]
            fn fetch<'a>(archetype: &'a Archetype, index: usize) -> Self::Item<'a> {
                ($($T::fetch(archetype, index),)*)
            }

            #[allow(non_snake_case)]
            fn fetch_mut<'a>(archetype: &'a mut Archetype, index: usize) -> Self::Item<'a> {
                // SAFETY: We use raw pointers to allow multiple mutable borrows of different components
                let arch_ptr = archetype as *mut Archetype;
                unsafe { ($($T::fetch_mut(&mut *arch_ptr, index),)*) }
            }
        }
    };
}

// Implement QueryTarget for tuples of size 1 to 4 (can be extended as needed)
impl_query_target_tuple!(A);
impl_query_target_tuple!(A, B);
impl_query_target_tuple!(A, B, C);
impl_query_target_tuple!(A, B, C, D);

// ============================================================================
// Query
// ============================================================================

/// Query for iterating over entities matching a component pattern.
///
/// # Example
/// ```ignore
/// fn my_system(mut query: Query<(Entity, &Transform, &mut Velocity)>) {
///     for (entity, transform, velocity) in query.iter_mut() {
///         velocity.y -= 9.8 * 0.016; // gravity
///     }
/// }
/// ```
pub struct Query<'w, Q: QueryTarget> {
    world: &'w mut World,
    _phantom: std::marker::PhantomData<Q>,
}

impl<'w, Q: QueryTarget> Query<'w, Q> {
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Build component mask from query requirements.
    fn build_query_mask(&self) -> ComponentMask {
        let mut mask = ComponentMask::empty();
        for component_id in &Q::component_ids() {
            if let Some(bit) = self.world.component_registry.get_bit(component_id) {
                mask.set(bit);
            }
        }
        mask
    }

    /// Create a sequential iterator over all matching entities.
    #[inline]
    pub fn iter_mut(&mut self) -> QueryIterMut<'_, Q> {
        let query_mask = self.build_query_mask();

        let matching_archetypes: Vec<ArchetypeId> = self
            .world
            .archetypes
            .iter()
            .filter(|(_, arch)| arch.matches_mask(&query_mask))
            .map(|(id, _)| *id)
            .collect();

        QueryIterMut {
            world_ptr: self.world as *mut World,
            matching_archetypes,
            current_archetype_idx: 0,
            current_entity_idx: 0,
            current_archetype_len: 0,
            current_state: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get the first matching entity's components.
    #[inline]
    pub fn first(&mut self) -> Option<Q::Item<'_>> {
        self.iter_mut().next()
    }

    /// Check if any entity matches this query.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entity_count() == 0
    }

    /// Count the number of entities matching this query.
    ///
    /// This is O(n) where n is the number of archetypes, but does not
    /// iterate over individual entities.
    #[inline]
    pub fn entity_count(&self) -> usize {
        let mut mask = ComponentMask::empty();
        for component_id in &Q::component_ids() {
            if let Some(bit) = self.world.component_registry.get_bit(component_id) {
                mask.set(bit);
            }
        }

        self.world
            .archetypes
            .values()
            .filter(|arch| arch.matches_mask(&mask))
            .map(|arch| arch.entity_count())
            .sum()
    }

    /// Create a parallel iterator over all matching entities.
    #[inline]
    pub fn par_iter_mut(&mut self) -> ParQueryIter<'_, Q>
    where
        Q::State: Send + Sync,
        for<'a> Q::Item<'a>: Send,
    {
        let query_mask = self.build_query_mask();

        let archetype_ranges: Vec<ArchetypeRange<Q::State>> = self
            .world
            .archetypes
            .iter_mut()
            .filter(|(_, arch)| arch.matches_mask(&query_mask))
            .filter(|(_, arch)| !arch.is_empty())
            .map(|(id, arch)| (*id, Q::init_state(arch), arch.len()))
            .collect();

        ParQueryIter {
            archetype_ranges,
            min_batch_size: None,
            tracked: false,
            _phantom: std::marker::PhantomData,
        }
    }
}

// ============================================================================
// Sequential Iterator
// ============================================================================

/// Sequential iterator for mutable queries.
pub struct QueryIterMut<'w, Q: QueryTarget> {
    world_ptr: *mut World,
    matching_archetypes: Vec<ArchetypeId>,
    current_archetype_idx: usize,
    current_entity_idx: usize,
    // Cache the current archetype length to avoid repeated lookups
    current_archetype_len: usize,
    // Cache component storage pointers (always Some during iteration)
    current_state: Option<Q::State>,
    _phantom: std::marker::PhantomData<&'w mut Q>,
}

impl<'w, Q: QueryTarget> Iterator for QueryIterMut<'w, Q> {
    type Item = Q::Item<'w>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Hot path: iterate within current archetype
            if self.current_entity_idx < self.current_archetype_len {
                let index = self.current_entity_idx;
                self.current_entity_idx += 1;

                // SAFETY: current_state is always Some during iteration in the hot path
                // We use unwrap_unchecked to eliminate branch misprediction overhead
                let state = unsafe { self.current_state.as_ref().unwrap_unchecked() };
                return Some(Q::fetch_with_state(state, index));
            }

            // Cold path: advance to next archetype
            // This happens infrequently (once per archetype)
            self.advance_archetype()?;
        }
    }
}

impl<'w, Q: QueryTarget> QueryIterMut<'w, Q> {
    /// Advance to the next archetype (cold path, separated for better branch prediction)
    #[inline(never)]
    fn advance_archetype(&mut self) -> Option<()> {
        // Check if all archetypes have been exhausted
        if self.current_archetype_idx >= self.matching_archetypes.len() {
            return None;
        }

        // SAFETY: This function is safe because:
        // 1. world_ptr was created from a valid &mut World reference in iter_mut()
        // 2. The QueryIterMut holds exclusive access to World through its lifetime 'w
        // 3. We never yield references that outlive the iterator itself
        // 4. Each archetype_id comes from matching_archetypes which was populated from valid archetypes
        // 5. The HashMap lookup can fail (returning None) but that's handled by the ? operator
        // 6. init_state() caches raw pointers to component storage, which remain valid because:
        //    - We hold exclusive access to World
        //    - Archetypes are not moved/reallocated during iteration
        //    - Component storage vectors maintain stable addresses while we iterate
        unsafe {
            let world = &mut *self.world_ptr;
            let archetype_id = self.matching_archetypes[self.current_archetype_idx];
            let archetype = world.archetypes.get_mut(&archetype_id)?;

            // Cache archetype length and component storage pointers
            self.current_archetype_len = archetype.len();
            self.current_state = Some(Q::init_state(archetype));
            self.current_entity_idx = 0;
            self.current_archetype_idx += 1;
        }

        Some(())
    }
}

// ============================================================================
// Parallel Iterator
// ============================================================================

/// Parallel iterator for queries using Rayon.
///
/// Supports method chaining for configuration:
/// - `.with_batch_size(n)` - Set minimum batch size
/// - `.tracked()` - Enable batch statistics collection
/// - `.for_each(f)` - Execute closure on each entity
pub struct ParQueryIter<'w, Q: QueryTarget> {
    archetype_ranges: Vec<ArchetypeRange<Q::State>>,
    min_batch_size: Option<usize>,
    tracked: bool,
    _phantom: std::marker::PhantomData<&'w mut Q>,
}

// SAFETY: ParQueryIterMut can be sent between threads because:
// - archetype_ranges contains owned data (Vec) and raw pointers in Q::State
// - The raw pointers in Q::State point to component storage that remains valid
//   for the lifetime of the query (exclusive World access)
// - Each thread accesses different entity indices, so no data races occur
unsafe impl<'w, Q: QueryTarget> Send for ParQueryIter<'w, Q> where Q::State: Send {}
unsafe impl<'w, Q: QueryTarget> Sync for ParQueryIter<'w, Q> where Q::State: Sync {}

impl<'w, Q: QueryTarget> ParQueryIter<'w, Q>
where
    Q::State: Send + Sync,
    for<'a> Q::Item<'a>: Send,
{
    /// Get the number of threads available in Rayon's thread pool
    pub fn num_threads() -> usize {
        rayon::current_num_threads()
    }

    /// Get the total number of entities that will be processed
    pub fn entity_count(&self) -> usize {
        self.archetype_ranges.iter().map(|(_, _, len)| *len).sum()
    }

    /// Set minimum batch size for parallel iteration.
    ///
    /// Larger batches reduce overhead but may cause load imbalance.
    /// Smaller batches improve load balancing but increase overhead.
    ///
    /// Guidelines:
    /// - Light work (simple math): larger batches (1000-10000)
    /// - Heavy work (complex calculations): smaller batches (10-100)
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.min_batch_size = Some(size);
        self
    }

    /// Enable batch statistics tracking.
    pub fn tracked(mut self) -> Self {
        self.tracked = true;
        self
    }

    /// Execute closure on each entity in parallel.
    ///
    /// Returns `BatchStats` if `.tracked()` was called, otherwise `()`.
    pub fn for_each<F>(self, f: F) -> ParForEachResult
    where
        F: Fn(Q::Item<'_>) + Send + Sync,
    {
        if self.tracked {
            ParForEachResult::Tracked(self.execute_tracked(f))
        } else {
            self.execute_untracked(f);
            ParForEachResult::Untracked
        }
    }

    fn execute_untracked<F>(self, f: F)
    where
        F: Fn(Q::Item<'_>) + Send + Sync,
    {
        let min_len = self.min_batch_size.unwrap_or(1);

        self.archetype_ranges
            .into_par_iter()
            .for_each(|(_, state, len)| {
                (0..len)
                    .into_par_iter()
                    .with_min_len(min_len)
                    .for_each(|index| {
                        f(Q::fetch_with_state(&state, index));
                    });
            });
    }

    fn execute_tracked<F>(self, f: F) -> BatchStats
    where
        F: Fn(Q::Item<'_>) + Send + Sync,
    {
        let num_threads = rayon::current_num_threads();
        let total_entities = self.entity_count();
        let min_len = self.min_batch_size.unwrap_or(1);

        let batch_count = Arc::new(AtomicUsize::new(0));
        let min_batch = Arc::new(AtomicUsize::new(usize::MAX));
        let max_batch = Arc::new(AtomicUsize::new(0));

        self.archetype_ranges
            .into_par_iter()
            .for_each(|(_, state, len)| {
                let batch_count = Arc::clone(&batch_count);
                let min_batch = Arc::clone(&min_batch);
                let max_batch = Arc::clone(&max_batch);

                (0..len)
                    .into_par_iter()
                    .with_min_len(min_len)
                    .fold_with(0usize, |count, index| {
                        f(Q::fetch_with_state(&state, index));
                        count + 1
                    })
                    .for_each(|size| {
                        batch_count.fetch_add(1, Ordering::Relaxed);
                        min_batch.fetch_min(size, Ordering::Relaxed);
                        max_batch.fetch_max(size, Ordering::Relaxed);
                    });
            });

        let batch_count = batch_count.load(Ordering::Relaxed);
        let min_batch_size = min_batch.load(Ordering::Relaxed);
        let max_batch_size = max_batch.load(Ordering::Relaxed);

        BatchStats {
            num_threads,
            batch_count,
            total_entities,
            min_batch_size: if batch_count > 0 { min_batch_size } else { 0 },
            max_batch_size,
            avg_batch_size: if batch_count > 0 {
                total_entities as f64 / batch_count as f64
            } else {
                0.0
            },
        }
    }
}

/// Result of parallel for_each execution.
#[derive(Debug, Clone, Default)]
pub enum ParForEachResult {
    #[default]
    Untracked,
    Tracked(BatchStats),
}

impl ParForEachResult {
    /// Get batch stats if tracking was enabled.
    pub fn stats(self) -> Option<BatchStats> {
        match self {
            ParForEachResult::Tracked(stats) => Some(stats),
            ParForEachResult::Untracked => None,
        }
    }

    /// Unwrap batch stats, panicking if not tracked.
    pub fn unwrap(self) -> BatchStats {
        match self {
            ParForEachResult::Tracked(stats) => stats,
            ParForEachResult::Untracked => panic!("for_each was not tracked"),
        }
    }
}

impl From<ParForEachResult> for Option<BatchStats> {
    fn from(result: ParForEachResult) -> Self {
        result.stats()
    }
}

impl std::fmt::Display for ParForEachResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParForEachResult::Tracked(stats) => write!(f, "{}", stats),
            ParForEachResult::Untracked => write!(f, "Untracked"),
        }
    }
}

// ============================================================================
// Resource Queries
// ============================================================================

/// Immutable resource access for systems.
///
/// Use `Res<T>` as a system parameter to read a resource without mutation.
/// The scheduler tracks this as a read and allows multiple systems to
/// read the same resource in parallel.
///
/// # Example
/// ```ignore
/// fn my_system(time: Res<GameTime>) {
///     if let Some(time) = time.get() {
///         println!("Elapsed: {}", time.elapsed);
///     }
/// }
/// ```
pub struct Res<'w, T: Resource> {
    world: &'w World,
    _phantom: std::marker::PhantomData<T>,
}

impl<'w, T: Resource> Res<'w, T> {
    pub fn new(world: &'w World) -> Self {
        Self {
            world,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get immutable reference to the resource.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }
}

/// Mutable resource access for systems.
///
/// Use `ResMut<T>` as a system parameter to read and write a resource.
/// The scheduler tracks this as a write and prevents other systems from
/// accessing the same resource in parallel.
///
/// # Example
/// ```ignore
/// fn my_system(mut time: ResMut<GameTime>) {
///     if let Some(time) = time.get_mut() {
///         time.elapsed += time.delta;
///     }
/// }
/// ```
pub struct ResMut<'w, T: Resource> {
    world: &'w mut World,
    _phantom: std::marker::PhantomData<T>,
}

impl<'w, T: Resource> ResMut<'w, T> {
    pub fn new(world: &'w mut World) -> Self {
        Self {
            world,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Get immutable reference to the resource.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get(&self) -> Option<&T> {
        self.world.get_resource::<T>()
    }

    /// Get mutable reference to the resource.
    ///
    /// Returns `None` if the resource has not been inserted into the World.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.world.get_resource_mut::<T>()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::component::Component;
    use trait_type_map::impl_trait_accessible;

    // Test components
    #[derive(Debug, Clone, PartialEq)]
    struct Position {
        x: f32,
        y: f32,
    }
    impl Component for Position {}

    #[derive(Debug, Clone, PartialEq)]
    struct Velocity {
        x: f32,
        y: f32,
    }
    impl Component for Velocity {}

    #[derive(Debug, Clone, PartialEq)]
    struct Health(i32);
    impl Component for Health {}

    impl_trait_accessible!(dyn Component; Position, Velocity, Health);

    fn setup_world() -> World {
        let mut world = World::new();
        world.register_component::<Position>();
        world.register_component::<Velocity>();
        world.register_component::<Health>();
        world
    }

    // ------------------------------------------------------------------------
    // Basic Query Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_query_empty_world() {
        let mut world = setup_world();
        let mut query = Query::<(&Position,)>::new(&mut world);
        assert_eq!(query.iter_mut().count(), 0);
    }

    #[test]
    fn test_query_single_entity() {
        let mut world = setup_world();
        world
            .create_entity()
            .with(Position { x: 1.0, y: 2.0 })
            .with(Velocity { x: 0.5, y: 0.5 })
            .build();

        let mut query = Query::<(&Position, &Velocity)>::new(&mut world);
        let results: Vec<_> = query.iter_mut().collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.x, 1.0);
        assert_eq!(results[0].1.x, 0.5);
    }

    #[test]
    fn test_query_multiple_entities() {
        let mut world = setup_world();

        for i in 0..10 {
            world
                .create_entity()
                .with(Position {
                    x: i as f32,
                    y: 0.0,
                })
                .build();
        }

        let mut query = Query::<(&Position,)>::new(&mut world);
        assert_eq!(query.iter_mut().count(), 10);
    }

    #[test]
    fn test_query_filters_by_components() {
        let mut world = setup_world();

        // Entity with Position only
        world
            .create_entity()
            .with(Position { x: 1.0, y: 1.0 })
            .build();

        // Entity with Position and Velocity
        world
            .create_entity()
            .with(Position { x: 2.0, y: 2.0 })
            .with(Velocity { x: 1.0, y: 1.0 })
            .build();

        // Query for entities with both Position and Velocity
        let mut query = Query::<(&Position, &Velocity)>::new(&mut world);
        let results: Vec<_> = query.iter_mut().collect();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.x, 2.0);
    }

    #[test]
    fn test_query_mutable_modification() {
        let mut world = setup_world();

        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .with(Velocity { x: 1.0, y: 2.0 })
            .build();

        // Modify position based on velocity
        {
            let mut query = Query::<(&mut Position, &Velocity)>::new(&mut world);
            for (pos, vel) in query.iter_mut() {
                pos.x += vel.x;
                pos.y += vel.y;
            }
        }

        // Verify modification
        let mut query = Query::<(&Position,)>::new(&mut world);
        let (pos,) = query.first().unwrap();
        assert_eq!(pos.x, 1.0);
        assert_eq!(pos.y, 2.0);
    }

    #[test]
    fn test_query_first() {
        let mut world = setup_world();

        world
            .create_entity()
            .with(Position { x: 5.0, y: 5.0 })
            .build();

        let mut query = Query::<(&Position,)>::new(&mut world);
        let first = query.first();

        assert!(first.is_some());
        assert_eq!(first.unwrap().0.x, 5.0);
    }

    #[test]
    fn test_query_first_empty() {
        let mut world = setup_world();
        let mut query = Query::<(&Position,)>::new(&mut world);
        assert!(query.first().is_none());
    }

    #[test]
    fn test_query_entity_access() {
        let mut world = setup_world();

        let entity = world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();

        let mut query = Query::<(Entity, &Position)>::new(&mut world);
        let (queried_entity, _) = query.first().unwrap();

        assert_eq!(queried_entity.id, entity.id);
    }

    // ------------------------------------------------------------------------
    // Parallel Iterator Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_par_iter_basic() {
        let mut world = setup_world();

        for i in 0..100 {
            world
                .create_entity()
                .with(Position {
                    x: i as f32,
                    y: 0.0,
                })
                .build();
        }

        let mut query = Query::<(&mut Position,)>::new(&mut world);
        query.par_iter_mut().for_each(|(pos,)| {
            pos.x += 1.0;
        });

        // Verify all were modified
        let mut verify_query = Query::<(&Position,)>::new(&mut world);
        for (pos,) in verify_query.iter_mut() {
            assert!(pos.x >= 1.0);
        }
    }

    #[test]
    fn test_par_iter_with_batch_size() {
        let mut world = setup_world();

        for _ in 0..1000 {
            world
                .create_entity()
                .with(Position { x: 0.0, y: 0.0 })
                .build();
        }

        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let stats = query
            .par_iter_mut()
            .with_batch_size(100)
            .tracked()
            .for_each(|(pos,)| {
                pos.x = 1.0;
            });

        let stats = stats.unwrap();
        assert_eq!(stats.total_entities, 1000);
        assert!(stats.batch_count > 0);
        assert!(stats.min_batch_size >= 100 || stats.batch_count == 1);
    }

    #[test]
    fn test_par_iter_tracked_stats() {
        let mut world = setup_world();

        for _ in 0..500 {
            world
                .create_entity()
                .with(Position { x: 0.0, y: 0.0 })
                .build();
        }

        let mut query = Query::<(&Position,)>::new(&mut world);
        let result = query.par_iter_mut().tracked().for_each(|_| {});

        match result {
            ParForEachResult::Tracked(stats) => {
                assert_eq!(stats.total_entities, 500);
                assert!(stats.batch_count > 0);
                assert!(stats.num_threads > 0);
                assert!(stats.avg_batch_size > 0.0);
            }
            ParForEachResult::Untracked => panic!("Expected tracked result"),
        }
    }

    #[test]
    fn test_par_iter_untracked() {
        let mut world = setup_world();

        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build();

        let mut query = Query::<(&Position,)>::new(&mut world);
        let result = query.par_iter_mut().for_each(|_| {});

        assert!(matches!(result, ParForEachResult::Untracked));
        assert!(result.stats().is_none());
    }

    #[test]
    fn test_par_iter_entity_count() {
        let mut world = setup_world();

        for _ in 0..250 {
            world
                .create_entity()
                .with(Position { x: 0.0, y: 0.0 })
                .build();
        }

        let mut query = Query::<(&Position,)>::new(&mut world);
        let par_iter = query.par_iter_mut();

        assert_eq!(par_iter.entity_count(), 250);
    }

    // ------------------------------------------------------------------------
    // BatchStats Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_batch_stats_display() {
        let stats = BatchStats {
            num_threads: 8,
            batch_count: 10,
            total_entities: 1000,
            min_batch_size: 90,
            max_batch_size: 110,
            avg_batch_size: 100.0,
        };

        let display = format!("{}", stats);
        assert!(display.contains("threads: 8"));
        assert!(display.contains("batches: 10"));
        assert!(display.contains("entities: 1000"));
    }

    #[test]
    fn test_par_for_each_result_display() {
        let stats = BatchStats {
            num_threads: 4,
            batch_count: 5,
            total_entities: 100,
            min_batch_size: 20,
            max_batch_size: 20,
            avg_batch_size: 20.0,
        };

        let tracked = ParForEachResult::Tracked(stats);
        let untracked = ParForEachResult::Untracked;

        assert!(format!("{}", tracked).contains("threads: 4"));
        assert_eq!(format!("{}", untracked), "Untracked");
    }

    // ------------------------------------------------------------------------
    // QueryTarget Trait Tests
    // ------------------------------------------------------------------------

    #[test]
    fn test_component_ids() {
        let ids = <(&Position, &Velocity)>::component_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&ComponentId::of::<Position>()));
        assert!(ids.contains(&ComponentId::of::<Velocity>()));
    }

    #[test]
    fn test_report_component_access_read() {
        let (reads, writes) = <(&Position,)>::report_component_access();
        assert_eq!(reads.len(), 1);
        assert_eq!(writes.len(), 0);
    }

    #[test]
    fn test_report_component_access_write() {
        let (reads, writes) = <(&mut Position,)>::report_component_access();
        assert_eq!(reads.len(), 0);
        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn test_report_component_access_mixed() {
        let (reads, writes) = <(&Position, &mut Velocity)>::report_component_access();
        assert_eq!(reads.len(), 1);
        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn test_entity_has_no_component_ids() {
        let ids = Entity::component_ids();
        assert!(ids.is_empty());
    }
}
