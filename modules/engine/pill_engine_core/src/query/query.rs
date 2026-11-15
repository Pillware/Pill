//! The [`Query`] entry point - holds an exclusive borrow of the [`World`]
//! and produces sequential or parallel iterators.
//!
//! # Responsibilities
//!
//! - Builds a query from a `&mut World` and caches the archetypes it matches.
//! - Builds a [`ComponentMask`] for the target and one mask pair per filter
//!   branch, so an archetype that cannot match is skipped.
//! - Delegates iteration to [`QueryIterMut`] (sequential) or [`ParQueryIter`]
//!   (parallel).
//!
//! # Design
//!
//! The query caches the ids of the archetypes it matches, together with the
//! world's archetype generation at the time of the scan. A later call to
//! [`iter_mut`](Query::iter_mut) reuses that list while the generation is
//! unchanged, and scans again only after archetypes were added or removed.

// Current crate
use crate::archetype::{Archetype, ArchetypeId};
use crate::component::{ComponentId, ComponentMask};
use crate::world::World;

use super::filter::QueryFilter;
use super::iter::{ParQueryIter, QueryIterMut};
use super::target::QueryTarget;
use super::FilteredArchetypeRange;

// =============================================================================
// Query
// =============================================================================

/// A query over the entities that hold a matching set of components.
///
/// `Q` is the data fetched for each row, for example
/// `(Entity, &Transform, &mut Velocity)`. `F` is an optional row filter; the
/// default, `()`, accepts every row.
///
/// # Examples
/// ```no_run
/// # use pill_engine::*;
/// # #[derive(Debug, Clone)] struct Transform { x: f32, y: f32 }
/// # impl Component for Transform {}
/// # #[derive(Debug, Clone)] struct Velocity { vx: f32, vy: f32 }
/// # impl Component for Velocity {}
/// # #[derive(Debug, Clone)] struct Health(f32);
/// # impl Component for Health {}
/// # #[derive(Debug, Clone)] struct Enemy;
/// # impl Component for Enemy {}
/// # #[derive(Debug, Clone)] struct Frozen;
/// # impl Component for Frozen {}
/// // Plain query - data only
/// fn movement(mut q: Query<(&mut Transform, &Velocity)>) { /* ... */ }
///
/// // Filter on change detection
/// fn react_to_moves(mut q: Query<(Entity, &Transform), Changed<Transform>>) {
///     for (e, t) in q.iter_mut() {
///         println!("entity {:?} moved to {:?}", e.id(), (t.x, t.y));
///     }
/// }
///
/// // With/Without to scope the iteration
/// fn enemies_only(mut q: Query<&Health, (With<Enemy>, Without<Frozen>)>) {
///     /* ... */
/// }
/// ```
pub struct Query<'w, Q: QueryTarget, F: QueryFilter = ()> {
    /// The world, borrowed exclusively for as long as this query lives.
    world: &'w mut World,
    /// Mask of the components the target fetches. Built once when the query is
    /// constructed, so it is the same for every query with this `Q`.
    target_mask: ComponentMask,
    /// One `(include, exclude)` mask pair per branch of the filter, built once
    /// at construction. A filter with no restrictions produces no pair, an
    /// ordinary filter produces one, and [`Or`](crate::query::Or) produces one
    /// per branch.
    filter_pairs: Vec<(ComponentMask, ComponentMask)>,
    /// The first target id the component registry could not resolve, if any.
    ///
    /// A component with no registered bit cannot be fetched, so the query
    /// matches no archetype at all. Leaving the bit out of the mask instead
    /// would make the mask less restrictive: an unregistered component would
    /// match archetypes that cannot serve it, and iteration would then panic in
    /// the column lookup.
    unresolved_target: Option<ComponentId>,
    /// A shared component whose layout this binary declares differently from
    /// the one registered now (see `World::find_stale_shared_reader`).
    ///
    /// Reading it would apply this binary's field offsets to rows stored with
    /// the other layout, so the query matches nothing, exactly like an
    /// unresolved target. The world logs the refusal once per component and
    /// declared layout.
    stale_shared_component: Option<ComponentId>,
    /// Whether every filter pair was dropped because its include side held an
    /// unresolved id. No archetype can satisfy a dropped pair, and dropping
    /// them all would leave no restriction at all - `With<T>` for an
    /// unregistered `T` would then match everything - so the query matches
    /// nothing instead.
    filters_unsatisfiable: bool,
    /// Total size in bytes of the components the target fetches, but never
    /// less than 8. It sets the entity count of one parallel slice.
    total_components_size_bytes: usize,
    /// Ids of the archetypes this query matches. Valid while the world's
    /// archetype generation still equals `cached_generation`.
    cached_matches: Vec<ArchetypeId>,
    /// The world's archetype generation at the last refresh of
    /// `cached_matches`, or `None` before the first refresh. The cache is stale
    /// whenever this differs from the world's generation.
    ///
    /// `None` rather than `0` as the "never filled" state, because `0` is also
    /// the generation a fresh world reports.
    cached_generation: Option<u64>,
    /// Keeps the `Q` and `F` type parameters in the struct, which is what
    /// variance and the drop check look at.
    _phantom: std::marker::PhantomData<(Q, F)>,
}

impl<'w, Q: QueryTarget, F: QueryFilter> Query<'w, Q, F> {
    /// Builds a query over the given world, computing the target and filter
    /// masks once here.
    ///
    /// The world stays borrowed exclusively for the query's whole lifetime, so
    /// no other system can mutate it while the query exists. The list of
    /// matching archetypes is built on the first lookup and cached after that
    /// (see `matching_archetype_ids`).
    ///
    /// # Panics
    ///
    /// Panics when the target reads and writes the same component, because
    /// iterating such a query would create two overlapping borrows of one row.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pill_engine::*;
    /// # #[derive(Debug, Clone)] struct Position { x: f32, y: f32 }
    /// # impl Component for Position {}
    /// # #[derive(Debug, Clone)] struct Velocity { x: f32, y: f32 }
    /// # impl Component for Velocity {}
    /// # let mut world = World::new();
    /// # world.register_component::<Position>();
    /// # world.register_component::<Velocity>();
    /// let mut query = Query::<(&Position, &Velocity)>::new(&mut world);
    /// assert_eq!(query.iter_mut().count(), 0);
    /// ```
    pub fn new(world: &'w mut World) -> Self {
        // Step 1: Describe the target once. The aliasing check, the target
        // mask, the byte cost of a row and the profile label all read this one
        // description, so they cannot disagree about what the query touches.
        let (read_component_ids, write_component_ids) = Q::report_component_access();
        let component_ids: Vec<ComponentId> = read_component_ids
            .iter()
            .chain(write_component_ids.iter())
            .copied()
            .collect();
        let requested_filter_pairs = F::archetype_filter_pairs();

        // Step 2: Open a Tracy profiling zone around the rest of the
        // construction, so this time shows up in the profiler.
        let _zone = crate::profile_scope!(
            "create query",
            [
                ("Query target component types: {}", component_ids.len()),
                (
                    "Filter archetype mask pairs: {}",
                    requested_filter_pairs.len()
                )
            ]
        );

        // Step 3: Refuse a target that reads and writes the same component.
        // The tuple targets already assert this themselves in
        // `report_component_access`; the check here also covers any other
        // target that reports an aliasing set. Iterating such a query would
        // create two overlapping borrows of one row, which is undefined
        // behavior.
        if let Some(aliasing_component_id) =
            crate::query::target::find_aliasing_component(&read_component_ids, &write_component_ids)
        {
            panic!(
                "query target `{}` reads and writes the same component (`{aliasing_component_id:?}`); \
                 an aliasing query is undefined behavior",
                std::any::type_name::<Q>()
            );
        }

        // Step 4: Build the target mask and the filter mask pairs from the
        // component registry. A target id the registry cannot resolve is
        // recorded, not skipped: skipping would drop the requirement and let
        // archetypes that cannot serve the target match, which panics once
        // iteration reaches a column that is not there.
        let (target_mask, unresolved_target) = match Self::build_target_mask(world, &component_ids)
        {
            Ok(mask) => (mask, None),
            Err(component_id) => (ComponentMask::empty(), Some(component_id)),
        };
        let (filter_pairs, filters_unsatisfiable) =
            Self::build_filter_mask_pairs(world, requested_filter_pairs);

        // Step 5: Refuse a stale reader of a shared component. This binary was
        // built against a field layout the registry does not hold, so reading
        // such a component would use the wrong offsets. Checked once here per
        // query, not once per row.
        let declared_shared_schemas = Q::declared_shared_schemas();
        let stale_shared_component = if declared_shared_schemas.is_empty() {
            None
        } else {
            world.find_stale_shared_reader(&declared_shared_schemas, std::any::type_name::<Q>())
        };

        // Step 6: Sum the sizes of the fetched component types, but never go
        // below 8 bytes. `default_entities_per_slice` divides by this value, so
        // the floor both avoids a division by zero and keeps the bytes per
        // slice bounded. The sizes come from the storage factories that build
        // the columns, which is where a component's layout is recorded.
        let total_components_size_bytes = component_ids
            .iter()
            .filter_map(|id| world.component_layout(*id).map(|(size, _)| size))
            .sum::<usize>()
            .max(8);

        // Step 7: Build the query with empty caches; the first lookup of
        // matching archetypes fills them.
        Self {
            world,
            target_mask,
            filter_pairs,
            unresolved_target,
            stale_shared_component,
            filters_unsatisfiable,
            total_components_size_bytes,
            cached_matches: Vec::new(),
            cached_generation: None,
            _phantom: std::marker::PhantomData,
        }
    }

    /// The shared component this query refused, because this binary declares a
    /// different field layout than the one registered, if any.
    ///
    /// A refused query matches nothing; this returns the component that caused
    /// it.
    pub fn stale_shared_component(&self) -> Option<ComponentId> {
        self.stale_shared_component
    }

    /// Builds the target mask from the world's component registry.
    ///
    /// Called once from [`new`](Self::new). A target id with no registered bit
    /// cannot be fetched, and the caller turns that into a query that matches
    /// no archetype.
    ///
    /// # Errors
    ///
    /// Returns the first target component id the registry has no bit for.
    fn build_target_mask(
        world: &World,
        component_ids: &[ComponentId],
    ) -> Result<ComponentMask, ComponentId> {
        let mut mask = ComponentMask::empty();
        for component_id in component_ids {
            match world.component_registry.get_bit(component_id) {
                Some(bit) => mask.set(bit),
                None => return Err(*component_id),
            }
        }
        Ok(mask)
    }

    /// Builds the filter mask pairs from the world's component registry.
    ///
    /// Called once from [`new`](Self::new) with the pairs the filter reported.
    ///
    /// Returns the usable pairs, plus whether every requested pair was
    /// dropped. A pair is dropped when an id on its include side is not
    /// registered: no archetype can hold a component that was never
    /// registered. An unresolved id on the exclude side is harmless, because
    /// no archetype can hold that component either.
    fn build_filter_mask_pairs(
        world: &World,
        requested_filter_pairs: Vec<(Vec<ComponentId>, Vec<ComponentId>)>,
    ) -> (Vec<(ComponentMask, ComponentMask)>, bool) {
        let _zone = crate::profile_scope!(
            "build component query filter mask",
            [(
                "Filter archetype mask pairs to evaluate: {}",
                requested_filter_pairs.len()
            )]
        );

        let registry = &world.component_registry;
        let requested_pair_count = requested_filter_pairs.len();
        let mut unsatisfiable_pair_count = 0;
        let pairs: Vec<(ComponentMask, ComponentMask)> = requested_filter_pairs
            .into_iter()
            .filter_map(|(included_ids, excluded_ids)| {
                let mut included_mask = ComponentMask::empty();
                for component_id in &included_ids {
                    match registry.get_bit(component_id) {
                        Some(bit) => included_mask.set(bit),
                        None => {
                            unsatisfiable_pair_count += 1;
                            return None;
                        }
                    }
                }
                let mut excluded_mask = ComponentMask::empty();
                for component_id in &excluded_ids {
                    if let Some(bit) = registry.get_bit(component_id) {
                        excluded_mask.set(bit);
                    }
                }
                Some((included_mask, excluded_mask))
            })
            .collect();
        let nothing_matches =
            requested_pair_count != 0 && unsatisfiable_pair_count == requested_pair_count;
        (pairs, nothing_matches)
    }

    /// Returns `true` when `archetype` satisfies this query's target mask and
    /// filter.
    ///
    /// The target mask must match entirely: the archetype has to hold every
    /// component the query fetches. A filter pair requires all of its include
    /// bits and none of its exclude bits, and the archetype passes when it
    /// satisfies any one pair. Only [`Or`](crate::query::Or) produces more than
    /// one pair, one per branch, so `Or<(With<A>, With<B>)>` becomes two pairs.
    fn archetype_matches(
        archetype: &Archetype,
        target_mask: &ComponentMask,
        filter_pairs: &[(ComponentMask, ComponentMask)],
    ) -> bool {
        // The archetype must hold every component the query fetches.
        if !archetype.matches_mask(target_mask) {
            return false;
        }
        match filter_pairs.len() {
            // No filter pair means no restriction: the target mask alone
            // decides.
            0 => true,

            // The common case: one include/exclude pair.
            1 => {
                let (included_mask, excluded_mask) = &filter_pairs[0];
                archetype.matches_mask(included_mask)
                    && (excluded_mask.is_empty()
                        || ComponentMask::intersection(&archetype.component_mask, excluded_mask)
                            .is_empty())
            }

            // More than one pair, which only `Or` produces: any matching pair
            // is enough.
            _ => filter_pairs.iter().any(|(included_mask, excluded_mask)| {
                archetype.matches_mask(included_mask)
                    && (excluded_mask.is_empty()
                        || ComponentMask::intersection(&archetype.component_mask, excluded_mask)
                            .is_empty())
            }),
        }
    }

    /// Ids of the archetypes that match this query.
    ///
    /// The list is cached together with the world's archetype generation, so it
    /// is scanned again only when archetypes were added or removed since the
    /// last call.
    #[inline]
    fn matching_archetype_ids(&mut self) -> &[ArchetypeId] {
        // Step 1: An unresolved target, a stale shared component or an
        // unsatisfiable filter means the answer is empty whatever the world
        // holds. Returning early also keeps a stale cache from being refreshed
        // into the result.
        if self.unresolved_target.is_some()
            || self.stale_shared_component.is_some()
            || self.filters_unsatisfiable
        {
            return &[];
        }

        // Step 2: Keep the cached list while the world's archetype generation
        // is unchanged since the last fill; scan again when it moved.
        if self.cached_generation != Some(self.world.storage.archetype_generation) {
            // Step 3: Scan every archetype and keep the ones that match the
            // target mask and the filter pairs.
            let _zone = crate::profile_scope!(
                "find matching archetypes",
                [
                    (
                        "All archetypes in world: {}",
                        self.world.storage.archetypes.len()
                    ),
                    ("Cached archetype generation: {:?}", self.cached_generation),
                    (
                        "Current archetype generation: {}",
                        self.world.storage.archetype_generation
                    )
                ]
            );
            let mut matching_archetype_ids: Vec<ArchetypeId> = self
                .world
                .storage
                .archetypes
                .iter()
                .filter(|(_, archetype)| {
                    Self::archetype_matches(archetype, &self.target_mask, &self.filter_pairs)
                })
                .map(|(id, _)| *id)
                .collect();
            // Sort the ids so iteration order is the same in every run.
            matching_archetype_ids.sort();
            crate::profile_message!(
                "query archetype cache miss: generation {:?} -> {}, rescanned {} archetypes -> {} matched for this query",
                self.cached_generation,
                self.world.storage.archetype_generation,
                self.world.storage.archetypes.len(),
                matching_archetype_ids.len(),
            );

            // Step 4: Store the new list and the generation it was built from.
            self.cached_matches = matching_archetype_ids;
            self.cached_generation = Some(self.world.storage.archetype_generation);
        }
        &self.cached_matches
    }

    /// Returns a sequential iterator over the entities this query matches.
    ///
    /// Each call increases the world's change tick by one and reads the current
    /// system last-run tick, so change-detection filters such as
    /// [`Changed`](crate::query::Changed) compare against a fresh baseline.
    #[inline]
    pub fn iter_mut(&mut self) -> QueryIterMut<'_, Q, F> {
        // Step 1: Open a Tracy profiling zone around the iterator construction.
        let _zone = crate::profile_scope!(
            "create sequential query iterator",
            [(
                "Archetypes matching this query: {}",
                self.matching_archetype_ids().len()
            )]
        );

        // Step 2: Increase the change tick by one and capture this run's
        // baseline, so change-detection filters see the current frame.
        let this_run = self.world.increment_change_tick();
        let last_run = self.world.system_last_run();

        // Step 3: Copy the cached matching archetype ids and hand them to the
        // iterator, which takes ownership of the list.
        let matching_archetype_ids = self.matching_archetype_ids().to_vec();

        QueryIterMut::new(
            self.world as *mut World,
            matching_archetype_ids,
            this_run,
            last_run,
        )
    }

    /// The first matching archetype that holds at least one row, in the same
    /// order [`iter_mut`](Self::iter_mut) visits archetypes.
    ///
    /// The cached list is copied first, so the borrow of the query ends before
    /// the caller visits archetypes mutably. `first` and `is_empty` both take
    /// their fast path through this lookup, so neither can disagree with
    /// `iter_mut` about which archetypes match or in what order.
    fn first_non_empty_matching_archetype(&mut self) -> Option<ArchetypeId> {
        let matching_archetype_ids = self.matching_archetype_ids().to_vec();
        matching_archetype_ids.into_iter().find(|archetype_id| {
            self.world
                .storage
                .archetypes
                .get(archetype_id)
                .is_some_and(|archetype| !archetype.is_empty())
        })
    }

    /// Total rows across every matching archetype.
    ///
    /// Only called when the filter accepts every row, so this is exactly the
    /// number of entities a full iteration would yield.
    fn matching_row_count(&mut self) -> usize {
        let matching_archetype_ids = self.matching_archetype_ids().to_vec();
        matching_archetype_ids
            .iter()
            .filter_map(|archetype_id| self.world.storage.archetypes.get(archetype_id))
            .map(|archetype| archetype.len())
            .sum()
    }

    /// Returns the data of the first entity this query matches, if any.
    ///
    /// When the filter accepts every row, this reads the sorted list of
    /// matching archetypes directly, without building an iterator, and visits
    /// them in the same order as [`iter_mut`](Self::iter_mut). With any other
    /// filter it falls back to [`iter_mut`](Self::iter_mut) and returns its
    /// first row.
    #[inline]
    pub fn first(&mut self) -> Option<Q::Item<'_>> {
        // Fast path: the filter accepts every row, so take the first entity of
        // the first non-empty matching archetype. Reading the sorted list
        // instead of the archetype `HashMap` keeps the same order as
        // `iter_mut().next()`; the map's order is randomized per instance.
        if F::ACCEPTS_ALL {
            let this_run = self.world.increment_change_tick();
            let archetype_id = self.first_non_empty_matching_archetype()?;
            let archetype = self.world.storage.archetypes.get_mut(&archetype_id)?;
            let state = Q::init_state(archetype, this_run);
            return Some(Q::fetch_with_state(&state, 0));
        }
        self.iter_mut().next()
    }

    /// Returns `true` when no entity matches this query.
    ///
    /// When the filter accepts every row, this is O(archetypes): it only reads
    /// the row counts of the matching archetypes. With any other filter it asks
    /// the iterator for the first row, so every row test runs.
    #[inline]
    pub fn is_empty(&mut self) -> bool {
        // Fast path: the filter accepts every row, so no row test can fail. An
        // empty pair list alone does not prove that - a row-level filter such
        // as `Changed` declares no archetype restriction either - only
        // `F::ACCEPTS_ALL` says every row is accepted.
        if F::ACCEPTS_ALL {
            return self.first_non_empty_matching_archetype().is_none();
        }
        // Slow path: the filter has to test rows.
        self.iter_mut().next().is_none()
    }

    /// Returns the number of entities matching this query.
    ///
    /// When the filter accepts every row (`F::ACCEPTS_ALL`, which `()` sets),
    /// this is O(archetypes): it adds up the row counts without visiting a row.
    /// With any other filter, such as `Changed<T>`, every row has to be tested,
    /// so the cost is O(n).
    #[inline]
    pub fn entity_count(&mut self) -> usize {
        // Fast path: the filter accepts every row, so add up the row counts of
        // the matching archetypes.
        if F::ACCEPTS_ALL {
            return self.matching_row_count();
        }
        // Slow path: the filter has to test rows.
        self.iter_mut().count()
    }

    /// Returns a parallel iterator over all matching entities, spreading the
    /// work across the shared Rayon thread pool.
    ///
    /// The target state, the filter state and every fetched item must be
    /// [`Send`], so that one archetype can be processed on a worker thread.
    /// Those bounds are on the method signature.
    ///
    /// # Examples
    ///
    /// ```
    /// # use pill_engine::*;
    /// # #[derive(Debug, Clone)] struct Position { x: f32, y: f32 }
    /// # impl Component for Position {}
    /// # let mut world = World::new();
    /// # world.register_component::<Position>();
    /// # world.create_entity().with(Position { x: 1.0, y: 2.0 }).build().unwrap();
    /// let mut query = Query::<&Position>::new(&mut world);
    /// query.par_iter_mut().for_each(|position| {
    ///     println!("position x = {}", position.x);
    /// });
    /// ```
    #[inline]
    pub fn par_iter_mut(&mut self) -> ParQueryIter<'_, Q, F>
    where
        Q::State: Send + Sync,
        F::State: Send + Sync,
        for<'a> Q::Item<'a>: Send,
    {
        // Step 1: Increase the change tick by one and capture this run's
        // baseline for change-detection filters.
        let this_run = self.world.increment_change_tick();
        let last_run = self.world.system_last_run();

        // Step 2: Copy the cached matching archetype ids. Nothing is rescanned
        // here, but the per-archetype states are built fresh, so they capture
        // the current tick.
        let matching_archetype_ids = self.matching_archetype_ids().to_vec();
        let _zone = crate::profile_scope!(
            "create parallel query iterator",
            [(
                "Archetypes matching this query: {}",
                matching_archetype_ids.len()
            )]
        );

        // Step 3: Build one target state and one filter state per matching
        // archetype, and keep only archetypes that hold entities.
        let archetype_ranges: Vec<FilteredArchetypeRange<Q::State, F::State>> =
            matching_archetype_ids
                .iter()
                .filter_map(|id| {
                    // `get_mut` already returns the `&mut Archetype` that
                    // `init_state` takes, so a reborrow is enough. The filter
                    // call runs after it, one at a time, so no raw pointer and
                    // no SAFETY comment is needed here.
                    self.world.storage.archetypes.get_mut(id).map(|archetype| {
                        let target_state = Q::init_state(archetype, this_run);
                        let filter_state = F::init_state(archetype, last_run, this_run);
                        FilteredArchetypeRange {
                            target_state,
                            filter_state,
                            entity_count: archetype.len(),
                        }
                    })
                })
                .filter(|range| range.entity_count > 0)
                .collect();
        // The ids are already sorted, and `filter_map` keeps their order, so
        // `archetype_ranges` is sorted too and needs no extra `sort_by_key`.

        // Step 4: Hand the sorted ranges and the shared timing data to the
        // parallel iterator.
        ParQueryIter::new(
            archetype_ranges,
            self.world.iterator_timings.clone(),
            self.total_components_size_bytes,
        )
    }
}
