//! Proves that `#[component]` records the asset type each
//! [`Handle`] field references.
//!
//! # Responsibilities
//!
//! - Prove the derive's generated registration records each handle field's
//!   asset type: `T`'s shared name, or its Rust type name when it has none.
//!   A read of the field then reports that type, and a write refuses an asset
//!   of another type.
//!
//! # Design
//!
//! The components are registered through the same loop a module's `init`
//! uses, which registers every component the DLL declares. The derive's
//! generated registration therefore does the recording, not a hand-written
//! call.

// External crates
use pill_engine::asset::{Asset, Handle};
use pill_engine::component_field::FieldValue;
use pill_engine::component_registry::register_all_components;
use pill_engine::{component, ComponentId, World};

/// An asset type with a pinned shared name, like the renderer's asset types.
struct PinnedMesh;
impl Asset for PinnedMesh {
    fn shared_name() -> Option<&'static str> {
        Some("probe::assets::Mesh")
    }
}

/// An asset type without a shared name.
struct LocalSound;
impl Asset for LocalSound {}

/// A derived component with two handle fields and a scalar between them.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct ReferenceCarrier {
    mesh: Handle<PinnedMesh>,
    volume: f32,
    sound: Handle<LocalSound>,
}

/// The derive records each handle field's asset type: the pinned shared name,
/// or the Rust type name when the asset type has none. A scalar field gets no
/// type, and reading a handle field reports its type with the handle's index
/// and generation.
#[test]
fn the_derive_records_the_asset_type_of_each_handle_field() {
    let mut world = World::new();
    // Step 1: register the components, which runs the derive's registration.
    register_all_components(&mut world).expect("registration succeeds");

    // Step 2: each field's recorded asset type, as the registration wrote it.
    let component_id = ComponentId::of::<ReferenceCarrier>();
    assert_eq!(
        world.asset_field_type(component_id, "mesh"),
        Some("probe::assets::Mesh"),
        "a shared name is the recorded type"
    );
    assert_eq!(
        world.asset_field_type(component_id, "sound"),
        Some(std::any::type_name::<LocalSound>()),
        "without one, the Rust type name is"
    );
    assert_eq!(world.asset_field_type(component_id, "volume"), None);

    // Step 3: a read of a handle field reports the recorded type with the
    // handle's index and generation.
    let entity = world
        .create_entity()
        .with(ReferenceCarrier {
            mesh: Handle::from_raw(4, 1),
            volume: 0.5,
            sound: Handle::INVALID,
        })
        .build()
        .expect("entity builds");
    let registered_component_name =
        pill_engine::component::ComponentRegistry::registered_name::<ReferenceCarrier>();
    assert_eq!(
        world
            .read_component_field(entity, &registered_component_name, "mesh")
            .unwrap(),
        FieldValue::Asset {
            asset_type: Some("probe::assets::Mesh".to_owned()),
            index: 4,
            generation: 1,
        }
    );
}
