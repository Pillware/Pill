//! The [`QueryFilter`] trait and the filter types the engine provides for
//! deciding which entities a query yields.
//!
//! # Responsibilities
//!
//! - Defines the [`QueryFilter`] trait that layers row-level predicates on queries.
//! - Implements [`With`] / [`Without`] for archetype-level component scoping.
//! - Implements [`Changed`] / [`Added`] for per-entity change-detection filtering.
//! - Implements [`Or`] for logical-OR composition of multiple filters.
//!
//! # Design
//!
//! Filters produce `(include, exclude)` component-id pairs that the query
//! turns into masks. An archetype matches a pair when it holds every include
//! and none of the excludes. `With<T>` adds `T` to the include set, and
//! `Without<T>` adds `T` to the exclude set. `Changed<T>` and `Added<T>`
//! compare the ticks stored for each row against the baseline the calling
//! system recorded on its last run. `Or` produces several pairs, and an
//! archetype matches it when any one of them passes.

// Current crate
use crate::archetype::Archetype;
use crate::component::{Component, ComponentId, ComponentTicks, Tick};

use super::ptr::SendPtr;

// =============================================================================
// QueryFilter
// =============================================================================

/// Trait for predicates that decide which entities a query yields beyond
/// the component-set match the [`QueryTarget`] already implies.
///
/// A filter answers at two levels:
///
/// 1. Archetype level: [`archetype_filter_pairs`] returns `(include, exclude)`
///    component-id pairs, and an archetype passes when it satisfies any one
///    pair (a filter that returns no pair at all restricts nothing). The
///    default builds one pair from [`included_component_ids`] and
///    [`excluded_component_ids`]; [`Or`] overrides it to return the pairs of
///    every inner filter.
/// 2. Row level: [`init_state`] builds one state per archetype (for the tick
///    filters, a pointer to the column's ticks) and [`matches`] runs for every
///    candidate row. This is how [`Changed`] and [`Added`] work.
///
/// The trivial filter `()` matches every row.
///
/// [`QueryTarget`]: super::QueryTarget
/// [`archetype_filter_pairs`]: QueryFilter::archetype_filter_pairs
/// [`included_component_ids`]: QueryFilter::included_component_ids
/// [`excluded_component_ids`]: QueryFilter::excluded_component_ids
/// [`init_state`]: QueryFilter::init_state
/// [`matches`]: QueryFilter::matches
pub trait QueryFilter {
    /// Per-archetype cached state used by [`Self::matches`].
    type State: Send + Sync;

    /// True when this filter accepts every row unconditionally (e.g. `()`).
    /// Queries read it to skip filter evaluation in the inner loop and to
    /// take their fast paths in `first`, `is_empty` and `entity_count`. It
    /// stays `false` by default because most filters do real work.
    const ACCEPTS_ALL: bool = false;

    /// Components whose storage this filter inspects.
    ///
    /// Two consumers read this list. The default `archetype_filter_pairs`
    /// turns it into the must-contain half of one pair, and a query's access
    /// report declares every listed component as read, so the scheduler never
    /// runs a writer of that component beside this system ([`Or`] overrides
    /// this method to return the union of its branches). The query's
    /// archetype mask is built from `archetype_filter_pairs`, not from this
    /// method.
    fn included_component_ids() -> Vec<ComponentId> {
        Vec::new()
    }

    /// Components that a matching archetype must not contain. An archetype
    /// that holds any of them is excluded.
    fn excluded_component_ids() -> Vec<ComponentId> {
        Vec::new()
    }

    /// Returns `(include_ids, exclude_ids)` pairs that scope which archetypes
    /// this filter accepts.
    ///
    /// One pair means: the archetype must contain all of `include_ids` and
    /// none of `exclude_ids`. An archetype passes the filter when it passes
    /// any one of the pairs, so the pairs combine with OR.
    ///
    /// The default implementation returns one pair built from
    /// [`included_component_ids`] and [`excluded_component_ids`]. It returns
    /// no pair at all when both lists are empty, which the query reads as "no
    /// archetype restriction". [`Or`] overrides this method to return the
    /// pairs of every inner filter, so a branch that accepts every archetype
    /// makes the whole `Or` accept every archetype.
    ///
    /// [`included_component_ids`]: QueryFilter::included_component_ids
    /// [`excluded_component_ids`]: QueryFilter::excluded_component_ids
    /// [`Or`]: Or
    fn archetype_filter_pairs() -> Vec<(Vec<ComponentId>, Vec<ComponentId>)> {
        // Collect this filter's include and exclude component sets.
        let included_component_ids = Self::included_component_ids();
        let excluded_component_ids = Self::excluded_component_ids();
        // A filter with no restrictions lets every archetype through, so it
        // contributes no pair. Any other filter contributes exactly one.
        if included_component_ids.is_empty() && excluded_component_ids.is_empty() {
            Vec::new()
        } else {
            vec![(included_component_ids, excluded_component_ids)]
        }
    }

    /// Builds the state this filter needs to test the rows of one archetype.
    ///
    /// The archetype arrives as a shared reference because that is all a
    /// filter needs: it only reads a column's ticks. The shared borrow also
    /// lets a tuple or [`Or`] build the state of every member in one
    /// expression.
    ///
    /// `last_run` and `this_run` bound the ticks a row must fall between to
    /// count as changed or added.
    fn init_state(archetype: &Archetype, last_run: Tick, this_run: Tick) -> Self::State;

    /// Returns `true` when the row at `index` passes this filter.
    ///
    /// `state` is the state [`init_state`] built for the archetype the row
    /// belongs to.
    fn matches(state: &Self::State, index: usize) -> bool;
}

// =============================================================================
// Trivial Filter: ()
// =============================================================================

/// The unit type acts as a no-op filter that accepts every row.
impl QueryFilter for () {
    type State = ();
    const ACCEPTS_ALL: bool = true;

    fn init_state(_archetype: &Archetype, _last_run: Tick, _this_run: Tick) -> Self::State {}

    #[inline(always)]
    fn matches(_state: &Self::State, _index: usize) -> bool {
        true
    }
}

// =============================================================================
// With
// =============================================================================

/// Archetype filter that yields only the rows whose archetype contains `T`.
///
/// Use it when the query does not need `T`'s data - so `T` does not appear in
/// the [`QueryTarget`](super::QueryTarget) - but iteration should still be
/// limited to the entities that have it.
pub struct With<T: Component>(std::marker::PhantomData<T>);

impl<T: Component> QueryFilter for With<T> {
    type State = ();

    fn included_component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn init_state(_archetype: &Archetype, _last_run: Tick, _this_run: Tick) -> Self::State {}

    #[inline(always)]
    fn matches(_state: &Self::State, _index: usize) -> bool {
        true
    }
}

// =============================================================================
// Without
// =============================================================================

/// Archetype filter that yields only the rows whose archetype does not
/// contain `T`.
pub struct Without<T: Component>(std::marker::PhantomData<T>);

impl<T: Component> QueryFilter for Without<T> {
    type State = ();

    fn excluded_component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn init_state(_archetype: &Archetype, _last_run: Tick, _this_run: Tick) -> Self::State {}

    #[inline(always)]
    fn matches(_state: &Self::State, _index: usize) -> bool {
        true
    }
}

// =============================================================================
// TickFilterState
// =============================================================================

/// Per-archetype state that `Changed<T>` and `Added<T>` use to test one row
/// at a time: a thread-safe pointer to the filtered component's ticks plus
/// the two ticks that bound the search.
///
/// The component can be missing from the archetype when one of those filters
/// sits inside an [`Or`] whose other branch matched. The state then holds no
/// pointer, and every row is rejected.
pub struct TickFilterState {
    /// Raw pointer to the first tick of the filtered component's column;
    /// `None` when the component is absent from the archetype.
    ticks_pointer: Option<SendPtr<ComponentTicks>>,
    /// How many ticks that pointer addresses, for the debug bounds check.
    tick_count: usize,
    /// Tick of the owning system's previous run.
    last_run: Tick,
    /// Tick of the owning system's current run.
    this_run: Tick,
}

impl TickFilterState {
    /// Builds state pointing at the ticks of a column that is present in the
    /// archetype being iterated.
    fn new(column_ticks: &[ComponentTicks], last_run: Tick, this_run: Tick) -> Self {
        Self {
            ticks_pointer: Some(SendPtr::new(column_ticks.as_ptr())),
            tick_count: column_ticks.len(),
            last_run,
            this_run,
        }
    }

    /// State for an archetype that does not hold the component; every row is
    /// then rejected.
    fn missing() -> Self {
        Self {
            ticks_pointer: None,
            tick_count: 0,
            last_run: Tick(0),
            this_run: Tick(0),
        }
    }

    /// Returns `true` when the archetype this state was built for holds the
    /// component.
    #[inline]
    fn is_present(&self) -> bool {
        self.ticks_pointer.is_some()
    }

    /// # Safety
    ///
    /// The caller must have seen [`is_present`](Self::is_present) return
    /// `true`.
    ///
    /// # Panics
    ///
    /// Panics in debug builds when the component is missing, or when `index`
    /// is past the last tick of the column.
    #[inline]
    unsafe fn ticks_at(&self, index: usize) -> &ComponentTicks {
        debug_assert!(self.is_present(), "ticks_at called on missing component");
        // SAFETY: the caller's `# Safety` contract makes the `Option` `Some`
        // before this runs. `new` built the pointer from a `&[ComponentTicks]`
        // that belongs to a column of the archetype, and the query holds the
        // world borrowed for as long as the state exists, so that tick slice
        // is neither mutated nor freed. A column stores one tick per row of
        // its archetype, so `index`, a row index there, is in bounds.
        let ticks = unsafe { self.ticks_pointer.as_ref().unwrap_unchecked().as_ptr() };
        debug_assert!(
            index < self.tick_count,
            "row {index} is out of bounds for {} component ticks",
            self.tick_count
        );
        // SAFETY: `index` is a row index in the archetype this state was
        // built from, and the column holds exactly one tick per row, so the
        // address is a live `ComponentTicks`.
        unsafe { &*ticks.add(index) }
    }
}

// =============================================================================
// Changed
// =============================================================================

/// Row filter that yields only the entities whose `T` changed (or was added)
/// since the system that owns this query last ran.
///
/// It is built on [`ComponentTicks::is_changed`].
pub struct Changed<T: Component>(std::marker::PhantomData<T>);

impl<T: Component> QueryFilter for Changed<T> {
    type State = TickFilterState;

    fn included_component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn init_state(archetype: &Archetype, last_run: Tick, this_run: Tick) -> Self::State {
        match archetype.component_storages.get(ComponentId::of::<T>()) {
            Some(column) => TickFilterState::new(column.ticks(), last_run, this_run),
            // The component is not in this archetype. That happens when
            // `Changed<T>` sits inside an `Or` whose other branch matched the
            // archetype.
            None => TickFilterState::missing(),
        }
    }

    #[inline]
    fn matches(state: &Self::State, index: usize) -> bool {
        // When the archetype lacks the component, no row can match. This
        // happens inside an `Or` whose other branch matched the archetype.
        if !state.is_present() {
            return false;
        }
        // SAFETY: `is_present()` returned `true` in the check above, so
        // `ticks_at`'s documented precondition is satisfied. The returned
        // reference is valid for the duration of the call and is only read.
        unsafe {
            state
                .ticks_at(index)
                .is_changed(state.last_run, state.this_run)
        }
    }
}

// =============================================================================
// Added
// =============================================================================

/// Row filter that yields only the entities whose `T` was added since the
/// system that owns this query last ran.
pub struct Added<T: Component>(std::marker::PhantomData<T>);

impl<T: Component> QueryFilter for Added<T> {
    type State = TickFilterState;

    fn included_component_ids() -> Vec<ComponentId> {
        vec![ComponentId::of::<T>()]
    }

    fn init_state(archetype: &Archetype, last_run: Tick, this_run: Tick) -> Self::State {
        match archetype.component_storages.get(ComponentId::of::<T>()) {
            Some(column) => TickFilterState::new(column.ticks(), last_run, this_run),
            None => TickFilterState::missing(),
        }
    }

    #[inline]
    fn matches(state: &Self::State, index: usize) -> bool {
        // An archetype that does not hold the component yields no row at all.
        // This happens inside an `Or` whose other branch matched.
        if !state.is_present() {
            return false;
        }
        // SAFETY: `is_present()` returned `true` in the check above, so
        // `ticks_at`'s documented precondition is satisfied. The returned
        // reference is valid for the duration of the call and is only read.
        unsafe {
            state
                .ticks_at(index)
                .is_added(state.last_run, state.this_run)
        }
    }
}

// =============================================================================
// Tuple Filters (AND)
// =============================================================================

macro_rules! impl_query_filter_tuple {
    ($($T:ident),*) => {
        impl<$($T: QueryFilter),*> QueryFilter for ($($T,)*) {
            type State = ($($T::State,)*);

            fn included_component_ids() -> Vec<ComponentId> {
                let mut component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                $(component_ids.extend($T::included_component_ids());)*
                component_ids
            }

            fn excluded_component_ids() -> Vec<ComponentId> {
                let mut component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                $(component_ids.extend($T::excluded_component_ids());)*
                component_ids
            }

            /// Combines the inner filters with AND: the result holds every
            /// combination of one pair taken from each inner filter.
            ///
            /// For `(Or<(With<A>, With<B>)>, Without<C>)` this gives
            /// `[({A},{C}), ({B},{C})]`, that is (has A and lacks C) or
            /// (has B and lacks C).
            fn archetype_filter_pairs() -> Vec<(Vec<ComponentId>, Vec<ComponentId>)> {
                let pairs_per_filter = [$($T::archetype_filter_pairs()),*];
                and_filter_pairs(&pairs_per_filter)
            }

            #[allow(non_snake_case)]
            fn init_state(archetype: &Archetype, last_run: Tick, this_run: Tick) -> Self::State {
                ($($T::init_state(archetype, last_run, this_run),)*)
            }

            #[allow(non_snake_case)]
            fn matches(state: &Self::State, index: usize) -> bool {
                let ($($T,)*) = state;
                $(if !$T::matches($T, index) { return false; })*
                true
            }
        }
    };
}

impl_query_filter_tuple!(A);
impl_query_filter_tuple!(A, B);
impl_query_filter_tuple!(A, B, C);
impl_query_filter_tuple!(A, B, C, D);

// =============================================================================
// Or
// =============================================================================

/// A logical OR over a tuple of filters: a row matches when at least one
/// inner filter matches it.
///
/// `Or` applies that OR at both levels. An archetype is kept when it matches
/// the pairs of any branch (see [`archetype_filter_pairs`]), and a row is
/// yielded when any branch accepts it (see [`matches`]).
///
/// [`archetype_filter_pairs`]: QueryFilter::archetype_filter_pairs
/// [`matches`]: QueryFilter::matches
pub struct Or<F>(std::marker::PhantomData<F>);

macro_rules! impl_query_filter_or {
    ($($T:ident),*) => {
        impl<$($T: QueryFilter),*> QueryFilter for Or<($($T,)*)> {
            type State = ($($T::State,)*);

            // This union feeds the query's access report: `matches` may read
            // the storage of any branch, so the scheduler has to treat all of
            // them as read. Archetype scoping ignores the list -
            // `archetype_filter_pairs` keeps an archetype that matches any one
            // branch.
            fn included_component_ids() -> Vec<ComponentId> {
                let mut component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                $(component_ids.extend($T::included_component_ids());)*
                component_ids
            }

            fn excluded_component_ids() -> Vec<ComponentId> {
                let mut component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                $(component_ids.extend($T::excluded_component_ids());)*
                component_ids
            }

            /// Returns the pairs of every inner filter: an archetype matches
            /// the `Or` when it matches any branch.
            ///
            /// A filter that returns no pairs accepts every archetype. If any
            /// branch is like that, the `Or` returns no pairs either, so it
            /// accepts every archetype too.
            fn archetype_filter_pairs() -> Vec<(Vec<ComponentId>, Vec<ComponentId>)> {
                let mut filter_pairs = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_FILTER_PAIRS_CAPACITY);
                $(
                    let inner_filter_pairs = $T::archetype_filter_pairs();
                    if inner_filter_pairs.is_empty() {
                        // This branch restricts nothing, so the whole `Or`
                        // restricts nothing either.
                        return Vec::new();
                    }
                    filter_pairs.extend(inner_filter_pairs);
                )*
                filter_pairs
            }

            #[allow(non_snake_case)]
            fn init_state(archetype: &Archetype, last_run: Tick, this_run: Tick) -> Self::State {
                ($($T::init_state(archetype, last_run, this_run),)*)
            }

            #[allow(non_snake_case)]
            fn matches(state: &Self::State, index: usize) -> bool {
                let ($($T,)*) = state;
                $(if $T::matches($T, index) { return true; })*
                false
            }
        }
    };
}

impl_query_filter_or!(A, B);
impl_query_filter_or!(A, B, C);
impl_query_filter_or!(A, B, C, D);

// =============================================================================
// Free Functions
// =============================================================================

/// Combines the filter pairs of several filters with AND.
///
/// Each inner slice holds the pairs of one filter. The result holds every
/// combination of one pair per filter, with the include and exclude lists
/// merged. For two filters that is the Cartesian product of their pairs.
///
/// A filter with no pairs restricts nothing, so it is skipped and adds no
/// combination of its own.
fn and_filter_pairs(
    pairs_per_filter: &[Vec<(Vec<ComponentId>, Vec<ComponentId>)>],
) -> Vec<(Vec<ComponentId>, Vec<ComponentId>)> {
    // Step 1: start from one pair that restricts nothing, so the loop below
    // has a neutral starting value.
    let mut cross_product_pairs: Vec<(Vec<ComponentId>, Vec<ComponentId>)> =
        vec![(Vec::new(), Vec::new())];

    // Step 2: for each filter, pair every combination built so far with each
    // of its pairs, merging the include and exclude lists.
    for filter_pairs in pairs_per_filter {
        if filter_pairs.is_empty() {
            // This filter restricts nothing, so it adds no combination.
            continue;
        }
        let mut merged_pairs = Vec::with_capacity(cross_product_pairs.len() * filter_pairs.len());
        for (included_component_ids, excluded_component_ids) in &cross_product_pairs {
            for (filter_included_component_ids, filter_excluded_component_ids) in filter_pairs {
                let mut merged_included_component_ids = included_component_ids.clone();
                let mut merged_excluded_component_ids = excluded_component_ids.clone();
                merged_included_component_ids.extend_from_slice(filter_included_component_ids);
                merged_excluded_component_ids.extend_from_slice(filter_excluded_component_ids);
                merged_pairs.push((merged_included_component_ids, merged_excluded_component_ids));
            }
        }
        cross_product_pairs = merged_pairs;
    }
    cross_product_pairs
}
