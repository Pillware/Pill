//! Tests for copying a world at Play: the entity storage and the game-state
//! resources.
//!
//! # Responsibilities
//!
//! - Show that a copy carries every column its own way - a `Clone` type row by
//!   row, a heap-free layout and a foreign (descriptor) column byte for byte -
//!   and that the copy is independent of the original.
//! - Show that a type the copy cannot include refuses the whole copy and is
//!   named, with every such type listed.
//! - Show that entity handles, the allocator's counters and the stable ids are
//!   identical in the copy.
//! - Show the resource copy: a `Clone` resource and a foreign payload are
//!   copied independently, a `Keep` or `Rebuild` resource is left out, and an
//!   unmarked resource the engine cannot copy is refused by name.
//!
//! # Design
//!
//! The tests live beside the world rather than in the facade because
//! `copy_entity_storage` and the `EntityStorage` it answers with are crate
//! private: a copy is a value the engine holds for a session, not something
//! game code builds. The session itself is covered from the outside, through
//! `Engine::enter_play` and `Engine::exit_play`.

// Standard library
use std::collections::HashMap;

// Current crate
use super::*;
use crate::component_registry::ComponentFieldDescriptor;
use crate::entity::EntityId;
use crate::resource::ErasedResource;
use crate::test_fixtures::Position;

// =============================================================================
// Fixtures
// =============================================================================

/// A component that owns heap data and implements `Clone`, so a copy has to
/// clone its rows rather than copy their bytes.
#[derive(Debug, Clone, PartialEq)]
struct Inventory {
    /// The item ids the entity carries.
    items: Vec<u32>,
}

impl Component for Inventory {}

/// A component that owns heap data and is not `Clone`, so no copy rule fits it.
///
/// The field exists to make the type own heap data; the tests never read it.
#[derive(Debug)]
#[allow(dead_code)]
struct Ledger {
    /// The ledger's entries.
    entries: Vec<u32>,
}

impl Component for Ledger {}

/// A second uncopyable component, so one refusal has to name two types.
#[derive(Debug)]
#[allow(dead_code)]
struct Journal {
    /// The journal's entries.
    lines: String,
}

impl Component for Journal {}

/// The declared layout of [`Position`]: two `f32` fields and no container.
const POSITION_FIELDS: &[ComponentFieldDescriptor] = &[
    ComponentFieldDescriptor {
        name: "x",
        type_tag: "f32",
        offset: 0,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
    ComponentFieldDescriptor {
        name: "y",
        type_tag: "f32",
        offset: 4,
        size: 4,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    },
];

/// A game-state resource the copy clones.
#[derive(Debug, Clone, PartialEq)]
struct SessionScore {
    /// The running total.
    points: u32,
}

impl crate::resource::Resource for SessionScore {
    // A hand-written resource answers the copy hook itself; the derive writes
    // this for a type it generates.
    fn clone_erased(
        value: &crate::resource::ErasedResource,
    ) -> Option<crate::resource::ErasedResource> {
        Some(crate::resource::ErasedResource::new(
            value.get::<Self>()?.clone(),
        ))
    }
}

/// An infrastructure resource, which a session leaves live.
///
/// The field exists so the value is a plain `Restore` candidate; the tests
/// never read it.
#[allow(dead_code)]
struct KeptRegistry {
    /// How many entries the registry holds.
    entries: u32,
}

impl crate::resource::Resource for KeptRegistry {
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

/// A manager that mirrors the world elsewhere, which a session leaves live and
/// rebuilds afterwards.
#[allow(dead_code)]
struct RebuiltMirror {
    /// How many bodies the mirror holds.
    bodies: u32,
}

impl crate::resource::Resource for RebuiltMirror {
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Rebuild
    }
}

/// A resource that owns heap data, is not `Clone` and carries no play-state
/// marker.
struct UncopyableResource {
    /// The reason it cannot be copied: its own heap buffer.
    #[allow(dead_code)]
    buffer: Vec<u8>,
}

impl crate::resource::Resource for UncopyableResource {}

/// A resource that is not `Clone` but declares persistence, so the copy can
/// carry it as its persisted bytes.
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct PersistedLedger {
    /// The running total.
    points: u32,
    /// The entries the session started with.
    history: Vec<u32>,
}

impl crate::resource::Resource for PersistedLedger {}

/// The rows a storage holds for one component, read as `T`.
fn rows_of<T: Component + Clone>(storage: &EntityStorage, component_id: ComponentId) -> Vec<T> {
    let mut rows = Vec::new();
    for archetype in storage.archetypes.values() {
        let Some(column) = archetype.component_storages.get(component_id) else {
            continue;
        };
        rows.extend(column.rows::<T>().iter().cloned());
    }
    rows
}

// =============================================================================
// The entity storage
// =============================================================================

/// A copy carries a `Clone` component, a heap-free layout component and a
/// descriptor row, and changing either side leaves the other alone.
#[test]
fn a_copy_duplicates_every_column_by_the_rule_that_fits_it() {
    let mut world = World::new();
    world.register_component::<Inventory>();
    world.record_component_clone::<Inventory>();
    world.register_component::<Position>();
    world.record_component_clone::<Position>();
    let position_id = ComponentId::of::<Position>();
    world.record_component_field_layout(position_id, POSITION_FIELDS);
    let descriptor_id = world
        .register_component_descriptor(
            0xD1,
            "Project.Foreign",
            8,
            4,
            41,
            Blittability::engine_verified(),
        )
        .expect("the descriptor registers");
    let inventory_id = ComponentId::of::<Inventory>();

    let entity = world
        .create_entity()
        .with(Inventory {
            items: vec![1, 2, 3],
        })
        .with(Position { x: 1.5, y: -2.5 })
        .build()
        .expect("the entity builds");
    let mut descriptor_row = Vec::new();
    descriptor_row.extend_from_slice(&7u32.to_ne_bytes());
    descriptor_row.extend_from_slice(&9u32.to_ne_bytes());
    world
        .add_descriptor_component(entity, descriptor_id, &descriptor_row)
        .expect("the descriptor row is added");

    let copy = world.copy_entity_storage().unwrap_or_else(|refusal| {
        panic!(
            "every column follows a copy rule, but these could not: {:?}",
            refusal.type_names
        )
    });

    // The `Clone` column is cloned, so both sides own their own buffer.
    assert_eq!(
        rows_of::<Inventory>(&copy, inventory_id),
        vec![Inventory {
            items: vec![1, 2, 3]
        }]
    );
    // The heap-free layout column and the descriptor column are copied as
    // bytes, and both still read back as the values they held.
    assert_eq!(
        rows_of::<Position>(&copy, position_id),
        vec![Position { x: 1.5, y: -2.5 }]
    );
    let mut copied_descriptor_rows = Vec::new();
    for archetype in copy.archetypes.values() {
        let Some(column) = archetype.component_storages.get(descriptor_id) else {
            continue;
        };
        for index in 0..column.len() {
            copied_descriptor_rows.push(column.bytes(index).expect("the row exists").to_vec());
        }
    }
    assert_eq!(copied_descriptor_rows, vec![descriptor_row.clone()]);

    // The original is untouched, and changing it does not reach the copy.
    world
        .get_component_mut::<Inventory>(entity)
        .expect("the row exists")
        .items
        .push(4);
    assert_eq!(
        rows_of::<Inventory>(&copy, inventory_id),
        vec![Inventory {
            items: vec![1, 2, 3]
        }]
    );
    // Both values are dropped at the end of the test; a byte copy of a type
    // that owned heap data would free the same buffer twice here.
}

/// Every type the copy cannot include is named, not just the first.
#[test]
fn uncopyable_components_are_all_named_in_one_refusal() {
    let mut world = World::new();
    world.register_component::<Ledger>();
    world.register_component::<Journal>();
    world
        .create_entity()
        .with(Ledger {
            entries: vec![1, 2],
        })
        .with(Journal {
            lines: String::from("first"),
        })
        .build()
        .expect("the entity builds");

    let refusal = world
        .copy_entity_storage()
        .err()
        .expect("neither type can be copied");
    let mut names = refusal.type_names;
    names.sort();
    let mut expected = vec![
        crate::type_names::stable_type_name::<Ledger>().to_string(),
        crate::type_names::stable_type_name::<Journal>().to_string(),
    ];
    expected.sort();
    assert_eq!(names, expected);
}

/// Handles, the allocator's counters and the stable ids are identical in the
/// copy, so the two worlds describe the same entities.
#[test]
fn a_copy_keeps_the_handles_the_allocator_and_the_stable_ids() {
    let mut world = World::new();
    world.register_component::<Position>();
    world.record_component_clone::<Position>();
    let first = world
        .create_entity()
        .with(Position { x: 1.0, y: 1.0 })
        .build()
        .expect("the first entity builds");
    let second = world
        .create_entity()
        .with(Position { x: 2.0, y: 2.0 })
        .build()
        .expect("the second entity builds");
    // A destroyed entity leaves a free id behind, which the copy has to carry
    // as well.
    assert!(world.destroy_entity(first), "the first entity dies");
    let stable_ids_before: HashMap<Entity, EntityId> = world
        .storage
        .entity_ids
        .iter()
        .map(|(entity, stable_id)| (*entity, *stable_id))
        .collect();

    let copy = world
        .copy_entity_storage()
        .expect("the copy takes a clone type");

    let stable_ids_after: HashMap<Entity, EntityId> = copy
        .entity_ids
        .iter()
        .map(|(entity, stable_id)| (*entity, *stable_id))
        .collect();
    assert_eq!(stable_ids_after, stable_ids_before);
    assert!(stable_ids_after.contains_key(&second));
    assert_eq!(
        copy.next_fresh_entity_id,
        world.storage.next_fresh_entity_id
    );
    assert_eq!(copy.free_entity_ids, world.storage.free_entity_ids);
    assert_eq!(
        copy.entity_locations.len(),
        world.storage.entity_locations.len(),
        "the copy locates every entity the original does"
    );
    assert!(
        copy.entity_locations.contains_key(&second),
        "the surviving entity keeps its row in the copy"
    );
    assert_eq!(
        copy.archetype_generation,
        world.storage.archetype_generation
    );
}

// =============================================================================
// The game-state resources
// =============================================================================

/// A `Clone` resource and a foreign payload are copied, and changing the live
/// value leaves the copy alone.
#[test]
fn the_resource_copy_clones_a_value_and_copies_a_foreign_payload() {
    let mut world = World::new();
    world.insert_resource(SessionScore { points: 3 });
    let foreign_id = world
        .register_foreign_resource(
            "project::SharedSettings",
            "Project.SharedSettings",
            8,
            4,
            91,
        )
        .expect("the foreign resource registers");
    let mut payload = Vec::new();
    payload.extend_from_slice(&11u32.to_ne_bytes());
    payload.extend_from_slice(&13u32.to_ne_bytes());
    world
        .insert_foreign_resource_bytes(foreign_id, &payload)
        .expect("the payload is stored");

    let copy = world
        .copy_game_state_resources()
        .expect("both values can be copied");

    // Both are in the copy, and the cloned value is its own value.
    let score_id = crate::resource::ResourceId::of::<SessionScore>();
    let copied_score = copy
        .values
        .get(&score_id)
        .and_then(|value| value.get::<SessionScore>())
        .expect("the score is copied");
    assert_eq!(copied_score.points, 3);
    assert_eq!(
        copy.values
            .get(&foreign_id)
            .map(|value| value.bytes().to_vec()),
        Some(payload.clone())
    );

    // A change game code makes while the session runs stays live only.
    world
        .get_resource_mut::<SessionScore>()
        .expect("the score is live")
        .points = 99;
    assert_eq!(
        copy.values
            .get(&score_id)
            .and_then(|value| value.get::<SessionScore>())
            .expect("the score is copied")
            .points,
        3
    );
}

/// A `Keep` and a `Rebuild` resource are left out of the copy, so Stop cannot
/// put a stale value back over them.
#[test]
fn kept_and_rebuilt_resources_are_left_out_of_the_copy() {
    let mut world = World::new();
    world.insert_resource(KeptRegistry { entries: 5 });
    world.insert_resource(RebuiltMirror { bodies: 7 });

    let copy = world
        .copy_game_state_resources()
        .expect("nothing has to be copied");

    assert!(copy.values.is_empty());
}

/// A resource that is neither `Clone` nor foreign and carries no marker refuses
/// the copy by name.
#[test]
fn an_unmarked_uncopyable_resource_is_refused_by_name() {
    let mut world = World::new();
    world.insert_resource(UncopyableResource {
        buffer: vec![1, 2, 3],
    });

    let refusal = world
        .copy_game_state_resources()
        .err()
        .expect("the resource cannot be copied");

    assert_eq!(refusal.type_names.len(), 1);
    assert!(
        refusal.type_names[0].ends_with("UncopyableResource"),
        "the refusal names the type: {:?}",
        refusal.type_names
    );
}

/// A reload that drops a `Restore` resource's live value drops its held copy
/// too, and Stop then keeps the value the game holds rather than removing it.
#[test]
fn a_reload_that_drops_a_copy_leaves_the_live_value_alone() {
    let mut world = World::new();
    world.insert_resource(SessionScore { points: 3 });
    let copy = world
        .copy_game_state_resources()
        .expect("the score can be copied");
    world.hold_resource_copy(copy);

    // The reload retires the type: the live value and its copy are released
    // while the image that owns them is still mapped.
    let dropped = world.drop_resources(&[crate::resource::ResourceId::of::<SessionScore>()]);
    assert_eq!(
        dropped, 2,
        "the live value and its held copy are both released"
    );

    // The game stores a value of the type again while the session runs. Stop has
    // no copy for it, and the type is one a reload dropped, so the value stays
    // rather than being removed as a play-only insertion.
    world.insert_resource(SessionScore { points: 42 });
    assert!(world.restore_held_resources());

    assert_eq!(
        world
            .get_resource::<SessionScore>()
            .map(|score| score.points),
        Some(42),
        "Stop keeps the value the game holds when the copy is gone"
    );
}

/// A persistable resource that is not `Clone` is copied as its own bytes and
/// comes back through the type's deserializer at Stop.
#[test]
fn a_persistable_resource_is_copied_as_its_bytes() {
    let mut world = World::new();
    world.register_persistable_resource::<PersistedLedger>();
    world.insert_resource(PersistedLedger {
        points: 3,
        history: vec![1, 2],
    });

    let copy = world
        .copy_game_state_resources()
        .expect("a persistable resource can be copied");

    assert!(
        copy.values.is_empty(),
        "the value is copied as bytes, not as an erased value"
    );
    assert!(
        copy.persisted_values
            .contains_key(&crate::resource::ResourceId::of::<PersistedLedger>()),
        "the copy holds the resource's persisted bytes"
    );

    // The game changes it while the session runs; Stop puts the edit value back
    // through the same deserializer a snapshot load uses.
    world
        .get_resource_mut::<PersistedLedger>()
        .expect("the value is live")
        .points = 99;
    world.hold_resource_copy(copy);
    assert!(world.restore_held_resources(), "a copy was held");

    assert_eq!(
        world.get_resource::<PersistedLedger>(),
        Some(&PersistedLedger {
            points: 3,
            history: vec![1, 2],
        }),
        "Stop restores the edit-world value"
    );
}

/// Putting a copy back replaces the live value, which is what Stop does.
#[test]
fn putting_a_resource_copy_back_replaces_the_live_value() {
    let mut world = World::new();
    world.insert_resource(SessionScore { points: 3 });
    let copy = world
        .copy_game_state_resources()
        .expect("the score can be copied");
    world
        .get_resource_mut::<SessionScore>()
        .expect("the score is live")
        .points = 99;

    let score_id = crate::resource::ResourceId::of::<SessionScore>();
    let restored: ErasedResource = copy
        .values
        .get(&score_id)
        .map(|value| value.get::<SessionScore>().expect("the copy is a score"))
        .map(|value| ErasedResource::new(value.clone()))
        .expect("the score is in the copy");
    world.put_back_resource(score_id, restored);

    assert_eq!(
        world
            .get_resource::<SessionScore>()
            .expect("the score is live")
            .points,
        3
    );
}
