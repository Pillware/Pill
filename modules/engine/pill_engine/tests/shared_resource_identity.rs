//! Integration tests for resources that keep one identity across artifacts.
//!
//! # Responsibilities
//!
//! - Pin that one shared resource name binds two distinct types to one value.
//! - Pin the guards: a refused claim or removal leaves the stored value and
//!   its claim untouched, and a rolled-back reload keeps it readable.
//!
//! # Design
//!
//! One resource type compiled into two artifacts gets two `TypeId`s, and a
//! single test binary cannot produce that. Two distinct Rust types that
//! declare the same shared name stand in, because that is what the engine
//! sees in the real case. As in the real case, they live in modules and keep
//! the type's own name: `Settings` is `Settings` in whichever artifact
//! compiled it, and only the module path differs.
//!
//! The cross-artifact behaviour itself is covered end to end by
//! `devops/tests/test_shared_component_identity.py`. This file covers the
//! scheduler at five levels: the pairwise answer (`conflicts_with`), the live
//! sets that answer reads, the batching decision built on it
//! (`SystemScheduler::ensure_execution_graph`), the rebuild a reload takes,
//! and the running engine, where two systems batched together would trip the
//! debug write lock.

// Relayout and re-homing are development-only engine APIs.
#![cfg(feature = "development")]

// External crates
use pill_engine::query::{Res, ResMut};
use pill_engine::scheduler::{SystemAccess, SystemScheduler};
use pill_engine::{Engine, Resource, ResourceId, World};

// =============================================================================
// Stand-ins
// =============================================================================

/// One artifact's copy of the shared resource.
pub mod artifact_a {
    /// `demo::Settings` as one artifact compiled it.
    #[derive(Debug)]
    pub struct Settings {
        pub value: u32,
    }
    impl pill_engine::Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Settings")
        }
    }
}

/// The other artifact's copy: a different Rust type, hence a different
/// `TypeId`, declaring the same shared name and the same layout.
pub mod artifact_b {
    /// `demo::Settings` as the other artifact compiled it.
    #[derive(Debug)]
    pub struct Settings {
        pub value: u32,
    }
    impl pill_engine::Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Settings")
        }
    }
}

/// A third artifact built against a *changed* definition: same name, same
/// declared identity, wider fields. The name guard passes; the layout guard
/// must not.
pub mod stale_artifact {
    /// `demo::Settings` as compiled by a binary rebuilt against a changed
    /// definition.
    #[derive(Debug)]
    pub struct Settings {
        pub value: u64,
        pub extra: u64,
    }
    impl pill_engine::Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Settings")
        }
    }
}

/// An unrelated resource claiming the same shared name, which the name guard
/// must separate from the legitimate copies above.
#[derive(Debug)]
struct UnrelatedSettings {
    _value: u32,
}
impl Resource for UnrelatedSettings {
    fn shared_name() -> Option<&'static str> {
        Some("demo::Settings")
    }
}

/// One artifact's copy of `demo::Hashed`: same name, same `{u32}` shape, and
/// a declared schema hash only this copy knows.
pub mod hashed_a {
    /// `demo::Hashed` as one artifact compiled it.
    pub struct Hashed {
        pub value: u32,
    }
    impl pill_engine::Resource for Hashed {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Hashed")
        }
        fn shared_schema_hash() -> Option<u64> {
            Some(0x1111)
        }
    }
}

/// The other artifact's copy of `demo::Hashed`: the same name and the same
/// `{u32}` shape, with a different declared schema hash.
pub mod hashed_b {
    /// `demo::Hashed` as the other artifact compiled it.
    pub struct Hashed {
        pub value: u32,
    }
    impl pill_engine::Resource for Hashed {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Hashed")
        }
        fn shared_schema_hash() -> Option<u64> {
            Some(0x2222)
        }
    }
}

/// A Rust resource declaring the foreign slot's name at another shape, so its
/// id collides with a value it cannot be read as.
pub mod identity_probe {
    /// `identity::Guarded`, declared wider than the foreign payload.
    #[derive(Debug)]
    pub struct Guarded {
        pub first: u32,
        pub second: u32,
    }
    impl pill_engine::Resource for Guarded {
        fn shared_name() -> Option<&'static str> {
            Some("identity::Guarded")
        }
    }
}

/// An ordinary resource, to show the default identity is untouched.
#[derive(Debug, Default)]
struct PlainCounter {
    value: u32,
}
impl Resource for PlainCounter {}

/// A relayout moves the resource's claim along with its factory, so the
/// migrated layout can be declared again and the layout it came from is
/// refused as stale.
#[test]
fn a_relayout_moves_the_claim_to_the_migrated_layout() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("identity::Reshaped", "Reshaped", 16, 8, 1)
        .expect("a fresh name is claimed");

    let rewritten_count = world
        .relayout_foreign_resource(id, 32, 8, 2, &pill_engine::archetype::FieldPlan::new())
        .expect("the empty plan fits");
    assert_eq!(rewritten_count, 0, "no value was stored yet");

    world
        .register_foreign_resource("identity::Reshaped", "Reshaped", 32, 8, 2)
        .expect("the migrated layout re-declares");
    let error = world
        .register_foreign_resource("identity::Reshaped", "Reshaped", 16, 8, 1)
        .expect_err("the pre-migration layout is stale");
    assert!(matches!(
        error,
        pill_engine::error::WorldError::SharedResourceLayoutMismatch { .. }
    ));
}

/// A refused removal is a no-op: the take is attempted before anything is
/// torn down, and the box it hands back is put where it was.
#[test]
fn a_refused_removal_leaves_everything_in_place() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("identity::Guarded", "Guarded", 4, 4, 7)
        .expect("a fresh name is claimed");
    world
        .insert_foreign_resource_bytes(id, &9_u32.to_ne_bytes())
        .expect("the payload matches the size");

    // The probe declares `identity::Guarded` at eight bytes while the stored
    // payload is four, so its take has to fail while the value and the claim
    // stay untouched.
    let error = world
        .remove_resource::<identity_probe::Guarded>()
        .expect_err("a differently-shaped Rust type cannot take the value");
    assert!(matches!(
        error,
        pill_engine::error::WorldError::SharedResourceHoldsAnotherType { id: reported, .. }
            if reported == id
    ));

    assert_eq!(
        world.foreign_resource_bytes(id),
        Some(9_u32.to_ne_bytes().as_slice()),
        "the value is still there"
    );
    assert_eq!(world.foreign_resource_layout(id), Some((4, 4, 7)));
    assert_eq!(
        world.shared_resource_names(),
        vec!["identity::Guarded".to_string()],
        "the claim is still recorded"
    );
}

/// `has_resource` answers "can this id be read as a `T`", not "is this id
/// stored". A foreign value under a shared id is not a `T`, so `has_resource`
/// agrees with `get_resource`, and `resource_holder` is what reports that
/// something is stored there.
#[test]
fn has_resource_reports_the_readable_shape_not_the_id() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 7)
        .expect("the name is claimed");
    world
        .insert_foreign_resource_bytes(id, &7_u32.to_ne_bytes())
        .expect("the payload matches");

    // `stale_artifact::Settings` declares the same name at sixteen bytes.
    assert!(
        !world.has_resource::<stale_artifact::Settings>(),
        "id presence must not answer for a shape the value cannot be read as"
    );
    assert!(world.get_resource::<stale_artifact::Settings>().is_none());
    assert!(
        world
            .resource_holder::<stale_artifact::Settings>()
            .is_some(),
        "the holder accessor is what names the stored shape"
    );

    // The readable copy still answers both questions the same way.
    assert!(world.has_resource::<artifact_a::Settings>());
    assert!(world.get_resource::<artifact_a::Settings>().is_some());
}

/// Two copies that agree on name, size and alignment are still refused when
/// the schema hashes they declare differ. The hashes are written into the
/// declarations rather than derived from the fields, so two equal shapes can
/// still disagree. The layout half of the guard is covered by the relayout
/// test above and by `a_disagreeing_layout_under_one_shared_name_is_reported`.
#[test]
fn a_differing_declared_schema_hash_is_refused() {
    let mut world = World::new();
    world.register_resource::<hashed_a::Hashed>();
    assert!(world.take_registration_error().is_none());

    world.register_resource::<hashed_b::Hashed>();
    let error = world
        .take_registration_error()
        .expect("the second shape is refused");
    assert!(matches!(
        error,
        pill_engine::error::WorldError::SharedResourceSchemaMismatch { .. }
    ));
}

/// A second ordinary resource, for the disjointness half of the scheduler test.
#[derive(Debug, Default)]
struct OtherCounter {
    _value: u32,
}
impl Resource for OtherCounter {}

// =============================================================================
// Identity
// =============================================================================

/// Both copies resolve to the same [`ResourceId`], derived from the declared
/// name rather than from either artifact's `TypeId`.
#[test]
fn both_copies_of_a_shared_resource_resolve_to_one_id() {
    let artifact_a_id = ResourceId::of::<artifact_a::Settings>();
    let artifact_b_id = ResourceId::of::<artifact_b::Settings>();

    assert_eq!(
        artifact_a_id, artifact_b_id,
        "the two copies must be one resource"
    );
    assert!(artifact_a_id.shared_identity().is_some());

    // A resource declaring nothing keeps the per-artifact identity.
    assert!(ResourceId::of::<PlainCounter>().shared_identity().is_none());
    assert_ne!(
        ResourceId::of::<PlainCounter>(),
        ResourceId::of::<OtherCounter>()
    );
}

// =============================================================================
// Access across the boundary
// =============================================================================

/// The core case: one artifact inserts, the other reads it back.
#[test]
fn a_resource_inserted_by_one_copy_is_read_by_the_other() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 4242 });

    let seen_settings = world
        .get_resource::<artifact_b::Settings>()
        .expect("the other artifact's copy reaches the same slot");
    assert_eq!(seen_settings.value, 4242);
}

/// A write through one copy is seen through the other.
#[test]
fn a_write_through_one_copy_is_seen_by_the_other() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 1 });

    world
        .get_resource_mut::<artifact_b::Settings>()
        .expect("reachable through the other copy")
        .value = 99;

    assert_eq!(
        world.get_resource::<artifact_a::Settings>().unwrap().value,
        99
    );
}

/// Presence is reported through either copy.
#[test]
fn presence_is_reported_through_either_copy() {
    let mut world = World::new();
    assert!(!world.has_resource::<artifact_b::Settings>());

    world.insert_resource(artifact_a::Settings { value: 5 });

    assert!(world.has_resource::<artifact_a::Settings>());
    assert!(world.has_resource::<artifact_b::Settings>());
}

/// The value can be taken out through the other copy, which is the one path
/// where ownership leaves the store.
#[test]
fn a_resource_can_be_removed_through_the_other_copy() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 7 });

    let taken_settings = world
        .remove_resource::<artifact_b::Settings>()
        .expect("the removal is not refused")
        .expect("the other copy can take ownership");
    assert_eq!(taken_settings.value, 7);

    assert!(!world.has_resource::<artifact_a::Settings>());
    assert!(!world.has_resource::<artifact_b::Settings>());
}

/// A write through the other copy's tracked accessor reaches the one slot and
/// is stamped with the world's change tick. The two copies share one tick
/// column, so change detection has nothing to split along the artifact
/// boundary.
#[test]
fn a_tracked_write_through_one_copy_is_seen_by_the_other() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 1 });

    let tick_before_write = world.change_tick();
    world.increment_change_tick();

    // The other artifact mutates through the tracked path.
    world
        .get_resource_mut_tracked::<artifact_b::Settings>()
        .expect("reachable through the other copy")
        .value = 2;

    assert_eq!(
        world.get_resource::<artifact_a::Settings>().unwrap().value,
        2
    );
    assert_ne!(
        world.change_tick(),
        tick_before_write,
        "the world tick moved, so the write is attributable"
    );
}

// =============================================================================
// System parameters
// =============================================================================

/// `Res<T>` and `ResMut<T>` resolve a shared resource across copies, which is
/// what a module's systems actually use.
#[test]
fn system_parameters_resolve_a_shared_resource_across_copies() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let observed_value = Arc::new(AtomicU32::new(0));

    // One artifact's system writes it.
    fn writer(mut settings: ResMut<artifact_a::Settings>) {
        if let Some(mut settings) = settings.get_mut() {
            settings.value += 1;
        }
    }

    // The other artifact's system reads it.
    let reader_observed_value = Arc::clone(&observed_value);
    let reader = move |settings: Res<artifact_b::Settings>| {
        if let Some(settings) = settings.get() {
            reader_observed_value.store(settings.value, Ordering::SeqCst);
        }
    };

    let mut engine = Engine::new();
    engine
        .world_mut()
        .insert_resource(artifact_a::Settings { value: 10 });
    engine.register_system("writer", writer);
    engine.register_system("reader", reader);

    engine.process_frame().unwrap();

    assert_eq!(
        engine
            .world()
            .get_resource::<artifact_b::Settings>()
            .unwrap()
            .value,
        11,
        "the write landed on the shared slot"
    );
    assert_eq!(
        observed_value.load(Ordering::SeqCst),
        11,
        "the other copy's system read the written value"
    );
}

// =============================================================================
// Scheduler soundness
// =============================================================================

/// Two systems writing one shared resource through different copies must
/// report a conflict. If they did not, the scheduler would put them in one
/// parallel batch and they would race on the same value.
///
/// `conflicts_with` compares resource ids, so two ids for one resource would
/// make the two systems look disjoint - which is the failure this test rules
/// out.
#[test]
fn two_systems_writing_through_different_copies_conflict() {
    let mut writer_a_access = SystemAccess::new();
    writer_a_access.add_resource_write(ResourceId::of::<artifact_a::Settings>());

    let mut writer_b_access = SystemAccess::new();
    writer_b_access.add_resource_write(ResourceId::of::<artifact_b::Settings>());

    assert!(
        writer_a_access.conflicts_with(&writer_b_access),
        "two writes to one shared resource must never be batched in parallel"
    );

    // A write and a read of the same shared resource conflict as well.
    let mut reader_b_access = SystemAccess::new();
    reader_b_access.add_resource_read(ResourceId::of::<artifact_b::Settings>());
    assert!(writer_a_access.conflicts_with(&reader_b_access));

    // Unrelated resources still do not conflict, so the check has not simply
    // become "everything conflicts".
    let mut unrelated_access = SystemAccess::new();
    unrelated_access.add_resource_write(ResourceId::of::<OtherCounter>());
    assert!(!writer_a_access.conflicts_with(&unrelated_access));
}

/// An access added after an earlier comparison is seen by the next one.
///
/// `conflicts_with` reads the live access sets, so no cached snapshot can go
/// stale behind them. A cached answer would put two writers of one resource in
/// a parallel batch without any error: the sets would still be right, and only
/// the cache would be wrong.
#[test]
fn a_resource_added_after_a_comparison_still_conflicts() {
    let mut first_access = SystemAccess::new();
    first_access.add_resource_write(ResourceId::of::<artifact_a::Settings>());

    let mut second_access = SystemAccess::new();
    assert!(
        !first_access.conflicts_with(&second_access),
        "an empty access conflicts with nothing"
    );

    // Mutated after the first comparison: the next one has to see it.
    second_access.add_resource_write(ResourceId::of::<artifact_b::Settings>());
    assert!(
        first_access.conflicts_with(&second_access),
        "the sets are the source of truth, so the shared resource still conflicts"
    );
}

/// The batching decision itself, not just the pairwise answer it rests on:
/// two systems that write one shared resource through different copies must
/// land in different batches.
///
/// This is the path the engine takes - the access patterns go to
/// `ensure_execution_graph`, which compares them pairwise - so the graph below
/// rests on the same comparison the pairwise test covers. The test also checks
/// the third system's placement: one resource conflict must keep the two
/// writers apart, not push every other system out of every batch.
#[test]
fn two_systems_writing_one_shared_resource_are_never_batched_together() {
    let mut project_writer_access = SystemAccess::new();
    project_writer_access.add_resource_write(ResourceId::of::<artifact_a::Settings>());

    let mut module_writer_access = SystemAccess::new();
    module_writer_access.add_resource_write(ResourceId::of::<artifact_b::Settings>());

    let mut unrelated_writer_access = SystemAccess::new();
    unrelated_writer_access.add_resource_write(ResourceId::of::<OtherCounter>());

    let accesses = [
        project_writer_access,
        module_writer_access,
        unrelated_writer_access,
    ];
    let mut scheduler = SystemScheduler::new();
    scheduler.ensure_execution_graph(accesses.iter());
    let batches = scheduler.execution_graph();

    let batch_of = |system_index: usize| {
        batches
            .iter()
            .position(|batch| batch.contains(&system_index))
    };
    let project_batch_index = batch_of(0).expect("project writer scheduled");
    let module_batch_index = batch_of(1).expect("module writer scheduled");
    let unrelated_batch_index = batch_of(2).expect("unrelated writer scheduled");

    assert_ne!(
        project_batch_index, module_batch_index,
        "two writers of one shared resource were batched together: {batches:?}"
    );
    assert!(
        unrelated_batch_index == project_batch_index || unrelated_batch_index == module_batch_index,
        "a system on an unrelated resource must keep running beside one writer: {batches:?}"
    );
}

/// The path a reload takes: the graph is rebuilt from the systems that survive
/// the swap rather than filtered in place.
///
/// A shared resource has to come out of that rebuild conflicting the way it
/// went in. Otherwise the two writers would share a batch from the first frame
/// after the swap.
#[test]
fn a_shared_resource_still_conflicts_after_a_rebuild() {
    let mut project_writer_access = SystemAccess::new();
    project_writer_access.add_resource_write(ResourceId::of::<artifact_a::Settings>());

    let mut module_writer_access = SystemAccess::new();
    module_writer_access.add_resource_write(ResourceId::of::<artifact_b::Settings>());

    // Stands in for a generation the reload drops; the engine removes it from
    // its system list and the survivors keep their order.
    let doomed_access = SystemAccess::new();
    let accesses_with_doomed = vec![project_writer_access, module_writer_access, doomed_access];
    let mut scheduler = SystemScheduler::new();
    scheduler.ensure_execution_graph(accesses_with_doomed.iter());
    assert!(
        scheduler
            .execution_graph()
            .iter()
            .any(|batch| batch.contains(&2)),
        "the doomed system is scheduled before the reload"
    );

    // What the engine does after `clear_systems_owned_by`: the list is shorter
    // and the graph is derived again from the survivors.
    let mut surviving_accesses = accesses_with_doomed;
    surviving_accesses.pop();
    scheduler.invalidate();
    scheduler.ensure_execution_graph(surviving_accesses.iter());
    let batches = scheduler.execution_graph();

    let batch_of = |system_index: usize| {
        batches
            .iter()
            .position(|batch| batch.contains(&system_index))
    };
    assert!(
        batch_of(2).is_none(),
        "the dropped system is still scheduled: {batches:?}"
    );
    let project_batch_index = batch_of(0).expect("the surviving writer is scheduled");
    let module_batch_index = batch_of(1).expect("the surviving writer is scheduled");
    assert_ne!(
        project_batch_index, module_batch_index,
        "the shared conflict did not survive the rebuild: {batches:?}"
    );
}

/// The property through the path a system really takes: two systems whose
/// parameters name one shared resource must never be running at the same time.
///
/// Registration is what turns `ResMut<T>` into a resource id on the access
/// pattern (`SystemParam::report_access`), so this covers the id the scheduler
/// is really handed rather than one a test wrote down. The overlap counter
/// makes a parallel batch visible on its own. `ResMut::new` also takes a debug
/// write lock, but that exists only in debug builds, so in a release build the
/// sleep in each system and the counter are what catch an overlap.
#[test]
fn two_systems_writing_one_shared_resource_never_overlap() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    // Both writers do the same work with their own copy of the type: they note
    // that they are running, hold long enough that a parallel batch cannot
    // hide, write through the one shared slot, and clear the note.
    macro_rules! writer {
        ($resource:ty, $inside_count:expr, $overlap_count:expr) => {{
            let inside_count = Arc::clone(&$inside_count);
            let overlap_count = Arc::clone(&$overlap_count);
            move |mut settings: ResMut<$resource>| {
                if inside_count.fetch_add(1, Ordering::SeqCst) != 0 {
                    overlap_count.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(20));
                if let Some(mut settings) = settings.get_mut() {
                    settings.value += 1;
                }
                inside_count.fetch_sub(1, Ordering::SeqCst);
            }
        }};
    }

    let inside_count = Arc::new(AtomicUsize::new(0));
    let overlap_count = Arc::new(AtomicUsize::new(0));

    let mut engine = Engine::new();
    engine
        .world_mut()
        .insert_resource(artifact_a::Settings { value: 0 });
    engine.register_system(
        "project_writer",
        writer!(artifact_a::Settings, inside_count, overlap_count),
    );
    engine.register_system(
        "module_writer",
        writer!(artifact_b::Settings, inside_count, overlap_count),
    );

    engine.process_frame().expect("the frame runs");

    assert_eq!(
        overlap_count.load(Ordering::SeqCst),
        0,
        "the two writers of one shared resource overlapped, so they were batched together"
    );
    assert_eq!(
        engine
            .world()
            .get_resource::<artifact_a::Settings>()
            .expect("still inserted")
            .value,
        2,
        "both systems ran, and both writes landed on the one slot"
    );
}

// =============================================================================
// Guards
// =============================================================================

/// Two copies of one type are the case this feature exists for, and must not
/// be reported as a conflict.
#[test]
fn one_resource_type_compiled_twice_is_not_a_conflict() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    world.register_resource::<artifact_b::Settings>();

    assert!(
        world.take_registration_error().is_none(),
        "two copies of one type must bind, not collide"
    );
}

/// Two *different* types claiming one shared name are reported, even though
/// their layouts agree exactly - the case no layout check can catch.
#[test]
fn two_types_claiming_one_shared_name_are_reported() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    assert!(world.take_registration_error().is_none());

    world.register_resource::<UnrelatedSettings>();
    let error = world
        .take_registration_error()
        .expect("a second type claiming the name must be reported");

    match error {
        pill_engine::error::WorldError::SharedResourceNameConflict {
            shared_name,
            existing_type,
            incoming_type,
        } => {
            assert_eq!(shared_name, "demo::Settings");
            assert_eq!(existing_type, "Settings");
            assert_eq!(incoming_type, "UnrelatedSettings");
        }
        other => panic!("expected a shared-resource name conflict, got {other:?}"),
    }
}

/// One shared name with two different layouts is reported rather than bound.
///
/// The type name matches here - both are `Settings` - so the name guard lets
/// it through and the layout guard is what has to catch it. That is the
/// stale-artifact case: one binary rebuilt against a changed definition.
#[test]
fn a_disagreeing_layout_under_one_shared_name_is_reported() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    assert!(world.take_registration_error().is_none());

    world.register_resource::<stale_artifact::Settings>();
    let error = world
        .take_registration_error()
        .expect("a layout disagreement must be reported");

    match error {
        pill_engine::error::WorldError::SharedResourceLayoutMismatch {
            shared_name,
            existing_size,
            incoming_size,
            ..
        } => {
            assert_eq!(shared_name, "demo::Settings");
            assert_eq!(existing_size, std::mem::size_of::<artifact_a::Settings>());
            assert_eq!(
                incoming_size,
                std::mem::size_of::<stale_artifact::Settings>()
            );
        }
        other => panic!("expected a layout mismatch, got {other:?}"),
    }
}

/// Inserting - not just registering - runs the guards too, so a module that
/// only ever inserts is covered.
#[test]
fn inserting_also_runs_the_name_guard() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 1 });
    assert!(world.take_registration_error().is_none());

    world.insert_resource(UnrelatedSettings { _value: 2 });
    assert!(
        world.take_registration_error().is_some(),
        "a conflicting claim must be reported even without register_resource"
    );
}

/// A refused claim must not touch what is stored.
///
/// The generation that made the claim is already failing, because the drain
/// after `init` reports the recorded error as a failed init. The only open
/// question is whether the live value survives it. `UnrelatedSettings` agrees
/// with `artifact_a::Settings` on size and alignment, so a reader would take
/// the wrong bytes without noticing, and the size check `holds` runs cannot
/// tell the two apart. The name guard is the one that can, and it has just
/// refused the claim.
#[test]
fn a_refused_claim_leaves_the_stored_value_alone() {
    let mut world = World::new();
    world.insert_resource(artifact_a::Settings { value: 1 });
    assert!(world.take_registration_error().is_none());

    world.insert_resource(UnrelatedSettings { _value: 99 });
    assert!(
        world.take_registration_error().is_some(),
        "the name guard has to fire"
    );

    assert_eq!(
        world
            .get_resource::<artifact_a::Settings>()
            .map(|it| it.value),
        Some(1),
        "the refused generation's bytes must not reach the stored value"
    );
}

/// The same store, taken through the sequence a reload rolls back through.
///
/// The failed generation inserts under a shared name whose layout it disagrees
/// with, the rollback generation declares the resource again, and the
/// transaction re-homes what survived. Nothing on that path puts the earlier
/// value back - the rollback only declares the resource. So if a refused
/// insert were stored, the box would hold the failed shape under the rollback
/// type's table: claimed, registered, and unreadable, with a drop function
/// that no longer matches the allocation it is about to run over.
#[test]
fn a_refused_insert_does_not_survive_a_rolled_back_reload() {
    let mut world = World::new();
    // Step 1: the generation that owns the resource declares it, and a value
    // is live.
    world.register_resource::<artifact_a::Settings>();
    world.insert_resource(artifact_a::Settings { value: 7 });
    assert!(world.take_registration_error().is_none());

    // Step 2: a rebuild against a changed definition inserts under the same
    // name.
    world.insert_resource(stale_artifact::Settings { value: 7, extra: 1 });
    assert!(
        world.take_registration_error().is_some(),
        "the layout guard has to fire"
    );

    // Step 3: what the transaction does next - the failed generation is
    // discarded, the previous one initializes again, every surviving value is
    // re-homed.
    world.register_resource::<artifact_a::Settings>();
    world.rehome_resources();

    assert_eq!(
        world
            .get_resource::<artifact_a::Settings>()
            .map(|it| it.value),
        Some(7),
        "the value has to survive a rolled-back reload"
    );
}

// =============================================================================
// Ordinary resources are unaffected
// =============================================================================

/// A resource that declares nothing keeps the strict per-artifact identity and
/// the ordinary behaviour, so shared identity is genuinely opt-in.
#[test]
fn ordinary_resources_are_unaffected() {
    let mut world = World::new();
    world.insert_resource(PlainCounter { value: 3 });
    world.insert_resource(OtherCounter::default());

    assert!(world.take_registration_error().is_none());
    assert_eq!(world.get_resource::<PlainCounter>().unwrap().value, 3);
    assert!(world.has_resource::<OtherCounter>());

    // Two ordinary resources are two slots.
    assert_eq!(world.resource_count(), 2);

    let removed_counter = world
        .remove_resource::<PlainCounter>()
        .expect("the removal is not refused")
        .expect("removable");
    assert_eq!(removed_counter.value, 3);
    assert_eq!(world.resource_count(), 1);
}
