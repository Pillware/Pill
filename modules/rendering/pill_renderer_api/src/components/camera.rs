//! The camera a frame renders through.
//!
//! # Responsibilities
//!
//! - Define [`CameraComponent`], the component holding the perspective settings
//!   the renderer builds its projection matrix from.
//!
//! # Design
//!
//! The camera is an ordinary scene component. It is shared, so the host, a
//! project and the managed side all see the same one. It is persistable, so its
//! values are saved with a scene and survive a reload. Its shared name is
//! pinned rather than derived from the module path; see [`crate::components`].
//!
//! **Renderer contract.** [`crate::frame::RenderFrame`] carries this camera on
//! every surface it draws through. That is why the component stays in
//! `pill_renderer_api` for every renderer, unlike a renderer's own components,
//! which live in that renderer's data crate.

// External crates
use pill_engine::{component, World};

// =============================================================================
// CameraComponent
// =============================================================================

/// A perspective camera: the renderer builds its projection matrix from it and
/// its view matrix from the transform stored beside it.
///
/// When the renderer picks a camera out of the world, it uses one enabled
/// camera. The highest `priority` wins, and equal priorities go to the lowest
/// entity id, so the choice cannot change with the order the entities are
/// visited. A surface with its own camera binding uses that camera instead.
///
/// A value the projection cannot use is replaced with a default while the
/// renderer builds the matrix. That keeps a mistyped field of view or near/far
/// pair from producing NaN values in the matrix.
#[component(
    shared = "pill_master_renderer::component::CameraComponent",
    persistable
)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CameraComponent {
    /// Whether the renderer may pick this camera. A disabled camera stays in
    /// the world but is never picked for a surface.
    pub enabled: bool,
    /// Which camera the frame prefers when the world holds several: the highest
    /// value wins.
    pub priority: i32,
    /// Vertical field of view, in degrees. The renderer replaces a value outside
    /// `(0, 180)` with the default.
    pub vertical_fov: f32,
    /// Distance to the near clip plane, in world units. A value that is not a
    /// finite positive number is replaced with the default.
    pub near: f32,
    /// Distance to the far clip plane, in world units. A value that is not
    /// finite or not beyond the near plane is replaced with a distance that is.
    pub far: f32,
}

impl Default for CameraComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: 0,
            vertical_fov: 60.0,
            near: 0.1,
            far: 1000.0,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Registers [`CameraComponent`] and its field layout with the world.
///
/// The derive creates `__pill_register_CameraComponent` as a private item of
/// this module, so [`crate::components::register_contract_components`] calls it
/// through this function instead of naming it directly.
pub(crate) fn register(world: &mut World) {
    __pill_register_CameraComponent(world);
}
