//! Component and resource persistence across a hot reload: what each
//! persistable type registers, and the migration a reload runs on it.
//!
//! # Responsibilities
//!
//! - Stores the serialize, deserialize and insert functions of every
//!   persistable component type, registered alongside the component itself.
//! - Migrates, in place, only the component types whose schema a reload
//!   changed. Old and new components are matched by type name, not by
//!   `TypeId`, because a rebuilt image gives every type a fresh `TypeId` and
//!   the name is the only thing two generations agree on.
//! - Owns the resource snapshot, its restore path and the resource half of
//!   the reload migration.
//! - Keeps the reload bookkeeping the host compares across a swap: the
//!   component and resource manifests, the registration counters, and the
//!   records of registrations whose module is gone.
//!
//! # Design
//!
//! Before the swap, the reload captures the metadata of every persistable
//! type. After the arriving generation registers its types, only the columns
//! whose schema hash changed are migrated. Every other column keeps its
//! bytes, so a reload costs work in proportion to what changed, not to the
//! size of the world.
//!
//! The payload is serde_json rather than a binary format because JSON is
//! self-describing: field names travel with the values, so an added field
//! takes `Default::default()` and a removed one is ignored.
//!
//! Each persistable component type registers three monomorphized functions,
//! stored in the [`World`] as plain `fn` pointers:
//!
//! | Function | Signature | Purpose |
//! |----------|-----------|---------|
//! | `serialize_component` | `fn(&ComponentColumns, index) → Option<Vec<u8>>` | Read the component from its archetype column and encode it as JSON; `None` when the type's own `Serialize` refuses the value |
//! | `deserialize_component` | `fn(&[u8]) → Option<Box<dyn Component>>` | Decode JSON bytes back into a component; `None` when the bytes cannot be decoded into the type |
//! | `insert_boxed_component` | `fn(&mut ComponentColumns, Box<dyn Component>)` | Downcast the box and push the value into the archetype column |
//!
//! They are monomorphized in the DLL that defines the concrete types, so a
//! reload replaces them with a plain `HashMap::insert` of the new function.
//! No destructor and no vtable call then runs through an image that is about
//! to be unmapped.
//!
//! During migration, each old row of a changed column is read through the
//! *retiring* generation's `serialize_component`, decoded with the arriving
//! generation's `deserialize_component`, and placed with its
//! `insert_boxed_component`. Rows the arriving generation spawned during its
//! `init` already hold values in the new layout, so they are read with the
//! current serializer instead.

// Standard library
use std::collections::hash_map::DefaultHasher;
#[cfg(feature = "development")]
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

// External crates
#[cfg(feature = "development")]
use pill_core::debug;
use pill_core::{error, info, warn};
use serde::{de::DeserializeOwned, Serialize};

// Current crate
use crate::archetype::ComponentColumns;
use crate::component::{Component, ComponentId};
use crate::entity::Entity;
#[cfg(feature = "development")]
use crate::error::PersistenceError;
use crate::error::{
    WorldError, COMPONENT_LAYOUT_CHANGED_REFUSAL, COMPONENT_NAME_COLLISION_REFUSAL,
};
use crate::resource::{ErasedResource, Resource, ResourceId};
use crate::world::World;

// =============================================================================
// Module Tree
// =============================================================================

mod components;
#[cfg(feature = "development")]
mod migration;
mod registry;
mod resources;

pub use components::*;
pub use registry::*;
pub use resources::*;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archetype::Blittability;

    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct DropTestForgottenComponent {
        value: u32,
    }
    impl Component for DropTestForgottenComponent {}

    #[derive(Clone, Debug)]
    struct DropTestKeptComponent {
        value: u32,
    }
    impl Component for DropTestKeptComponent {}

    /// A separate type that declares another type's name, which is what a
    /// rebuilt image registers: the same name under a fresh `TypeId`.
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct DropTestSupersedingComponent {
        value: u32,
    }
    impl Component for DropTestSupersedingComponent {
        fn shared_name() -> Option<&'static str> {
            Some(crate::type_names::stable_type_name::<
                DropTestForgottenComponent,
            >())
        }
    }

    /// A persistable component whose column takes 8 bytes per row and is
    /// 4-byte aligned.
    #[cfg(feature = "development")]
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct LayoutHostComponent {
        a: u32,
        b: u32,
    }
    #[cfg(feature = "development")]
    impl Component for LayoutHostComponent {}

    /// Declares the host's name while widening `b` to `f64`. This is the
    /// widening shape that registers 16 bytes per row with 8-byte alignment
    /// over the host's column.
    #[cfg(feature = "development")]
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct LayoutWidenedComponent {
        a: u32,
        b: f64,
    }
    #[cfg(feature = "development")]
    impl Component for LayoutWidenedComponent {
        fn shared_name() -> Option<&'static str> {
            Some(crate::type_names::stable_type_name::<LayoutHostComponent>())
        }
    }

    /// Same name and same alignment, one field more: the add-a-field shape
    /// that must keep registering and keep migrating.
    #[cfg(feature = "development")]
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct LayoutGrownComponent {
        a: u32,
        b: u32,
        c: u32,
    }
    #[cfg(feature = "development")]
    impl Component for LayoutGrownComponent {
        fn shared_name() -> Option<&'static str> {
            Some(crate::type_names::stable_type_name::<LayoutHostComponent>())
        }
    }

    /// The refusal sentence reaches host logs verbatim and people search the
    /// logs for it, so it must carry no line break or double space left by
    /// source wrapping.
    #[test]
    fn collision_refusal_message_has_no_wrap_spaces() {
        assert!(
            !COMPONENT_NAME_COLLISION_REFUSAL.contains("  "),
            "{COMPONENT_NAME_COLLISION_REFUSAL}"
        );
        assert!(!COMPONENT_NAME_COLLISION_REFUSAL.contains('\n'));
        assert!(COMPONENT_NAME_COLLISION_REFUSAL.ends_with("persist entries"));
    }

    /// A superseding registration whose layout the old column cannot hold is
    /// refused before anything registers. The host then rolls the reload back,
    /// so the running generation keeps its storage instead of the migration
    /// reading the new alignment out of the old column's slots.
    #[cfg(feature = "development")]
    #[test]
    fn a_layout_the_old_column_cannot_host_is_refused() {
        let mut world = World::new();
        world.register_persistable_component::<LayoutHostComponent>();
        let host_id = ComponentId::of::<LayoutHostComponent>();
        let type_name = crate::type_names::stable_type_name::<LayoutHostComponent>().to_string();
        let entity = world
            .create_entity()
            .with(LayoutHostComponent { a: 1, b: 2 })
            .build()
            .unwrap();
        let _ = world.take_registration_error();

        // The rebuilt image's registration: same name, wider alignment.
        world.supersede_persist_registrations(std::slice::from_ref(&type_name));
        world.register_persistable_component::<LayoutWidenedComponent>();

        let error = world
            .take_registration_error()
            .expect("a layout the old column cannot host must be refused");
        let message = error.to_string();
        assert!(
            message.contains("was re-registered with a different size"),
            "the refusal keeps the sentence the migration suite greps for: {message}"
        );
        match error {
            WorldError::ComponentLayoutChanged {
                existing_size,
                existing_align,
                incoming_size,
                incoming_align,
                ..
            } => {
                assert_eq!((existing_size, existing_align), (8, 4));
                assert_eq!((incoming_size, incoming_align), (16, 8));
            }
            other => panic!("expected a ComponentLayoutChanged refusal, got {other:?}"),
        }

        // Nothing new was registered, and the predecessor kept its
        // registration.
        assert_eq!(
            world.component_registry().registered_components().count(),
            1,
            "a refused registration leaves the registry as it was"
        );
        assert_eq!(world.live_row_count(host_id), 1);
        assert!(world.storage.entity_locations.contains_key(&entity));
        assert!(!world
            .storage_factories
            .contains_key(&ComponentId::of::<LayoutWidenedComponent>()));
    }

    /// A size change that keeps the alignment registers, because the old
    /// column's slots stay correctly aligned for the incoming type. The
    /// add-a-field reload cases depend on this.
    #[cfg(feature = "development")]
    #[test]
    fn a_size_change_that_keeps_the_alignment_registers() {
        let mut world = World::new();
        world.register_persistable_component::<LayoutHostComponent>();
        let type_name = crate::type_names::stable_type_name::<LayoutHostComponent>().to_string();
        let _ = world.take_registration_error();

        world.supersede_persist_registrations(std::slice::from_ref(&type_name));
        world.register_persistable_component::<LayoutGrownComponent>();

        assert!(
            world.take_registration_error().is_none(),
            "a wider field of the same alignment must register"
        );
        let grown_layout = world
            .component_registry()
            .get_layout(&ComponentId::of::<LayoutGrownComponent>())
            .expect("the successor is registered");
        assert_eq!((grown_layout.size, grown_layout.align), (12, 4));
    }

    /// Dropping a forgotten type removes its data from every entity. Entities
    /// that also carry other components survive with those intact; entities
    /// that carried only the forgotten type are destroyed.
    #[test]
    fn drop_forgotten_components_removes_only_the_forgotten_data() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        world.register_component::<DropTestKeptComponent>();

        let forgotten_id = ComponentId::of::<DropTestForgottenComponent>();
        let kept_id = ComponentId::of::<DropTestKeptComponent>();

        let entity_only_forgotten = world
            .create_entity()
            .with(DropTestForgottenComponent { value: 1 })
            .build()
            .unwrap();
        let entity_with_both = world
            .create_entity()
            .with(DropTestForgottenComponent { value: 2 })
            .with(DropTestKeptComponent { value: 3 })
            .build()
            .unwrap();

        // The data is readable before the drop.
        assert_eq!(
            world
                .get_component::<DropTestForgottenComponent>(entity_with_both)
                .unwrap()
                .value,
            2
        );

        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();
        let dropped_entity_count =
            world.drop_forgotten_components(std::slice::from_ref(&type_name));
        assert_eq!(
            dropped_entity_count, 2,
            "both entities carried the forgotten component"
        );

        // The entity that carried only the forgotten component is destroyed.
        // The entity that also carries a kept component survives.
        assert!(!world
            .storage
            .entity_locations
            .contains_key(&entity_only_forgotten));
        assert!(world
            .storage
            .entity_locations
            .contains_key(&entity_with_both));
        assert_eq!(world.storage.entity_locations.len(), 1);

        // The surviving entity's archetype keeps the kept component and no
        // longer contains the forgotten one.
        let location = world.storage.entity_locations[&entity_with_both];
        let archetype = &world.storage.archetypes[&location.archetype_id];
        assert!(archetype.component_types.contains(&kept_id));
        assert!(!archetype.component_types.contains(&forgotten_id));

        // The kept component's value survives untouched.
        let kept_component = world
            .get_component::<DropTestKeptComponent>(entity_with_both)
            .expect("kept component should still be readable");
        assert_eq!(kept_component.value, 3);

        // The persistable manifest no longer knows the forgotten type.
        assert!(world
            .persist_type_manifest()
            .iter()
            .all(|entry| entry.type_name != type_name));

        // Re-registering the forgotten type works and fresh data can be seeded.
        world.register_persistable_component::<DropTestForgottenComponent>();
        let reseeded = world
            .create_entity()
            .with(DropTestForgottenComponent { value: 9 })
            .build()
            .unwrap();
        assert!(world.storage.entity_locations.contains_key(&reseeded));
    }

    /// Every native id that shares the forgotten name loses its rows, not only
    /// the one a name resolver would return. An ambiguous name resolves to
    /// nothing, so a purge driven by one id would leave the other generation's
    /// rows behind while `forget_component_type` removed all their
    /// registrations.
    #[cfg(feature = "development")]
    #[test]
    fn forget_strips_rows_for_every_id_sharing_the_name() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let native_id = ComponentId::of::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();

        let native_entity = world
            .create_entity()
            .with(DropTestForgottenComponent { value: 1 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(native_id), 1);

        // A rebuilt image's registration: same name, fresh id, its own rows.
        world.supersede_persist_registrations(std::slice::from_ref(&type_name));
        world.register_persistable_component::<DropTestSupersedingComponent>();
        let successor_id = ComponentId::of::<DropTestSupersedingComponent>();
        assert_ne!(successor_id, native_id);
        let successor_entity = world
            .create_entity()
            .with(DropTestSupersedingComponent { value: 2 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(successor_id), 1);

        let dropped_entity_count =
            world.drop_forgotten_components(std::slice::from_ref(&type_name));
        assert_eq!(
            dropped_entity_count, 2,
            "both generations' rows are stripped, not just one id's"
        );
        assert!(!world.storage.entity_locations.contains_key(&native_entity));
        assert!(!world
            .storage
            .entity_locations
            .contains_key(&successor_entity));
        assert!(!world.storage_factories.contains_key(&native_id));
        assert!(!world.storage_factories.contains_key(&successor_id));
    }

    /// A refused persistable registration changes nothing. The guard runs
    /// before the registration does, so a call that returns without
    /// registering creates no bit, no name entry, no storage factory and no
    /// registration-log entry.
    #[test]
    fn a_refused_persistable_registration_changes_nothing() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        // A peer holds live rows under the same name. Without an announcement
        // that this name supersedes the peer, the guard refuses the
        // registration rather than evicting the peer.
        world.register_persistable_component::<DropTestSupersedingComponent>();
        let peer_id = ComponentId::of::<DropTestSupersedingComponent>();
        let peer_entity = world
            .create_entity()
            .with(DropTestSupersedingComponent { value: 1 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(peer_id), 1);
        let _ = world.take_registration_error();

        let registry_count = world.component_registry().registered_components().count();
        for attempt in 0..3 {
            world.register_persistable_component::<DropTestForgottenComponent>();
            let error = world
                .take_registration_error()
                .unwrap_or_else(|| panic!("attempt {attempt} must be refused"));
            assert!(matches!(error, WorldError::ComponentNameCollision { .. }));
            assert_eq!(
                world.component_registry().registered_components().count(),
                registry_count,
                "a refused attempt leaves the registry as it was"
            );
        }
        assert!(world.storage.entity_locations.contains_key(&peer_entity));
        assert_eq!(
            world.live_row_count(peer_id),
            1,
            "the peer keeps its rows across every refused attempt"
        );
    }

    /// The id-keyed purge removes the rows and all registration entries of the
    /// ids it is given. A rollback generation that re-registers the same name
    /// under a fresh id keeps the name-keyed entries alive.
    #[cfg(feature = "development")]
    #[test]
    fn stranded_ids_lose_their_rows_and_factories() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let stranded_id = ComponentId::of::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();

        let entity = world
            .create_entity()
            .with(DropTestForgottenComponent { value: 4 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(stranded_id), 1);

        // The rollback generation re-registers the same name under a fresh id,
        // exactly as a rebuilt image does.
        world.supersede_persist_registrations(std::slice::from_ref(&type_name));
        world.register_persistable_component::<DropTestSupersedingComponent>();
        let successor_id = ComponentId::of::<DropTestSupersedingComponent>();
        assert_ne!(successor_id, stranded_id);

        let dropped_entity_count = world.retire_component_storage(&[stranded_id]);
        assert_eq!(
            dropped_entity_count, 1,
            "the stranded type's row was removed with it"
        );

        // The failed generation's id holds nothing anywhere...
        assert!(!world.storage.entity_locations.contains_key(&entity));
        assert!(!world.storage_factories.contains_key(&stranded_id));
        assert!(!world.persist_serializers.contains_key(&stranded_id));
        assert!(!world.persist_inserters.contains_key(&stranded_id));

        // ...while the rollback generation's own entries survive, including
        // the name-keyed ones the two registrations share.
        assert!(world.persist_serializers.contains_key(&successor_id));
        assert!(world.persist_inserters.contains_key(&successor_id));
        assert!(world.persist_deserializers.contains_key(&type_name));
        assert!(world.persist_schema_hashes.contains_key(&type_name));
    }

    /// The two registration logs differ: the component log lists every type
    /// registered since a point, the persistable log only the persistable
    /// ones. That difference is what tells a dropped type apart from one
    /// downgraded to a plain component.
    #[test]
    fn registered_component_names_since_covers_plain_components() {
        let mut world = World::new();
        let registration_sequence = world.component_registration_sequence();

        world.register_persistable_component::<DropTestForgottenComponent>();
        world.register_component::<DropTestKeptComponent>();

        let registered_component_names =
            world.registered_component_names_since(registration_sequence);
        assert!(registered_component_names
            .iter()
            .any(|name| name.contains("DropTestForgottenComponent")));
        assert!(registered_component_names
            .iter()
            .any(|name| name.contains("DropTestKeptComponent")));

        let persistable_type_names =
            world.persist_type_names_registered_since(registration_sequence);
        assert!(persistable_type_names
            .iter()
            .any(|name| name.contains("DropTestForgottenComponent")));
        assert!(!persistable_type_names
            .iter()
            .any(|name| name.contains("DropTestKeptComponent")));
    }

    /// Re-registering the same persistable type changes nothing in the persist
    /// maps: there is still one serializer and one inserter for its
    /// `ComponentId`, and one deserializer and one schema hash for its name.
    #[test]
    fn persistable_registration_is_idempotent_across_repeated_calls() {
        let mut world = World::new();
        let component_id = ComponentId::of::<DropTestForgottenComponent>();
        world.register_persistable_component::<DropTestForgottenComponent>();
        world.register_persistable_component::<DropTestForgottenComponent>();

        assert_eq!(world.persist_serializers.len(), 1);
        assert!(world.persist_serializers.contains_key(&component_id));
        assert_eq!(world.persist_inserters.len(), 1);
        assert!(world.persist_inserters.contains_key(&component_id));

        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();
        assert_eq!(world.persist_deserializers.len(), 1);
        assert!(world.persist_deserializers.contains_key(&type_name));
        assert_eq!(world.persist_schema_hashes.len(), 1);
        assert!(world.persist_schema_hashes.contains_key(&type_name));
    }

    // =============================================================================
    // Concurrent-peer guard
    // =============================================================================

    /// A second registration of a type name whose column still holds rows
    /// belongs to a concurrent peer, not to a superseded generation. It is
    /// reported as a collision instead of replacing the peer's persist entries
    /// and silently dropping its rows at the next reload.
    #[test]
    fn a_same_name_registration_over_live_rows_is_reported_not_evicted() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let native_id = ComponentId::of::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();

        // Give the native column a live row, which is what makes the second
        // registration a peer rather than a dead generation.
        world
            .create_entity()
            .with(DropTestForgottenComponent { value: 7 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(native_id), 1);

        // A second component claims the same name. This stands in for a second
        // binary in the same process that linked the same type.
        let error = world
            .register_component_descriptor(
                0x5EED,
                type_name.clone(),
                4,
                4,
                0,
                Blittability::engine_verified(),
            )
            .unwrap_err();

        match error {
            WorldError::ComponentNameCollision {
                type_name: reported_name,
                existing_id,
                live_rows,
                ..
            } => {
                assert_eq!(reported_name, type_name);
                assert_eq!(existing_id, native_id);
                assert_eq!(live_rows, 1);
            }
            other => panic!("expected a name collision, got {other:?}"),
        }

        // The peer's persist entries are untouched, so its rows still restore.
        assert!(world.persist_inserters.contains_key(&native_id));
        assert!(world.persist_serializers.contains_key(&native_id));
    }

    /// The same collision is allowed through once the existing column holds no
    /// rows. That is a superseded generation, and replacing it is the
    /// behaviour hot reload depends on.
    #[test]
    fn a_same_name_registration_over_an_empty_column_still_succeeds() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();

        // No entity is created, so the native column holds nothing.
        assert_eq!(
            world.live_row_count(ComponentId::of::<DropTestForgottenComponent>()),
            0
        );

        let result = world.register_component_descriptor(
            0x5EED,
            type_name,
            4,
            4,
            0,
            Blittability::engine_verified(),
        );
        assert!(
            result.is_ok(),
            "an empty same-name column is a dead generation, not a peer: {result:?}"
        );
    }

    /// The same collision shape as the peer test above (same name, different
    /// id, live rows), but here the host announces the name as the retiring
    /// generation's. That registration is the predecessor, and replacing its
    /// entries is the path every project reload runs through.
    #[cfg(feature = "development")]
    #[test]
    fn an_announced_name_replaces_its_live_predecessor() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let predecessor_id = ComponentId::of::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();
        world
            .create_entity()
            .with(DropTestForgottenComponent { value: 7 })
            .build()
            .unwrap();
        assert_eq!(world.live_row_count(predecessor_id), 1);

        let successor_id = ComponentId::of::<DropTestSupersedingComponent>();
        world.supersede_persist_registrations(&[type_name]);
        world.register_persistable_component::<DropTestSupersedingComponent>();

        // Replaced, not refused: the successor's entries are the live ones now.
        assert!(world.take_registration_error().is_none());
        assert!(!world.persist_serializers.contains_key(&predecessor_id));
        assert!(world.persist_serializers.contains_key(&successor_id));
        assert!(world.persist_inserters.contains_key(&successor_id));
        // The predecessor's rows are untouched. Moving them to the successor
        // is the reload's job, not the registry's.
        assert_eq!(world.live_row_count(predecessor_id), 1);

        // That one registration used up the announcement. The successor now
        // holds a row of its own, so the same arrival that the announcement
        // allowed a moment ago is refused again.
        world
            .create_entity()
            .with(DropTestSupersedingComponent { value: 1 })
            .build()
            .unwrap();
        world.register_persistable_component::<DropTestForgottenComponent>();
        assert!(matches!(
            world.take_registration_error(),
            Some(WorldError::ComponentNameCollision { .. })
        ));
    }

    // =============================================================================
    // Retiring destructors across an in-place migration
    // =============================================================================

    /// How many rows each stand-in's destructor has dropped. A migration test
    /// reads these counters to tell which generation released the old values.
    #[cfg(feature = "development")]
    static RETIRING_GLUE_DROP_COUNT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);
    #[cfg(feature = "development")]
    static ARRIVING_GLUE_DROP_COUNT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    /// The retiring generation of a shared persistable type.
    #[cfg(feature = "development")]
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct RetiringGlueComponent {
        // The field gives the type the shape its serde row and the size check
        // see. The drop counter never reads it.
        #[allow(dead_code)]
        value: u32,
    }
    #[cfg(feature = "development")]
    impl Component for RetiringGlueComponent {
        fn shared_name() -> Option<&'static str> {
            Some("audit::DropGlue")
        }
    }
    #[cfg(feature = "development")]
    impl Drop for RetiringGlueComponent {
        fn drop(&mut self) {
            RETIRING_GLUE_DROP_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The arriving generation: the same declared name and shape, but a
    /// different destructor.
    #[cfg(feature = "development")]
    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct ArrivingGlueComponent {
        // Same shape as the retiring copy. Nothing reads this value either.
        #[allow(dead_code)]
        value: u32,
    }
    #[cfg(feature = "development")]
    impl Component for ArrivingGlueComponent {
        fn shared_name() -> Option<&'static str> {
            Some("audit::DropGlue")
        }
    }
    #[cfg(feature = "development")]
    impl Drop for ArrivingGlueComponent {
        fn drop(&mut self) {
            ARRIVING_GLUE_DROP_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// The in-place migration drops old-layout rows through the destructor
    /// that allocated them. The dangerous order is: the arriving generation's
    /// registration replaces the factory, `rehome_native_columns` points the
    /// still-old column at the arriving function table, and only then does the
    /// migration consume that column.
    #[cfg(feature = "development")]
    #[test]
    fn in_place_migration_drops_old_rows_through_the_retiring_glue() {
        use std::sync::atomic::Ordering;

        let mut world = World::new();
        world.register_persistable_component::<RetiringGlueComponent>();
        let component_id = ComponentId::of::<RetiringGlueComponent>();
        let type_name =
            crate::component::ComponentRegistry::registered_name::<RetiringGlueComponent>()
                .to_string();

        let entity = world
            .create_entity()
            .with(RetiringGlueComponent { value: 3 })
            .build()
            .unwrap();
        assert!(world.storage.entity_locations.contains_key(&entity));
        world.rehome_native_columns();

        // Capture the retiring generation's serializer while it is still the
        // registered one, then stand in for the rebuilt image's registration.
        let serialize_retiring_component = world.persist_serializers[&component_id];
        world.register_persistable_component::<ArrivingGlueComponent>();
        let serialize_current_component = world.persist_serializers[&component_id];
        let deserialize_current_component = world.persist_deserializers[&type_name];
        let insert_current_component = world.persist_inserters[&component_id];

        // The re-home the reload runs before migration points the old column
        // at the arriving generation's function table. The migration must
        // still release the old rows through the retiring destructor.
        world.rehome_native_columns();

        let retiring_glue_drop_count_before = RETIRING_GLUE_DROP_COUNT.load(Ordering::SeqCst);
        let arriving_glue_drop_count_before = ARRIVING_GLUE_DROP_COUNT.load(Ordering::SeqCst);
        let migrated_row_count = world
            .migrate_component_column_in_place(
                component_id,
                serialize_retiring_component,
                serialize_current_component,
                deserialize_current_component,
                insert_current_component,
                None,
            )
            .expect("the in-place migration runs on a native column");
        assert_eq!(migrated_row_count, 1);

        assert_eq!(
            RETIRING_GLUE_DROP_COUNT.load(Ordering::SeqCst),
            retiring_glue_drop_count_before + 1,
            "the old row is released by the generation that allocated it"
        );
        // `deserialize_component` builds a `T::default()` schema baseline per
        // row, and that baseline is dropped by the arriving type's destructor.
        // One row was migrated, so exactly one arriving drop is the baseline;
        // the old row itself must not be among them.
        assert_eq!(
            ARRIVING_GLUE_DROP_COUNT.load(Ordering::SeqCst),
            arriving_glue_drop_count_before + 1,
            "the arriving glue released only its own schema baseline, never the old row"
        );
    }

    /// Re-registering a persistable type while its own column holds rows is
    /// the ordinary hot-reload path and must not trip the peer guard, which
    /// only looks at *other* component ids.
    #[test]
    fn re_registering_the_same_type_over_live_rows_is_not_a_collision() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        world
            .create_entity()
            .with(DropTestForgottenComponent { value: 1 })
            .build()
            .unwrap();

        world.register_persistable_component::<DropTestForgottenComponent>();
        assert!(
            world.take_registration_error().is_none(),
            "a reload re-registering its own type is not a peer collision"
        );
    }

    // =============================================================================
    // Name resolution
    // =============================================================================

    /// A name claimed by exactly one registration resolves to it, through both
    /// the persistable-filtered resolver and the unfiltered one.
    #[test]
    fn an_unambiguous_name_resolves_through_both_resolvers() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let component_id = ComponentId::of::<DropTestForgottenComponent>();
        let type_name = crate::type_names::stable_type_name::<DropTestForgottenComponent>();

        assert_eq!(
            world.resolve_component_id_by_name(type_name).unwrap(),
            Some(component_id)
        );
        assert_eq!(
            world.resolve_component_id_by_name_any(type_name).unwrap(),
            Some(component_id)
        );
    }

    /// A name claimed by two registrations is reported as ambiguous instead of
    /// being resolved by a bit-index tiebreak. The bit index does not record
    /// which registration is newer, so picking one could bind callers to the
    /// wrong column.
    #[test]
    fn a_name_claimed_twice_resolves_to_an_ambiguity_error() {
        let mut world = World::new();
        world.register_persistable_component::<DropTestForgottenComponent>();
        let type_name =
            crate::type_names::stable_type_name::<DropTestForgottenComponent>().to_string();

        // The native column is empty, so a second claim on the name registers
        // (see the empty-column test above). Both registrations are now
        // visible to the unfiltered resolver.
        world
            .register_component_descriptor(
                0x5EED,
                type_name.clone(),
                4,
                4,
                0,
                Blittability::engine_verified(),
            )
            .unwrap();

        let error = world
            .resolve_component_id_by_name_any(&type_name)
            .unwrap_err();
        match error {
            WorldError::ComponentNameAmbiguous {
                type_name: reported_name,
                count,
            } => {
                assert_eq!(reported_name, type_name);
                assert_eq!(count, 2);
            }
            other => panic!("expected an ambiguity error, got {other:?}"),
        }

        // The persistable-filtered resolver still collapses to one, because
        // only the native registration has an inserter.
        assert_eq!(
            world.resolve_component_id_by_name(&type_name).unwrap(),
            Some(ComponentId::of::<DropTestForgottenComponent>())
        );
    }

    /// A name nothing claims resolves to `None` rather than an error - an
    /// unregistered type is an ordinary outcome, not an ambiguity.
    #[test]
    fn an_unclaimed_name_resolves_to_none() {
        let world = World::new();
        assert_eq!(
            world
                .resolve_component_id_by_name("nothing::Registered")
                .unwrap(),
            None
        );
        assert_eq!(
            world
                .resolve_component_id_by_name_any("nothing::Registered")
                .unwrap(),
            None
        );
    }

    // =============================================================================
    // Resource persistence
    // =============================================================================

    /// An ordinary project resource: registered under its Rust type path, not
    /// a shared name, with a shape serde can handle.
    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct PersistProbeScore {
        points: u32,
        label: String,
    }
    impl crate::resource::Resource for PersistProbeScore {}

    /// A resource that is registered but not for persistence, so the opt-in
    /// stays pinned.
    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct PersistProbeOpaque {
        counter: u32,
    }
    impl crate::resource::Resource for PersistProbeOpaque {}

    /// The first build of the settings resource, which stores only `volume`.
    ///
    /// The test process cannot rebuild itself, so a reload is modelled the way
    /// the component tests model it: two distinct Rust types registered under
    /// one persistence name. A real reload produces exactly this state - the
    /// arriving build is a different type to this process, and the name is the
    /// only thing the two generations agree on.
    ///
    /// These are deliberately not shared resources. A shared name is claimed
    /// by one declaring type with one layout, so two types could never both
    /// hold it, which is why the two builds use a separate persistence name.
    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct PersistProbeVolumeSettings {
        volume: f32,
    }
    impl crate::resource::Resource for PersistProbeVolumeSettings {}

    /// The arriving build of the settings resource: the same persistence name,
    /// one field wider.
    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct PersistProbeVolumeBrightnessSettings {
        volume: f32,
        brightness: f32,
    }
    impl crate::resource::Resource for PersistProbeVolumeBrightnessSettings {}

    /// The persistence name both builds of the settings resource register under.
    const PROBE_SETTINGS_NAME: &str = "probe::Settings";

    /// A registered resource's value survives a snapshot and a restore.
    /// Without the resource half of persistence, a loaded world comes back
    /// with every resource missing while looking complete.
    #[test]
    fn a_persistable_resource_round_trips_through_a_snapshot() {
        let mut world = World::new();
        world.register_persistable_resource::<PersistProbeScore>();
        world.insert_resource(PersistProbeScore {
            points: 42,
            label: "wave three".to_string(),
        });

        let snapshot = world.snapshot_resources();
        assert_eq!(snapshot.resource_count(), 1, "the resource was captured");

        // A fresh world stands in for the loading process. It declares the
        // resource but has never been given a value.
        let mut loaded = World::new();
        loaded.register_persistable_resource::<PersistProbeScore>();
        assert!(loaded.get_resource::<PersistProbeScore>().is_none());

        loaded.restore_resources(&snapshot);
        assert_eq!(
            loaded.get_resource::<PersistProbeScore>(),
            Some(&PersistProbeScore {
                points: 42,
                label: "wave three".to_string(),
            }),
            "every field comes back"
        );
    }

    /// Registration is what opts a resource into a snapshot.
    #[test]
    fn an_unregistered_resource_stays_out_of_the_snapshot() {
        let mut world = World::new();
        world.register_persistable_resource::<PersistProbeScore>();
        world.insert_resource(PersistProbeScore::default());
        // Inserted, never registered as persistable.
        world.insert_resource(PersistProbeOpaque { counter: 9 });

        let snapshot = world.snapshot_resources();
        assert_eq!(
            snapshot.resource_count(),
            1,
            "only the registered resource is captured"
        );
        assert!(
            snapshot
                .payload(crate::type_names::stable_type_name::<PersistProbeScore>())
                .is_some(),
            "and it is the one that opted in"
        );
    }

    /// A resource that was never inserted contributes no entry.
    ///
    /// "Registered" and "has a value" are different states, and a snapshot
    /// records values. Writing a null would make the loading generation
    /// overwrite whatever it inserted itself with an empty default.
    #[test]
    fn a_registered_resource_with_no_value_is_absent_from_the_snapshot() {
        let mut world = World::new();
        world.register_persistable_resource::<PersistProbeScore>();

        assert_eq!(world.snapshot_resources().resource_count(), 0);
    }

    /// A name the loading generation no longer declares is skipped.
    #[test]
    fn a_resource_the_loader_does_not_declare_is_skipped() {
        let mut world = World::new();
        world.register_persistable_resource::<PersistProbeScore>();
        world.insert_resource(PersistProbeScore {
            points: 3,
            label: String::new(),
        });
        let snapshot = world.snapshot_resources();

        // The loading world declares nothing, as a project that dropped the
        // resource type between builds would.
        let mut loaded = World::new();
        loaded.restore_resources(&snapshot);
        assert!(
            loaded.get_resource::<PersistProbeScore>().is_none(),
            "nothing is resurrected for a type this generation does not own"
        );
    }

    /// A field added since the snapshot takes the new type's default value.
    #[test]
    fn a_field_added_since_the_resource_snapshot_restores_defaulted() {
        let mut world = World::new();
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        world.insert_resource(PersistProbeVolumeSettings { volume: 0.75 });
        let snapshot = world.snapshot_resources();

        // The next build of the same resource, one field wider, registered
        // under the same persistence name.
        let mut loaded = World::new();
        loaded.register_persistable_resource_as::<PersistProbeVolumeBrightnessSettings>(
            PROBE_SETTINGS_NAME,
        );
        loaded.restore_resources(&snapshot);

        assert_eq!(
            loaded.get_resource::<PersistProbeVolumeBrightnessSettings>(),
            Some(&PersistProbeVolumeBrightnessSettings {
                volume: 0.75,
                brightness: 0.0,
            }),
            "the carried field survives and the new one starts defined"
        );
    }

    /// A resource declared by another language round-trips as raw bytes.
    #[test]
    fn a_foreign_resource_round_trips_as_bytes() {
        let mut world = World::new();
        let id = world
            .register_foreign_resource("probe::ForeignTuning", "Probe.Tuning", 8, 4, 0x1234)
            .expect("a valid foreign layout registers");
        assert!(world.register_persistable_foreign_resource("probe::ForeignTuning"));
        world
            .insert_foreign_resource_bytes(id, &[1, 2, 3, 4, 5, 6, 7, 8])
            .expect("the payload is the declared width");

        let snapshot = world.snapshot_resources();
        assert_eq!(snapshot.resource_count(), 1);

        let mut loaded = World::new();
        let loaded_id = loaded
            .register_foreign_resource("probe::ForeignTuning", "Probe.Tuning", 8, 4, 0x1234)
            .expect("the loading generation declares the same shape");
        loaded.restore_resources(&snapshot);

        assert_eq!(
            loaded.foreign_resource_bytes(loaded_id),
            Some(&[1u8, 2, 3, 4, 5, 6, 7, 8][..]),
            "every byte survives the round trip"
        );
    }

    /// A foreign payload whose width disagrees with the live declaration is
    /// refused rather than padded.
    #[test]
    fn a_resized_foreign_resource_payload_is_refused() {
        let mut world = World::new();
        let id = world
            .register_foreign_resource("probe::ForeignResized", "Probe.Resized", 8, 4, 1)
            .expect("a valid foreign layout registers");
        assert!(world.register_persistable_foreign_resource("probe::ForeignResized"));
        world
            .insert_foreign_resource_bytes(id, &[9; 8])
            .expect("the payload is the declared width");
        let snapshot = world.snapshot_resources();

        // The loading generation declares the same resource four bytes wider.
        let mut loaded = World::new();
        let loaded_id = loaded
            .register_foreign_resource("probe::ForeignResized", "Probe.Resized", 12, 4, 2)
            .expect("the wider layout is still valid");
        loaded.restore_resources(&snapshot);

        assert_eq!(
            loaded.foreign_resource_bytes(loaded_id),
            None,
            "an unreconcilable payload leaves the resource unset instead of \
             writing a partly-defined value"
        );
    }

    /// A snapshot comes out in name order, which is what makes two identical
    /// captures byte-reproducible.
    #[test]
    fn a_resource_snapshot_is_ordered_by_name() {
        let mut world = World::new();
        world.register_persistable_resource::<PersistProbeScore>();
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        world.insert_resource(PersistProbeScore::default());
        world.insert_resource(PersistProbeVolumeSettings::default());

        let snapshot_names: Vec<String> = world
            .snapshot_resources()
            .entries
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let mut sorted_snapshot_names = snapshot_names.clone();
        sorted_snapshot_names.sort();
        assert_eq!(
            snapshot_names, sorted_snapshot_names,
            "entries come out in name order"
        );
    }

    /// A reshaped resource is migrated across a reload, not reinterpreted.
    ///
    /// `rehome_resources` gives a live resource the arriving function table
    /// but leaves its bytes alone. Without this pass, the arriving generation
    /// would read the old value's memory through the new shape.
    #[cfg(feature = "development")]
    #[test]
    fn a_reshaped_resource_is_migrated_across_a_reload() {
        let mut world = World::new();
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        world.insert_resource(PersistProbeVolumeSettings { volume: 0.5 });

        // What the host captures immediately before the swap.
        let previous_manifest = world.persist_resource_manifest();
        assert_eq!(previous_manifest.len(), 1);

        // The arriving generation registers the wider shape under the same
        // persistence name, which supersedes the outgoing build's entries.
        world.register_persistable_resource_as::<PersistProbeVolumeBrightnessSettings>(
            PROBE_SETTINGS_NAME,
        );
        let migrated_resource_names =
            world.migrate_changed_persistable_resources(&previous_manifest);

        assert_eq!(
            migrated_resource_names,
            vec![PROBE_SETTINGS_NAME.to_string()]
        );
        assert_eq!(
            world.get_resource::<PersistProbeVolumeBrightnessSettings>(),
            Some(&PersistProbeVolumeBrightnessSettings {
                volume: 0.5,
                brightness: 0.0,
            }),
            "the value carries across and the added field is defaulted"
        );
    }

    /// An unchanged resource keeps its value untouched across a reload.
    #[cfg(feature = "development")]
    #[test]
    fn an_unchanged_resource_is_not_migrated() {
        let mut world = World::new();
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        world.insert_resource(PersistProbeVolumeSettings { volume: 0.25 });

        let previous_manifest = world.persist_resource_manifest();
        // The same shape registers again, as an unedited reload does.
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);

        assert!(
            world
                .migrate_changed_persistable_resources(&previous_manifest)
                .is_empty(),
            "an unchanged schema takes the fast path"
        );
        assert_eq!(
            world.get_resource::<PersistProbeVolumeSettings>(),
            Some(&PersistProbeVolumeSettings { volume: 0.25 }),
            "and the value is left exactly as it was"
        );
    }

    /// Changing only a default *value* is not a schema change.
    ///
    /// The schema hash keeps only the kind of each value (string, number,
    /// array, ...), so editing a literal in a `Default` impl does not make
    /// every stored value look reshaped.
    #[test]
    fn a_changed_default_value_does_not_count_as_a_reshape() {
        let mut world = World::new();
        world.register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        world.insert_resource(PersistProbeVolumeSettings { volume: 0.9 });
        let first_world_manifest = world.persist_resource_manifest();

        let mut other_world = World::new();
        other_world
            .register_persistable_resource_as::<PersistProbeVolumeSettings>(PROBE_SETTINGS_NAME);
        let second_world_manifest = other_world.persist_resource_manifest();

        assert_eq!(
            first_world_manifest[0].schema_hash,
            second_world_manifest[0].schema_hash
        );
    }

    /// A component whose own `Serialize` refuses every value.
    #[derive(Clone, Debug, Default)]
    struct RefusingSerializeComponent {
        value: u32,
    }

    impl Component for RefusingSerializeComponent {}

    impl serde::Serialize for RefusingSerializeComponent {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(
                "this component refuses to serialize",
            ))
        }
    }

    /// A refused serialization reports `None` instead of aborting the reload.
    ///
    /// The migration reads every row through this function, and its inverse
    /// already reports a type it cannot decode as a skipped type. A type it
    /// cannot encode must reach the same report: a panic here would abort the
    /// transaction after earlier archetypes of the type had already been
    /// swapped.
    #[test]
    fn a_refusing_serializer_reports_none_instead_of_panicking() {
        let mut world = World::new();
        world.register_component::<RefusingSerializeComponent>();
        world
            .create_entity()
            .with(RefusingSerializeComponent { value: 7 })
            .build()
            .expect("the entity builds");

        let component_id = ComponentId::of::<RefusingSerializeComponent>();
        let mut serialize_answer = None;
        for archetype in world.archetypes_iter() {
            if archetype.component_types.contains(&component_id) {
                let column = archetype
                    .component_storages
                    .column_of::<RefusingSerializeComponent>();
                assert_eq!(
                    column.get::<RefusingSerializeComponent>(0).value,
                    7,
                    "the row the glue refused to encode is the one this test wrote"
                );
                serialize_answer = Some(super::components::serialize_component::<
                    RefusingSerializeComponent,
                >(&archetype.component_storages, 0));
            }
        }

        assert!(
            matches!(serialize_answer, Some(None)),
            "a refused serialization is a report, not a panic"
        );
    }
}
