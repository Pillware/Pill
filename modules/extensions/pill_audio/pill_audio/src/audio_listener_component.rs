//! The [`AudioListenerComponent`] component: where the world is heard from.
//!
//! # Responsibilities
//!
//! - Declares the component that marks the entity whose position the listener
//!   hears from.
//!
//! # Design
//!
//! The listener stores its own facing angle rather than reading the
//! transform's rotation. A transform keeps its rotation as a quaternion, and
//! panning needs only a single yaw angle, so
//! [`facing_degrees`](AudioListenerComponent::facing_degrees) holds it.

// External crates
use pill_engine::component;

// =============================================================================
// AudioListenerComponent
// =============================================================================

/// Marks the entity whose position the listener hears from.
///
/// Only one listener is used at a time: the system takes the first enabled
/// one it finds. The entity also needs a
/// [`TransformComponent`](pill_engine::TransformComponent), because the
/// listener's position comes from there.
#[component(persistable, shared)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioListenerComponent {
    /// Whether the system may use this listener.
    pub enabled: bool,
    /// Direction the listener faces, in degrees counter-clockwise from +X in
    /// the scene's XY plane.
    ///
    /// The angle is kept here, not read from the transform's rotation, because
    /// panning needs a single yaw value.
    pub facing_degrees: f32,
}

impl core::default::Default for AudioListenerComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            facing_degrees: 0.0,
        }
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Registers [`AudioListenerComponent`] with the world, including its field
/// layout.
///
/// The derive generates `__pill_register_AudioListenerComponent` as a private
/// item of this module, so the crate's [`register`](crate::register) function
/// cannot call it by path; it calls this function instead. The layout is what
/// lets the Inspector show the two fields as rows and the C# code generator
/// give them types. Without it, both see one untyped block of bytes.
pub(crate) fn register(world: &mut pill_engine::World) {
    __pill_register_AudioListenerComponent(world);
}
