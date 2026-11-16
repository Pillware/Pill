//! [`QueryTarget`] trait and built-in implementations for query data shapes.
//!
//! # Responsibilities
//!
//! - Defines the [`QueryTarget`] trait that determines what data a query yields per row.
//! - Implements `QueryTarget` for `&T`, `&mut T`, [`Entity`], and tuples of these.
//! - Detects an aliasing target - two writes of one component, or a read beside
//!   a write - at construction time.
//!
//! # Design
//!
//! Each implementation reports the components it reads and the components it
//! writes. A query builds its component mask from that report, and a system
//! declares the same access to the scheduler. The mask and the declared access
//! then cannot disagree about what the target touches.
//!
//! An implementation also builds the state a query keeps for one archetype and
//! reads one row out of it by index. That state holds raw pointers into
//! component storage, so the parallel iterator can share it only when it is
//! `Send + Sync`.

// Current crate
use super::change_detection::Mut;
use super::ptr::{SendPtr, SendPtrMut};
use crate::archetype::Archetype;
use crate::component::{Component, ComponentId, ComponentTicks, Tick};
use crate::entity::Entity;

// =============================================================================
// QueryTarget
// =============================================================================

/// Trait for fetching components from archetypes.
///
/// Implemented for:
/// - [`Entity`]: Access to entity IDs
/// - `&T`: Immutable component reference
/// - `&mut T`: Mutable component reference (yielded as [`Mut<T>`])
/// - Tuples up to arity 5
///
/// A query keeps one [`State`](QueryTarget::State) per archetype it matches.
/// [`init_state`] builds that state once for an archetype, usually by caching
/// a raw pointer into the archetype's storage.
/// [`fetch_with_state`](QueryTarget::fetch_with_state) then reads one row out
/// of it.
///
/// [`init_state`]: QueryTarget::init_state
pub trait QueryTarget {
    /// The type yielded per row by this query target.
    type Item<'a>;
    /// The state a query keeps for one matching archetype, usually a pointer
    /// into that archetype's storage.
    type State;

    /// Returns the component ids this target reads, followed by the ids it
    /// writes.
    ///
    /// The default takes both lists from
    /// [`report_component_access`](QueryTarget::report_component_access)
    /// rather than have each implementation spell the ids out a second time.
    /// The mask a query builds and the access a system reports then come from
    /// one description, so they cannot disagree.
    ///
    /// # Panics
    ///
    /// Panics for a tuple target that reads and writes the same component,
    /// because `report_component_access` refuses it there.
    fn component_ids() -> Vec<ComponentId> {
        let (read_component_ids, write_component_ids) = Self::report_component_access();
        read_component_ids
            .into_iter()
            .chain(write_component_ids)
            .collect()
    }

    /// Reports which components this target reads and which it writes, for
    /// system dependency analysis.
    ///
    /// The first vector holds the ids read, the second the ids written.
    ///
    /// # Panics
    ///
    /// A tuple target panics here when it reads and writes the same component,
    /// or writes one twice, because iterating it would create overlapping
    /// borrows of one row. The check runs in every build, not only in debug
    /// builds.
    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>);

    /// Reports the shared components this target reads or writes, each with
    /// the schema hash (a hash of the field layout) that this binary's type
    /// declares for it.
    ///
    /// [`crate::Query::new`] compares each hash with the one the component
    /// registry holds. A mismatch means this binary was built against a
    /// different layout of the component than the one registered now. Reading
    /// the current rows through its field offsets would then be undefined
    /// behavior, so the query matches nothing instead.
    ///
    /// Components that are not shared, and components whose type declares no
    /// layout, are left out because there is no hash to compare. For those,
    /// the component's identity or its size and alignment is the only check
    /// there is. The default reports none.
    fn declared_shared_schemas() -> Vec<(ComponentId, u64)> {
        Vec::new()
    }

    /// Builds the state this target keeps for one archetype, usually by
    /// caching a pointer into the archetype's storage.
    ///
    /// `this_run` is the tick of the current run. Mutable targets store it so
    /// a write through [`Mut<T>`] can record when it happened. Read-only
    /// targets ignore it.
    fn init_state(archetype: &mut Archetype, this_run: Tick) -> Self::State;

    /// Returns the row at `index`, read through the state [`init_state`] built.
    ///
    /// The sequential and the parallel iterator both call this, so a target
    /// keeps one fetch implementation for the two paths.
    ///
    /// [`init_state`]: QueryTarget::init_state
    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a>;
}

// =============================================================================
// Entity
// =============================================================================

/// Allows queries to include `Entity` in the data tuple, e.g.
/// `Query<(Entity, &Transform)>` to get the entity along with its components.
impl QueryTarget for Entity {
    type Item<'a> = Entity;
    type State = SendPtr<Vec<Entity>>;

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (Vec::new(), Vec::new())
    }

    fn init_state(archetype: &mut Archetype, _this_run: Tick) -> Self::State {
        let _zone = crate::profile_scope!(
            "create state for entity query target",
            [("Entities in this archetype: {}", archetype.entity_count())]
        );
        SendPtr::new(&archetype.entities as *const Vec<Entity>)
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        // SAFETY: The query loop only calls this with `index` below
        // `archetype.len()`, which is the length of its entity vector, so the
        // unchecked index is in bounds. `init_state` captured the pointer from
        // `&archetype.entities`, and the vector is not reallocated while a
        // query iterates, so the reference is valid for `'a`. `Entity` is
        // `Copy`, so returning a copy creates no second reference to the row.
        unsafe { *(&*state.as_ptr()).get_unchecked(index) }
    }
}

// =============================================================================
// &T (Immutable Reference)
// =============================================================================

/// Allows queries to include immutable component references, e.g.
/// `Query<(&Transform, &Velocity)>`. A read yields a plain `&T` and is
/// reported as a read for system dependency analysis.
///
/// The `T: Sync` bound is what makes the shared reference sound. Two systems
/// that only read a component do not conflict, so the scheduler may run them
/// in the same parallel batch, and each thread then holds a `&T` into the same
/// column. A type with interior mutability would then be read from two
/// threads at once, which `Sync` rules out.
impl<T: Component + Sync> QueryTarget for &T {
    type Item<'a> = &'a T;
    type State = SendPtr<crate::archetype::ComponentColumn>;

    fn declared_shared_schemas() -> Vec<(ComponentId, u64)> {
        declared_shared_schema::<T>().into_iter().collect()
    }

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (vec![ComponentId::of::<T>()], Vec::new())
    }

    fn init_state(archetype: &mut Archetype, _this_run: Tick) -> Self::State {
        let _zone = crate::profile_scope!(
            "create state for immutable component query target",
            [
                ("Entities in archetype: {}", archetype.entity_count()),
                ("Component type: {}", std::any::type_name::<T>())
            ]
        );
        SendPtr::new(archetype.component_storages.column_of::<T>()
            as *const crate::archetype::ComponentColumn)
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        // SAFETY: The query loop only calls this with `index` below
        // `archetype.len()`. Every column in an archetype holds one value per
        // entity, so the index is in bounds for this column too.
        unsafe { (*state.as_ptr()).get_unchecked::<T>(index) }
    }
}

// =============================================================================
// ReadOnlyQueryTarget
// =============================================================================

/// A query target that only ever reads, so a
/// [`run_if`](crate::SystemOptions::run_if) condition may query it.
///
/// The tuples whose members all implement this trait implement it themselves,
/// so `(Entity, &Transform)` is a valid condition parameter. `&mut T` has no
/// implementation on purpose: a condition that could change the world it gates
/// would make the skip decision depend on the order it ran in.
#[diagnostic::on_unimplemented(
    message = "a `run_if` condition may only read, and `{Self}` writes through the world",
    label = "this query target writes",
    note = "take `Res<T>`, `&T` or `Entity`, and leave writing to the system the condition gates"
)]
pub trait ReadOnlyQueryTarget: QueryTarget {}

impl ReadOnlyQueryTarget for Entity {}

impl<T: Component + Sync> ReadOnlyQueryTarget for &T {}

/// Implements [`ReadOnlyQueryTarget`] for tuples whose members all read, so a
/// condition can query several things at once.
macro_rules! impl_read_only_query_target_tuple {
    ($($T:ident),*) => {
        impl<$($T: ReadOnlyQueryTarget),*> ReadOnlyQueryTarget for ($($T,)*) {}
    };
}

impl_read_only_query_target_tuple!(A);
impl_read_only_query_target_tuple!(A, B);
impl_read_only_query_target_tuple!(A, B, C);
impl_read_only_query_target_tuple!(A, B, C, D);
impl_read_only_query_target_tuple!(A, B, C, D, E);

// =============================================================================
// MutFetchState
// =============================================================================/// The state a mutable component query keeps for one archetype. It holds a
/// pointer to the component column, so a row fetch does not look the column up
/// again.
pub struct MutFetchState<T: Component> {
    /// Pointer to the component column. The column owns both the row values
    /// and their change-detection ticks, so this one pointer reaches the value
    /// and its metadata.
    component_column: SendPtrMut<crate::archetype::ComponentColumn>,
    /// The tick of the current run. Every `Mut<T>` this state yields copies
    /// it, so a write can record when it happened.
    this_run: Tick,
    /// Makes the state carry the component type it was built for.
    _marker: std::marker::PhantomData<T>,
}

// SAFETY: The cached pointer names a component column that outlives the
// query, and no other thread mutates it while this state is in use.
//
// Two systems in one parallel batch touch disjoint components.
// `SystemAccess::conflicts_with` compares the component and resource sets the
// two systems declared, and the scheduler never puts two systems that conflict
// in the same batch. Two systems writing one component therefore never run
// together.
//
// Inside a batch, `ParQueryIter` gives each thread its own ranges of rows, so
// no two threads fetch the same row.
unsafe impl<T: Component> Send for MutFetchState<T> {}
// SAFETY: Code that shares the state only reads the cached pointer and the
// run tick. The mutation a `Mut<T>` performs stays exclusive by the same
// disjoint-row guarantee the `Send` impl above explains.
unsafe impl<T: Component> Sync for MutFetchState<T> {}

// =============================================================================
// &mut T (Mutable Reference)
// =============================================================================

/// Allows queries to include mutable component references, e.g.
/// `Query<&mut Velocity>`. A write yields [`Mut<T>`], which records the change,
/// and is reported as a write for system dependency analysis.
impl<T: Component> QueryTarget for &mut T {
    type Item<'a> = Mut<'a, T>;
    type State = MutFetchState<T>;

    fn declared_shared_schemas() -> Vec<(ComponentId, u64)> {
        declared_shared_schema::<T>().into_iter().collect()
    }

    fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
        (Vec::new(), vec![ComponentId::of::<T>()])
    }

    fn init_state(archetype: &mut Archetype, this_run: Tick) -> Self::State {
        let _zone = crate::profile_scope!(
            "create state for mutable component query target",
            [
                ("Entities in archetype: {}", archetype.entity_count()),
                ("Component type (mutable): {}", std::any::type_name::<T>())
            ]
        );
        let component_column = SendPtrMut::new(archetype.component_storages.column_of_mut::<T>()
            as *mut crate::archetype::ComponentColumn);
        MutFetchState {
            component_column,
            this_run,
            _marker: std::marker::PhantomData,
        }
    }

    fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
        // SAFETY: The scheduler never runs two systems that write one
        // component in the same batch, and inside a batch each row belongs to
        // one thread. This is therefore the only borrow of the row.
        //
        // The cached pointer stays valid for the whole iteration. The query
        // holds the world exclusively, and nothing reallocates the column while
        // it runs.
        //
        // The query loop only calls this with `index` below
        // `archetype.len()`, and a column holds one row per entity, so the
        // unchecked index is in bounds. The row values and the ticks live in
        // two allocations owned by the column, so the two mutable references
        // do not overlap. A write through `Mut::deref_mut` updates only this
        // row's `changed` tick, which is why it needs no atomics.
        unsafe {
            let column = state.component_column.as_ptr();
            let component_value: &'a mut T = (*column).get_mut_unchecked::<T>(index);
            let row_ticks: &'a mut ComponentTicks = (*column).row_ticks_mut_unchecked(index);
            Mut::new(component_value, row_ticks, state.this_run)
        }
    }
}

// =============================================================================
// Tuple Implementations
// =============================================================================

/// Implements `QueryTarget` for tuples up to arity 5, e.g.
/// `(Entity, &Transform, &mut Velocity)`.
macro_rules! impl_query_target_tuple {
    ($($T:ident),*) => {
        impl<$($T: QueryTarget),*> QueryTarget for ($($T,)*) {
            type Item<'a> = ($($T::Item<'a>,)*);
            type State = ($($T::State,)*);

            fn declared_shared_schemas() -> Vec<(ComponentId, u64)> {
                let mut schemas = Vec::new();
                $(schemas.extend($T::declared_shared_schemas());)*
                schemas
            }

            fn report_component_access() -> (Vec<ComponentId>, Vec<ComponentId>) {
                let mut read_component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                let mut write_component_ids = Vec::with_capacity(crate::config::QueryConfig::DEFAULT_TUPLE_COMPONENT_IDS_CAPACITY);
                $(
                    let (element_read_ids, element_write_ids) = $T::report_component_access();
                    read_component_ids.extend(element_read_ids);
                    write_component_ids.extend(element_write_ids);
                )*

                // A tuple that aliases one component - two writes, or a read
                // beside a write - hands out two borrows of the same row. That
                // is undefined behavior as soon as iteration starts. The check
                // runs in every build, not only in debug builds, so a release
                // build cannot iterate an aliasing tuple either.
                assert!(
                    $crate::query::target::find_aliasing_component(&read_component_ids, &write_component_ids).is_none(),
                    "query tuple reads and writes the same component; \
                     Query<(&mut T, &mut T)> and Query<(&T, &mut T)> are not allowed"
                );
                (read_component_ids, write_component_ids)
            }

            #[allow(non_snake_case)]
            fn init_state(archetype: &mut Archetype, this_run: Tick) -> Self::State {
                let _zone = crate::profile_scope!(
                    "create state for tuple query target",
                    [("Entities in archetype: {}", archetype.entity_count())]
                );
                let archetype_ptr = archetype as *mut Archetype;
                // SAFETY: Every element gets its own `&mut Archetype` derived
                // from the same raw pointer, and uses it only inside its own
                // `init_state` call, so the mutable borrows never overlap. The
                // archetype came in as a `&mut` that outlives this call, so the
                // pointer is valid for every element. `init_state` only reads
                // storage pointers out of the archetype and never adds or
                // reallocates a column, so the references it hands back stay
                // valid.
                unsafe { ($($T::init_state(&mut *archetype_ptr, this_run),)*) }
            }

            #[allow(non_snake_case)]
            fn fetch_with_state<'a>(state: &Self::State, index: usize) -> Self::Item<'a> {
                let ($($T,)*) = state;
                ($($T::fetch_with_state($T, index),)*)
            }
        }
    };
}

impl_query_target_tuple!(A);
impl_query_target_tuple!(A, B);
impl_query_target_tuple!(A, B, C);
impl_query_target_tuple!(A, B, C, D);
impl_query_target_tuple!(A, B, C, D, E);

// =============================================================================
// Free Functions
// =============================================================================

/// Returns `T`'s component id together with the schema hash `T` declares, or
/// `None` when `T` is not shared or declares no layout.
///
/// This is the single-type half of [`QueryTarget::declared_shared_schemas`].
fn declared_shared_schema<T: Component>() -> Option<(ComponentId, u64)> {
    T::shared_name()?;
    Some((ComponentId::of::<T>(), T::declared_schema_hash()?))
}

/// Returns the first component that a target writes twice or both reads and
/// writes, or `None` when nothing is aliased.
///
/// Such a component produces overlapping `&` and `&mut` borrows of one row,
/// which is undefined behavior. Two shapes are caught: writing one component
/// twice (`Query<(&mut T, &mut T)>`) and reading one while also writing it
/// (`Query<(&T, &mut T)>`).
#[inline]
pub(crate) fn find_aliasing_component(
    read_component_ids: &[ComponentId],
    write_component_ids: &[ComponentId],
) -> Option<ComponentId> {
    // Small-N linear scan: a query tuple names a handful of components, so a
    // quadratic scan over them is cheaper than building a HashSet on the
    // construction path.
    for (index, component_id) in write_component_ids.iter().enumerate() {
        if write_component_ids[index + 1..].contains(component_id)
            || read_component_ids.contains(component_id)
        {
            return Some(*component_id);
        }
    }
    None
}
