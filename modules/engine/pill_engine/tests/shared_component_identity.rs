//! Integration tests for components that keep one identity across binaries.
//!
//! # Responsibilities
//!
//! - Pin that one shared name binds two distinct types to one component.
//! - Pin the refusals: a layout mismatch, and a second unrelated type claiming
//!   the same name.
//! - Pin that both copies' rows survive a reload's migration, and that only a
//!   subject current for the component may re-lay it out.
//!
//! # What is being simulated
//!
//! The situation these tests exist for cannot be reproduced literally in one
//! test binary: it needs one component type compiled into two artifacts loaded
//! into the same process, which is what gives that single type two `TypeId`s.
//!
//! Two distinct Rust types declaring the *same* `#[pill(shared = "...")]` name
//! reproduce it faithfully, because that is what the engine compares in the
//! real case too: two `TypeId`s, one declared identity, the type's own name and
//! identical layouts. `ProjectSpline` stands for the project's copy of the type
//! and `ModuleSpline` for the module DLL's copy; everything below is written
//! from that reading.

// Relayout and re-homing are development-only engine APIs.
#![cfg(feature = "development")]

// Standard library
use std::collections::HashSet;

// External crates
use pill_engine::component::ComponentRegistry;
use pill_engine::query::{Changed, Query};
use pill_engine::{component, ComponentId, World};

// Current crate
use module_copy::Spline as ModuleSpline;
use module_persisted::PersistedSpline as ModulePersistedSpline;
use project_copy::Spline as ProjectSpline;
use project_persisted::PersistedSpline as ProjectPersistedSpline;

/// The shared name both stand-in types declare, spelled once so a test can
/// assert against the identity it produces.
const SHARED_NAME: &str = "pill_spline::Spline";

// The two copies live in modules that stand for the two artifacts and share the
// type's own name, as the real case does: `Spline` is `Spline` in whichever
// binary compiled it. Only the module path differs, which is the one thing an
// in-process simulation cannot reproduce.

/// The project's copy of the shared component.
pub mod project_copy {
    /// `pill_spline::Spline` as the project compiled it.
    #[pill_engine::component(shared = "pill_spline::Spline")]
    #[derive(Clone, Debug, Default)]
    #[repr(C)]
    pub struct Spline {
        pub tension: f32,
        pub segments: u32,
    }
}

/// The module DLL's copy of the same component: a different Rust type, hence a
/// different `TypeId`, declaring the same shared name and the same layout.
pub mod module_copy {
    /// `pill_spline::Spline` as the module DLL compiled it.
    #[pill_engine::component(shared = "pill_spline::Spline")]
    #[derive(Clone, Debug, Default)]
    #[repr(C)]
    pub struct Spline {
        pub tension: f32,
        pub segments: u32,
    }
}

/// An ordinary component, to show the default identity is untouched.
#[component]
#[derive(Clone, Debug, Default)]
struct PlainMarker {
    value: u32,
}

/// A second ordinary component that forces a different archetype.
#[component]
#[derive(Clone, Debug, Default)]
struct OtherMarker {
    value: u32,
}

// =============================================================================
// Identity
// =============================================================================

/// Both copies of the type resolve to the same [`ComponentId`], derived from
/// the declared name rather than from either binary's `TypeId`.
#[test]
fn both_copies_of_a_shared_type_resolve_to_one_component_id() {
    let project_id = ComponentId::of::<ProjectSpline>();
    let module_id = ComponentId::of::<ModuleSpline>();

    assert_eq!(
        project_id, module_id,
        "the two copies must be one component"
    );
    assert_eq!(
        project_id,
        ComponentId::Shared(pill_engine::component::shared_component_identity(
            SHARED_NAME
        )),
        "the identity is a function of the declared name alone"
    );
    // A component that declares nothing keeps the per-binary identity.
    assert!(matches!(
        ComponentId::of::<PlainMarker>(),
        ComponentId::Native(_)
    ));
}

/// The declared name is what the registry records, so every name-keyed lookup
/// finds the component under the name both binaries agreed on.
#[test]
fn a_shared_component_is_registered_under_its_declared_name() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();

    let component_id = ComponentId::of::<ProjectSpline>();
    assert_eq!(
        world.component_registry().get_name(&component_id),
        Some(SHARED_NAME)
    );
    assert_eq!(
        world.resolve_component_id_by_name_any(SHARED_NAME).unwrap(),
        Some(component_id)
    );
    assert_eq!(
        ComponentRegistry::registered_name::<ModuleSpline>(),
        SHARED_NAME,
        "the other binary's copy registers under the same name"
    );
}

// =============================================================================
// Binding: one bit, one column
// =============================================================================

/// The second registration binds to the column the first created rather than
/// allocating a second one: one bit is consumed, not two.
#[test]
fn the_second_registration_binds_instead_of_allocating() {
    let mut world = World::new();

    world.register_component::<ProjectSpline>();
    let component_count_after_first_registration = world.component_registry().len();
    let component_bit = world
        .component_registry()
        .get_bit(&ComponentId::of::<ProjectSpline>())
        .expect("the first registration creates the bit");

    // The module's copy registers into the same world, as it would when its
    // DLL's `init` runs against the host's world.
    world.register_component::<ModuleSpline>();

    assert_eq!(
        world.component_registry().len(),
        component_count_after_first_registration,
        "binding must not add a second registry entry"
    );
    assert_eq!(
        world
            .component_registry()
            .get_bit(&ComponentId::of::<ModuleSpline>()),
        Some(component_bit),
        "both copies must resolve to one mask bit"
    );
    assert!(
        world.take_registration_error().is_none(),
        "binding two matching layouts is not an error"
    );
}

/// Entities spawned through either copy land in the same archetype, which is
/// the outcome the whole feature exists to produce.
#[test]
fn entities_spawned_from_either_copy_share_one_archetype() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();

    let entity_from_project = world
        .create_entity()
        .with(ProjectSpline {
            tension: 1.0,
            segments: 4,
        })
        .build()
        .unwrap();
    let entity_from_module = world
        .create_entity()
        .with(ModuleSpline {
            tension: 2.0,
            segments: 8,
        })
        .build()
        .unwrap();

    let archetypes: Vec<_> = world
        .archetypes_iter()
        .filter(|archetype| !archetype.entities.is_empty())
        .collect();
    assert_eq!(
        archetypes.len(),
        1,
        "two copies of one component must not produce two archetypes"
    );
    assert_eq!(archetypes[0].entities.len(), 2);
    assert!(archetypes[0].entities.contains(&entity_from_project));
    assert!(archetypes[0].entities.contains(&entity_from_module));

    // And both rows live in one column.
    assert_eq!(
        world.live_row_count(ComponentId::of::<ProjectSpline>()),
        2,
        "both entities' rows belong to the same component"
    );
}

// =============================================================================
// Access across the boundary
// =============================================================================

/// The decisive test: one copy writes through an ordinary typed query and the
/// other reads the result, because both address the same rows.
#[test]
fn a_write_through_one_copy_is_observed_through_the_other() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();

    let entity = world
        .create_entity()
        .with(ProjectSpline {
            tension: 1.0,
            segments: 4,
        })
        .build()
        .unwrap();

    // The module queries the component under its own type and writes through
    // it.
    {
        let mut query = Query::<(&mut ModuleSpline,)>::new(&mut world);
        let mut row_count = 0;
        for (mut spline,) in query.iter_mut() {
            spline.tension = 9.5;
            spline.segments = 16;
            row_count += 1;
        }
        assert_eq!(
            row_count, 1,
            "the module's query must see the project's entity"
        );
    }

    // The project reads its own type back and sees the module's writes.
    let observed = world
        .get_component::<ProjectSpline>(entity)
        .expect("the project still reaches the row through its own type");
    assert_eq!(observed.tension, 9.5);
    assert_eq!(observed.segments, 16);
}

/// Reading through the other copy works for plain `&T` queries too, and yields
/// every row regardless of which copy created it.
#[test]
fn a_read_query_through_one_copy_sees_rows_created_by_the_other() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();

    world
        .create_entity()
        .with(ProjectSpline {
            tension: 1.0,
            segments: 1,
        })
        .build()
        .unwrap();
    world
        .create_entity()
        .with(ModuleSpline {
            tension: 2.0,
            segments: 2,
        })
        .build()
        .unwrap();

    let mut query = Query::<(&ProjectSpline,)>::new(&mut world);
    let mut tensions: Vec<f32> = query.iter_mut().map(|(spline,)| spline.tension).collect();
    tensions.sort_by(f32::total_cmp);
    assert_eq!(tensions, vec![1.0, 2.0]);
}

/// A change tick written through one copy is seen by a `Changed` filter on the
/// other, so change detection does not split at the binary boundary.
#[test]
fn a_change_tick_written_by_one_copy_is_seen_by_the_other() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();

    world
        .create_entity()
        .with(ProjectSpline {
            tension: 1.0,
            segments: 1,
        })
        .build()
        .unwrap();

    // Move the world past the spawn so the row is not merely "just added".
    world.increment_change_tick();
    world.set_system_last_run(world.change_tick());
    world.increment_change_tick();

    // The module mutates the row.
    {
        let mut query = Query::<(&mut ModuleSpline,)>::new(&mut world);
        for (mut spline,) in query.iter_mut() {
            spline.tension = 4.0;
        }
    }

    // The project's `Changed` filter, keyed on its own type, observes it.
    let mut query = Query::<(&ProjectSpline,), Changed<ProjectSpline>>::new(&mut world);
    assert_eq!(
        query.iter_mut().count(),
        1,
        "the mutation must be visible to the other copy's change filter"
    );
}

// =============================================================================
// Scheduler soundness
// =============================================================================

/// Two systems writing the same shared component through different copies must
/// report a conflict, or the scheduler would batch them in parallel and they
/// would race on the same rows.
///
/// This is about soundness, not only correctness: the scheduler runs systems
/// that report no conflict in parallel, so batched writers would race on the
/// same rows. It is also why the id and the column arrive together: both copies
/// resolve to one id in the same registration step.
#[test]
fn two_systems_writing_through_different_copies_conflict() {
    use pill_engine::scheduler::SystemAccess;

    // The project's system writes the component under its own type.
    let mut project_access = SystemAccess::new();
    project_access.add_write(ComponentId::of::<ProjectSpline>());

    // The module's system writes it under its own type.
    let mut module_access = SystemAccess::new();
    module_access.add_write(ComponentId::of::<ModuleSpline>());

    assert!(
        project_access.conflicts_with(&module_access),
        "two writes to one column must never be batched in parallel"
    );

    // A write and a read of the same shared component conflict as well.
    let mut reader_access = SystemAccess::new();
    reader_access.add_read(ComponentId::of::<ModuleSpline>());
    assert!(project_access.conflicts_with(&reader_access));

    // An unrelated component still does not conflict, so the check has not
    // simply become "everything conflicts".
    let mut unrelated_access = SystemAccess::new();
    unrelated_access.add_write(ComponentId::of::<PlainMarker>());
    assert!(!project_access.conflicts_with(&unrelated_access));
}

// =============================================================================
// Layout verification
// =============================================================================

/// The layout check rejects a second copy whose memory shape disagrees, rather
/// than binding it and letting one binary misread the other's rows.
#[test]
fn a_disagreeing_layout_is_reported_instead_of_bound() {
    // A third declaration of the same shared name whose fields are wider. In
    // the real case this is a module rebuilt against a changed definition while
    // the project still holds the old one.
    mod stale_copy {
        /// `pill_spline::Spline` as a module rebuilt against a changed
        /// definition declares it: same name, same declared identity, wider
        /// fields.
        #[pill_engine::component(shared = "pill_spline::Spline")]
        #[derive(Clone, Debug, Default)]
        #[repr(C)]
        pub struct Spline {
            pub tension: f64,
            pub segments: u64,
        }
    }
    use stale_copy::Spline as StaleSpline;

    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    assert!(world.take_registration_error().is_none());

    world.register_component::<StaleSpline>();
    let error = world
        .take_registration_error()
        .expect("a layout disagreement must be reported");

    match error {
        pill_engine::error::WorldError::SharedComponentLayoutMismatch {
            shared_name,
            existing_size,
            incoming_size,
            ..
        } => {
            assert_eq!(shared_name, SHARED_NAME);
            assert_eq!(existing_size, std::mem::size_of::<ProjectSpline>());
            assert_eq!(incoming_size, std::mem::size_of::<StaleSpline>());
        }
        other => panic!("expected a layout mismatch, got {other:?}"),
    }
}

/// Size and alignment alone are not layout: `{f32, f32}` and `{u32, u32}` agree
/// on both, and binding them would reinterpret every row's bytes silently. The
/// schema hash is what tells them apart, so this pins that a field's type
/// changes it.
#[test]
fn a_same_sized_but_differently_typed_layout_is_still_rejected() {
    use pill_engine::component::component_schema_hash;
    use pill_engine::component_registry::ComponentFieldDescriptor;

    const FIELDS_AS_FLOATS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "tension",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "segments",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];
    const FIELDS_AS_INTEGERS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "tension",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "segments",
            type_tag: "u32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    assert_ne!(
        component_schema_hash(FIELDS_AS_FLOATS),
        component_schema_hash(FIELDS_AS_INTEGERS),
        "a field's type must change the schema hash even when its size does not"
    );

    // Reordering fields changes it too, though the struct's size does not.
    const REORDERED_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "segments",
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "tension",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];
    assert_ne!(
        component_schema_hash(FIELDS_AS_INTEGERS),
        component_schema_hash(REORDERED_FIELDS),
        "a field reorder must change the schema hash"
    );
}

// =============================================================================
// Ordinary components are unaffected
// =============================================================================

/// Components that declare nothing keep entirely separate identities, columns
/// and archetypes, so shared identity is genuinely opt-in.
#[test]
fn ordinary_components_keep_separate_identities() {
    let mut world = World::new();
    world.register_component::<PlainMarker>();
    world.register_component::<OtherMarker>();

    assert_ne!(
        ComponentId::of::<PlainMarker>(),
        ComponentId::of::<OtherMarker>()
    );
    assert_eq!(world.component_registry().len(), 2, "two bits, not one");

    world
        .create_entity()
        .with(PlainMarker { value: 1 })
        .build()
        .unwrap();
    world
        .create_entity()
        .with(OtherMarker { value: 2 })
        .build()
        .unwrap();

    let populated_archetype_count = world
        .archetypes_iter()
        .filter(|archetype| !archetype.entities.is_empty())
        .count();
    assert_eq!(
        populated_archetype_count, 2,
        "unrelated components stay in their own columns"
    );
}

// =============================================================================
// Paths that move rows rather than read them
// =============================================================================

/// Moving an entity between archetypes copies its shared component's row to the
/// destination column byte for byte. Both copies resolve to one column, so this
/// pins that a row written through one copy survives a move and is still
/// readable through the other.
#[test]
fn a_shared_row_survives_an_archetype_move() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();
    world.register_component::<PlainMarker>();

    let entity = world
        .create_entity()
        .with(ProjectSpline {
            tension: 3.25,
            segments: 12,
        })
        .build()
        .unwrap();

    // Adding a second component moves the entity into a different archetype,
    // copying every existing column's row across.
    world
        .add_component(entity, PlainMarker { value: 7 })
        .unwrap();

    let moved_spline = world
        .get_component::<ProjectSpline>(entity)
        .expect("the row must survive the move");
    assert_eq!(moved_spline.tension, 3.25);
    assert_eq!(moved_spline.segments, 12);

    // And it is still the same single column, reachable from the other copy.
    let seen_from_module_copy = world
        .get_component::<ModuleSpline>(entity)
        .expect("the other copy reaches the moved row too");
    assert_eq!(seen_from_module_copy.tension, 3.25);
}

/// Removing a shared component drops its row with the column's registered
/// destructor and leaves the entity's other components untouched.
#[test]
fn a_shared_row_can_be_removed_without_disturbing_its_neighbours() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();
    world.register_component::<PlainMarker>();

    let entity = world
        .create_entity()
        .with(ProjectSpline {
            tension: 1.5,
            segments: 3,
        })
        .with(PlainMarker { value: 11 })
        .build()
        .unwrap();

    // Removal is requested through the *other* copy's type.
    world.remove_component::<ModuleSpline>(entity).unwrap();

    assert!(world.get_component::<ProjectSpline>(entity).is_none());
    assert_eq!(
        world.get_component::<PlainMarker>(entity).unwrap().value,
        11
    );
    assert_eq!(world.live_row_count(ComponentId::of::<ProjectSpline>()), 0);
}

// =============================================================================
// Hot reload
// =============================================================================

/// A persistable shared component, as each artifact compiled it.
pub mod project_persisted {
    /// `pill_spline::PersistedSpline` as the project compiled it.
    #[pill_engine::component(persistable, shared = "pill_spline::PersistedSpline")]
    #[derive(Clone, Debug, Default)]
    #[repr(C)]
    pub struct PersistedSpline {
        pub tension: f32,
        pub segments: u32,
    }
}

/// The other binary's copy of the same persistable shared component.
pub mod module_persisted {
    /// `pill_spline::PersistedSpline` as the module DLL compiled it.
    #[pill_engine::component(persistable, shared = "pill_spline::PersistedSpline")]
    #[derive(Clone, Debug, Default)]
    #[repr(C)]
    pub struct PersistedSpline {
        pub tension: f32,
        pub segments: u32,
    }
}

/// Rows survive a reload migration no matter which copy registered them.
///
/// Both copies resolve to one identity, so the persist maps hold one
/// registration per shared name. The selective migration resolves the name to
/// that single id and carries every row across, whichever copy created it.
#[test]
fn shared_rows_survive_a_migration_from_either_copy() {
    let mut world = World::new();
    world.register_persistable_component::<ProjectPersistedSpline>();
    world.register_persistable_component::<ModulePersistedSpline>();
    assert!(
        world.take_registration_error().is_none(),
        "registering both copies must not report a peer collision"
    );

    // Rows created through each copy.
    world
        .create_entity()
        .with(ProjectPersistedSpline {
            tension: 1.0,
            segments: 1,
        })
        .build()
        .unwrap();
    world
        .create_entity()
        .with(ModulePersistedSpline {
            tension: 2.0,
            segments: 2,
        })
        .build()
        .unwrap();

    // The reload the host performs: capture the retiring generation's
    // serializer, let both copies register again exactly as their `init`
    // functions would, and migrate the shared name. If the copies had registered
    // as two types, only one copy's rows could move, and the assertions below
    // would show it.
    let previous_metadata = world.capture_persist_type_metadata();
    world.register_persistable_component::<ProjectPersistedSpline>();
    world.register_persistable_component::<ModulePersistedSpline>();
    let changed_type_names = HashSet::from(["pill_spline::PersistedSpline".to_string()]);
    let migration_report =
        world.migrate_changed_persistable_components(&previous_metadata, &changed_type_names, None);
    assert_eq!(
        migration_report.migrated_type_count, 1,
        "the shared name migrates as one type"
    );
    assert!(
        migration_report.skipped_types.is_empty(),
        "no copy may be skipped: {:?}",
        migration_report.skipped_types
    );
    assert_eq!(
        migration_report.migrated_entity_count, 2,
        "both copies' rows are carried across"
    );

    let mut query = Query::<(&ProjectPersistedSpline,)>::new(&mut world);
    let mut migrated_rows: Vec<(f32, u32)> = query
        .iter_mut()
        .map(|(spline,)| (spline.tension, spline.segments))
        .collect();
    migrated_rows.sort_by(|left, right| left.0.total_cmp(&right.0));

    assert_eq!(
        migrated_rows,
        vec![(1.0, 1), (2.0, 2)],
        "neither copy's rows may be dropped by the migration"
    );
}

/// The persistable manifest carries one entry for a shared component, under
/// its declared name, so the reload path sees one type and not two
/// registrations of one name.
#[test]
fn a_shared_persistable_component_appears_once_in_the_manifest() {
    let mut world = World::new();
    world.register_persistable_component::<ProjectPersistedSpline>();
    world.register_persistable_component::<ModulePersistedSpline>();

    let manifest_entries: Vec<_> = world
        .persist_type_manifest()
        .into_iter()
        .filter(|entry| entry.type_name == "pill_spline::PersistedSpline")
        .collect();

    assert_eq!(
        manifest_entries.len(),
        1,
        "one component, one manifest entry"
    );
    assert_eq!(
        manifest_entries[0].component_id,
        ComponentId::of::<ProjectPersistedSpline>()
    );
}

// =============================================================================
// Name collision between unrelated crates
// =============================================================================

// Two artifacts deliberately writing the *same qualified* name is the one
// collision the derive cannot catch: at the macro, it looks exactly like two
// copies of one type declaring it. A bare `"Transform"` is a compile error, so
// a collision needs a qualified name written on purpose.

/// One crate's `Transform`.
#[component(shared = "engine::Transform")]
#[derive(Clone, Debug, Default)]
#[repr(C)]
struct PhysicsTransform {
    x: f32,
    y: f32,
    z: f32,
}

/// An unrelated crate's `Transform` claiming the same qualified name, with a
/// *different* layout.
#[component(shared = "engine::Transform")]
#[derive(Clone, Debug, Default)]
#[repr(C)]
struct UiTransform {
    x: f64,
    y: f64,
}

/// An unrelated crate's `Transform` claiming the same qualified name with a
/// byte-identical layout but a completely different meaning.
#[component(shared = "engine::Transform")]
#[derive(Clone, Debug, Default)]
#[repr(C)]
struct AudioTransform {
    x: f32,
    y: f32,
    z: f32,
}

/// The same qualified name with a wider layout is refused at registration.
#[test]
fn colliding_names_with_different_layouts_are_caught() {
    let mut world = World::new();
    world.register_component::<PhysicsTransform>();
    assert!(world.take_registration_error().is_none());

    world.register_component::<UiTransform>();
    assert!(
        world.take_registration_error().is_some(),
        "a layout disagreement under one name must be reported"
    );
}

/// Two different types claiming one shared name are rejected even when their
/// layouts agree exactly. No layout check can catch that case: every read
/// through either type would succeed and quietly return the other component's
/// rows.
///
/// The discriminator is the Rust type's own name: two copies of one type agree
/// on it (`Spline` is `Spline` in whichever artifact compiled it), while two
/// unrelated components do not.
#[test]
fn identical_layouts_under_one_qualified_name_are_rejected() {
    let mut world = World::new();
    world.register_component::<PhysicsTransform>();
    assert!(world.take_registration_error().is_none());

    // Same declared name, byte-identical layout, entirely unrelated component.
    world.register_component::<AudioTransform>();
    let error = world
        .take_registration_error()
        .expect("two types claiming one shared name must be reported");

    match error {
        pill_engine::error::WorldError::SharedComponentNameConflict {
            shared_name,
            existing_type,
            incoming_type,
        } => {
            assert_eq!(shared_name, "engine::Transform");
            assert_eq!(existing_type, "PhysicsTransform");
            assert_eq!(incoming_type, "AudioTransform");
        }
        other => panic!("expected a shared-name conflict, got {other:?}"),
    }
}

/// The guard must not reject what the feature exists for: one type compiled
/// into two binaries. Those agree on the type's own name, which is exactly
/// what separates them from the collision above.
#[test]
fn one_type_compiled_twice_is_not_a_name_conflict() {
    let mut world = World::new();
    world.register_component::<ProjectSpline>();
    world.register_component::<ModuleSpline>();
    assert!(
        world.take_registration_error().is_none(),
        "two copies of one type must still bind"
    );
}

/// One persistable shared component in two layouts: the one now in force and
/// the one a stale binary was built against. Same size, fields swapped.
pub mod relaid_current {
    /// The layout the reloaded subject declares.
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    #[repr(C)]
    pub struct Relaid {
        pub first: u32,
        pub second: u32,
    }
    impl pill_engine::component::Component for Relaid {
        fn shared_name() -> Option<&'static str> {
            Some("audit::Relaid")
        }
    }
}

/// The layout the same shared component had before [`relaid_current::Relaid`]
/// replaced it.
pub mod relaid_previous {
    /// The layout a stale binary declares, with the fields swapped.
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    #[repr(C)]
    pub struct Relaid {
        pub second: u32,
        pub first: u32,
    }
    impl pill_engine::component::Component for Relaid {
        fn shared_name() -> Option<&'static str> {
            Some("audit::Relaid")
        }
    }
}

/// A two-field `u32` layout, with the field names in declaration order.
const fn relaid_fields(
    leading: &'static str,
    trailing: &'static str,
) -> [pill_engine::component_registry::ComponentFieldDescriptor; 2] {
    [
        pill_engine::component_registry::ComponentFieldDescriptor {
            name: leading,
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        pill_engine::component_registry::ComponentFieldDescriptor {
            name: trailing,
            type_tag: "u32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ]
}

static RELAID_CURRENT_FIELDS: [pill_engine::component_registry::ComponentFieldDescriptor; 2] =
    relaid_fields("first", "second");
static RELAID_PREVIOUS_FIELDS: [pill_engine::component_registry::ComponentFieldDescriptor; 2] =
    relaid_fields("second", "first");

/// A reload may re-lay out a shared component only if the subject was current
/// for it. A subject already stale for it - the rollback of a failed rebuild -
/// may not put the old layout back. Without the host's announcement, an old
/// layout is refused as the mismatch it is.
#[test]
fn only_a_current_subject_may_re_lay_out_a_shared_component() {
    const RELAID_SHARED_NAME: &str = "audit::Relaid";
    let mut world = World::new();
    // Step 1: register the previous layout, the one a stale binary declares.
    world.register_persistable_component_with_layout::<relaid_previous::Relaid>(
        &RELAID_PREVIOUS_FIELDS,
    );
    assert!(world.take_registration_error().is_none());
    let previous_hash = world
        .persist_schema_hash(RELAID_SHARED_NAME)
        .expect("registered");

    // Step 2: a reload of a subject current for the component: the new layout
    // wins.
    world.supersede_persist_registrations(&[RELAID_SHARED_NAME.to_owned()]);
    world.register_persistable_component_with_layout::<relaid_current::Relaid>(
        &RELAID_CURRENT_FIELDS,
    );
    world.clear_superseded_persist_registrations();
    assert!(
        world.take_registration_error().is_none(),
        "a superseding registration re-lays the component out"
    );
    let current_hash = world
        .persist_schema_hash(RELAID_SHARED_NAME)
        .expect("registered");
    assert_ne!(current_hash, previous_hash, "the new layout is recorded");

    // Step 3: the rollback of a subject stale for it: skipped, the layout is
    // kept.
    world.supersede_persist_registrations(&[RELAID_SHARED_NAME.to_owned()]);
    world.mark_stale_superseded_persist_registrations(&[RELAID_SHARED_NAME.to_owned()]);
    world.register_persistable_component_with_layout::<relaid_previous::Relaid>(
        &RELAID_PREVIOUS_FIELDS,
    );
    world.clear_superseded_persist_registrations();
    assert!(
        world.take_registration_error().is_none(),
        "a stale subject's registration is skipped, not failed"
    );
    assert_eq!(
        world.persist_schema_hash(RELAID_SHARED_NAME),
        Some(current_hash),
        "a stale subject cannot take the layout back"
    );

    // Step 4: unannounced, the old layout is the mismatch it always was.
    world.register_persistable_component_with_layout::<relaid_previous::Relaid>(
        &RELAID_PREVIOUS_FIELDS,
    );
    assert!(matches!(
        world.take_registration_error(),
        Some(pill_engine::error::WorldError::SharedComponentLayoutMismatch { .. })
    ));
}
