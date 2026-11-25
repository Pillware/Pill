//! The surfaces one renderer draws into: their identities, how each one
//! presents, and which camera draws into it.
//!
//! # Responsibilities
//!
//! - Name a renderer's surfaces ([`SurfaceId`]); the window a renderer is
//!   attached with is [`SurfaceId::PRIMARY`], every other one is attached
//!   through [`crate::PillRenderer::attach_surface`].
//! - Describe how a surface presents ([`SurfaceOptions`], [`PresentPreference`]).
//! - Bind a camera to a surface ([`CameraBinding`], the [`SurfaceCameras`]
//!   resource) and carry each surface's resolved camera in the frame
//!   ([`SurfaceView`]).
//!
//! # Design
//!
//! Everything here is plain data, so a frontend, the host and any renderer
//! agree on it without sharing a graphics type. A camera binding either names
//! a camera entity or carries the camera itself. The editor's own camera is
//! the second kind, which keeps it out of the world.
//!
//! The frontend decides when each surface presents, because only it knows the
//! window system. Presenting into a Wayland subsurface before the compositor
//! asks for a frame blocks under Mesa's FIFO mode. So the frontend tells the
//! renderer, for each surface and each frame, whether that surface may present
//! now ([`crate::PillRenderer::set_surface_ready`]), and the renderer skips a
//! surface that may not. One blocked surface never stalls another surface or
//! the event loop.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::common_components::TransformComponent;
use pill_engine::{Entity, Resource};

// Current crate
use crate::CameraComponent;

/// One surface a renderer draws into, unique within that renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SurfaceId(pub u32);

impl SurfaceId {
    /// The surface the renderer was attached with.
    pub const PRIMARY: Self = Self(0);
}

/// How a surface would like to present.
///
/// The renderer uses the requested mode when the surface supports it, and
/// vsync otherwise. Vsync is the default because every surface supports it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PresentPreference {
    /// Wait for the display (`Fifo`).
    #[default]
    Vsync,
    /// Replace a queued frame instead of waiting (`Mailbox`), for a surface
    /// the frontend paces itself.
    LowLatency,
    /// Present at once, tearing allowed (`Immediate`).
    Immediate,
}

/// Creation options of one surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceOptions {
    /// How the surface would like to present.
    pub present: PresentPreference,
}

/// Which camera draws into a surface.
#[derive(Debug, Clone, Copy)]
pub enum CameraBinding {
    /// A camera entity in the world. The surface shows nothing while that
    /// entity has no enabled camera.
    Entity(Entity),
    /// A camera given as plain data instead of an entity. The editor's own
    /// camera is one of these.
    View {
        /// The projection.
        camera: CameraComponent,
        /// Where the camera is and where it looks.
        transform: TransformComponent,
    },
}

/// The camera bindings of every surface, by surface id.
///
/// The frontend writes this into the world as a resource. A surface with no
/// entry draws through the frame's chosen camera when it is the primary one,
/// and draws nothing otherwise. An entry for [`SurfaceId::PRIMARY`] replaces
/// the choice the frame would make among the world's cameras.
#[derive(Debug, Clone, Default)]
pub struct SurfaceCameras {
    /// Binding per surface.
    pub bindings: BTreeMap<SurfaceId, CameraBinding>,
    /// The scene grid the view-bound surfaces draw. It is off until a
    /// frontend turns it on.
    pub grid: crate::editor::SceneGrid,
}

impl SurfaceCameras {
    /// Binds `binding` to `surface`, or removes the surface's binding with
    /// `None`.
    pub fn set(&mut self, surface: SurfaceId, binding: Option<CameraBinding>) {
        match binding {
            Some(binding) => {
                self.bindings.insert(surface, binding);
            }
            None => {
                self.bindings.remove(&surface);
            }
        }
    }
}

impl Resource for SurfaceCameras {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::SurfaceCameras")
    }
}

/// The camera one surface draws through this frame.
///
/// The frame resolves it from the surface's binding or, for the primary
/// surface without one, from the world's cameras.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceView {
    /// The projection.
    pub camera: CameraComponent,
    /// Where the camera is and where it looks.
    pub transform: TransformComponent,
}

/// Finds the camera a binding names among the world's cameras.
///
/// `cameras` yields each camera entity with its camera and transform. A
/// [`CameraBinding::View`] resolves to itself. An entity binding resolves to
/// that entity's camera when the entity is in `cameras` and the camera is
/// enabled, and to `None` otherwise.
pub fn resolve_camera_binding(
    binding: &CameraBinding,
    cameras: impl IntoIterator<Item = (Entity, CameraComponent, TransformComponent)>,
) -> Option<SurfaceView> {
    match binding {
        CameraBinding::View { camera, transform } => Some(SurfaceView {
            camera: *camera,
            transform: *transform,
        }),
        CameraBinding::Entity(wanted) => cameras
            .into_iter()
            .find(|(entity, camera, _)| entity == wanted && camera.enabled)
            .map(|(_, camera, transform)| SurfaceView { camera, transform }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera as plain data resolves to itself, without any entity.
    #[test]
    fn a_view_binding_resolves_to_itself() {
        let transform = TransformComponent {
            translation: [1.0, 2.0, 3.0],
            ..Default::default()
        };
        let binding = CameraBinding::View {
            camera: CameraComponent::default(),
            transform,
        };
        let view = resolve_camera_binding(&binding, std::iter::empty()).unwrap();
        assert_eq!(view.transform.translation, [1.0, 2.0, 3.0]);
    }

    /// An entity binding finds that entity's enabled camera, and nothing for a
    /// missing or disabled one.
    #[test]
    fn an_entity_binding_resolves_through_the_world() {
        let mut world = pill_engine::World::new();
        let wanted = world.reserve_entity();
        let other = world.reserve_entity();
        let missing = world.reserve_entity();
        let enabled_camera = CameraComponent::default();
        let disabled_camera = CameraComponent {
            enabled: false,
            ..Default::default()
        };
        let cameras = || {
            [
                (other, enabled_camera, TransformComponent::default()),
                (wanted, enabled_camera, TransformComponent::default()),
            ]
        };
        assert!(resolve_camera_binding(&CameraBinding::Entity(wanted), cameras()).is_some());
        assert!(resolve_camera_binding(&CameraBinding::Entity(missing), cameras()).is_none());
        assert!(resolve_camera_binding(
            &CameraBinding::Entity(wanted),
            [(wanted, disabled_camera, TransformComponent::default())]
        )
        .is_none());
    }

    /// Setting `None` removes a surface's binding.
    #[test]
    fn a_binding_can_be_removed() {
        let mut world = pill_engine::World::new();
        let entity = world.reserve_entity();
        let mut cameras = SurfaceCameras::default();
        cameras.set(SurfaceId(1), Some(CameraBinding::Entity(entity)));
        assert!(cameras.bindings.contains_key(&SurfaceId(1)));
        cameras.set(SurfaceId(1), None);
        assert!(cameras.bindings.is_empty());
    }
}
