//! Components the engine defines because more than one consumer needs them.
//!
//! # Responsibilities
//!
//! - Defines [`TransformComponent`], an entity's placement in the scene.
//!   Rendering, physics, audio and gameplay code all read it.
//! - Declares its editor field layout and the one call that registers it
//!   ([`register_transform_component`]), so a world that uses it shows the
//!   transform's fields in the inspector.
//!
//! # Design
//!
//! The transform belongs to the scene rather than to a renderer: where a thing
//! is has nothing to do with a GPU. Renderer-specific mesh, camera and material
//! contracts live in the rendering crates, and a renderer that wants a
//! component of its own - a pixel position for a sprite, say - defines it
//! beside the code that draws it.
//!
//! The struct's bytes are a shared ABI. Another language binds the same row
//! through the name the component registers under and the `repr(C)` size it
//! checks. A `TypeId` would not work for this, because it is computed per
//! compilation unit; [`TransformComponent`] declares a shared name instead, so
//! every binary that registers it resolves to the same column. The component is
//! declared by hand rather than with `#[derive(PillComponent)]`, whose
//! inventory entry would make every artifact that links the engine register the
//! component - and add it to that artifact's persistable-schema fingerprint -
//! whether the artifact used it or not.

// External crates
// The derive macro for the field layout: the layout half of `PillComponent`,
// without the inventory entry that registers the type in every artifact.
use pill_engine_macros::PillLayout;
use serde::{Deserialize, Serialize};

// Current crate
use crate::component::Component;
use crate::world::World;

// =============================================================================
// Components
// =============================================================================

/// The shared name [`TransformComponent`] registers under.
///
/// The renderer family publishes its components under one prefix, so the name
/// is pinned to `pill_master_renderer` rather than derived from this crate.
/// It must never change: a different name would register a different
/// component, and a live world's stored column and every scene that names it
/// would no longer be found.
pub const TRANSFORM_SHARED_NAME: &str = "pill_master_renderer::TransformComponent";

/// Placement of an entity in the scene: position, orientation and scale.
///
/// Renderers read it for every drawable and for the camera they render
/// through, and any other system may read or write it. It is shared and
/// persistable, so a write is visible to the managed side and the value
/// survives a project reload.
#[repr(C)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PillLayout)]
pub struct TransformComponent {
    /// Position of the entity's origin, in world units.
    pub translation: [f32; 3],
    /// Orientation as a unit quaternion `[x, y, z, w]`. The identity is
    /// `[0, 0, 0, 1]`. A non-finite or near-zero value is read as the identity
    /// instead of being used as it is.
    pub rotation: [f32; 4],
    /// Per-axis scale applied to the mesh when it is drawn.
    pub scale: [f32; 3],
}

impl Component for TransformComponent {
    fn shared_name() -> Option<&'static str> {
        Some(TRANSFORM_SHARED_NAME)
    }

    fn shared_identity() -> Option<u128> {
        // A `const`, as the derive emits it, so the name is hashed at compile
        // time rather than on every `ComponentId::of` call.
        const IDENTITY: u128 = crate::component::shared_component_identity(TRANSFORM_SHARED_NAME);
        Some(IDENTITY)
    }

    fn declared_schema_hash() -> Option<u64> {
        // The same hash the derive would emit, computed over the field layout
        // that registration uses.
        Some(crate::component::component_schema_hash(
            TransformComponent::FIELD_LAYOUT,
        ))
    }
}

impl Default for TransformComponent {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

/// Squared length at or below which a rotation quaternion is read as the
/// identity orientation.
///
/// Normalizing a quaternion this close to zero would amplify rounding noise
/// rather than an angle, so such a value is not a usable orientation.
pub const ROTATION_IDENTITY_EPSILON: f32 = 1.0e-8;

/// Reads a [`TransformComponent::rotation`] value and returns it as a unit
/// quaternion, or as the identity when it is not usable.
///
/// This is the only implementation of the rule the field's documentation
/// states, so systems, projects and the renderer share it and its tolerance
/// instead of each writing its own.
pub fn rotation_or_identity(rotation: [f32; 4]) -> [f32; 4] {
    let length_squared: f32 = rotation.iter().map(|value| value * value).sum();
    if rotation.iter().all(|value| value.is_finite()) && length_squared > ROTATION_IDENTITY_EPSILON
    {
        let inverse_length = length_squared.sqrt().recip();
        rotation.map(|value| value * inverse_length)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Registers [`TransformComponent`] together with its editor field layout.
///
/// The component is registered as persistable, so its values are migrated
/// across reloads. Registering it again is safe and is what a hot reload does
/// when it re-runs `init`. The layout comes from `#[derive(PillLayout)]`, which
/// is what lets the inspector edit the fields instead of showing one opaque
/// blob.
///
/// A renderer calls this from its registration entry point, before adding
/// whatever it draws with.
pub fn register_transform_component(world: &mut World) {
    world.register_persistable_component_with_layout::<TransformComponent>(
        TransformComponent::FIELD_LAYOUT,
    );
    // An added transform starts at the identity: unit scale, no rotation.
    world.record_component_default::<TransformComponent>();
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Registration attaches the layout, so the inspector sees fields rather
    /// than an opaque blob.
    #[test]
    fn registration_attaches_the_field_layout() {
        let mut world = World::new();
        register_transform_component(&mut world);

        let transform = crate::component::ComponentId::of::<TransformComponent>();
        assert_eq!(
            world.component_field_layout(transform).map(<[_]>::len),
            Some(3)
        );
    }

    /// Registering twice is what a hot reload does, and it must not fail.
    #[test]
    fn registration_is_idempotent() {
        let mut world = World::new();
        register_transform_component(&mut world);
        register_transform_component(&mut world);

        assert!(
            world.take_registration_error().is_none(),
            "re-registering the same type is what every reload does"
        );
    }

    /// The transform's pinned string is its identity across binaries, so the
    /// test holds the name in place.
    #[test]
    fn the_transform_keeps_its_pinned_shared_name() {
        assert_eq!(
            TransformComponent::shared_name(),
            Some("pill_master_renderer::TransformComponent")
        );
        assert_eq!(
            TransformComponent::shared_identity(),
            Some(crate::component::shared_component_identity(
                "pill_master_renderer::TransformComponent"
            ))
        );
    }

    /// A world registering only the engine's component has the transform, with
    /// its three described fields and a persistence entry.
    #[test]
    fn registration_includes_a_persistable_transform() {
        let mut world = World::new();
        register_transform_component(&mut world);

        let transform = crate::component::ComponentId::of::<TransformComponent>();
        assert_eq!(
            world.component_field_layout(transform).map(<[_]>::len),
            Some(3)
        );
        assert!(
            world
                .persist_schema_hashes
                .contains_key(TRANSFORM_SHARED_NAME),
            "the transform is registered for migration under its shared name"
        );
        assert!(world.take_registration_error().is_none());
    }

    /// The layout covers the struct byte for byte: three positions, a
    /// quaternion and three scales, all `f32` arrays.
    #[test]
    fn the_transform_layout_matches_its_struct() {
        let fields = TransformComponent::FIELD_LAYOUT;
        let names: Vec<&str> = fields.iter().map(|field| field.name).collect();
        assert_eq!(names, ["translation", "rotation", "scale"]);
        assert_eq!(
            fields.iter().map(|field| field.size).sum::<usize>(),
            std::mem::size_of::<TransformComponent>()
        );
        assert_eq!(
            fields
                .iter()
                .map(|field| field.element_count)
                .collect::<Vec<_>>(),
            [3, 4, 3]
        );
    }

    /// The default transform is the identity placement.
    #[test]
    fn the_default_transform_is_the_identity() {
        let transform = TransformComponent::default();
        assert_eq!(transform.translation, [0.0; 3]);
        assert_eq!(transform.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(transform.scale, [1.0; 3]);
    }
}
