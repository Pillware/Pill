//! Unit tests for [`World`].
//!
//! # Responsibilities
//!
//! - Exercises the world's own behaviour: entity lifecycle and recycling,
//!   archetype migration, descriptor registration, relayout and remap, change
//!   ticks, resource re-homing, shared and foreign resources, zero-sized
//!   components, and script dispatch.
//! - Checks the readback the editor and the other tools use: component names
//!   and resolution, the registry listing and the editor's entity rows.
//!
//! # Design
//!
//! One test module that can reach every private part of the world, so a test
//! builds a world out of the parts it needs instead of declaring its own
//! fixtures.

// Current crate
use super::*;
#[cfg(feature = "development")]
use crate::archetype::{FieldSource, LayoutField};
use crate::engine::RunMode;
use crate::scripting::{ScriptDeclaration, ScriptDeclarationData};
use crate::test_fixtures::{setup_world, Health, Position, Velocity};

/// Returns the change-detection ticks of one entity's row for one component.
fn ticks_of_row(world: &World, entity: Entity, component_id: ComponentId) -> ComponentTicks {
    let location = world.storage.entity_locations[&entity];
    *world.storage.archetypes[&location.archetype_id]
        .component_storages
        .get(component_id)
        .expect("the archetype owns the column")
        .row_ticks(location.index_in_archetype)
        .expect("the row exists")
}

/// Builds a row of two `u32` fields, the shape the relayout tests start from.
fn two_u32_row(first_field_value: u32, second_field_value: u32) -> Vec<u8> {
    let mut bytes = first_field_value.to_ne_bytes().to_vec();
    bytes.extend_from_slice(&second_field_value.to_ne_bytes());
    bytes
}

/// A resource with a marker value, so a test can see which insertion left the
/// value it reads back.
#[derive(Debug, Default)]
struct AccountedResource {
    registration_marker: u32,
}
impl crate::resource::Resource for AccountedResource {}

/// A second resource type, so a test can tell a fresh registration from a
/// replacement of an existing one.
#[derive(Debug, Default)]
struct FreshAccountedResource {
    registration_marker: u32,
}
impl crate::resource::Resource for FreshAccountedResource {}

/// Returns a component type's registered name straight from the type, so the
/// tests below do not depend on the order types were registered in.
fn type_name_of<T: 'static>() -> String {
    crate::type_names::stable_type_name::<T>().to_string()
}

// =============================================================================
// Descriptor component storage
// =============================================================================

/// Three descriptor components can be registered next to each other, and an
/// entity's rows and its stable id survive the archetype migrations that
/// adding and removing one of them causes.
#[test]
fn descriptor_components_coexist_and_survive_archetype_migration() {
    let mut world = World::new();
    let a = world
        .register_component_descriptor(0xA1, "Project.A", 4, 4, 11, Blittability::engine_verified())
        .unwrap();
    let b = world
        .register_component_descriptor(0xB2, "Project.B", 4, 4, 22, Blittability::engine_verified())
        .unwrap();
    let c = world
        .register_component_descriptor(0xC3, "Project.C", 8, 8, 33, Blittability::engine_verified())
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[
            (a, 10_u32.to_ne_bytes().to_vec()),
            (b, 20_u32.to_ne_bytes().to_vec()),
        ])
        .unwrap();

    assert_eq!(
        world.descriptor_component_bytes(entity, a).unwrap(),
        10_u32.to_ne_bytes()
    );
    assert_eq!(
        world.descriptor_component_bytes(entity, b).unwrap(),
        20_u32.to_ne_bytes()
    );

    world.add_descriptor_component_default(entity, c).unwrap();
    assert_eq!(
        world.descriptor_component_bytes(entity, a).unwrap(),
        10_u32.to_ne_bytes()
    );
    assert_eq!(
        world.descriptor_component_bytes(entity, b).unwrap(),
        20_u32.to_ne_bytes()
    );
    assert_eq!(world.descriptor_component_bytes(entity, c).unwrap(), [0; 8]);

    world.remove_component_by_id(entity, b).unwrap();
    assert_eq!(
        world.descriptor_component_bytes(entity, a).unwrap(),
        10_u32.to_ne_bytes()
    );
    assert!(world.descriptor_component_bytes(entity, b).is_none());
    assert_eq!(world.descriptor_component_bytes(entity, c).unwrap(), [0; 8]);

    // The migrations above change an entity's archetype the way a reload's
    // remap does. The stable id is world metadata, so the move leaves it alone.
    let id = world
        .entity_id(entity)
        .expect("a descriptor-created entity has an id");
    assert_eq!(world.entity_by_id(id), Some(entity));
}

/// A relayout rewrites each row in place. Entities keep their rows, values
/// follow their field names, and a field that the previous layout did not have
/// starts at zero instead of reusing a byte of the old shape.
#[cfg(feature = "development")]
#[test]
fn relayout_migrates_rows_and_keeps_entities_in_their_rows() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD4,
            "Project.Relayout",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let first = world
        .create_descriptor_entity(&[(component_id, two_u32_row(1, 2))])
        .unwrap();
    let second = world
        .create_descriptor_entity(&[(component_id, two_u32_row(3, 4))])
        .unwrap();

    let archetype_id = world.storage.entity_locations[&first].archetype_id;
    let first_row_index = world.storage.entity_locations[&first].index_in_archetype;
    let second_row_index = world.storage.entity_locations[&second].index_in_archetype;
    assert_eq!(
        world.storage.entity_locations[&second].archetype_id, archetype_id,
        "identical component sets share one archetype"
    );

    // The new layout is `b`, then `a`, then an eight-byte field that the old
    // one did not have.
    let plan = FieldPlan::between(
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
        &[
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
            LayoutField {
                name: "added",
                type_tag: "u32",
                offset: 8,
                size: 8,
            },
        ],
    );

    let migrated_row_count = world
        .relayout_descriptor_component(component_id, 16, 8, 200, &plan)
        .unwrap();

    assert_eq!(migrated_row_count, 2);
    let mut expected_first = two_u32_row(2, 1);
    expected_first.extend_from_slice(&[0_u8; 8]);
    assert_eq!(
        world
            .descriptor_component_bytes(first, component_id)
            .unwrap(),
        expected_first.as_slice()
    );
    let mut expected_second = two_u32_row(4, 3);
    expected_second.extend_from_slice(&[0_u8; 8]);
    assert_eq!(
        world
            .descriptor_component_bytes(second, component_id)
            .unwrap(),
        expected_second.as_slice()
    );
    assert_eq!(
        world.storage.entity_locations[&first].archetype_id, archetype_id,
        "a relayout moves no entity between archetypes"
    );
    assert_eq!(
        world.storage.entity_locations[&first].index_in_archetype,
        first_row_index
    );
    assert_eq!(
        world.storage.entity_locations[&second].index_in_archetype,
        second_row_index
    );
    assert_eq!(world.component_layout(component_id), Some((16, 8)));
}

/// A relayout is a structural edit, not an add, so rows keep their ticks. No
/// `Added` filter fires, and a row that was already changed stays changed.
#[cfg(feature = "development")]
#[test]
fn relayout_leaves_change_ticks_alone() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD5,
            "Project.Ticks",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[(component_id, two_u32_row(1, 2))])
        .unwrap();

    let changed_tick = world.increment_change_tick();
    let location = world.storage.entity_locations[&entity];
    world
        .storage
        .archetypes
        .get_mut(&location.archetype_id)
        .unwrap()
        .component_storages
        .get_mut(component_id)
        .unwrap()
        .row_ticks_mut(location.index_in_archetype)
        .unwrap()
        .set_changed(changed_tick);
    let before = ticks_of_row(&world, entity, component_id);

    world
        .relayout_descriptor_component(component_id, 16, 8, 200, &FieldPlan::new())
        .unwrap();

    let after = ticks_of_row(&world, entity, component_id);
    assert_eq!(after.added, before.added);
    assert_eq!(after.changed, changed_tick);
}

/// A remap moves the rows from the predecessor registration to the successor.
/// The values move with them, the successor answers to its own name, and the
/// predecessor's registration is retired. An entity whose only component this
/// is survives the move.
#[cfg(feature = "development")]
#[test]
fn remap_moves_rows_to_the_successor_and_retires_the_source() {
    let mut world = World::new();
    let source_component_id = world
        .register_component_descriptor(
            0xE5,
            "Project.OldName",
            8,
            4,
            200,
            Blittability::engine_verified(),
        )
        .unwrap();
    let successor_component_id = world
        .register_component_descriptor(
            0xF6,
            "Project.NewName",
            8,
            4,
            200,
            Blittability::engine_verified(),
        )
        .unwrap();
    let companion = world
        .register_component_descriptor(
            0x1A,
            "Project.Companion",
            4,
            4,
            9,
            Blittability::engine_verified(),
        )
        .unwrap();
    // One entity carries only the component being moved. The remap adds the
    // new column before it removes the old one, which is what keeps it alive.
    let alone = world
        .create_descriptor_entity(&[(source_component_id, two_u32_row(7, 8))])
        .unwrap();
    let paired = world
        .create_descriptor_entity(&[
            (source_component_id, two_u32_row(1, 2)),
            (companion, 3_u32.to_ne_bytes().to_vec()),
        ])
        .unwrap();

    let layout_fields = vec![
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];
    let plan = FieldPlan::between(&layout_fields, &layout_fields);
    let moved_row_count = world
        .remap_descriptor_component(source_component_id, successor_component_id, &plan)
        .unwrap();

    assert_eq!(moved_row_count, 2);
    assert_eq!(
        world
            .descriptor_component_bytes(alone, successor_component_id)
            .unwrap(),
        two_u32_row(7, 8)
    );
    assert_eq!(
        world
            .descriptor_component_bytes(paired, successor_component_id)
            .unwrap(),
        two_u32_row(1, 2)
    );
    assert_eq!(
        world.descriptor_component_bytes(paired, companion).unwrap(),
        3_u32.to_ne_bytes(),
        "the companion column survives the move"
    );
    assert!(world
        .descriptor_component_bytes(alone, source_component_id)
        .is_none());
    assert!(
        !world.storage_factories.contains_key(&source_component_id),
        "the predecessor's registration is retired"
    );
    assert_eq!(
        world
            .resolve_component_id_by_name_any("Project.OldName")
            .unwrap(),
        None,
        "the old name is free again"
    );
    assert_eq!(
        world
            .resolve_component_id_by_name_any("Project.NewName")
            .unwrap(),
        Some(successor_component_id)
    );
}

/// The plan's field mapping decides what moves, not the raw bytes. A shape
/// with a moved field and an added one is reordered and zero-filled, just as a
/// relayout would do it.
#[cfg(feature = "development")]
#[test]
fn remap_reshapes_rows_through_the_plan() {
    let mut world = World::new();
    let source_component_id = world
        .register_component_descriptor(
            0xE6,
            "Project.PlanOld",
            8,
            4,
            300,
            Blittability::engine_verified(),
        )
        .unwrap();
    let successor_component_id = world
        .register_component_descriptor(
            0xF7,
            "Project.PlanNew",
            12,
            4,
            301,
            Blittability::engine_verified(),
        )
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[(source_component_id, two_u32_row(1, 2))])
        .unwrap();

    let plan = FieldPlan::between(
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
        &[
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
            LayoutField {
                name: "added",
                type_tag: "u32",
                offset: 8,
                size: 4,
            },
        ],
    );
    world
        .remap_descriptor_component(source_component_id, successor_component_id, &plan)
        .unwrap();

    let mut expected = 2_u32.to_ne_bytes().to_vec();
    expected.extend_from_slice(&1_u32.to_ne_bytes());
    expected.extend_from_slice(&0_u32.to_ne_bytes());
    assert_eq!(
        world
            .descriptor_component_bytes(entity, successor_component_id)
            .unwrap(),
        expected.as_slice()
    );
}

/// A plan can carry default bytes. An added field is filled with them instead
/// of zero, and a default set on a copied field is accepted but ignored, so it
/// does not overwrite a value that was copied.
#[cfg(feature = "development")]
#[test]
fn a_plan_default_fills_an_added_field_and_never_overrides_a_copied_one() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD8,
            "Project.Defaults",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[(component_id, two_u32_row(1, 2))])
        .unwrap();

    let mut plan = FieldPlan::between(
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
            LayoutField {
                name: "added",
                type_tag: "u32",
                offset: 8,
                size: 4,
            },
        ],
    );
    plan.set_default(8, &6_u32.to_ne_bytes())
        .expect("the added field has a planned slot");
    // A default on a copied field is accepted and then ignored, so the value
    // that was copied survives.
    plan.set_default(0, &9_u32.to_ne_bytes())
        .expect("setting on a copied field is accepted");

    world
        .relayout_descriptor_component(component_id, 12, 4, 101, &plan)
        .unwrap();

    let mut expected = 1_u32.to_ne_bytes().to_vec();
    expected.extend_from_slice(&2_u32.to_ne_bytes());
    expected.extend_from_slice(&6_u32.to_ne_bytes());
    assert_eq!(
        world
            .descriptor_component_bytes(entity, component_id)
            .unwrap(),
        expected.as_slice(),
        "the added field takes its default and the copied fields keep their values"
    );
}

/// Retiring a descriptor component's storage removes its rows and its
/// registration, and frees the name and the bit for a later declaration. A
/// managed manifest relies on this when a type is removed.
#[cfg(feature = "development")]
#[test]
fn retire_component_storage_frees_a_descriptor_component() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xB9,
            "Project.Retired",
            4,
            4,
            400,
            Blittability::engine_verified(),
        )
        .unwrap();
    let companion = world
        .register_component_descriptor(
            0xCA,
            "Project.Stays",
            4,
            4,
            401,
            Blittability::engine_verified(),
        )
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[
            (component_id, 5_u32.to_ne_bytes().to_vec()),
            (companion, 6_u32.to_ne_bytes().to_vec()),
        ])
        .unwrap();

    let affected_row_count = world.retire_component_storage(&[component_id]);

    assert_eq!(affected_row_count, 1, "the retired component's row went");
    assert!(world
        .descriptor_component_bytes(entity, component_id)
        .is_none());
    assert_eq!(
        world.descriptor_component_bytes(entity, companion).unwrap(),
        6_u32.to_ne_bytes(),
        "the companion column survives the retirement"
    );
    assert!(
        !world.storage_factories.contains_key(&component_id),
        "the registration is forgotten"
    );
    assert_eq!(
        world
            .resolve_component_id_by_name_any("Project.Retired")
            .unwrap(),
        None,
        "the name is free again"
    );

    // The name and the bit can be taken by a later declaration.
    let replacement = world
        .register_component_descriptor(
            0xCB,
            "Project.Retired",
            4,
            4,
            402,
            Blittability::engine_verified(),
        )
        .unwrap();
    assert_eq!(
        world
            .resolve_component_id_by_name_any("Project.Retired")
            .unwrap(),
        Some(replacement)
    );
}

/// The resource version of the remap. The value moves to the successor's id,
/// the claims move with it, and the source declaration is dropped after they
/// do.
#[cfg(feature = "development")]
#[test]
fn remap_foreign_resource_moves_the_value_and_the_claim() {
    let mut world = World::new();
    let source_resource_id = world
        .register_foreign_resource("Project.OldSettings", "Project.OldSettings", 8, 4, 77)
        .unwrap();
    world
        .insert_foreign_resource_bytes(source_resource_id, &two_u32_row(5, 6))
        .unwrap();
    let successor_resource_id = world
        .register_foreign_resource("Project.NewSettings", "Project.NewSettings", 8, 4, 77)
        .unwrap();
    // The host seeds a fresh declaration with zero bytes. The moved value
    // replaces that seed.
    world
        .insert_foreign_resource_bytes(successor_resource_id, &[0_u8; 8])
        .unwrap();
    world.retain_resource_claims(&[source_resource_id]);

    let layout_fields = vec![
        LayoutField {
            name: "a",
            type_tag: "u32",
            offset: 0,
            size: 4,
        },
        LayoutField {
            name: "b",
            type_tag: "u32",
            offset: 4,
            size: 4,
        },
    ];
    let plan = FieldPlan::between(&layout_fields, &layout_fields);
    let moved_value_count = world
        .remap_foreign_resource(source_resource_id, successor_resource_id, &plan)
        .unwrap();

    assert_eq!(moved_value_count, 1);
    assert_eq!(
        world.foreign_resource_bytes(successor_resource_id).unwrap(),
        two_u32_row(5, 6)
    );
    assert!(
        world.foreign_resource_layout(source_resource_id).is_none(),
        "the source declaration is dropped once its claims moved"
    );
    assert_eq!(
        world
            .resource_claim_counts
            .get(&successor_resource_id)
            .copied(),
        Some(1)
    );
    assert!(!world
        .resource_claim_counts
        .contains_key(&source_resource_id));
}

/// The id and the registry bit survive a relayout, because archetype masks and
/// the scheduler's access sets hold them. A relayout must update the layout in
/// place and never register the component again.
#[cfg(feature = "development")]
#[test]
fn relayout_keeps_the_component_id_and_its_bit() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD6,
            "Project.Bit",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let bit = world.component_registry().get_bit(&component_id).unwrap();

    world
        .relayout_descriptor_component(component_id, 16, 8, 200, &FieldPlan::new())
        .unwrap();

    // The same id still names the same component; only its size moved.
    assert_eq!(world.component_registry().get_bit(&component_id), Some(bit));
    assert_eq!(world.component_layout(component_id), Some((16, 8)));
    assert_eq!(
        world.component_registry().get_name(&component_id),
        Some("Project.Bit")
    );
}

/// A relayout must also reach an archetype whose last row is gone. The next
/// entity added there has to be stored at the new size, so the push below only
/// succeeds when that column really was rewritten.
#[cfg(feature = "development")]
#[test]
fn relayout_reaches_an_archetype_that_lost_its_last_row() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD7,
            "Project.Empty",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let disposable = world
        .create_descriptor_entity(&[(component_id, two_u32_row(1, 2))])
        .unwrap();
    let archetype_id = world.storage.entity_locations[&disposable].archetype_id;
    assert!(world.destroy_entity(disposable));

    world
        .relayout_descriptor_component(component_id, 16, 8, 200, &FieldPlan::new())
        .unwrap();

    let entity = world
        .create_descriptor_entity(&[(component_id, vec![7_u8; 16])])
        .unwrap();
    assert_eq!(
        world.storage.entity_locations[&entity].archetype_id, archetype_id,
        "the empty archetype is reused, so its column is the one being tested"
    );
    assert_eq!(
        world
            .descriptor_component_bytes(entity, component_id)
            .unwrap(),
        [7_u8; 16].as_slice()
    );
}

/// Only a registered descriptor component has a layout to replace, and a plan
/// that does not fit both layouts is refused before anything is touched.
#[cfg(feature = "development")]
#[test]
fn relayout_refuses_what_it_cannot_migrate() {
    let mut world = World::new();
    world.register_component::<Position>();
    assert!(matches!(
        world.relayout_descriptor_component(
            ComponentId::of::<Position>(),
            16,
            8,
            1,
            &FieldPlan::new()
        ),
        Err(WorldError::DescriptorComponentNotRegistered { .. })
    ));
    assert!(matches!(
        world.relayout_descriptor_component(
            ComponentId::descriptor(0xEE),
            16,
            8,
            1,
            &FieldPlan::new()
        ),
        Err(WorldError::DescriptorComponentNotRegistered { .. })
    ));

    let component_id = world
        .register_component_descriptor(
            0xD8,
            "Project.Refused",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    let mut too_wide = FieldPlan::new();
    too_wide.push(4, 8, FieldSource::OldOffset(0));
    assert!(matches!(
        world.relayout_descriptor_component(component_id, 8, 4, 100, &too_wide),
        Err(WorldError::DescriptorRowInvalid)
    ));
    assert_eq!(
        world.component_layout(component_id),
        Some((8, 4)),
        "a refused plan leaves the registered layout as it was"
    );

    // A column that goes missing is reported before the first migration,
    // so the archetypes holding rows keep them. The destination archetype
    // is made to list the component without storing a column for it.
    let holder = world
        .create_descriptor_entity(&[(component_id, 8_u64.to_ne_bytes().to_vec())])
        .unwrap();
    let stripped = world.get_or_create_archetype(vec![component_id, ComponentId::of::<Position>()]);
    assert!(world
        .storage
        .archetypes
        .get_mut(&stripped)
        .unwrap()
        .component_storages
        .remove(component_id)
        .is_some());
    let row_bytes_before = world
        .descriptor_component_bytes(holder, component_id)
        .expect("the entity has a row")
        .to_vec();
    assert!(matches!(
        world.relayout_descriptor_component(component_id, 8, 4, 100, &FieldPlan::new()),
        Err(WorldError::DescriptorStorageMissing { .. })
    ));
    assert_eq!(
        world.descriptor_component_bytes(holder, component_id),
        Some(row_bytes_before.as_slice()),
        "a refused relayout left the row it would have migrated alone"
    );

    // Put the stripped column back, then give the live column a shape the
    // factory does not know about: 16 bytes while the factory still says 8.
    // The mismatch is reported too, and the column keeps its shape instead of
    // being silently rewritten.
    world
        .storage
        .archetypes
        .get_mut(&stripped)
        .unwrap()
        .component_storages
        .insert(
            component_id,
            crate::archetype::ComponentColumn::new(ColumnLayout {
                size: 8,
                align: 4,
                schema_hash: 100,
                blittability: Blittability::engine_verified(),
            })
            .expect("the drifted layout is still a valid allocation layout"),
        );
    {
        let location = world.storage.entity_locations[&holder];
        let column = world
            .storage
            .archetypes
            .get_mut(&location.archetype_id)
            .unwrap()
            .component_storages
            .get_mut(component_id)
            .unwrap();
        column.relayout_validated(
            ColumnLayout {
                size: 16,
                align: 4,
                schema_hash: 100,
                blittability: Blittability::engine_verified(),
            },
            &FieldPlan::new(),
            8,
        );
    }
    assert!(matches!(
        world.relayout_descriptor_component(component_id, 8, 4, 100, &FieldPlan::new()),
        Err(WorldError::ComponentColumnLayoutMismatch { .. })
    ));
    assert_eq!(
        world.storage.archetypes[&world.storage.entity_locations[&holder].archetype_id]
            .component_storages
            .get(component_id)
            .expect("column")
            .element_size(),
        16,
        "the mismatched column was reported, not rewritten"
    );
    assert_eq!(
        world.component_layout(component_id),
        Some((8, 4)),
        "the factory still carries the registered layout"
    );
}

/// A failed migration changes nothing. The pre-flight check refuses the
/// destination before the entity's row, its columns or its location move.
#[test]
fn migration_failure_leaves_both_archetypes_untouched() {
    let mut world = World::new();
    world.register_component::<Position>();
    let first = world
        .register_component_descriptor(
            0xE1,
            "Project.First",
            4,
            4,
            1,
            Blittability::engine_verified(),
        )
        .unwrap();
    let second = world
        .register_component_descriptor(
            0xE2,
            "Project.Second",
            4,
            4,
            2,
            Blittability::engine_verified(),
        )
        .unwrap();

    // Create the `{first, second}` destination first, then break it on
    // purpose: it lists `second` without storing a column for it.
    let destination = world.get_or_create_archetype(vec![first, second]);
    assert!(world
        .storage
        .archetypes
        .get_mut(&destination)
        .unwrap()
        .component_storages
        .remove(second)
        .is_some());

    let entity = world
        .create_descriptor_entity(&[(first, 7_u32.to_ne_bytes().to_vec())])
        .unwrap();
    let source = world.storage.entity_locations[&entity].archetype_id;
    let source_rows = world.storage.archetypes[&source].entities.len();
    let source_column = world.storage.archetypes[&source]
        .component_storages
        .get(first)
        .expect("column")
        .len();
    let destination_rows = world.storage.archetypes[&destination].entities.len();

    assert!(matches!(
        world.add_descriptor_component_default(entity, second),
        Err(WorldError::DescriptorStorageMissing { .. })
    ));

    assert_eq!(world.storage.entity_locations[&entity].archetype_id, source);
    assert_eq!(
        world.storage.archetypes[&source].entities.len(),
        source_rows,
        "the source kept its row while the destination refused the migration"
    );
    assert_eq!(
        world.storage.archetypes[&source]
            .component_storages
            .get(first)
            .expect("column")
            .len(),
        source_column
    );
    assert_eq!(
        world.storage.archetypes[&destination].entities.len(),
        destination_rows,
        "the destination never received the entity"
    );
}

/// Adding and removing a descriptor component leave the other columns' change
/// ticks alone, and the new column starts with the tick of the add.
#[test]
fn descriptor_component_ticks_survive_archetype_migration() {
    fn ticks_for(world: &World, entity: Entity, component_id: ComponentId) -> ComponentTicks {
        let location = world.storage.entity_locations[&entity];
        *world.storage.archetypes[&location.archetype_id]
            .component_storages
            .get(component_id)
            .expect("the archetype owns the column")
            .row_ticks(location.index_in_archetype)
            .expect("the row exists")
    }

    let mut world = World::new();
    let retained_component_id = world
        .register_component_descriptor(
            0xA1,
            "Project.Retained",
            4,
            4,
            11,
            Blittability::engine_verified(),
        )
        .unwrap();
    let removed_component_id = world
        .register_component_descriptor(
            0xB2,
            "Project.Removed",
            4,
            4,
            22,
            Blittability::engine_verified(),
        )
        .unwrap();
    let added_component_id = world
        .register_component_descriptor(
            0xC3,
            "Project.Added",
            8,
            8,
            33,
            Blittability::engine_verified(),
        )
        .unwrap();
    let entity = world
        .create_descriptor_entity(&[
            (retained_component_id, 10_u32.to_ne_bytes().to_vec()),
            (removed_component_id, 20_u32.to_ne_bytes().to_vec()),
        ])
        .unwrap();

    let original_retained_ticks = ticks_for(&world, entity, retained_component_id);
    let original_removed_ticks = ticks_for(&world, entity, removed_component_id);
    let changed_tick = world.increment_change_tick();
    let location = world.storage.entity_locations[&entity];
    world
        .storage
        .archetypes
        .get_mut(&location.archetype_id)
        .unwrap()
        .component_storages
        .get_mut(retained_component_id)
        .unwrap()
        .row_ticks_mut(location.index_in_archetype)
        .unwrap()
        .set_changed(changed_tick);
    let retained_before_migration = ticks_for(&world, entity, retained_component_id);
    assert_eq!(
        retained_before_migration.added,
        original_retained_ticks.added
    );
    assert_eq!(retained_before_migration.changed, changed_tick);

    let addition_tick = world.increment_change_tick();
    world
        .add_descriptor_component_default(entity, added_component_id)
        .unwrap();

    let retained_after_add = ticks_for(&world, entity, retained_component_id);
    let removed_after_add = ticks_for(&world, entity, removed_component_id);
    let added_after_add = ticks_for(&world, entity, added_component_id);
    assert_eq!(retained_after_add.added, retained_before_migration.added);
    assert_eq!(
        retained_after_add.changed,
        retained_before_migration.changed
    );
    assert_eq!(removed_after_add.added, original_removed_ticks.added);
    assert_eq!(removed_after_add.changed, original_removed_ticks.changed);
    assert_eq!(added_after_add.added, addition_tick);
    assert_eq!(added_after_add.changed, addition_tick);

    world.increment_change_tick();
    world
        .remove_component_by_id(entity, removed_component_id)
        .unwrap();

    let retained_after_remove = ticks_for(&world, entity, retained_component_id);
    let added_after_remove = ticks_for(&world, entity, added_component_id);
    assert_eq!(retained_after_remove.added, retained_after_add.added);
    assert_eq!(retained_after_remove.changed, retained_after_add.changed);
    assert_eq!(added_after_remove.added, added_after_add.added);
    assert_eq!(added_after_remove.changed, added_after_add.changed);
    assert!(world
        .descriptor_component_bytes(entity, removed_component_id)
        .is_none());
}

/// The native byte-chunk accessor exposes a native column's rows as raw
/// bytes with the correct element size, mirroring the descriptor path used
/// by the C# backend for extension components. Descriptor components
/// are rejected by it.
#[test]
fn native_component_chunk_mut_exposes_raw_rows() {
    let mut world = World::new();
    world.register_component::<Position>();
    let entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .unwrap();
    let component_id = ComponentId::of::<Position>();
    // Copy the chunk's fields out inside a short scope, so the mutable borrow
    // ends before the world is borrowed again below.
    let (archetype_id, rows_pointer, row_count, element_size, tick_count) = {
        let chunk = world
            .native_component_chunk_mut(component_id, 0)
            .expect("one archetype column should exist");
        (
            chunk.archetype,
            chunk.data,
            chunk.len,
            chunk.element_size,
            chunk.ticks.len(),
        )
    };
    assert_eq!(
        archetype_id,
        world.storage.entity_locations[&entity].archetype_id
    );
    assert_eq!(row_count, 1);
    assert_eq!(element_size, std::mem::size_of::<Position>());
    assert_eq!(tick_count, 1);
    // SAFETY: `row_count` is 1 and `element_size` is the Position size, so the
    // returned buffer holds exactly one valid Position.
    let row = unsafe { &*rows_pointer.cast::<Position>() };
    assert_eq!(row.x, 1.0);
    assert_eq!(row.y, 2.0);

    // Descriptor components are served by the descriptor accessor, not this one.
    let descriptor_component_id = world
        .register_component_descriptor(
            0xD1,
            "NativeChunkTest.Descriptor",
            4,
            4,
            1,
            Blittability::engine_verified(),
        )
        .unwrap();
    assert!(world
        .native_component_chunk_mut(descriptor_component_id, 0)
        .is_none());
    // A registered descriptor id with no rows is refused too.
    let unused_descriptor_id = world
        .register_component_descriptor(
            0xD2,
            "NativeChunkTest.Unknown",
            4,
            4,
            2,
            Blittability::engine_verified(),
        )
        .unwrap();
    assert!(world
        .native_component_chunk_mut(unused_descriptor_id, 0)
        .is_none());
}

/// A byte component adder writes raw ABI bytes into a native column. The C#
/// backend uses it to create or add extension components whose concrete Rust
/// type the host never names.
#[test]
fn byte_component_adder_writes_native_bytes() {
    use crate::commands::{ByteComponentAdder, CommandQueue, ComponentAdder};

    let mut world = World::new();
    world.register_component::<Position>();
    let entity = world.reserve_entity();
    let component_id = ComponentId::of::<Position>();

    // ABI payload: x = 7.5, y = -3.25, as native-endian f32s.
    let mut bytes = Vec::with_capacity(std::mem::size_of::<Position>());
    bytes.extend_from_slice(&7.5_f32.to_ne_bytes());
    bytes.extend_from_slice(&(-3.25_f32).to_ne_bytes());

    let adder = ByteComponentAdder::new(component_id, bytes);
    let mut queue = CommandQueue::new();
    queue.create_mixed_entity(
        entity,
        vec![Box::new(adder) as Box<dyn ComponentAdder>],
        Vec::new(),
    );
    queue.execute_queued_commands(&mut world, true).unwrap();

    let position = world
        .get_component::<Position>(entity)
        .expect("row was created");
    assert_eq!(position.x, 7.5);
    assert_eq!(position.y, -3.25);
}

/// The sweep by id frees a column whose factory is gone, whether the column
/// still holds rows or not. The archetype stops listing the id, so the host
/// can unmap a retired image without leaving a column whose drop code lived in
/// it.
#[cfg(feature = "development")]
#[test]
fn a_column_whose_factory_vanished_is_dropped_before_eviction() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    let component_id = ComponentId::of::<Position>();
    // Two components, so the entity survives the sweep and its destination
    // archetype can be inspected.
    let entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .with(Velocity { x: 3.0, y: 4.0 })
        .build()
        .unwrap();

    // This is what `forget_component_type` leaves behind when a rebuilt image
    // registers the name again under a fresh id. The archetype keeps the old id
    // and its column, while the factory that describes them is gone.
    world.storage_factories.remove(&component_id);
    assert_eq!(
        world.columns_without_factory(),
        1,
        "the column is the only one without a factory"
    );

    let dropped_column_count = world.drop_columns_without_factory();
    assert_eq!(dropped_column_count, 1, "the orphaned id was the one swept");
    assert_eq!(
        world.columns_without_factory(),
        0,
        "no column without a function table survives the sweep"
    );
    let location = world.storage.entity_locations[&entity];
    assert!(
        !world.storage.archetypes[&location.archetype_id]
            .component_types
            .contains(&component_id),
        "the entity no longer lists the swept component"
    );
    assert_eq!(
        world.get_component::<Velocity>(entity).unwrap().x,
        3.0,
        "the surviving component kept its value through the migration"
    );
}

/// A descriptor declaration with a zero width, a bad alignment, an oversized
/// width, a reused id or a reused schema hash is refused, and so is a row
/// whose payload does not match the declared size.
#[test]
fn invalid_descriptor_component_layouts_are_rejected() {
    let mut world = World::new();
    assert!(world
        .register_component_descriptor(1, "Zero", 0, 1, 0, Blittability::engine_verified())
        .is_err());
    assert!(world
        .register_component_descriptor(2, "BadAlign", 4, 3, 0, Blittability::engine_verified())
        .is_err());
    assert!(world
        .register_component_descriptor(
            4,
            "Oversized",
            usize::MAX,
            1,
            0,
            Blittability::engine_verified()
        )
        .is_err());
    world
        .register_component_descriptor(3, "SchemaA", 4, 4, 10, Blittability::engine_verified())
        .unwrap();
    assert!(world
        .register_component_descriptor(
            3,
            "SameSchemaDifferentName",
            4,
            4,
            10,
            Blittability::engine_verified()
        )
        .is_err());
    assert!(world
        .register_component_descriptor(3, "SchemaB", 8, 8, 20, Blittability::engine_verified())
        .is_err());

    let valid_component_id = world
        .register_component_descriptor(5, "Valid", 4, 4, 30, Blittability::engine_verified())
        .unwrap();
    assert!(world
        .create_descriptor_entity(&[(valid_component_id, vec![0; 3])])
        .is_err());
    assert_eq!(world.entity_count(), 0);
}

// =============================================================================
// Entity lifecycle
// =============================================================================

/// One entity per component combination, one archetype per combination, and
/// every entity tracked in `entity_locations`.
#[test]
fn test_create_entities_with_different_components() {
    let mut world = setup_world();

    // Create entity with Position + Velocity
    let entity1 = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    // Create entity with Position only
    let entity2 = world
        .create_entity()
        .with(Position { x: 5.0, y: 15.0 })
        .build()
        .unwrap();

    // Create entity with all three components
    let entity3 = world
        .create_entity()
        .with(Position { x: 100.0, y: 200.0 })
        .with(Velocity { x: 5.0, y: 10.0 })
        .with(Health(100))
        .build()
        .unwrap();

    assert_eq!(world.storage.entity_locations.len(), 3);
    assert_eq!(world.storage.archetypes.len(), 3);
    assert!(world.storage.entity_locations.contains_key(&entity1));
    assert!(world.storage.entity_locations.contains_key(&entity2));
    assert!(world.storage.entity_locations.contains_key(&entity3));

    world.print_archetypes();

    // Verify each archetype's component mask matches expected components
    for (archetype_id, archetype) in world.storage.archetypes.iter() {
        println!("\n--- Verifying Archetype {:?} ---", archetype_id);

        let comp_names: Vec<String> = archetype
            .component_types
            .iter()
            .filter_map(|component_id| {
                world
                    .component_registry
                    .get_name(component_id)
                    .map(String::from)
            })
            .collect();

        println!("Components: {:?}", comp_names);

        // Build expected mask from component types
        let mut expected_mask = ComponentMask::empty();
        for component_id in &archetype.component_types {
            if let Some(bit) = world.component_registry.get_bit(component_id) {
                expected_mask.set(bit);
                println!(
                    "  - {:?} -> bit {}",
                    world
                        .component_registry
                        .get_name(component_id)
                        .unwrap_or("Unknown"),
                    bit
                );
            }
        }

        assert_eq!(
            archetype.component_mask, expected_mask,
            "Archetype {:?} mask mismatch!\nActual:   {:?}\nExpected: {:?}",
            archetype_id, archetype.component_mask, expected_mask
        );

        println!("âœ“ Mask verified: {:?}", archetype.component_mask);
    }

    println!("\nâœ“ All 3 archetypes verified successfully!");
}

/// Adding a component migrates the entity into the archetype that carries it,
/// and the archetype it left is removed once it is empty.
#[test]
fn test_add_component_to_entity() {
    let mut world = setup_world();

    let entity = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    assert_eq!(
        world.storage.archetypes.len(),
        1,
        "Should have 1 archetype initially"
    );

    let add_result = world.add_component(entity, Health(50));

    assert!(add_result.is_ok(), "Should successfully add component");
    assert!(world.storage.entity_locations.contains_key(&entity));

    // This is the only entity, so the archetype it leaves is empty and is
    // removed at once.
    assert_eq!(
        world.storage.archetypes.len(),
        1,
        "Should have 1 archetype after adding Health (old one auto-removed)"
    );

    world.print_archetypes();
}

/// Adding to a handle the world never handed out reports an error rather than
/// panicking or touching the world.
#[test]
fn test_add_component_to_nonexistent_entity() {
    let mut world = World::new();
    world.register_component::<Position>();

    let unissued_entity = crate::Entity::new_for_test(9999, 0);
    let add_result = world.add_component(unissued_entity, Position { x: 0.0, y: 0.0 });

    assert_eq!(
        add_result,
        Err(AddComponentError::EntityNotFound),
        "Should fail to add component to non-existent entity"
    );
}

/// A component type that was never registered.
///
/// Registration is what gives a component its archetype bit, so adding this
/// type must be refused rather than silently resolved back to the source
/// archetype.
#[derive(Debug, Clone, Copy, PartialEq)]
struct UnregisteredComponent(pub u32);
impl crate::component::Component for UnregisteredComponent {}

/// Adding a component whose type the world never registered is refused.
///
/// The destination archetype is derived from component bits, so an
/// unregistered type contributes none and the destination resolves to the
/// entity's current archetype. Without the refusal the migration would
/// short-circuit, and the add would report success while storing nothing.
#[test]
fn test_add_component_refuses_unregistered_type() {
    let mut world = setup_world();
    let entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    let add_result = world.add_component(entity, UnregisteredComponent(7));

    assert_eq!(
        add_result,
        Err(AddComponentError::ComponentNotRegistered {
            id: ComponentId::of::<UnregisteredComponent>(),
        }),
        "adding an unregistered component must be refused"
    );
    assert!(
        world
            .get_component::<UnregisteredComponent>(entity)
            .is_none(),
        "the refused component must not be stored"
    );
    assert!(
        world.get_component::<Position>(entity).is_some(),
        "the refusal must leave the existing components alone"
    );
}

/// Removing one component leaves the entity and its other components in
/// place, in an archetype that no longer lists the removed type.
#[test]
fn test_remove_component_from_entity() {
    let mut world = setup_world();

    let entity = world
        .create_entity()
        .with(Position { x: 100.0, y: 200.0 })
        .with(Velocity { x: 5.0, y: 10.0 })
        .with(Health(100))
        .build()
        .unwrap();

    let remove_result = world.remove_component::<Velocity>(entity);

    assert_eq!(world.storage.archetypes.len(), 1, "Should have 1 archetype");

    assert!(
        remove_result.is_ok(),
        "Should successfully remove component"
    );
    assert!(world.storage.entity_locations.contains_key(&entity));

    let location = world.storage.entity_locations.get(&entity).unwrap();
    let archetype = world
        .storage
        .archetypes
        .get(&location.archetype_id)
        .unwrap();

    assert_eq!(
        archetype.component_types.len(),
        2,
        "Should have 2 component types"
    );

    // The archetype lists Position and Health but not Velocity. The ids below
    // carry those three types.
    let position_id = ComponentId::of::<Position>();
    let health_id = ComponentId::of::<Health>();
    let velocity_id = ComponentId::of::<Velocity>();

    assert!(
        archetype.component_types.contains(&position_id),
        "Archetype should contain Position component"
    );
    assert!(
        archetype.component_types.contains(&health_id),
        "Archetype should contain Health component"
    );
    assert!(
        !archetype.component_types.contains(&velocity_id),
        "Archetype should not contain Velocity component"
    );
}

/// Removing from a handle the world never handed out reports an error and
/// changes nothing.
#[test]
fn test_remove_component_from_nonexistent_entity() {
    let mut world = World::new();
    world.register_component::<Velocity>();

    let unissued_entity = crate::Entity::new_for_test(9999, 0);
    let remove_result = world.remove_component::<Velocity>(unissued_entity);

    assert_eq!(
        remove_result,
        Err(RemoveComponentError::EntityNotFound),
        "Should fail to remove component from non-existent entity"
    );
}

/// An entity whose last component is removed is destroyed rather than kept as
/// an empty row, and the archetype it emptied goes with it.
#[test]
fn test_remove_last_component_destroys_entity() {
    let mut world = World::new();
    world.register_component::<Position>();

    let entity = world
        .create_entity()
        .with(Position { x: 5.0, y: 15.0 })
        .build()
        .unwrap();

    assert_eq!(world.storage.entity_locations.len(), 1);

    // The component is the only one, so removing it destroys the entity.
    let remove_result = world.remove_component::<Position>(entity);

    assert!(
        remove_result.is_ok(),
        "Should successfully remove component"
    );
    assert_eq!(
        world.storage.entity_locations.len(),
        0,
        "Entity should be destroyed"
    );
    assert!(!world.storage.entity_locations.contains_key(&entity));

    assert!(
        world.storage.archetypes.is_empty(),
        "No archetypes should remain"
    );
}

/// Destroying one entity removes it from every tracking structure and leaves
/// the others untouched.
#[test]
fn test_destroy_entity() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();

    let entity1 = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .build()
        .unwrap();

    let entity2 = world
        .create_entity()
        .with(Position { x: 5.0, y: 15.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    assert_eq!(world.storage.entity_locations.len(), 2);

    let destroy_result = world.destroy_entity(entity1);

    assert!(destroy_result, "Should successfully destroy entity");
    assert_eq!(world.storage.entity_locations.len(), 1);
    assert!(!world.storage.entity_locations.contains_key(&entity1));
    assert!(world.storage.entity_locations.contains_key(&entity2));
}

/// Destroying a handle the world never handed out reports `false` instead of
/// panicking.
#[test]
fn test_destroy_nonexistent_entity() {
    let mut world = World::new();
    let unissued_entity = crate::Entity::new_for_test(9999, 0);

    let destroy_result = world.destroy_entity(unissued_entity);

    assert!(
        !destroy_result,
        "Should fail to destroy non-existent entity"
    );
}

/// A second destroy of the same handle is refused, so a double destroy cannot
/// free a slot a later entity owns.
#[test]
fn test_destroy_already_destroyed_entity() {
    let mut world = World::new();
    world.register_component::<Position>();

    let entity = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .build()
        .unwrap();

    let result1 = world.destroy_entity(entity);
    assert!(result1);

    let result2 = world.destroy_entity(entity);
    assert!(!result2, "Should fail to destroy already-destroyed entity");
}

/// `cleanup_empty_archetypes` drops the archetype an entity left behind and
/// keeps the one that still holds a row.
#[test]
fn test_cleanup_empty_archetypes() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();

    let entity1 = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .build()
        .unwrap();

    let entity2 = world
        .create_entity()
        .with(Position { x: 5.0, y: 15.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    let initial_archetypes = world.storage.archetypes.len();
    assert_eq!(initial_archetypes, 2);

    // Destroy one entity, leaving one archetype empty
    let _ = world.destroy_entity(entity1);

    world.cleanup_empty_archetypes();

    assert!(world.storage.archetypes.len() < initial_archetypes);
    assert!(world.storage.entity_locations.contains_key(&entity2));
}

/// Each add and each remove moves the entity to a different archetype, and no
/// two steps of the migration end in the same one.
#[test]
fn test_entity_archetype_migration() {
    let mut world = setup_world();

    // Start with Position + Velocity
    let entity = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    let initial_location = *world.storage.entity_locations.get(&entity).unwrap();

    // Adding Health moves the entity to an archetype that has it.
    world.add_component(entity, Health(100)).unwrap();

    let after_add_location = *world.storage.entity_locations.get(&entity).unwrap();
    assert_ne!(
        initial_location.archetype_id, after_add_location.archetype_id,
        "Entity should be in different archetype after adding component"
    );

    // Removing Velocity moves the entity again.
    world.remove_component::<Velocity>(entity).unwrap();

    let after_remove_location = *world.storage.entity_locations.get(&entity).unwrap();
    assert_ne!(
        after_add_location.archetype_id, after_remove_location.archetype_id,
        "Entity should be in different archetype after removing component"
    );
}

/// Migrating the last entity out of an archetype removes that archetype
/// without a `cleanup_empty_archetypes` call.
#[test]
fn test_automatic_empty_archetype_cleanup() {
    let mut world = setup_world();

    // Create single entity with Position + Velocity
    let entity = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    assert_eq!(
        world.storage.archetypes.len(),
        1,
        "Should have 1 archetype initially"
    );

    // Adding Health moves the entity to a new archetype. The one it leaves
    // becomes empty, so it is removed at once.
    world.add_component(entity, Health(100)).unwrap();

    assert_eq!(
        world.storage.archetypes.len(),
        1,
        "Should still have 1 archetype after migration (old one auto-removed)"
    );
    assert!(world.storage.entity_locations.contains_key(&entity));

    // Verify the entity is in the correct archetype with all 3 components
    let location = world.storage.entity_locations.get(&entity).unwrap();
    let archetype = world
        .storage
        .archetypes
        .get(&location.archetype_id)
        .unwrap();
    assert_eq!(
        archetype.component_types.len(),
        3,
        "Entity should have 3 components"
    );

    println!("âœ“ Empty archetype automatically cleaned up after entity migration");
}

/// Prints the archetype report, then checks the entity count, the archetype's
/// rows and the component names it printed.
#[test]
fn test_archetype_print_info() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();

    world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .with(Velocity { x: 1.0, y: 2.0 })
        .build()
        .unwrap();

    world
        .create_entity()
        .with(Position { x: 5.0, y: 15.0 })
        .with(Velocity { x: 0.5, y: 1.5 })
        .build()
        .unwrap();

    world.print_archetypes();

    assert_eq!(
        world.storage.entity_locations.len(),
        2,
        "Should have 2 entities"
    );
    assert_eq!(world.storage.archetypes.len(), 1, "Should have 1 archetype");

    let archetype = world.storage.archetypes.values().next().unwrap();
    assert_eq!(
        archetype.entities.len(),
        2,
        "Archetype should contain 2 entities"
    );
    assert_eq!(
        archetype.component_types.len(),
        2,
        "Archetype should have 2 component types"
    );

    let comp_names: Vec<String> = archetype
        .component_types
        .iter()
        .filter_map(|component_id| {
            world
                .component_registry
                .get_name(component_id)
                .map(String::from)
        })
        .collect();

    assert_eq!(comp_names.len(), 2, "Should have 2 component names");

    let has_position = comp_names.iter().any(|name| name.contains("Position"));
    let has_velocity = comp_names.iter().any(|name| name.contains("Velocity"));

    assert!(
        has_position,
        "Should contain Position component, found: {:?}",
        comp_names
    );
    assert!(
        has_velocity,
        "Should contain Velocity component, found: {:?}",
        comp_names
    );

    println!("âœ“ Component names verified: {:?}", comp_names);
}

/// A recycled id carries the next generation, so the stale handle keeps
/// failing while the handle the world just handed out works.
#[test]
fn test_entity_generations() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();

    let entity1 = world
        .create_entity()
        .with(Position { x: 10.0, y: 20.0 })
        .build()
        .unwrap();

    println!("Entity1: id={}, gen={}", entity1.id, entity1.generation);
    assert_eq!(entity1.id, 0, "First entity should have ID 0");
    assert_eq!(
        entity1.generation, 0,
        "First entity should have generation 0"
    );

    assert!(world.is_entity_valid(entity1), "Entity1 should be valid");
    assert!(
        world.get_component::<Position>(entity1).is_some(),
        "Entity1 should have Position"
    );

    let destroyed = world.destroy_entity(entity1);
    assert!(destroyed, "Entity1 should be destroyed successfully");

    assert!(
        !world.is_entity_valid(entity1),
        "Entity1 should be invalid after destruction"
    );
    assert!(
        world.get_component::<Position>(entity1).is_none(),
        "Destroyed entity should not have components"
    );

    // The next entity reuses id 0 with generation 1.
    let entity2 = world
        .create_entity()
        .with(Velocity { x: 5.0, y: 10.0 })
        .build()
        .unwrap();

    println!("Entity2: id={}, gen={}", entity2.id, entity2.generation);
    assert_eq!(entity2.id, 0, "New entity should reuse ID 0");
    assert_eq!(entity2.generation, 1, "New entity should have generation 1");

    assert!(world.is_entity_valid(entity2), "Entity2 should be valid");
    assert!(
        world.get_component::<Velocity>(entity2).is_some(),
        "Entity2 should have Velocity"
    );

    // The old handle must not reach the new entity's data, which is what the
    // generation check is for.
    assert!(
        !world.is_entity_valid(entity1),
        "Old handle should still be invalid"
    );
    assert!(
        world.get_component::<Velocity>(entity1).is_none(),
        "Old handle should not access new entity's components"
    );
    assert!(
        world.get_component::<Position>(entity1).is_none(),
        "Old handle should not access any components"
    );

    assert_ne!(
        entity1, entity2,
        "Entities with different generations should not be equal"
    );

    println!("âœ“ Entity generation recycling works correctly!");
}

/// Repeated destroy/create cycles keep the one id and raise its generation by
/// one every round.
#[test]
fn test_multiple_entity_recycling_rounds() {
    let mut world = World::new();
    world.register_component::<Position>();

    let mut last_entity = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(last_entity.id, 0);
    assert_eq!(last_entity.generation, 0);

    for round in 1..=5 {
        let old_entity = last_entity;
        let _ = world.destroy_entity(old_entity);

        let new_entity = world
            .create_entity()
            .with(Position {
                x: round as f32,
                y: 0.0,
            })
            .build()
            .unwrap();

        assert_eq!(new_entity.id, 0, "Should reuse ID 0 in round {}", round);
        assert_eq!(
            new_entity.generation, round,
            "Generation should be {} in round {}",
            round, round
        );

        assert!(!world.is_entity_valid(old_entity));
        assert!(world.is_entity_valid(new_entity));

        last_entity = new_entity;
    }

    println!("âœ“ Multiple recycling rounds work correctly!");
}

/// The free list hands ids back last-in-first-out, and a drained list
/// allocates fresh ids at generation zero.
#[test]
fn test_free_list_lifo_order() {
    let mut world = World::new();
    world.register_component::<Position>();

    // Create 3 entities
    let entity0 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let entity1 = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .build()
        .unwrap();
    let entity2 = world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .build()
        .unwrap();

    assert_eq!(entity0.id, 0);
    assert_eq!(entity1.id, 1);
    assert_eq!(entity2.id, 2);

    // Destroy in order: entity0, entity1, entity2
    let _ = world.destroy_entity(entity0);
    let _ = world.destroy_entity(entity1);
    let _ = world.destroy_entity(entity2);

    // The free list now holds `(0, 1)`, `(1, 1)` and `(2, 1)`. A LIFO pop
    // returns id 2 first, then 1, then 0.

    let new_entity1 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(new_entity1.id, 2, "Should pop ID 2 first (LIFO)");
    assert_eq!(new_entity1.generation, 1);

    let new_entity2 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(new_entity2.id, 1, "Should pop ID 1 second");
    assert_eq!(new_entity2.generation, 1);

    let new_entity3 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(new_entity3.id, 0, "Should pop ID 0 third");
    assert_eq!(new_entity3.generation, 1);

    let new_entity4 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(new_entity4.id, 3, "Should allocate fresh ID 3");
    assert_eq!(new_entity4.generation, 0);

    println!("âœ“ Free list LIFO order works correctly!");
}

/// A slot whose generation reaches `u32::MAX` is retired instead of wrapped
/// back to zero.
///
/// Wrapping would let a handle from 2^32 recycles ago name an unrelated
/// entity, which is the ABA problem. The free list drops the slot instead, so
/// a stale handle can never pass the validity check against a recycled entity.
#[test]
fn slot_at_generation_max_is_retired_not_wrapped() {
    let mut world = World::new();
    world.register_component::<Position>();

    // Seed the free list so the next create reuses id 7 at the
    // second-highest representable generation.
    world.storage.free_entity_ids.push((7, u32::MAX - 1));

    // Start the slot's first life with a real entity one generation below the
    // ceiling.
    let stale = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(stale.id, 7);
    assert_eq!(stale.generation, u32::MAX - 1);

    // Destroying it recycles the slot one step closer to the ceiling.
    assert!(world.destroy_entity(stale));
    assert_eq!(world.storage.free_entity_ids.as_slice(), &[(7, u32::MAX)]);

    // The slot's second life starts at the ceiling itself.
    let ceiling = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(ceiling.generation, u32::MAX);

    // Destroying at the ceiling retires the slot instead of wrapping to 0.
    assert!(world.destroy_entity(ceiling));
    assert!(
        world.storage.free_entity_ids.is_empty(),
        "the slot must be retired, not wrapped back to generation 0"
    );

    // The id is never handed out again, so the stale handle can never
    // validate against a recycled entity.
    let replacement = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_ne!(replacement.id, 7, "a retired slot must not be recycled");
    assert!(!world.is_entity_valid(stale), "the stale handle stays dead");
}

/// Component removal keeps an entity's id and generation, and ids freed from
/// any archetype recycle into any other.
#[test]
fn test_generations_with_multiple_archetypes_and_component_removal() {
    let mut world = setup_world();

    // Create 3 entities:
    // entity1, entity2: Position + Velocity (same archetype)
    // entity3: Position + Health (different archetype)
    let entity1 = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .with(Velocity { x: 10.0, y: 10.0 })
        .build()
        .unwrap();

    let entity2 = world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .with(Velocity { x: 20.0, y: 20.0 })
        .build()
        .unwrap();

    let entity3 = world
        .create_entity()
        .with(Position { x: 3.0, y: 3.0 })
        .with(Health(100))
        .build()
        .unwrap();

    println!(
        "Created: entity1(id={}, gen={}), entity2(id={}, gen={}), entity3(id={}, gen={})",
        entity1.id,
        entity1.generation,
        entity2.id,
        entity2.generation,
        entity3.id,
        entity3.generation
    );

    assert_eq!(entity1.id, 0);
    assert_eq!(entity2.id, 1);
    assert_eq!(entity3.id, 2);
    assert_eq!(
        world.storage.archetypes.len(),
        2,
        "Should have 2 archetypes"
    );

    // Removing Velocity from entity1 moves it to the Position-only archetype.
    let old_entity1 = entity1;
    let remove_result = world.remove_component::<Velocity>(entity1);
    assert!(remove_result.is_ok(), "Should remove Velocity from entity1");

    // entity1 stays valid with the same id and generation, because removal is
    // not destruction.
    assert!(
        world.is_entity_valid(entity1),
        "entity1 should still be valid after component removal"
    );
    assert!(
        world.get_component::<Position>(entity1).is_some(),
        "entity1 should still have Position"
    );
    assert!(
        world.get_component::<Velocity>(entity1).is_none(),
        "entity1 should not have Velocity"
    );

    // Destroy entity2 (from Position+Velocity archetype)
    let old_entity2 = entity2;
    let _ = world.destroy_entity(entity2);
    assert!(
        !world.is_entity_valid(old_entity2),
        "entity2 should be invalid after destruction"
    );

    // Destroy entity3 (from Position+Health archetype)
    let old_entity3 = entity3;
    let _ = world.destroy_entity(entity3);
    assert!(
        !world.is_entity_valid(old_entity3),
        "entity3 should be invalid after destruction"
    );

    // The free list now holds `(1, 1)` and `(2, 1)`. entity1 keeps id 0, so
    // that slot is not free.

    // The next create reuses id 2, the last slot destroyed.
    let new_entity1 = world.create_entity().with(Health(50)).build().unwrap();

    println!(
        "new_entity1: id={}, gen={}",
        new_entity1.id, new_entity1.generation
    );
    assert_eq!(new_entity1.id, 2, "Should reuse ID 2 (LIFO)");
    assert_eq!(new_entity1.generation, 1, "Should have generation 1");

    // The old entity3 handle must not reach the new entity's data.
    assert!(
        !world.is_entity_valid(old_entity3),
        "Old entity3 handle should be invalid"
    );
    assert!(
        world.get_component::<Health>(old_entity3).is_none(),
        "Old handle should not access new entity"
    );

    // The next create reuses the other freed slot, id 1.
    let new_entity2 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    println!(
        "new_entity2: id={}, gen={}",
        new_entity2.id, new_entity2.generation
    );
    assert_eq!(new_entity2.id, 1, "Should reuse ID 2");
    assert_eq!(new_entity2.generation, 1, "Should have generation 1");

    // The old entity2 handle must not reach this entity either.
    assert!(
        !world.is_entity_valid(old_entity2),
        "Old entity2 handle should be invalid"
    );

    assert!(
        world.is_entity_valid(old_entity1),
        "Original entity1 should still be valid"
    );
    let pos = world.get_component::<Position>(old_entity1).unwrap();
    assert_eq!(pos.x, 1.0, "entity1 Position should be preserved");

    // Destroy entity1, the last live entity, and check that its id is recycled
    // too.
    let _ = world.destroy_entity(entity1);
    assert!(
        !world.is_entity_valid(old_entity1),
        "entity1 should be invalid after destruction"
    );

    let new_entity3 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    println!(
        "new_entity3: id={}, gen={}",
        new_entity3.id, new_entity3.generation
    );
    assert_eq!(new_entity3.id, 0, "Should reuse ID 1");
    assert_eq!(new_entity3.generation, 1, "Should have generation 1");

    println!("âœ“ Generations with multiple archetypes and component removal work correctly!");
}

/// Destroying an entity swap-removes its row from every column, so the entity
/// moved into the freed slot still reads its own component values.
#[test]
fn test_component_swap_remove_on_destroy() {
    let mut world = World::new();
    world.register_component::<Position>();

    // Create 3 entities in the same archetype
    let entity0 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let entity1 = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .build()
        .unwrap();
    let entity2 = world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .build()
        .unwrap();

    assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
    assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);
    assert_eq!(world.get_component::<Position>(entity2).unwrap().x, 2.0);

    // Archetype entity list: [entity0, entity1, entity2] (indices 0, 1, 2)
    // Component storage:     [Pos(0,0), Pos(1,1), Pos(2,2)]

    // Destroy entity0 (index 0). The entity list swap-removes: entity2 moves
    // from index 2 to index 0. The column has to swap-remove with it, or the
    // row at index 0 stays entity0's stale Position and entity2 reads it.
    let _ = world.destroy_entity(entity0);

    // entity1 sits at index 1, which the removal did not touch, so its values
    // stay.
    let pos1 = world.get_component::<Position>(entity1).unwrap();
    assert_eq!(pos1.x, 1.0, "entity1 Position.x should be 1.0");
    assert_eq!(pos1.y, 1.0, "entity1 Position.y should be 1.0");

    // entity2 now sits at index 0 and must read the row that followed it.
    let pos2 = world.get_component::<Position>(entity2).unwrap();

    println!(
        "entity2 Position after entity0 destroyed: ({}, {})",
        pos2.x, pos2.y
    );
    println!("Expected: (2.0, 2.0), Got: ({}, {})", pos2.x, pos2.y);

    // The row entity2 reads is its own.
    assert_eq!(
        pos2.x, 2.0,
        "BUG: entity2 should have Position.x = 2.0, but got {} (entity0's old data)",
        pos2.x
    );
    assert_eq!(
        pos2.y, 2.0,
        "BUG: entity2 should have Position.y = 2.0, but got {} (entity0's old data)",
        pos2.y
    );

    println!("âœ“ Component swap_remove works correctly!");
}

/// A migration swap-removes the entity's row from the archetype it leaves, so
/// the entities that stay behind still read their own component data and no
/// orphaned row is left allocated.
#[test]
fn test_component_cleanup_on_archetype_migration() {
    let mut world = setup_world();

    // Create 3 entities with Position + Velocity in the same archetype
    let entity0 = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .with(Velocity { x: 100.0, y: 100.0 })
        .build()
        .unwrap();
    let entity1 = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .with(Velocity { x: 101.0, y: 101.0 })
        .build()
        .unwrap();
    let entity2 = world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .with(Velocity { x: 102.0, y: 102.0 })
        .build()
        .unwrap();

    assert_eq!(
        world.storage.archetypes.len(),
        1,
        "Should have 1 archetype initially"
    );

    assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
    assert_eq!(world.get_component::<Velocity>(entity0).unwrap().x, 100.0);
    assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);
    assert_eq!(world.get_component::<Velocity>(entity1).unwrap().x, 101.0);
    assert_eq!(world.get_component::<Position>(entity2).unwrap().x, 2.0);
    assert_eq!(world.get_component::<Velocity>(entity2).unwrap().x, 102.0);

    // Adding Health moves entity0 to its own archetype. The archetype it
    // leaves swap-removes its row, which puts entity2 into the freed index 0.
    world.add_component(entity0, Health(50)).unwrap();

    assert!(world.get_component::<Position>(entity0).is_some());
    assert!(world.get_component::<Velocity>(entity0).is_some());
    assert!(world.get_component::<Health>(entity0).is_some());
    assert_eq!(world.get_component::<Position>(entity0).unwrap().x, 0.0);
    assert_eq!(world.get_component::<Velocity>(entity0).unwrap().x, 100.0);
    assert_eq!(world.get_component::<Health>(entity0).unwrap().0, 50);

    // entity1 and entity2 must still read their own rows. entity2 now sits at
    // the index entity0 left, so a column that did not swap-remove would give
    // it entity0's data.

    let pos1 = world.get_component::<Position>(entity1).unwrap();
    let vel1 = world.get_component::<Velocity>(entity1).unwrap();
    assert_eq!(pos1.x, 1.0, "entity1 Position.x should be 1.0");
    assert_eq!(vel1.x, 101.0, "entity1 Velocity.x should be 101.0");

    let pos2 = world.get_component::<Position>(entity2).unwrap();
    let vel2 = world.get_component::<Velocity>(entity2).unwrap();
    assert_eq!(
        pos2.x, 2.0,
        "entity2 Position.x should be 2.0, but got {} (possible swap_remove bug)",
        pos2.x
    );
    assert_eq!(
        vel2.x, 102.0,
        "entity2 Velocity.x should be 102.0, but got {} (possible swap_remove bug)",
        vel2.x
    );

    assert_eq!(
        world.storage.archetypes.len(),
        2,
        "Should have 2 archetypes now"
    );

    // Remove Velocity from entity1, so it moves to the Position-only
    // archetype.
    world.remove_component::<Velocity>(entity1).unwrap();

    // entity2 is now alone in the Position+Velocity archetype, and its values
    // must be unchanged.
    let pos2 = world.get_component::<Position>(entity2).unwrap();
    let vel2 = world.get_component::<Velocity>(entity2).unwrap();
    assert_eq!(pos2.x, 2.0, "entity2 Position.x should still be 2.0");
    assert_eq!(vel2.x, 102.0, "entity2 Velocity.x should still be 102.0");

    assert!(world.get_component::<Position>(entity1).is_some());
    assert!(world.get_component::<Velocity>(entity1).is_none());
    assert_eq!(world.get_component::<Position>(entity1).unwrap().x, 1.0);

    println!("âœ“ Component cleanup on archetype migration works correctly!");
}

// =============================================================================
// Query timings, editor rows and the registry
// =============================================================================

/// Two iterators with the same label would overwrite each other's average
/// duration, so `IteratorTimings` records the duplicate.
///
/// The test repeats the bookkeeping `ParQueryIter::for_each` does.
#[test]
fn test_per_label_duplicate_detection() {
    let timing = std::sync::Mutex::new(IteratorTimings::new());

    // Simulate iterator-1 with label "physics".
    {
        let mut timings = timing.lock().unwrap();
        assert!(!timings
            .visited_iterator_labels
            .iter()
            .any(|label| label == "physics"));
        timings.visited_iterator_labels.push("physics".to_owned());
        timings
            .per_iterator_label_average_duration
            .insert("physics".to_owned(), 120_000);
    }

    // Simulate iterator-2 with label "ai" - different label, no duplicate.
    {
        let mut timings = timing.lock().unwrap();
        assert!(!timings
            .visited_iterator_labels
            .iter()
            .any(|label| label == "ai"));
        timings.visited_iterator_labels.push("ai".to_owned());
        timings
            .per_iterator_label_average_duration
            .insert("ai".to_owned(), 50_000);
    }

    // A second iterator with the label "physics" is the duplicate case.
    {
        let mut timings = timing.lock().unwrap();
        assert!(timings
            .visited_iterator_labels
            .iter()
            .any(|label| label == "physics"));
        timings
            .visited_duplicated_iterator_labels
            .push("physics".to_owned());
        // This write replaces the first average, which is the problem the
        // duplicate flag records.
        timings
            .per_iterator_label_average_duration
            .insert("physics".to_owned(), 800_000);
    }

    let timings = timing.lock().unwrap();
    assert_eq!(timings.visited_duplicated_iterator_labels, vec!["physics"]);
    assert_eq!(timings.visited_iterator_labels, vec!["physics", "ai"]);
    // The average stored for "physics" is now the second write's value.
    assert_eq!(
        timings.per_iterator_label_average_duration["physics"],
        800_000
    );

    println!("âœ“ Duplicate label detection works correctly!");
}

/// The editor's Hierarchy source: `entity_rows` lists every live entity
/// with its component names, sorted by the entity's slot id.
#[test]
fn entity_rows_list_live_entities_with_components() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();

    let a = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let _b = world
        .create_entity()
        .with(Position { x: 3.0, y: 4.0 })
        .build()
        .unwrap();

    let rows = world.entity_rows();
    assert_eq!(rows.len(), 2);
    // The order follows the slot id, and `a` was created first with slot id 0.
    assert_eq!(rows[0].entity, a);
    assert_eq!(rows[0].components.len(), 2);
    assert!(rows[0]
        .components
        .iter()
        .any(|name| name.contains("Position")));
    assert!(rows[0]
        .components
        .iter()
        .any(|name| name.contains("Velocity")));
    assert_eq!(rows[1].components.len(), 1);

    // A destroyed entity no longer appears and reports no names.
    let _ = world.destroy_entity(a);
    assert_eq!(world.entity_rows().len(), 1);
    assert_eq!(world.entity_component_names(a), None);
}

/// `live_entities` lists the same entities as `entity_rows`, in the same slot
/// order, and drops a destroyed one.
#[test]
fn live_entities_match_entity_rows_in_slot_order() {
    let mut world = World::new();
    world.register_component::<Position>();

    let created: Vec<Entity> = (0..5)
        .map(|index| {
            world
                .create_entity()
                .with(Position {
                    x: index as f32,
                    y: 0.0,
                })
                .build()
                .unwrap()
        })
        .collect();
    let row_entities: Vec<Entity> = world.entity_rows().iter().map(|row| row.entity).collect();
    assert_eq!(world.live_entities(), row_entities);
    assert_eq!(world.live_entities(), created);

    let _ = world.destroy_entity(created[2]);
    let remaining = world.live_entities();
    assert_eq!(remaining.len(), 4);
    assert!(!remaining.contains(&created[2]));
}

/// `entity_component_names` includes runtime-defined (descriptor)
/// components, and `resolve_entity_component_id` is scoped to the entity's
/// archetype.
#[test]
fn descriptor_components_appear_and_resolution_is_archetype_scoped() {
    let mut world = World::new();
    world.register_component::<Position>();
    let descriptor_component_id = world
        .register_component_descriptor(
            0xABCD,
            "Demo.Thing",
            4,
            4,
            99,
            Blittability::engine_verified(),
        )
        .unwrap();

    let with_descriptor = world
        .create_descriptor_entity(&[(descriptor_component_id, 7_u32.to_ne_bytes().to_vec())])
        .unwrap();
    let with_position = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let names = world
        .entity_component_names(with_descriptor)
        .expect("entity alive");
    assert_eq!(names, vec!["Demo.Thing"]);

    // Resolution is per entity. The descriptor component resolves only on the
    // entity that carries it, and Position only on its own entity.
    assert_eq!(
        world.resolve_entity_component_id(with_descriptor, "Demo.Thing"),
        Some(descriptor_component_id)
    );
    assert_eq!(
        world.resolve_entity_component_id(with_position, "Demo.Thing"),
        None
    );
    assert_eq!(
        world.resolve_entity_component_id(with_position, &type_name_of::<Position>()),
        Some(ComponentId::of::<Position>())
    );
}

/// `registered_components` lists every type (native and descriptor), sorted.
#[test]
fn registered_components_lists_every_type_sorted() {
    let mut world = World::new();
    world.register_component::<Velocity>();
    world.register_component::<Position>();
    world
        .register_component_descriptor(
            0x1111,
            "Demo.Alpha",
            4,
            4,
            1,
            Blittability::engine_verified(),
        )
        .unwrap();

    let registered = world.registered_components();
    assert_eq!(registered.len(), 3);
    // Sorted by name: the uppercase demo name sorts before the full type paths.
    let names: Vec<String> = registered.iter().map(|(name, _)| name.clone()).collect();
    assert_eq!(names[0], "Demo.Alpha");
    assert!(names[1].contains("Position"));
    assert!(names[2].contains("Velocity"));
}

// =============================================================================
// Resource re-homing
// =============================================================================

/// Counts its own drops, so a re-home that corrupted the stored function
/// table shows up as a missing or doubled drop rather than as silence.
///
/// The counter is owned per instance rather than being a `static`: the
/// harness runs these tests in parallel, and a shared counter makes every
/// drop-count assertion race with the others.
#[derive(Debug)]
struct RehomeProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
impl crate::resource::Resource for RehomeProbe {}

impl Drop for RehomeProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// A fresh probe and the counter watching it.
fn rehome_probe() -> (RehomeProbe, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let drop_counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    (
        RehomeProbe(std::sync::Arc::clone(&drop_counter)),
        drop_counter,
    )
}

/// Reads one probe's counter.
fn drops_of(drop_counter: &std::sync::atomic::AtomicUsize) -> usize {
    drop_counter.load(std::sync::atomic::Ordering::SeqCst)
}

/// Inserting a resource records its function table, so a later reload has
/// something to re-home from even if it never inserts the value again.
#[test]
fn inserting_a_resource_records_its_function_table() {
    let mut world = World::new();
    let id = crate::resource::ResourceId::of::<RehomeProbe>();

    assert!(!world.resource_factories.contains_key(&id));
    world.insert_resource(rehome_probe().0);
    assert!(world.resource_factories.contains_key(&id));
}

/// `register_resource` records the table without inserting a value, which
/// is how a reloaded generation keeps an existing resource re-homeable.
#[test]
fn registering_a_resource_records_the_table_without_a_value() {
    let mut world = World::new();
    let id = crate::resource::ResourceId::of::<RehomeProbe>();

    world.register_resource::<RehomeProbe>();

    assert!(world.resource_factories.contains_key(&id));
    assert!(
        !world.has_resource::<RehomeProbe>(),
        "no value was inserted"
    );
}

/// Re-homing leaves the value intact and still droppable exactly once.
#[cfg(feature = "development")]
#[test]
fn rehoming_resources_preserves_the_value_and_its_drop() {
    let (probe, drop_counter) = rehome_probe();
    let mut world = World::new();
    world.insert_resource(probe);

    // Stands in for a reloaded generation re-registering the type.
    world.register_resource::<RehomeProbe>();
    world.rehome_resources();

    assert!(world.has_resource::<RehomeProbe>(), "the value survives");
    assert_eq!(drops_of(&drop_counter), 0, "nothing was dropped");

    drop(world);
    assert_eq!(
        drops_of(&drop_counter),
        1,
        "the value is dropped exactly once after a re-home"
    );
}

/// The registration log reports what one `init` claimed, which is what
/// distinguishes a retired owner from a live one.
///
/// `resource_factories` cannot answer that: it records which ids are declared,
/// not when or by which generation, so a retired module's entry looks identical
/// to a live one's.
#[test]
fn the_registration_log_reports_what_one_generation_claimed() {
    let mut world = World::new();
    world.insert_resource(rehome_probe().0);

    // Stands in for the moment just before a module's `init` runs.
    let before_init = world.resource_registration_sequence();
    world.register_resource::<artifact_a::Settings>();

    assert_eq!(
        world.resource_ids_registered_since(before_init),
        vec![crate::resource::ResourceId::of::<artifact_a::Settings>()],
        "only what this generation registered, not everything ever registered"
    );
}

/// Dropping a retired owner's resource releases its value, which is what
/// the host must do while the owning image is still mapped.
#[test]
fn dropping_a_retired_owners_resource_releases_its_value() {
    let (probe, drop_counter) = rehome_probe();
    let mut world = World::new();
    world.insert_resource(probe);
    let id = crate::resource::ResourceId::of::<RehomeProbe>();

    assert_eq!(world.drop_resources(&[id]), 1);
    assert_eq!(
        drops_of(&drop_counter),
        1,
        "the value was dropped, not leaked"
    );
    assert!(!world.has_resource::<RehomeProbe>());
    // Its bookkeeping goes too, so a later insert starts clean.
    assert!(!world.resource_factories.contains_key(&id));
}

/// Dropping an id the world does not hold is harmless and reports zero.
#[test]
fn dropping_an_absent_resource_is_a_no_op() {
    let mut world = World::new();
    let id = crate::resource::ResourceId::of::<RehomeProbe>();
    assert_eq!(world.drop_resources(&[id]), 0);
}

/// Removing a resource forgets its table, so a later resource registered
/// under the same id cannot inherit a stale one.
#[test]
fn removing_a_resource_forgets_its_function_table() {
    let mut world = World::new();
    let id = crate::resource::ResourceId::of::<RehomeProbe>();
    world.insert_resource(rehome_probe().0);

    world
        .remove_resource::<RehomeProbe>()
        .expect("it was there");

    assert!(!world.resource_factories.contains_key(&id));
}

// =============================================================================
// Shared resource identity
// =============================================================================

// A shared resource type, standing in for one artifact's copy of it. The
// behaviour across two artifacts lives in the `pill_engine` crate's
// `tests/shared_resource_identity.rs`. The tests here need the world's private
// bookkeeping, which an integration test cannot reach.
mod artifact_a {
    /// `demo::Settings` as one artifact compiled it.
    #[derive(Debug)]
    pub struct Settings {
        // The identity checks compare shapes, not values, so nothing reads
        // this field.
        #[allow(dead_code)]
        pub value: u32,
    }
    impl crate::resource::Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("demo::Settings")
        }
    }
}

/// An ordinary resource keeps its strict identity check: its id is its
/// `TypeId`, and the box compares that exactly.
#[test]
fn an_ordinary_resource_keeps_the_strict_identity_check() {
    let mut world = World::new();
    world.insert_resource(rehome_probe().0);

    assert!(!world
        .resources
        .get(&crate::resource::ResourceId::of::<RehomeProbe>())
        .unwrap()
        .has_shared_identity());
    assert!(world.take_registration_error().is_none());
}

/// A released bit cannot stand for two component sets. The archetype that
/// carried it is gone before the bit can be used again, so the next type to
/// take it gets its own column and its own archetype.
#[test]
fn recycled_bit_does_not_alias_archetypes() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.register_component::<Velocity>();
    let velocity_id = ComponentId::of::<Velocity>();
    let released_bit = world
        .component_registry()
        .get_bit(&velocity_id)
        .expect("registered");
    let kept_entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .with(Velocity { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let dropped_row_count = world.drop_forgotten_components(&[
        crate::type_names::stable_type_name::<Velocity>().to_string(),
    ]);
    assert_eq!(
        dropped_row_count, 1,
        "the entity's velocity row was rehomed out"
    );
    assert_eq!(
        world.component_registry().get_bit(&velocity_id),
        None,
        "the registration is gone with the rows"
    );

    // The next registration takes the freed bit...
    world.register_component::<Health>();
    let health_id = ComponentId::of::<Health>();
    assert_eq!(
        world.component_registry().get_bit(&health_id),
        Some(released_bit),
        "the freed bit is the one reused"
    );

    // ...and the archetype it creates is its own, holding exactly its row.
    let health_entity = world
        .create_entity()
        .with(Position { x: 3.0, y: 4.0 })
        .with(Health(5))
        .build()
        .unwrap();
    assert_eq!(world.live_row_count(health_id), 1);
    assert_eq!(world.live_row_count(ComponentId::of::<Position>()), 2);
    assert_eq!(
        world.storage.archetypes.len(),
        2,
        "position-only and position+health are two archetypes"
    );
    let location = world.storage.entity_locations[&health_entity];
    let archetype = &world.storage.archetypes[&location.archetype_id];
    assert!(archetype.component_types.contains(&health_id));
    assert!(!archetype.component_types.contains(&velocity_id));
    assert!(world.storage.entity_locations.contains_key(&kept_entity));
}

/// A re-registration updates the id's stamp instead of adding an entry. The
/// log then stays proportional to the number of distinct resources, and a
/// generation that registers its resources again still shows up once in the
/// host's reload diff.
#[test]
fn a_re_registered_resource_stays_one_registration() {
    let mut world = World::new();
    world.insert_resource(AccountedResource {
        registration_marker: 1,
    });
    let sequence = world.resource_registration_sequence();

    world.insert_resource(AccountedResource {
        registration_marker: 2,
    });
    world.insert_resource(AccountedResource {
        registration_marker: 3,
    });
    assert_eq!(
        world
            .get_resource::<AccountedResource>()
            .unwrap()
            .registration_marker,
        3,
        "the replacement really replaced the value"
    );
    let id = crate::resource::ResourceId::of::<AccountedResource>();
    assert_eq!(
        world.resource_ids_registered_since(sequence),
        vec![id],
        "a re-registration inside the window is reported, exactly once"
    );

    world.insert_resource(FreshAccountedResource {
        registration_marker: 4,
    });
    assert_eq!(
        world
            .get_resource::<FreshAccountedResource>()
            .unwrap()
            .registration_marker,
        4
    );
    let mut claimed = world.resource_ids_registered_since(sequence);
    claimed.sort_unstable();
    let mut expected = vec![
        id,
        crate::resource::ResourceId::of::<FreshAccountedResource>(),
    ];
    expected.sort_unstable();
    assert_eq!(
        claimed, expected,
        "the fresh id joins the window; nothing accumulates per insert"
    );
    assert_eq!(
        world.resource_registration_stamps.len(),
        2,
        "one entry per distinct id, however many times it was written"
    );
}

/// One subject's retirement does not drop a resource that another subject
/// still claims. The claim count keeps the value alive until the last claimant
/// releases it.
#[test]
fn a_shared_claim_survives_one_subjects_retirement() {
    let mut world = World::new();
    let id = crate::resource::ResourceId::of::<AccountedResource>();
    world.insert_resource(AccountedResource {
        registration_marker: 1,
    });

    // Two subjects registered it.
    world.retain_resource_claims(&[id]);
    world.retain_resource_claims(&[id]);

    // The first release leaves the value in place, because one claim is still
    // held.
    world.release_resource_claims(&[id]);
    assert_eq!(
        world.drop_resources(&[id]),
        0,
        "a live claim keeps the value standing"
    );
    assert!(world.get_resource::<AccountedResource>().is_some());

    // The last claimant retires it too.
    world.release_resource_claims(&[id]);
    assert_eq!(world.drop_resources(&[id]), 1, "the last release frees it");
    assert!(world.get_resource::<AccountedResource>().is_none());
}

/// A relayout moves the factory, the live column and the schema hash together.
///
/// Storage shape has one owner, so this pins the factory to the column that
/// allocates the rows and to the size readers consult.
#[cfg(feature = "development")]
#[test]
fn relayout_moves_the_factory_and_the_column_together() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xD9,
            "Project.Republished",
            8,
            4,
            100,
            Blittability::engine_verified(),
        )
        .unwrap();
    world
        .create_descriptor_entity(&[(component_id, vec![0; 8])])
        .unwrap();

    world
        .relayout_descriptor_component(component_id, 16, 8, 200, &FieldPlan::new())
        .expect("the empty plan fits both layouts");

    let Some(StorageFactory::Descriptor(layout)) = world.storage_factories.get(&component_id)
    else {
        panic!("the factory is not a descriptor factory");
    };
    assert_eq!(
        (layout.size, layout.align, layout.schema_hash),
        (16, 8, 200),
        "the factory carries the relayout"
    );
    assert_eq!(
        world.component_layout(component_id),
        Some((16, 8)),
        "the readers that ask for a component's storage shape see the new one"
    );
    let column = world
        .storage
        .archetypes
        .values()
        .find_map(|archetype| archetype.component_storages.get(component_id))
        .expect("the row's column");
    assert_eq!(
        (column.element_size(), column.alignment()),
        (16, 8),
        "the column that allocates the rows carries the same shape"
    );
}

// =============================================================================
// Foreign resources
// =============================================================================

/// A declared name is hashed the way the engine hashes type names, so a
/// declaration from another language and a Rust type that writes the same
/// string are one resource. That is why the name is declared instead of
/// derived.
#[test]
fn a_foreign_declaration_hashes_to_the_rust_types_id() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 7)
        .expect("a fresh name is claimed");

    assert_eq!(
        id,
        crate::resource::ResourceId::of::<artifact_a::Settings>()
    );
    assert_eq!(
        id,
        crate::resource::ResourceId::Shared(crate::component::shared_component_identity(
            "demo::Settings"
        ))
    );
    assert_eq!(world.foreign_resource_layout(id), Some((4, 4, 7)));
    assert_eq!(
        world.shared_resource_names(),
        vec!["demo::Settings".to_string()]
    );
}

/// The claim guard keeps the name check for two Rust registrations and uses
/// the layout for a cross-language join. A matching declaration joins, and a
/// mismatched one is refused.
#[test]
fn a_foreign_declaration_joins_by_layout_and_is_refused_by_a_mismatch() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    assert!(world.take_registration_error().is_none());

    let id = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 7)
        .expect("the same layout joins");
    assert_eq!(
        id,
        crate::resource::ResourceId::of::<artifact_a::Settings>()
    );
    assert!(
        world.take_registration_error().is_none(),
        "a matching declaration is not a conflict"
    );

    let error = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 16, 8, 7)
        .expect_err("a different layout cannot be joined");
    assert!(matches!(
        error,
        WorldError::SharedResourceLayoutMismatch { .. }
    ));
    assert!(
        world.take_registration_error().is_none(),
        "the refusal goes back to the caller that made it, not to the next init drain"
    );
}

/// Bytes round-trip through a declaration, the size is checked, and a
/// mutable view stamps the change tick as it is handed out.
#[test]
fn foreign_bytes_round_trip_and_stamp_the_change_tick() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("demo::Bytes", "Project.Bytes", 8, 4, 7)
        .expect("a fresh name is claimed");
    assert!(world.foreign_resource_bytes(id).is_none(), "no value yet");

    world
        .insert_foreign_resource_bytes(id, &[1, 2, 3, 4, 5, 6, 7, 8])
        .expect("the payload matches the size");
    assert_eq!(
        world.foreign_resource_bytes(id),
        Some([1_u8, 2, 3, 4, 5, 6, 7, 8].as_slice())
    );

    assert!(matches!(
        world.insert_foreign_resource_bytes(id, &[0; 4]),
        Err(WorldError::ForeignResourceBytesMismatch { .. })
    ));
    assert_eq!(
        world.foreign_resource_bytes(id).unwrap().len(),
        8,
        "a refused payload leaves the stored value alone"
    );

    world.increment_change_tick();
    let expected_tick = world.change_tick();
    let (bytes, ticks) = world
        .foreign_resource_bytes_mut(id)
        .expect("the value is there");
    bytes[0] = 9;
    assert_eq!(ticks.changed, expected_tick);
    assert_eq!(world.foreign_resource_bytes(id).unwrap()[0], 9);
}

/// A private resource has no byte view. Its id is a `TypeId`, which no other
/// language can name, so there is no id for one to address.
#[cfg(feature = "development")]
#[test]
fn a_private_resource_has_no_foreign_view() {
    let mut world = World::new();
    world.insert_resource(rehome_probe().0);
    let id = crate::resource::ResourceId::of::<RehomeProbe>();

    assert!(world.foreign_resource_bytes(id).is_none());
    assert!(world.foreign_resource_bytes_mut(id).is_none());
    assert!(matches!(
        world.insert_foreign_resource_bytes(id, &[0; 8]),
        Err(WorldError::SharedResourceNotRegistered { .. })
    ));
    assert!(matches!(
        world.relayout_foreign_resource(id, 8, 4, 1, &FieldPlan::new()),
        Err(WorldError::ForeignResourceNotRegistered { .. })
    ));
}

/// A relayout moves the stored bytes through the plan, updates the declared
/// layout, and reports how many values it migrated.
#[cfg(feature = "development")]
#[test]
fn relayout_migrates_a_foreign_value_through_the_plan() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("demo::Relayout", "Project.Relayout", 8, 4, 1)
        .expect("a fresh name is claimed");

    // `b`, then `a`, then an eight-byte field that did not exist.
    let plan = FieldPlan::between(
        &[
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
        &[
            LayoutField {
                name: "b",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "a",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
            LayoutField {
                name: "added",
                type_tag: "u32",
                offset: 8,
                size: 8,
            },
        ],
    );

    // With no value stored, the declaration moves and nothing is migrated.
    assert_eq!(
        world
            .relayout_foreign_resource(id, 16, 8, 2, &plan)
            .expect("the declaration moves"),
        0
    );
    assert_eq!(world.foreign_resource_layout(id), Some((16, 8, 2)));

    world
        .insert_foreign_resource_bytes(id, &[1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .expect("the payload matches the new size");
    assert_eq!(
        world
            .relayout_foreign_resource(id, 16, 8, 2, &FieldPlan::new())
            .expect("a value is there to migrate"),
        1
    );
    assert_eq!(
        world.foreign_resource_bytes(id),
        Some([0_u8; 16].as_slice()),
        "an empty plan zeroes the value it migrates"
    );
}

/// A Rust value's shape is its type, so a foreign declaration cannot
/// reshape it, and a plan that does not fit the payload is refused before
/// anything moves.
#[cfg(feature = "development")]
#[test]
fn relayout_refuses_a_rust_value_and_a_plan_that_leaves_the_payload() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    let id = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 1)
        .expect("the same layout joins");

    // A Rust value stored under the id brings its own function table. From
    // then on the declaration belongs to the Rust type, so a foreign relayout
    // finds no declaration to write to.
    world.insert_resource(artifact_a::Settings { value: 3 });
    assert!(matches!(
        world.relayout_foreign_resource(id, 8, 4, 2, &FieldPlan::new()),
        Err(WorldError::ForeignResourceNotRegistered { .. })
    ));

    // Declaring the id foreign again, which the managed side does on every
    // reload, is refused. Admitting the declaration would mean dropping the
    // stored value through the foreign table, so it is not admitted.
    assert!(matches!(
        world.register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 1),
        Err(WorldError::ForeignResourceHoldsRustValue { .. })
    ));
    assert_eq!(
        world.foreign_resource_layout(id),
        None,
        "the Rust value's table still owns the id"
    );

    // A declaration that was foreign from the start still checks its payload,
    // so there only the plan can refuse a resize.
    let fresh_resource_id = world
        .register_foreign_resource("demo::Other", "Project.Other", 4, 4, 1)
        .expect("a fresh name is claimed");
    world
        .insert_foreign_resource_bytes(fresh_resource_id, &3_u32.to_ne_bytes())
        .expect("the payload matches the size");
    let mut too_wide = FieldPlan::new();
    too_wide.push(2, 4, FieldSource::OldOffset(0));
    assert!(matches!(
        world.relayout_foreign_resource(fresh_resource_id, 4, 4, 2, &too_wide),
        Err(WorldError::ForeignResourcePlanOutOfBounds { .. })
    ));
    assert_eq!(
        world.foreign_resource_layout(fresh_resource_id),
        Some((4, 4, 1)),
        "a refused relayout leaves the declaration as it was"
    );
}

/// Re-homing leaves a foreign box's table alone. The bytes are not the Rust
/// type's to drop, even when the two share a name.
#[cfg(feature = "development")]
#[test]
fn rehoming_leaves_a_foreign_box_alone() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 1)
        .expect("a fresh name is claimed");
    world
        .insert_foreign_resource_bytes(id, &9_u32.to_ne_bytes())
        .expect("the payload matches the size");

    // The Rust owner registers the same name, which makes the declaration
    // Rust's while the stored bytes stay foreign. A re-home has to respect that
    // split: without the skip it would hand these bytes the Rust type's
    // destructor, which frees memory that type never allocated.
    world.register_resource::<artifact_a::Settings>();
    assert!(world.resources[&id].is_foreign());
    assert_eq!(
        world.resource_factories.get(&id).map(|ops| ops.foreign),
        Some(false),
        "the Rust registration owns the declaration again"
    );

    world.rehome_resources();

    assert!(
        world.resources[&id].is_foreign(),
        "a foreign box keeps its own drop, whatever the factories say"
    );
    assert_eq!(
        world.resources[&id].bytes(),
        9_u32.to_ne_bytes().as_slice(),
        "leaving the table alone leaves the payload alone too"
    );
}

/// A foreign declaration cannot take over an id that stores a Rust value. The
/// value's destructor belongs to its type, so its bytes are never read as
/// foreign, and a table that would have dropped nothing is not installed.
#[test]
fn foreign_registration_refuses_a_stored_rust_value() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    world.insert_resource(artifact_a::Settings { value: 7 });
    let id = crate::resource::ResourceId::of::<artifact_a::Settings>();

    assert!(matches!(
        world.register_foreign_resource("demo::Settings", "Project.Settings", 4, 4, 1),
        Err(WorldError::ForeignResourceHoldsRustValue { .. })
    ));
    assert_eq!(
        world.resource_factories.get(&id).map(|ops| ops.foreign),
        Some(false),
        "the Rust registration still owns the id"
    );
    assert_eq!(
        world
            .get_resource::<artifact_a::Settings>()
            .map(|v| v.value),
        Some(7),
        "the refusal left the value alone"
    );
}

/// Byte views exist for foreign payloads only. A Rust value that declares a
/// shared name is not served as bytes, and the refusal changes nothing: no
/// tick is stamped and no payload is replaced.
#[test]
fn foreign_byte_views_refuse_a_rust_owned_resource() {
    let mut world = World::new();
    world.register_resource::<artifact_a::Settings>();
    world.insert_resource(artifact_a::Settings { value: 5 });
    let id = crate::resource::ResourceId::of::<artifact_a::Settings>();

    assert!(world.foreign_resource_bytes(id).is_none());
    assert!(world.foreign_resource_bytes_mut(id).is_none());
    assert!(matches!(
        world.insert_foreign_resource_bytes(id, &5_u32.to_ne_bytes()),
        Err(WorldError::ForeignResourceFactoryIsNative { .. })
    ));
    assert_eq!(
        world
            .get_resource::<artifact_a::Settings>()
            .map(|v| v.value),
        Some(5),
        "the value survived the refusals"
    );
}

// =============================================================================
// Zero-sized components
// =============================================================================

/// A zero-sized Rust type is a component like any other.
///
/// This is the tag/marker idiom (`struct Enemy;`): a native declaration of zero
/// width is a filter, and every path that treats it as a row - spawn, migration,
/// removal, destruction - has to work on a column that allocates nothing.
#[test]
fn a_zero_sized_component_lives_a_full_life() {
    #[derive(Clone, Debug, Default)]
    struct Enemy;
    impl Component for Enemy {}

    #[derive(Clone, Debug, Default)]
    struct Health {
        points: u32,
    }
    impl Component for Health {}

    let mut world = World::new();
    world.register_component::<Enemy>();
    world.register_component::<Health>();

    // Spawn: a marker alone, and a marker beside a sized component, so the
    // zero-sized column is exercised both as an archetype's only column and as
    // one of several.
    let lone = world.create_entity().with(Enemy).build().unwrap();
    let paired = world
        .create_entity()
        .with(Enemy)
        .with(Health { points: 7 })
        .build()
        .unwrap();

    assert!(world.get_component::<Enemy>(lone).is_some());
    assert!(world.get_component::<Enemy>(paired).is_some());

    // Column contents: a marker is a filter, which is the whole reason to
    // declare one, so both rows have to be in the column the filter reads.
    let marker_id = ComponentId::of::<Enemy>();
    let row_count: usize = world
        .storage
        .archetypes
        .values()
        .filter_map(|archetype| archetype.component_storages.get(marker_id))
        .map(|column| column.len())
        .sum();
    assert_eq!(
        row_count, 2,
        "both markers occupy a row in the zero-sized column"
    );

    // Archetype move: adding a component migrates every column, the zero-sized
    // one included, and its rows have no bytes to carry.
    world
        .add_component(lone, Health { points: 3 })
        .expect("the marker's entity accepts another component");
    assert!(world.get_component::<Enemy>(lone).is_some());
    assert_eq!(world.get_component::<Health>(lone).unwrap().points, 3);

    // Removal: the swap-remove that closes the gap copies zero bytes, and the
    // surviving row must still be found.
    world
        .remove_component::<Enemy>(paired)
        .expect("the marker comes off");
    assert!(world.get_component::<Enemy>(paired).is_none());
    assert!(
        world.get_component::<Enemy>(lone).is_some(),
        "removing one marker leaves the other"
    );
    assert_eq!(
        world.get_component::<Health>(paired).unwrap().points,
        7,
        "the sized companion is untouched by the marker's removal"
    );

    // Destruction: the column frees a buffer it never allocated.
    assert!(world.destroy_entity(lone), "the entity is destroyed");
    assert!(world.get_component::<Enemy>(lone).is_none());
}

/// A zero-sized component with a wider alignment is still addressable.
///
/// A column that never allocates keeps its dangling pointer for life, so that
/// pointer has to be aligned for the element rather than for `u8`. Reading a
/// row through a misaligned pointer is undefined behaviour even when the row
/// has no bytes, so this pins the alignment rather than the size.
#[test]
fn an_over_aligned_zero_sized_component_is_addressable() {
    #[derive(Clone, Debug, Default)]
    #[repr(align(16))]
    struct AlignedTag;
    impl Component for AlignedTag {}

    assert_eq!(std::mem::size_of::<AlignedTag>(), 0);
    assert_eq!(std::mem::align_of::<AlignedTag>(), 16);

    let mut world = World::new();
    world.register_component::<AlignedTag>();
    let entity = world.create_entity().with(AlignedTag).build().unwrap();

    assert!(world.get_component::<AlignedTag>(entity).is_some());
}

/// A zero-width *descriptor* declaration is still refused.
///
/// The rule differs by source: a descriptor's width comes from a manifest
/// another language wrote, where zero means the declaration is wrong. The test
/// keeps the two rules separate, so a malformed manifest stays refused.
#[test]
fn a_zero_width_descriptor_component_is_still_refused() {
    let mut world = World::new();

    let refused = world.register_component_descriptor(
        0x2E_0001,
        "probe::ZeroWidth".to_string(),
        0,
        4,
        11,
        Blittability::from_manifest_fields(),
    );

    assert!(
        matches!(refused, Err(WorldError::DescriptorSizeZero)),
        "a zero-width manifest declaration is a declaration error, not a marker"
    );
}

// =============================================================================
// Resource field layouts
// =============================================================================

/// A foreign resource's declared fields are stored and served back.
///
/// Without this a foreign resource is opaque bytes: the editor has nothing
/// to name, and neither does anything else that wants more than a width.
#[test]
fn a_foreign_resource_serves_its_declared_field_layout() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("probe::Tuning", "Probe.Tuning", 8, 4, 1)
        .expect("a valid foreign layout registers");

    assert!(
        world.resource_field_layout(id).is_none(),
        "a resource registered without a layout is simply not inspectable"
    );

    world
        .register_resource_field_layout(
            id,
            vec![
                crate::component_registry::ComponentFieldDescriptor {
                    name: "speed",
                    type_tag: "f32",
                    offset: 0,
                    size: 4,
                    align: 4,
                    element_count: 0,
                    decorators: std::borrow::Cow::Borrowed(&[]),
                },
                crate::component_registry::ComponentFieldDescriptor {
                    name: "gain",
                    type_tag: "f32",
                    offset: 4,
                    size: 4,
                    align: 4,
                    element_count: 0,
                    decorators: std::borrow::Cow::Borrowed(&[]),
                },
            ],
        )
        .expect("the fields fit the registered size");

    let fields = world
        .resource_field_layout(id)
        .expect("the layout is stored");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "speed");
    assert_eq!(fields[1].offset, 4);
}

/// A field layout that cannot describe the resource is refused.
///
/// Both checks the descriptor-component path applies, for the same reason:
/// a foreign resource's bytes are as opaque as a descriptor row, so a
/// reader has to be kept inside the value and away from any pointer pair
/// the bytes never held.
#[test]
fn an_impossible_resource_field_layout_is_refused() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("probe::Narrow", "Probe.Narrow", 4, 4, 1)
        .expect("a valid foreign layout registers");

    let past_the_end = world.register_resource_field_layout(
        id,
        vec![crate::component_registry::ComponentFieldDescriptor {
            name: "wide",
            type_tag: "u64",
            offset: 0,
            size: 8,
            align: 8,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }],
    );
    assert!(
        past_the_end.is_err(),
        "a field may not reach past the value"
    );

    let container = world.register_resource_field_layout(
        id,
        vec![crate::component_registry::ComponentFieldDescriptor {
            name: "items",
            type_tag: "vec:f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }],
    );
    assert!(
        container.is_err(),
        "a container tag would be read as a pointer and length the bytes do not hold"
    );
}

/// A relayout drops the layout it described.
///
/// Serving the previous generation's offsets over migrated bytes would be
/// worse than serving nothing. The engine therefore forgets the layout, and
/// the declarer publishes it again, as the host does in the same pass.
#[cfg(feature = "development")]
#[test]
fn a_relayout_drops_the_stored_resource_field_layout() {
    let mut world = World::new();
    let id = world
        .register_foreign_resource("probe::Moving", "Probe.Moving", 4, 4, 1)
        .expect("a valid foreign layout registers");
    world
        .register_resource_field_layout(
            id,
            vec![crate::component_registry::ComponentFieldDescriptor {
                name: "value",
                type_tag: "u32",
                offset: 0,
                size: 4,
                align: 4,
                element_count: 0,
                decorators: std::borrow::Cow::Borrowed(&[]),
            }],
        )
        .expect("the field fits");
    assert!(world.resource_field_layout(id).is_some());

    let plan = FieldPlan::between(
        &[LayoutField {
            name: "value",
            type_tag: "u32",
            offset: 0,
            size: 4,
        }],
        &[
            LayoutField {
                name: "value",
                type_tag: "u32",
                offset: 0,
                size: 4,
            },
            LayoutField {
                name: "added",
                type_tag: "u32",
                offset: 4,
                size: 4,
            },
        ],
    );
    world
        .relayout_foreign_resource(id, 8, 4, 2, &plan)
        .expect("the wider shape is valid");

    assert!(
        world.resource_field_layout(id).is_none(),
        "the layout that described the old shape is forgotten"
    );
}

// =============================================================================
// Entity creation and identity
// =============================================================================

/// The stable-id index. Every created entity gets an id, the id resolves back
/// to the entity, and destroying the entity drops both directions, so a stale
/// id names nothing instead of a later occupant of the recycled slot.
#[test]
fn a_created_entity_has_an_id_that_dies_with_it() {
    let mut world = World::new();
    world.register_component::<Position>();
    let entity = world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .unwrap();
    let id = world.entity_id(entity).expect("a created entity has an id");
    assert_eq!(world.entity_by_id(id), Some(entity));

    let other = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let other_id = world.entity_id(other).expect("the second entity has one");
    assert_ne!(id, other_id, "ids are not derived from the slot");

    assert!(world.destroy_entity(entity));
    assert_eq!(world.entity_id(entity), None);
    assert_eq!(world.entity_by_id(id), None, "the id no longer resolves");

    // The slot is recycled by the next create, and the recycled entity gets a
    // fresh id: the old one must not come back with it.
    let recycled = world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    let recycled_id = world.entity_id(recycled).expect("it has one");
    assert_ne!(recycled_id, id);
}

/// A creator that supplies the id keeps it, which is what a scene load needs.
/// A second entity claiming the same id is refused before it is inserted.
#[test]
fn a_supplied_id_is_kept_and_a_duplicate_is_refused() {
    let mut world = World::new();
    world.register_component::<Position>();
    let id = EntityId::new(0x5eed);
    let loaded = world
        .create_entity()
        .with_id(id)
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(world.entity_by_id(id), Some(loaded));

    let refused = world
        .create_entity()
        .with_id(id)
        .with(Position { x: 0.0, y: 0.0 })
        .build();
    assert!(
        matches!(refused, Err(BuildError::DuplicateEntityId { .. })),
        "{refused:?}"
    );
    assert!(
        world.entity_count() == 1,
        "the refused build inserted nothing: {} entities",
        world.entity_count()
    );
}

/// A builder handed one component type twice is refused before the row exists.
///
/// The columns are keyed by component id, so a second inserter would push into
/// the column the first one already grew: one entity row and two component
/// rows. Every later entity would then read the row before its own, which is
/// what the assertion at the end measures. The archetype is created first,
/// because on a fresh component set the duplicate panics inside it instead, so
/// only the cached path shows the silent corruption.
#[test]
fn a_component_the_builder_is_given_twice_is_refused() {
    let mut world = World::new();
    world.register_component::<Position>();
    world
        .create_entity()
        .with(Position { x: 0.0, y: 0.0 })
        .build()
        .unwrap();

    let refused = world
        .create_entity()
        .with(Position { x: 1.0, y: 0.0 })
        .with(Position { x: 2.0, y: 0.0 })
        .build();
    assert!(
        matches!(refused, Err(BuildError::DuplicateComponent { .. })),
        "{refused:?}"
    );
    assert!(
        world.entity_count() == 1,
        "the refused build inserted nothing: {} entities",
        world.entity_count()
    );

    // The damage the refusal prevents: with the duplicate accepted, the column
    // ends one row ahead of the entity list, so this entity's component reads
    // the duplicate's second value instead of its own.
    let next = world
        .create_entity()
        .with(Position { x: 3.0, y: 0.0 })
        .build()
        .unwrap();
    assert_eq!(
        world.get_component::<Position>(next),
        Some(&Position { x: 3.0, y: 0.0 })
    );
}

// =============================================================================
// Change ticks
// =============================================================================

/// A tick counter that wrapped is not "nothing changed".
///
/// A row written after the counter wrapped carries a small tick, so a plain
/// `>` comparison would read it as older than the run before the wrap and the
/// change would never be reported again. `Tick::is_changed` compares across the
/// wrap instead.
#[test]
fn a_wrapped_tick_counter_still_reports_a_change() {
    let mut world = World::new();
    let component_id = world
        .register_component_descriptor(
            0xDA,
            "Project.Wrapped",
            8,
            4,
            101,
            Blittability::engine_verified(),
        )
        .unwrap();

    // Step 1: the system's last run sits just below the top of the counter.
    world.change_tick = u32::MAX - 2;
    let last_run = world.increment_change_tick();
    assert_eq!(last_run, Tick::new(u32::MAX - 1));

    // Step 2: three increments carry the counter past `u32::MAX`, so the run
    // that sees the write is tick 1, a smaller number than the last run.
    world.increment_change_tick();
    world.increment_change_tick();
    let this_run = world.increment_change_tick();
    assert_eq!(this_run, Tick::new(1));

    // Step 3: a row written now carries the small tick.
    let entity = world
        .create_descriptor_entity(&[(component_id, two_u32_row(1, 2))])
        .unwrap();
    let ticks = ticks_of_row(&world, entity, component_id);
    assert_eq!(ticks.changed, this_run);

    assert!(
        ticks.is_changed(last_run, this_run),
        "a write after the wrap is still newer than the run before it: \
         changed={:?} last_run={last_run:?} this_run={this_run:?}",
        ticks.changed
    );
    assert!(
        ticks.is_added(last_run, this_run),
        "an add after the wrap is still newer than the run before it"
    );
}

// =============================================================================
// Resource removal
// =============================================================================

/// A removal drops every per-id record, the declared field layout included.
///
/// A value re-registered under the same id must not inherit the previous
/// value's offsets, so the layout record goes with the value.
#[test]
fn removing_a_resource_drops_its_declared_field_layout() {
    #[derive(Debug, PartialEq)]
    struct RemovedProbe(u32);
    impl crate::resource::Resource for RemovedProbe {}

    let mut world = World::new();
    world.insert_resource(RemovedProbe(7));
    let id = ResourceId::of::<RemovedProbe>();
    world
        .register_resource_field_layout(
            id,
            vec![crate::component_registry::ComponentFieldDescriptor {
                name: "value",
                type_tag: "u32",
                offset: 0,
                size: 4,
                align: 4,
                element_count: 0,
                decorators: std::borrow::Cow::Borrowed(&[]),
            }],
        )
        .expect("the field fits the registered size");
    assert!(world.resource_field_layout(id).is_some());

    let removed_value = world
        .remove_resource::<RemovedProbe>()
        .expect("the value is a RemovedProbe");
    assert_eq!(removed_value, Some(RemovedProbe(7)));
    assert!(
        world.resource_field_layout(id).is_none(),
        "a removal must drop the id's layout record, not leave it for the next registration"
    );
}

// =============================================================================
// Script components
// =============================================================================

/// A script component that counts its own dispatches, so a double
/// registration is visible as a count above one after a single frame.
#[derive(Debug, Default)]
struct CountingScript {
    dispatches: u32,
}

impl Component for CountingScript {}

impl ScriptDeclaration for CountingScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(0);
}

impl ScriptComponent for CountingScript {
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        self.dispatches += 1;
    }
}

/// The sequence of script runs the ordering tests read.
#[derive(Debug, Default)]
struct ScriptRunLog {
    /// One entry per run, in the order the runs happened.
    entries: Vec<&'static str>,
}

impl Component for ScriptRunLog {}

/// Appends `name` to the run log, which lives on an entity of its own so the
/// borrow is never the running script's own row.
fn append_run_log_entry(
    log_entity: Entity,
    script_context: &mut ScriptContext,
    name: &'static str,
) {
    if let Some(run_log) = script_context.get_component_mut::<ScriptRunLog>(log_entity) {
        run_log.entries.push(name);
    }
}

/// A script that declares a late update order.
#[derive(Debug)]
struct LateScript {
    /// The entity holding the run log.
    log_entity: Entity,
}

impl Component for LateScript {}

impl ScriptDeclaration for LateScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(20);
}

impl ScriptComponent for LateScript {
    fn on_update(&mut self, script_context: &mut ScriptContext) {
        append_run_log_entry(self.log_entity, script_context, "late");
    }
}

/// A script that declares an early update order.
#[derive(Debug)]
struct EarlyScript {
    /// The entity holding the run log.
    log_entity: Entity,
}

impl Component for EarlyScript {}

impl ScriptDeclaration for EarlyScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(10);
}

impl ScriptComponent for EarlyScript {
    fn on_update(&mut self, script_context: &mut ScriptContext) {
        append_run_log_entry(self.log_entity, script_context, "early");
    }
}

/// A script whose update order ties with [`ZuluScript`]'s.
#[derive(Debug)]
struct AlphaScript {
    /// The entity holding the run log.
    log_entity: Entity,
}

impl Component for AlphaScript {}

impl ScriptDeclaration for AlphaScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(5);
}

impl ScriptComponent for AlphaScript {
    fn on_update(&mut self, script_context: &mut ScriptContext) {
        append_run_log_entry(self.log_entity, script_context, "alpha");
    }
}

/// A script whose update order ties with [`AlphaScript`]'s.
#[derive(Debug)]
struct ZuluScript {
    /// The entity holding the run log.
    log_entity: Entity,
}

impl Component for ZuluScript {}

impl ScriptDeclaration for ZuluScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(5);
}

impl ScriptComponent for ZuluScript {
    fn on_update(&mut self, script_context: &mut ScriptContext) {
        append_run_log_entry(self.log_entity, script_context, "zulu");
    }
}

/// A script that declares no update hook at all.
#[derive(Debug)]
struct CreateOnlyScript;

impl Component for CreateOnlyScript {}

impl ScriptDeclaration for CreateOnlyScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_create(0);
}

impl ScriptComponent for CreateOnlyScript {}

/// Returns the counting script's dispatch count as the world holds it.
fn dispatches_of(world: &World, entity: Entity) -> u32 {
    world
        .get_component::<CountingScript>(entity)
        .expect("the entity carries the script")
        .dispatches
}

/// Registering one script type twice must still dispatch it once per frame.
/// The updater table replaces its entry instead of queueing a second one.
#[test]
fn registering_a_script_component_twice_dispatches_it_once_per_frame() {
    let mut world = World::new();
    world.register_script_component::<CountingScript>();
    world.register_script_component::<CountingScript>();

    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, CountingScript::default())
        .expect("the script attaches");

    let mut commands = CommandQueue::new();
    world.update_scripts(&mut commands, RunMode::Playing);

    assert_eq!(
        dispatches_of(&world, entity),
        1,
        "a re-registered script type must not run twice per frame"
    );
}

/// A retired script type that registers again may get a different bit.
/// Dispatch reads the current bit, so the script keeps running.
#[test]
fn a_re_registered_script_component_dispatches_on_its_new_bit() {
    let mut world = World::new();
    world.register_script_component::<CountingScript>();
    world.retire_component_storage(&[ComponentId::of::<CountingScript>()]);
    world.register_script_component::<CountingScript>();

    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, CountingScript::default())
        .expect("the script attaches");

    let mut commands = CommandQueue::new();
    world.update_scripts(&mut commands, RunMode::Playing);

    assert_eq!(
        dispatches_of(&world, entity),
        1,
        "a re-registered script type must dispatch through its current bit"
    );
}

/// Two script types run in their declared order, whatever order they were
/// registered in.
#[test]
fn script_types_update_in_their_declared_order() {
    let mut world = World::new();
    world.register_script_component::<LateScript>();
    world.register_script_component::<EarlyScript>();
    world.register_component::<ScriptRunLog>();

    let log_entity = world
        .create_entity()
        .build()
        .expect("the log entity builds");
    world
        .add_component(log_entity, ScriptRunLog::default())
        .expect("the log attaches");
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, LateScript { log_entity })
        .expect("the late script attaches");
    world
        .add_component(entity, EarlyScript { log_entity })
        .expect("the early script attaches");

    let mut commands = CommandQueue::new();
    world.update_scripts(&mut commands, RunMode::Playing);

    let entries = world
        .get_component::<ScriptRunLog>(log_entity)
        .expect("the run log is on its entity")
        .entries
        .clone();
    assert_eq!(
        entries,
        ["early", "late"],
        "the lower declared order runs first, not the earlier registration"
    );
}

/// Two script types whose orders are equal run in type name order, so a frame
/// with no ordering information still runs the same way every time.
#[test]
fn script_types_with_equal_orders_update_in_type_name_order() {
    let mut world = World::new();
    world.register_script_component::<ZuluScript>();
    world.register_script_component::<AlphaScript>();
    world.register_component::<ScriptRunLog>();

    let log_entity = world
        .create_entity()
        .build()
        .expect("the log entity builds");
    world
        .add_component(log_entity, ScriptRunLog::default())
        .expect("the log attaches");
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, ZuluScript { log_entity })
        .expect("the zulu script attaches");
    world
        .add_component(entity, AlphaScript { log_entity })
        .expect("the alpha script attaches");

    let mut commands = CommandQueue::new();
    world.update_scripts(&mut commands, RunMode::Playing);

    let entries = world
        .get_component::<ScriptRunLog>(log_entity)
        .expect("the run log is on its entity")
        .entries
        .clone();
    assert_eq!(
        entries,
        ["alpha", "zulu"],
        "equal orders fall back to the type name"
    );
}

/// A script that declares no `on_update` is registered but left out of the
/// update pass.
#[test]
fn a_script_without_on_update_is_left_out_of_the_update_pass() {
    let mut world = World::new();
    world.register_script_component::<CountingScript>();
    world.register_script_component::<CreateOnlyScript>();

    let order = world.build_script_update_order();

    assert_eq!(
        order.len(),
        1,
        "only the type that implements on_update is walked"
    );
    assert!(
        order[0].ends_with("CountingScript"),
        "the walked type is the one that declared on_update, not {order:?}"
    );
}

// =============================================================================
// Shipping posture: the registration refusals
// =============================================================================

// A shipping build has no `development` feature, so the migration and
// re-homing code - and every API that could reshape a registered type's storage
// - is absent. The registration refusals below are what is left, and they run
// in both postures; `verify.py`'s `test-shipping-engine` step runs them in the
// build without `development`.

/// A copy of one shared resource type from another artifact: the two module
/// paths differ, so the declaring type's last segment is what matches.
mod shipping_schema_a {
    use crate::resource::Resource;

    /// The first copy's declared schema.
    pub(super) struct Settings;

    impl Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("shipping::Settings")
        }

        fn shared_schema_hash() -> Option<u64> {
            Some(1)
        }
    }
}

/// The same declared name and shape family, one schema generation later.
mod shipping_schema_b {
    use crate::resource::Resource;

    /// The second copy's declared schema.
    pub(super) struct Settings;

    impl Resource for Settings {
        fn shared_name() -> Option<&'static str> {
            Some("shipping::Settings")
        }

        fn shared_schema_hash() -> Option<u64> {
            Some(2)
        }
    }
}

/// A descriptor registered again with a changed layout is refused instead of
/// reshaping the live column.
#[test]
fn a_descriptor_registered_again_with_a_changed_layout_is_refused() {
    let mut world = World::new();
    world
        .register_component_descriptor(
            0x51,
            "shipping::Described",
            4,
            4,
            1,
            Blittability::engine_verified(),
        )
        .expect("the first layout registers");

    let error = world
        .register_component_descriptor(
            0x51,
            "shipping::Described",
            8,
            8,
            2,
            Blittability::engine_verified(),
        )
        .expect_err("the changed layout is refused");

    assert!(matches!(error, WorldError::DescriptorAlreadyRegistered));
}

/// A shared resource whose declared schema changed is refused at the claim, so
/// a stale artifact cannot bind to storage it does not understand.
#[test]
fn a_shared_resource_registered_again_with_a_changed_schema_is_refused() {
    let mut world = World::new();
    world.register_resource::<shipping_schema_a::Settings>();
    assert!(world.take_registration_error().is_none());

    world.register_resource::<shipping_schema_b::Settings>();

    let error = world
        .take_registration_error()
        .expect("the changed schema is refused");
    assert!(matches!(
        error,
        WorldError::SharedResourceSchemaMismatch { .. }
    ));
}

/// The reload passes that walk archetypes reach the held storage too.
///
/// A held storage is a second set of columns whose function tables point into
/// the image that created them, so a reload that visits only the live storage
/// would leave code calls into memory the reload has unmapped. This puts one
/// entity in each storage and checks that one name-keyed drop removes both
/// rows.
#[test]
fn a_reload_pass_that_walks_archetypes_reaches_the_held_storage() {
    // A component declared by hand carries no field layout, and a copy without
    // one is refused because nothing would say the rows hold no heap data. This
    // test is about the reload pass, not the refusal, so it declares the two
    // `f32` fields exactly as the derive would.
    static POSITION_FIELDS: &[crate::component_registry::ComponentFieldDescriptor] = &[
        crate::component_registry::ComponentFieldDescriptor {
            name: "x",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        crate::component_registry::ComponentFieldDescriptor {
            name: "y",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    let mut world = World::new();
    world.register_component::<Position>();
    world.record_component_field_layout(ComponentId::of::<Position>(), POSITION_FIELDS);
    let position_name = world
        .component_registry
        .registered_components()
        .find(|(component_id, _, _)| *component_id == ComponentId::of::<Position>())
        .map(|(_, _, name)| name.to_string())
        .expect("the component is registered under a name");
    world
        .create_entity()
        .with(Position { x: 1.0, y: 2.0 })
        .build()
        .expect("the first entity");

    // Copy the storage that entity lives in and hold the copy, exactly as a
    // play session does, then put a second entity into the live storage.
    let held_storage = world
        .copy_entity_storage()
        .expect("the storage copies: `Position` is plain data");
    world.hold_entity_storage(held_storage);
    world
        .create_entity()
        .with(Position { x: 3.0, y: 4.0 })
        .build()
        .expect("the second entity");
    assert_eq!(world.entity_count(), 2);
    assert_eq!(
        world
            .held_entity_storage
            .as_ref()
            .expect("the world holds a storage")
            .entity_count(),
        1
    );

    let dropped = world.drop_forgotten_components(&[position_name]);

    assert_eq!(dropped, 3, "every entity lost its row in its own storage");
    assert_eq!(world.entity_count(), 0, "the live storage kept its rows");
    assert_eq!(
        world
            .held_entity_storage
            .as_ref()
            .expect("the world still holds the storage")
            .entity_count(),
        0,
        "the held storage was never visited"
    );
}
