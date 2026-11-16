//! Unit tests for the query system: basic queries, parallel iteration,
//! change detection, and filter predicates.
//!
//! # Responsibilities
//!
//! - Runs queries over the shared [`Position`], [`Velocity`] and [`Health`]
//!   fixture components, both one row at a time and in parallel. It also
//!   checks the batch statistics a tracked parallel query reports, including
//!   a zero batch size.
//! - Checks how change detection behaves for `Mut<T>` and for the [`Added`]
//!   and [`Changed`] filters.
//! - Checks which archetypes the [`With`], [`Without`] and [`Or`] filters
//!   select.
//! - Checks that a query whose type declares a different layout of a shared
//!   component is refused, and that the refusal is reported once.
//!
//! # Design
//!
//! Most tests build a fresh [`World`] through [`setup_world`], with the three
//! fixture components registered. The registration, aliasing and shared-layout
//! tests build one directly.
//!
//! The filter tests work at two levels: the archetype level ([`With`],
//! [`Without`], [`Or`]) and the row level ([`Added`], [`Changed`]). Some of
//! them combine nested and mixed filters, and check that [`Or`] stops at the
//! first branch that matches. The target-tuple tests cover the aliasing check,
//! which refuses a target that reads and writes the same component.

// External crates
use pill_core::rayon;

// Current crate
use super::*;
use crate::component::{Component, ComponentId};
use crate::entity::Entity;
use crate::test_fixtures::{setup_world, Health, Position, Velocity};
use crate::world::World;

// =============================================================================
// Basic Query Tests
// =============================================================================

/// A query whose target component was never registered matches nothing and
/// does not panic. The unknown id stays in the query's component mask (the
/// set of components it requires) instead of being dropped, because a dropped
/// id would make "not registered" mean "matches everything".
#[test]
fn query_before_registration_matches_nothing() {
    let mut world = World::new();
    // `Position` is deliberately not registered here.
    let mut query = Query::<&Position>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0);

    // The mutable query type (`&mut Position`) takes the same path.
    let mut query = Query::<&mut Position>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0);
}

/// `With<T>` for an unregistered `T` matches nothing rather than everything:
/// no archetype can hold a component that was never registered.
#[test]
fn an_unregistered_with_filter_matches_nothing() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity,), With<Health>>::new(&mut world);
    assert_eq!(
        query.iter_mut().count(),
        0,
        "an unregistered With<T> cannot match"
    );

    // Once the component is registered, the filter matches what the world holds.
    world.register_component::<Health>();
    let mut query = Query::<(Entity,), With<Health>>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0, "the entity does not carry it");
    world.create_entity().with(Health(1)).build().unwrap();
    let mut query = Query::<(Entity,), With<Health>>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 1);
}

#[test]
fn an_empty_world_matches_nothing() {
    let mut world = setup_world();
    let mut query = Query::<(&Position,)>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0);
}

#[test]
fn a_single_entity_yields_its_own_components() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .with(Velocity { x: 0.5, y: 0.5 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position, &Velocity)>::new(&mut world);
    let matching_rows: Vec<_> = query.iter_mut().collect();

    assert_eq!(matching_rows.len(), 1, "one entity carries both components");
    let (position, velocity) = matching_rows[0];
    assert_eq!(
        position.x, 1.0,
        "the queried Position is the entity's own row"
    );
    assert_eq!(
        velocity.x, 0.5,
        "the queried Velocity is the entity's own row"
    );
}

#[test]
fn every_matching_entity_is_visited_once() {
    let mut world = setup_world();

    for i in 0..10 {
        world
            .create_entity()
            .with(Position {
                x: i as f32,
                y: 0.0,
            })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&Position,)>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 10);
}

#[test]
fn an_entity_missing_a_requested_component_is_skipped() {
    let mut world = setup_world();

    world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .build()
        .unwrap();

    world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .with(Velocity { x: 1.0, y: 1.0 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position, &Velocity)>::new(&mut world);
    let matching_rows: Vec<_> = query.iter_mut().collect();

    assert_eq!(matching_rows.len(), 1);
    assert_eq!(matching_rows[0].0.x, 2.0);
}

#[test]
fn a_mutable_query_writes_through_to_the_row() {
    let mut world = setup_world();

    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    // Step 1: integrate velocity into position through a mutable query.
    {
        let mut query = Query::<(&mut Position, &Velocity)>::new(&mut world);
        for (mut position, velocity) in query.iter_mut() {
            position.x += velocity.x;
            position.y += velocity.y;
        }
    }

    // Step 2: read the modified position back through a fresh query and
    // assert the integration result.
    let mut query = Query::<(&Position,)>::new(&mut world);
    let (position,) = query.first().unwrap();
    assert_eq!(position.x, 1.0);
    assert_eq!(position.y, 2.0);
}

#[test]
fn first_returns_the_first_matching_row() {
    let mut world = setup_world();

    world
        .create_entity()
        .with(Position { x: 5.0, y: 5.0 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position,)>::new(&mut world);
    let first_row = query.first();

    assert!(first_row.is_some());
    assert_eq!(first_row.unwrap().0.x, 5.0);
}

#[test]
fn first_is_none_when_nothing_matches() {
    let mut world = setup_world();
    let mut query = Query::<(&Position,)>::new(&mut world);
    assert!(query.first().is_none());
}

/// `first()` agrees with `iter_mut().next()` even with several matching
/// archetypes.
///
/// Both read the same cached list of matching archetypes, so `first` always
/// names the row `iter_mut().next()` returns. The cache sorts the archetype
/// ids, so that order is the same in every run.
#[test]
fn first_agrees_with_iteration_order() {
    let mut world = setup_world();

    // Four distinct archetypes containing `Position`, each with its own `x`
    // so a disagreement about which archetype comes first is visible.
    let archetype_cases: [(&str, f32); 4] = [
        ("position", 11.0),
        ("velocity", 22.0),
        ("health", 33.0),
        ("both", 44.0),
    ];
    for (archetype_shape, x) in archetype_cases {
        let mut builder = world.create_entity().with(Position { x, y: 0.0 });
        if archetype_shape == "velocity" || archetype_shape == "both" {
            builder = builder.with(Velocity { x: 0.0, y: 0.0 });
        }
        if archetype_shape == "health" || archetype_shape == "both" {
            builder = builder.with(Health(1));
        }
        builder.build().unwrap();
    }

    let mut query = Query::<(&Position,)>::new(&mut world);
    let first_x = query.first().map(|(position,)| position.x);
    let iter_x = query.iter_mut().next().map(|(position,)| position.x);
    assert!(
        first_x.is_some(),
        "the world holds four matching archetypes"
    );
    assert_eq!(
        first_x, iter_x,
        "`first` must return the row `iter_mut().next()` returns"
    );
}

/// A filter can reject rows without naming any component. Such a filter
/// produces no archetype filter pairs, and an empty pair list would read as
/// "accepts everything" if the row check were skipped. So a filter that
/// rejects every row must still make `first`, `is_empty` and `entity_count`
/// report nothing.
#[test]
fn row_only_filters_are_not_skipped() {
    /// Rejects every row without declaring any component scope.
    struct RejectAll;
    impl QueryFilter for RejectAll {
        type State = ();
        fn init_state(
            _archetype: &crate::archetype::Archetype,
            _last_run: crate::Tick,
            _this_run: crate::Tick,
        ) -> Self::State {
        }
        fn matches(_state: &Self::State, _index: usize) -> bool {
            false
        }
    }

    let mut world = setup_world();
    for index in 0..5 {
        world
            .create_entity()
            .with(Position {
                x: index as f32,
                y: 0.0,
            })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&Position,), RejectAll>::new(&mut world);
    assert!(query.first().is_none(), "a rejecting filter matches no row");
    assert!(query.is_empty(), "a rejecting filter matches no row");
    assert_eq!(query.entity_count(), 0, "a rejecting filter matches no row");
}

#[test]
fn the_entity_handle_is_part_of_the_query_data() {
    let mut world = setup_world();

    let entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity, &Position)>::new(&mut world);
    let (queried_entity, _) = query.first().unwrap();

    assert_eq!(queried_entity.id(), entity.id());
}

// =============================================================================
// Parallel Iterator Tests
// =============================================================================

#[test]
fn parallel_iteration_reaches_every_matching_row() {
    let mut world = setup_world();

    for i in 0..100 {
        world
            .create_entity()
            .with(Position {
                x: i as f32,
                y: 0.0,
            })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&mut Position,)>::new(&mut world);
    query.par_iter_mut().for_each(|(mut position,)| {
        position.x += 1.0;
    });

    // Every row must hold the incremented value now.
    let mut verify_query = Query::<(&Position,)>::new(&mut world);
    for (position,) in verify_query.iter_mut() {
        assert!(position.x >= 1.0);
    }
}

/// Checks that parallel iteration uses the batch size it is given.
///
/// The world is sized from the pool, so the parallel path always runs. A world
/// under `threads × MINIMUM_SLICE_SIZE` would take the sequential fallback
/// instead. That makes the expected numbers exact rather than a lower bound:
/// every slice is exactly 100 entities long, and the slices run in contiguous
/// groups, one per pool thread.
#[test]
fn an_explicit_batch_size_controls_the_slices() {
    let mut world = setup_world();
    let thread_count = rayon::current_num_threads();
    let batch_size = 100usize;
    let entity_count = (thread_count * 1024).div_ceil(batch_size) * batch_size;

    for _ in 0..entity_count {
        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&mut Position,)>::new(&mut world);
    let batch_stats = query
        .par_iter_mut()
        .with_batch_size(batch_size)
        .tracked()
        .for_each(|(mut position,)| {
            position.x = 1.0;
        });

    let batch_stats = batch_stats.unwrap();
    assert_eq!(batch_stats.total_entities, entity_count);

    // Every slice is exactly `batch_size` long, and the slices sit next to
    // each other in one group per thread. So each group processed
    // `batch_size × (its slice count)`: `base_slice_count` slices each, plus
    // one more in the first `extra_group_count` groups. With the default
    // span of 256..4096, a batch size that was ignored would break both
    // equalities.
    let slice_count = entity_count / batch_size;
    let base_slice_count = slice_count / thread_count;
    let extra_group_count = slice_count % thread_count;
    assert_eq!(
        batch_stats.batch_count, thread_count,
        "slices must be grouped contiguously across the pool"
    );
    assert_eq!(batch_stats.min_batch_size, batch_size * base_slice_count);
    assert_eq!(
        batch_stats.max_batch_size,
        batch_size * (base_slice_count + usize::from(extra_group_count > 0))
    );
}

/// Checks the statistics a tracked parallel iteration reports: the entity
/// count, a non-zero batch and thread count, and a positive average batch
/// size.
#[test]
fn tracked_parallel_iteration_reports_its_batches() {
    let mut world = setup_world();

    for _ in 0..500 {
        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&Position,)>::new(&mut world);
    let batch_stats = query
        .par_iter_mut()
        .tracked()
        .for_each(|_| {})
        .expect("tracked iteration returns stats");

    assert_eq!(batch_stats.total_entities, 500);
    assert!(batch_stats.batch_count > 0);
    assert!(batch_stats.num_threads > 0);
    assert!(batch_stats.avg_batch_size > 0.0);
}

/// Verifies that untracked parallel iteration returns no stats.
#[test]
fn untracked_parallel_iteration_reports_no_stats() {
    let mut world = setup_world();

    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position,)>::new(&mut world);
    let batch_stats = query.par_iter_mut().for_each(|_| {});

    assert!(batch_stats.is_none());
}

#[test]
fn a_parallel_iterator_counts_its_entities() {
    let mut world = setup_world();

    for _ in 0..250 {
        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build()
            .unwrap();
    }

    let mut query = Query::<(&Position,)>::new(&mut world);
    let parallel_iterator = query.par_iter_mut();

    assert_eq!(parallel_iterator.entity_count(), 250);
}

// =============================================================================
// BatchStats Tests
// =============================================================================

/// Checks that `BatchStats` prints a readable summary.
#[test]
fn batch_stats_render_their_counts() {
    let batch_stats = BatchStats {
        num_threads: 8,
        batch_count: 10,
        total_entities: 1000,
        min_batch_size: 90,
        max_batch_size: 110,
        avg_batch_size: 100.0,
    };

    let rendered_stats = format!("{}", batch_stats);
    assert!(rendered_stats.contains("threads: 8"));
    assert!(rendered_stats.contains("batches: 10"));
    assert!(rendered_stats.contains("entities: 1000"));
}

// =============================================================================
// QueryTarget Trait Tests
// =============================================================================

/// Verifies that `component_ids()` returns the correct set for a query target.
#[test]
fn a_query_reports_the_component_ids_it_matches() {
    let component_ids = <(&Position, &Velocity)>::component_ids();
    assert_eq!(component_ids.len(), 2);
    assert!(component_ids.contains(&ComponentId::of::<Position>()));
    assert!(component_ids.contains(&ComponentId::of::<Velocity>()));
}

/// The components a query matches and the access a system reports come from
/// one description: `component_ids()` is exactly `reads ++ writes`.
#[test]
fn the_reported_access_matches_the_component_ids() {
    let component_ids = <(&Position, &mut Velocity)>::component_ids();
    let (reads, writes) = <(&Position, &mut Velocity)>::report_component_access();

    let derived_component_ids: Vec<ComponentId> = reads.into_iter().chain(writes).collect();
    assert_eq!(component_ids, derived_component_ids);
    assert!(component_ids.contains(&ComponentId::of::<Position>()));
    assert!(component_ids.contains(&ComponentId::of::<Velocity>()));
}

/// Checks that `report_component_access` reports a read-only target as a read.
#[test]
fn a_read_access_is_reported_as_read() {
    let (reads, writes) = <(&Position,)>::report_component_access();
    assert_eq!(reads.len(), 1);
    assert_eq!(writes.len(), 0);
}

/// Checks that `report_component_access` reports a write-only target as a
/// write.
#[test]
fn a_write_access_is_reported_as_write() {
    let (reads, writes) = <(&mut Position,)>::report_component_access();
    assert_eq!(reads.len(), 0);
    assert_eq!(writes.len(), 1);
}

/// Checks that a target reading one component and writing another reports one
/// read and one write.
#[test]
fn a_mixed_query_reports_both_access_modes() {
    let (reads, writes) = <(&Position, &mut Velocity)>::report_component_access();
    assert_eq!(reads.len(), 1);
    assert_eq!(writes.len(), 1);
}

/// Verifies that `report_component_access` refuses duplicate mutable
/// component types in every build, not only debug ones.
///
/// A `Query<(&mut Position, &mut Position)>` would create aliasing `&mut`
/// references to the same storage, which is undefined behaviour.
#[test]
#[should_panic(expected = "reads and writes the same component")]
fn two_writable_views_of_one_component_are_refused() {
    let _ = <(&mut Position, &mut Position)>::report_component_access();
}

/// Checks what the aliasing check reports. Two writes of one component, or a
/// read beside a write of it, are found. Two reads of one component are not
/// aliasing and report nothing.
#[test]
fn the_aliasing_check_reports_the_aliasing_component() {
    use crate::query::target::find_aliasing_component;
    let position_component_id = ComponentId::of::<Position>();
    let velocity_component_id = ComponentId::of::<Velocity>();
    assert_eq!(find_aliasing_component(&[], &[]), None);
    assert_eq!(find_aliasing_component(&[], &[position_component_id]), None);
    assert_eq!(
        find_aliasing_component(&[position_component_id, velocity_component_id], &[]),
        None
    );
    assert_eq!(
        find_aliasing_component(&[position_component_id, position_component_id], &[]),
        None,
        "two reads of one component are not aliasing"
    );
    assert_eq!(
        find_aliasing_component(&[], &[position_component_id, position_component_id]),
        Some(position_component_id)
    );
    assert_eq!(
        find_aliasing_component(&[position_component_id], &[position_component_id]),
        Some(position_component_id)
    );
    assert_eq!(
        find_aliasing_component(&[velocity_component_id], &[position_component_id]),
        None
    );
}

/// Aliasing is refused when the query is built: `Query::new` runs the same
/// check the tuple's access report does, so a query constructed directly
/// cannot skip it.
#[test]
#[should_panic(expected = "reads and writes the same component")]
fn a_read_write_alias_is_refused_when_the_query_is_built() {
    let mut world = World::new();
    world.register_component::<Position>();
    let _ = Query::<(&Position, &mut Position)>::new(&mut world);
}

/// A duplicated read is not aliasing, and such a query must still build.
#[test]
fn two_read_views_of_one_component_are_allowed() {
    let mut world = World::new();
    world.register_component::<Position>();
    let mut query = Query::<(&Position, &Position)>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0);
}

/// Verifies that `Entity` reports zero component IDs.
#[test]
fn an_entity_only_query_names_no_component() {
    let component_ids = Entity::component_ids();
    assert!(component_ids.is_empty());
}

// =============================================================================
// Change Detection Tests
// =============================================================================

/// Checks that `DerefMut` on `Mut<T>` advances the changed tick.
#[test]
fn writing_through_a_mutable_view_bumps_the_changed_tick() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Step 1: record the tick at which the component was added (the insert
    // used whatever `world.change_tick` was at the time).
    let added_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        position.last_added()
    };

    // Step 2: write through `DerefMut`, which moves `changed` to the
    // iterator's `this_run` tick. That tick is strictly newer than `added`.
    let changed_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (mut position,) = query.first().unwrap();
        position.x += 5.0; // DerefMut path
        position.last_changed()
    };

    assert!(
        changed_tick > added_tick,
        "expected changed ({:?}) > added ({:?})",
        changed_tick,
        added_tick
    );
}

/// Checks that `Deref` on `Mut<T>`, which only reads, leaves the changed tick
/// alone.
#[test]
fn reading_through_a_shared_view_leaves_the_changed_tick() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    // Step 1: record the changed tick before any dereference.
    let baseline_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        position.last_changed()
    };

    // Step 2: read through an immutable deref - the changed tick must
    // not advance.
    let after_read_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        let _x = position.x; // immutable deref
        position.last_changed()
    };

    assert_eq!(baseline_tick, after_read_tick);
}

/// Checks that `bypass_change_detection()` gives raw access without moving
/// the change-detection ticks.
#[test]
fn a_bypassing_write_leaves_the_changed_tick() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Step 1: record the changed tick before any mutation.
    let baseline_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        position.last_changed()
    };

    // Step 2: mutate through `bypass_change_detection` - the changed
    // tick must not advance.
    let after_bypass_write_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (mut position,) = query.first().unwrap();
        position.bypass_change_detection().x = 99.0;
        position.last_changed()
    };

    assert_eq!(baseline_tick, after_bypass_write_tick);

    // Step 3: verify the raw mutation still took effect.
    let mut verify_query = Query::<(&Position,)>::new(&mut world);
    let (position,) = verify_query.first().unwrap();
    assert_eq!(position.x, 99.0);
}

/// Verifies that `Added` ticks survive archetype migration.
#[test]
fn the_added_tick_survives_an_archetype_move() {
    let mut world = setup_world();
    let entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Step 1: record the `added` tick of Position before migration.
    let original_added_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        position.last_added()
    };

    // Step 2: migrate to a new archetype by adding a component. The
    // Position component carries over and its `added` tick must be
    // preserved.
    world
        .add_component(entity, Velocity { x: 1.0, y: 1.0 })
        .unwrap();

    // Step 3: verify the migrated Position preserves its `added` tick.
    let migrated_added_tick = {
        let mut query = Query::<(&mut Position,)>::new(&mut world);
        let (position,) = query.first().unwrap();
        position.last_added()
    };

    assert_eq!(original_added_tick, migrated_added_tick);

    // Step 4: verify the newly attached Velocity has an `added` tick at
    // least as new as the original Position's.
    let velocity_added_tick = {
        let mut query = Query::<(&mut Velocity,)>::new(&mut world);
        let (velocity,) = query.first().unwrap();
        velocity.last_added()
    };
    assert!(velocity_added_tick >= original_added_tick);
}

// =============================================================================
// QueryFilter Tests
// =============================================================================

/// Checks that `With<T>` matches only archetypes that contain `T`.
#[test]
fn a_with_filter_skips_archetypes_without_the_component() {
    let mut world = setup_world();
    // Two entities: one with Health, one without.
    world
        .create_entity()
        .with(Position { x: 1.0, y: 0.0 })
        .with(Health(100))
        .build()
        .unwrap();
    world
        .create_entity()
        .with(Position { x: 2.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position,), With<Health>>::new(&mut world);
    let matching_x_values: Vec<f32> = query.iter_mut().map(|(position,)| position.x).collect();
    assert_eq!(matching_x_values, vec![1.0]);
}

/// Checks that `Without<T>` skips archetypes that contain `T`.
#[test]
fn a_without_filter_skips_archetypes_that_have_the_component() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 1.0, y: 0.0 })
        .with(Health(100))
        .build()
        .unwrap();
    world
        .create_entity()
        .with(Position { x: 2.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(&Position,), Without<Health>>::new(&mut world);
    let matching_x_values: Vec<f32> = query.iter_mut().map(|(position,)| position.x).collect();
    assert_eq!(matching_x_values, vec![2.0]);
}

/// Checks that `Changed<T>` yields only the rows whose component changed since
/// the last run, whether it was written to or just added.
#[test]
fn a_changed_filter_sees_only_mutated_rows() {
    let mut world = setup_world();
    let mutated_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let _unmodified_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let _another_unmodified_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Step 1: set the baseline so all "Added" ticks are in the past.
    world.set_system_last_run(world.change_tick());

    // Step 2: mutate only mutated_entity so its `changed` tick advances.
    {
        let mut query = Query::<(Entity, &mut Position)>::new(&mut world);
        for (entity, mut position) in query.iter_mut() {
            if entity == mutated_entity {
                position.x = 99.0;
            }
        }
    }

    // Step 3: filter on `Changed<Position>` - only mutated_entity should
    // appear.
    let mut query = Query::<(Entity, &Position), Changed<Position>>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity, _)| entity).collect();
    assert_eq!(matched_entities, vec![mutated_entity]);
}

/// Checks that `Added<T>` yields only rows whose component was inserted since
/// the last run.
#[test]
fn an_added_filter_sees_only_new_rows() {
    let mut world = setup_world();
    let _existing_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Step 1: take the current tick as the baseline, so the existing entity
    // does not count as added.
    world.set_system_last_run(world.change_tick());

    // Step 2: advance the tick (as a frame boundary does) and insert a new
    // entity. It should be the only `Added<Position>` hit.
    world.increment_change_tick();
    let new_entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity,), Added<Position>>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(matched_entities, vec![new_entity]);
}

/// Checks that an `Or` matches a row when either branch matches.
#[test]
fn an_or_filter_matches_either_branch() {
    let mut world = setup_world();
    let position_mutated_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let velocity_mutated_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    world.set_system_last_run(world.change_tick());

    // Step 1: mutate Position on position_mutated_entity and Velocity on
    // velocity_mutated_entity so each entity triggers a different `Changed`
    // branch.
    {
        let mut query = Query::<(Entity, &mut Position, &mut Velocity)>::new(&mut world);
        for (entity, mut position, mut velocity) in query.iter_mut() {
            if entity == position_mutated_entity {
                position.x = 1.0;
            } else if entity == velocity_mutated_entity {
                velocity.x = 1.0;
            }
        }
    }

    // Step 2: query with `Or<(Changed<Position>, Changed<Velocity>)>` -
    // both entities should match via their respective branches.
    let mut query = Query::<(Entity,), Or<(Changed<Position>, Changed<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());
    let mut expected = vec![position_mutated_entity, velocity_mutated_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Verifies that `Or<(With<A>, With<B>)>` matches entities with either component.
#[test]
fn an_or_of_two_with_filters_matches_either_component() {
    // `Or<(With<A>, With<B>)>` must match archetypes that contain A or B, not
    // only those that contain both. The two inner filters each have their own
    // required set ({A} and {B}); combining them into one required set would
    // wrongly ask for both.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let velocity_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let both_components_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    // This entity has neither A nor B - should be excluded.
    let _neither_component_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, With<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    let mut expected = vec![position_entity, velocity_entity, both_components_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(
        matched_entities, expected,
        "Or<(With<A>, With<B>)> should match A-only, B-only, and both"
    );
}

/// Verifies that an `Or` mixing `With` and `Without` picks the right
/// archetypes.
#[test]
fn an_or_of_with_and_without_filters_picks_the_right_archetypes() {
    // Or<(With<A>, Without<B>)>: match archetypes containing A, *or*
    // archetypes not containing B. An archetype with neither A nor B
    // should match via the Without<B> branch.
    let mut world = setup_world();
    let position_and_velocity_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    // The Velocity-only entity has no Position - excluded by both branches.
    let _velocity_only_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let health_only_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, Without<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    // Expected matches:
    // - Position+Velocity: has Position, so With<Position> matches.
    // - Velocity only: no Position, so that branch fails, and it has
    //   Velocity, so Without<Velocity> fails too. Excluded.
    // - Health only: has neither component, so Without<Velocity> matches.
    let mut expected = vec![position_and_velocity_entity, health_only_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Checks that an `Or` mixing `With` (archetype level) with `Changed` (row
/// level) applies both rules.
#[test]
fn an_or_branch_combined_with_changed_keeps_both_rules() {
    // Or<(With<A>, Changed<B>)>: one branch filters by archetype (With),
    // the other by row (Changed). An archetype with A but not B must not
    // panic when the Changed<B> branch builds its state.
    let mut world = setup_world();
    let position_only_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let velocity_only_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let both_components_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();

    world.set_system_last_run(world.change_tick());

    // Mutate Position on both_components_entity, Velocity on velocity_only_entity.
    {
        let mut query = Query::<(Entity, &mut Position, &mut Velocity)>::new(&mut world);
        for (entity, mut position, mut velocity) in query.iter_mut() {
            if entity == both_components_entity {
                position.x = 1.0;
            } else if entity == velocity_only_entity {
                velocity.x = 1.0;
            }
        }
    }

    // Expected matches:
    // - position_only_entity: has Position, so branch 1 matches. Branch 2
    //   asks about Velocity, which it does not have, and must not panic.
    // - velocity_only_entity: Velocity was written to, so branch 2 matches.
    // - both_components_entity: has Position, so branch 1 matches. Its
    //   Velocity also changed, but the `Or` stops at the first match.
    let mut query = Query::<(Entity,), Or<(With<Position>, Changed<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    let mut expected = vec![
        position_only_entity,
        velocity_only_entity,
        both_components_entity,
    ];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Verifies that `Or<(Without<A>, Without<B>)>` excludes only archetypes that
/// carry both components.
#[test]
fn an_or_of_two_without_filters_matches_everything_else() {
    // Or<(Without<A>, Without<B>)>: include archetypes that lack A *or*
    // lack B. Only archetypes containing BOTH A and B are excluded.
    let mut world = setup_world();
    let position_only_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let velocity_only_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    // The both-components entity has both Position and Velocity → excluded by
    // both Without branches
    let _both_components_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let health_only_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<(Without<Position>, Without<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    // Expected matches:
    // - position_only_entity: no Velocity, so Without<Velocity> matches.
    // - velocity_only_entity: no Position, so Without<Position> matches.
    // - both_components_entity: has both, so both branches fail.
    // - health_only_entity: has neither, so both branches match.
    let mut expected = vec![
        position_only_entity,
        velocity_only_entity,
        health_only_entity,
    ];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(
        matched_entities, expected,
        "Or<(Without<A>, Without<B>)> should exclude only (A+B) archetypes"
    );
}

/// Verifies that a three-way `Or` filter correctly matches all three branches.
#[test]
fn a_three_way_or_matches_any_branch() {
    // Or<(With<A>, With<B>, With<C>)>: 3-way OR at archetype level.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let velocity_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let health_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query =
        Query::<(Entity,), Or<(With<Position>, With<Velocity>, With<Health>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    assert_eq!(
        matched_entities.len(),
        3,
        "all three entities should match via different branches"
    );
    // position_entity matches via With<Position>, velocity_entity via
    // With<Velocity>, health_entity via With<Health>.
    let matched_entity_ids: Vec<u64> = matched_entities.iter().map(|entity| entity.id()).collect();
    assert!(matched_entity_ids.contains(&position_entity.id()));
    assert!(matched_entity_ids.contains(&velocity_entity.id()));
    assert!(matched_entity_ids.contains(&health_entity.id()));
}

/// Verifies that `Or` within an AND tuple correctly narrows the result set.
#[test]
fn an_or_nested_inside_an_and_tuple_still_applies() {
    // (Or<(With<A>, With<B>)>, Without<C>): Or inside an AND tuple.
    // Match archetypes that (have A or B) AND do NOT have C.
    let mut world = setup_world();
    // Archetype Position + Health: has A, no B, and has C, so Without<Health>
    // excludes it.
    let _position_and_health_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    // Archetype Position only: has A and no C, so it matches.
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    // Archetype Velocity only: has B and no C, so it matches.
    let velocity_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    // Archetype Health only: has no A and no B, and it has C, so both filters
    // fail.
    let _health_only_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query =
        Query::<(Entity,), (Or<(With<Position>, With<Velocity>)>, Without<Health>)>::new(
            &mut world,
        );
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    let mut expected = vec![position_entity, velocity_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Verifies that an `Or`-scoped query whose target is only `Entity` yields the
/// entities its branches match.
#[test]
fn an_entity_only_target_lets_the_or_decide() {
    // Query<(Entity,), Or<(With<A>, With<B>)>>: no component data is fetched,
    // so the `Or` filter alone decides which archetypes match.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let velocity_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let _neither_component_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, With<Velocity>)>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    let mut expected = vec![position_entity, velocity_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Checks that an entity matching several `Or` branches is visited once.
#[test]
fn an_or_matching_both_branches_visits_a_row_once() {
    // An entity in an archetype that matches multiple Or branches must
    // appear exactly once, not once per matching branch.
    let mut world = setup_world();
    let both_components_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, With<Velocity>)>>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(
        matched_entities.len(),
        1,
        "entity should appear once even if both branches match"
    );
    assert_eq!(matched_entities[0], both_components_entity);
}

/// Checks that an `Or` filter also works on the parallel path.
#[test]
fn an_or_filter_applies_on_the_parallel_path() {
    // A `Mutex` collects the results because `ParQueryIter` is not a
    // `rayon::ParallelIterator` itself; the filter still runs on the worker
    // threads inside `for_each`.
    use std::sync::Mutex;

    let mut world = setup_world();
    for i in 0..50 {
        if i % 2 == 0 {
            world
                .create_entity()
                .with(Position {
                    x: i as f32,
                    y: 0.0,
                })
                .build()
                .unwrap();
        } else {
            world
                .create_entity()
                .with(Velocity {
                    x: i as f32,
                    y: 0.0,
                })
                .build()
                .unwrap();
        }
    }

    let mut query = Query::<(Entity,), Or<(With<Position>, With<Velocity>)>>::new(&mut world);
    let matched_entities = Mutex::new(Vec::new());
    query.par_iter_mut().for_each(|(entity,)| {
        matched_entities.lock().unwrap().push(entity);
    });
    let matched_entities = matched_entities.into_inner().unwrap();
    assert_eq!(
        matched_entities.len(),
        50,
        "Or filter with par_iter should find all entities"
    );
    // Every matching entity must appear once. This is the test that runs the
    // `Or` filter's row check on worker threads, so a slice visited twice, or
    // not at all, changes the count above.
    let mut seen_entity_ids = std::collections::HashSet::new();
    for entity in &matched_entities {
        assert!(
            seen_entity_ids.insert(entity.id()),
            "an entity was visited by two slices"
        );
    }
}

/// Checks that an `Or` with a `()` branch matches every row, because `()`
/// always matches.
#[test]
fn an_or_with_a_unit_branch_matches_every_row() {
    // Or<(With<A>, ())>: the () branch has no restrictions and always
    // matches, so the whole Or should match EVERY entity regardless of
    // whether it has A or not.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let health_only_entity = world.create_entity().with(Health(10)).build().unwrap();
    let velocity_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, ())>>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    // All entities should match because the () branch has no restrictions.
    let mut expected = vec![position_entity, health_only_entity, velocity_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(
        matched_entities, expected,
        "Or<(... , ())> should match all entities"
    );
}

/// Checks that `Or` with `()` as its first branch still matches every row. A
/// branch that restricts nothing makes the whole `Or` restrict nothing.
#[test]
fn a_unit_branch_first_still_matches_every_row() {
    // Or<((), With<A>)>: order of branches shouldn't matter. The ()
    // branch (first position) still makes the whole Or match everything.
    let mut world = setup_world();
    let health_only_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<((), With<Position>)>>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(matched_entities.len(), 1);
    assert_eq!(matched_entities[0], health_only_entity);
}

/// Checks that repeating a filter inside an `Or` changes nothing.
#[test]
fn a_repeated_inner_filter_matches_once() {
    // Or<(With<A>, With<A>)>: the same filter twice. It should behave like one
    // With<A> filter: no panic, and no entity counted twice.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let _health_only_entity = world.create_entity().with(Health(10)).build().unwrap();

    let mut query = Query::<(Entity,), Or<(With<Position>, With<Position>)>>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(
        matched_entities.len(),
        1,
        "duplicate inner filters should not double-count"
    );
    assert_eq!(matched_entities[0], position_entity);
}

/// Checks that two `Or` filters in one tuple keep only the rows both accept.
#[test]
fn an_and_of_two_ors_requires_both() {
    // The tuple is (A or B) AND (not A or not B), which is the same as
    // (A and not B) or (B and not A). So an entity with only A matches, and
    // one with only B matches. An entity with both components matches
    // neither: A fails Without<A> and B fails Without<B>, so the second `Or`
    // rejects it.
    let mut world = setup_world();
    let position_only_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let velocity_only_entity = world
        .create_entity()
        .with(Velocity { x: 0.0, y: 0.0 })
        .with(Health(10))
        .build()
        .unwrap();
    let _both_components_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    type BothOrsFilter = (
        Or<(With<Position>, With<Velocity>)>,
        Or<(Without<Position>, Without<Velocity>)>,
    );
    let mut query = Query::<(Entity,), BothOrsFilter>::new(&mut world);
    let mut matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    matched_entities.sort_by_key(|entity| entity.id());

    // Expected matches:
    // - position_only_entity: has A and not B, so the first `Or` matches
    //   through With<Position> and the second through Without<Velocity>.
    // - velocity_only_entity: has B and not A, so the first `Or` matches
    //   through With<Velocity> and the second through Without<Position>.
    // - both_components_entity: the first `Or` matches, but both branches of
    //   the second fail, so it does not match.
    let mut expected = vec![position_only_entity, velocity_only_entity];
    expected.sort_by_key(|entity| entity.id());
    assert_eq!(matched_entities, expected);
}

/// Verifies that a single-element filter tuple works identically to the bare filter.
#[test]
fn a_one_element_filter_tuple_still_filters() {
    // (With<A>,): a one-element filter tuple should behave like With<A> alone.
    let mut world = setup_world();
    let position_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let mut query = Query::<(Entity,), (With<Position>,)>::new(&mut world);
    let matched_entities: Vec<Entity> = query.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(matched_entities.len(), 1);
    assert_eq!(matched_entities[0], position_entity);
}

/// Verifies that `Changed<T>` returns no rows when no mutation has occurred.
#[test]
fn a_changed_filter_is_empty_when_nothing_was_written() {
    let mut world = setup_world();
    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    // Baseline at the current tick - the existing component's
    // `changed` tick is not newer than it, so the filter must yield nothing.
    world.set_system_last_run(world.change_tick());

    let mut query = Query::<(Entity,), Changed<Position>>::new(&mut world);
    assert_eq!(query.iter_mut().count(), 0);
}

// =============================================================================
// Stale Shared Readers
// =============================================================================

/// The name all three point types below register under. One component
/// compiled into two binaries would share such a name.
const SHARED_POINT: &str = "query_tests::SharedPoint";

/// The layout the registry holds: `x`, then `y`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, pill_engine_macros::PillLayout)]
struct CurrentPoint {
    x: f32,
    y: f32,
}
impl Component for CurrentPoint {
    fn shared_name() -> Option<&'static str> {
        Some(SHARED_POINT)
    }
    fn declared_schema_hash() -> Option<u64> {
        Some(crate::component::component_schema_hash(Self::FIELD_LAYOUT))
    }
}

/// A reader built against an older layout of the same component. The size and
/// alignment match and only the field order is swapped, so a check on size and
/// alignment alone would miss it.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, pill_engine_macros::PillLayout)]
struct StalePoint {
    y: f32,
    x: f32,
}
impl Component for StalePoint {
    fn shared_name() -> Option<&'static str> {
        Some(SHARED_POINT)
    }
    fn declared_schema_hash() -> Option<u64> {
        Some(crate::component::component_schema_hash(Self::FIELD_LAYOUT))
    }
}

/// A reader that declares no layout, so only size and alignment are checked.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct UndeclaredPoint {
    x: f32,
    y: f32,
}
impl Component for UndeclaredPoint {
    fn shared_name() -> Option<&'static str> {
        Some(SHARED_POINT)
    }
}

/// A world holding one entity of the shared point, registered with
/// [`CurrentPoint`]'s layout.
fn world_with_a_current_point() -> World {
    let mut world = World::new();
    world.register_component_with_layout::<CurrentPoint>(CurrentPoint::FIELD_LAYOUT);
    world
        .create_entity()
        .with(CurrentPoint { x: 1.0, y: 2.0 })
        .build()
        .unwrap();
    world
}

/// A query whose type declares a different layout of a shared component than
/// the registered one is refused and matches nothing. That holds alone, in
/// mutable form and inside a tuple. A query using the current layout still
/// sees the row.
#[test]
fn a_stale_shared_reader_is_refused() {
    let mut world = world_with_a_current_point();
    let point_component_id = ComponentId::of::<CurrentPoint>();
    assert_eq!(
        ComponentId::of::<StalePoint>(),
        point_component_id,
        "one shared name, one component"
    );

    let mut stale_query = Query::<&StalePoint>::new(&mut world);
    assert_eq!(
        stale_query.stale_shared_component(),
        Some(point_component_id)
    );
    assert_eq!(stale_query.iter_mut().count(), 0);

    let mut stale_mutable_query = Query::<&mut StalePoint>::new(&mut world);
    assert_eq!(
        stale_mutable_query.stale_shared_component(),
        Some(point_component_id)
    );
    assert_eq!(stale_mutable_query.iter_mut().count(), 0);

    let mut stale_tuple_query = Query::<(Entity, &StalePoint)>::new(&mut world);
    assert_eq!(
        stale_tuple_query.stale_shared_component(),
        Some(point_component_id)
    );
    assert_eq!(stale_tuple_query.entity_count(), 0);

    let mut current_query = Query::<&CurrentPoint>::new(&mut world);
    assert_eq!(current_query.stale_shared_component(), None);
    let row_values: Vec<(f32, f32)> = current_query
        .iter_mut()
        .map(|point| (point.x, point.y))
        .collect();
    assert_eq!(row_values, [(1.0, 2.0)]);
}

/// A reader declaring no layout uses the size-and-alignment check alone, so it
/// still matches.
#[test]
fn a_reader_without_a_declared_layout_falls_back_to_size_and_alignment() {
    let mut world = world_with_a_current_point();
    let mut query = Query::<&UndeclaredPoint>::new(&mut world);
    assert_eq!(query.stale_shared_component(), None);
    assert_eq!(query.iter_mut().count(), 1);
}

/// A refused query is rebuilt every frame; the refusal is recorded once.
#[test]
fn a_stale_reader_is_reported_once() {
    let mut world = world_with_a_current_point();
    for _ in 0..3 {
        let _ = Query::<&StalePoint>::new(&mut world);
    }
    assert_eq!(world.reported_schema_mismatches.len(), 1);
}

/// A zero batch size cannot reach the slice arithmetic.
///
/// The requested size becomes `entities_per_slice`, the length used to cut
/// every archetype's rows into slices. A zero from the caller would divide by
/// zero inside `for_each` and crash the host mid-frame. `with_batch_size`
/// raises the value to one, the smallest slice the executor can make.
#[test]
fn a_zero_batch_size_does_not_panic() {
    let mut world = setup_world();
    let thread_count = rayon::current_num_threads();
    let entity_count = thread_count * 1024;
    for _ in 0..entity_count {
        world
            .create_entity()
            .with(Position { x: 0.0, y: 0.0 })
            .build()
            .unwrap();
    }

    // Enough entities to clear the sequential fallback, so the slice division
    // is reached rather than skipped.
    let mut query = Query::<(&mut Position,)>::new(&mut world);
    let batch_stats = query
        .par_iter_mut()
        .with_batch_size(0)
        .tracked()
        .for_each(|(mut position,)| {
            position.x = 1.0;
        })
        .unwrap();

    assert_eq!(batch_stats.total_entities, entity_count);
    // Slices of one entity, grouped contiguously across the pool, so every
    // thread group covers exactly the entities it was given.
    assert_eq!(batch_stats.min_batch_size, entity_count / thread_count);
    assert_eq!(batch_stats.max_batch_size, entity_count / thread_count);
}
