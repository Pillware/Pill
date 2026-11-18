//! A derived component's `Default` value is what a component built from field
//! values starts from.
//!
//! # Responsibilities
//!
//! - Show that a component registered through `register_all_components` with a
//!   `Default` impl is built - as the editor's Add Component builds it - from
//!   that default, and that a supplied field still overrides it.
//! - Show that a component without `Default` still derives, registers and
//!   builds, starting from zeros.
//! - Show that the engine's transform is built at unit scale.

// External crates
use pill_engine::component_registry::register_all_components;
use pill_engine::{component, FieldValue, TransformComponent, World};

/// A component whose default is not all zeros.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct SizedProbe {
    size: [f32; 3],
    weight: f32,
}

impl Default for SizedProbe {
    fn default() -> Self {
        Self {
            size: [1.0, 1.0, 1.0],
            weight: 2.5,
        }
    }
}

/// A component with no `Default` at all.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct PlainProbe {
    count: u32,
}

/// Builds `T`'s image from `fields` and reads it back as a `T`.
fn built<T: Copy>(world: &World, component_name: &str, fields: &[(String, FieldValue)]) -> T {
    let image = world
        .build_component_image(component_name, fields)
        .expect("the component builds");
    assert_eq!(image.len(), std::mem::size_of::<T>());
    // SAFETY: the image is exactly one `T`'s bytes, built by the field layer
    // for this component, and `T` is plain `Copy` data.
    unsafe { std::ptr::read_unaligned(image.as_ptr().cast::<T>()) }
}

/// The registered name of the component `T`, read from an entity carrying
/// `value`.
fn component_name<T: pill_engine::Component>(world: &mut World, value: T, suffix: &str) -> String {
    let entity = world
        .create_entity()
        .with(value)
        .build()
        .expect("entity builds");
    world
        .entity_component_names(entity)
        .and_then(|names| names.into_iter().find(|name| name.ends_with(suffix)))
        .unwrap_or_else(|| panic!("`{suffix}` is registered"))
}

#[test]
fn a_component_is_built_from_its_default_and_fields_override_it() {
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");
    let name = component_name(&mut world, SizedProbe::default(), "SizedProbe");

    let from_default: SizedProbe = built(&world, &name, &[]);
    assert_eq!(from_default.size, [1.0, 1.0, 1.0]);
    assert_eq!(from_default.weight, 2.5);

    let overridden: SizedProbe = built(
        &world,
        &name,
        &[("weight".to_string(), FieldValue::F32(7.0))],
    );
    assert_eq!(
        overridden.size,
        [1.0, 1.0, 1.0],
        "an unsupplied field keeps its default"
    );
    assert_eq!(overridden.weight, 7.0);
}

#[test]
fn a_component_without_default_starts_from_zeros() {
    let mut world = World::new();
    register_all_components(&mut world).expect("registration succeeds");
    let name = component_name(&mut world, PlainProbe { count: 3 }, "PlainProbe");
    let plain: PlainProbe = built(&world, &name, &[]);
    assert_eq!(plain.count, 0);
}

#[test]
fn an_added_transform_has_unit_scale() {
    let mut world = World::new();
    pill_engine::register_transform_component(&mut world);
    let name = component_name(
        &mut world,
        TransformComponent::default(),
        "TransformComponent",
    );
    let transform: TransformComponent = built(&world, &name, &[]);
    assert_eq!(transform.scale, [1.0, 1.0, 1.0]);
    assert_eq!(transform.rotation, [0.0, 0.0, 0.0, 1.0]);
}
