//! The scene grid: the infinite ground the view-bound surfaces draw.
//!
//! # Responsibilities
//!
//! - Carry a frontend's grid settings ([`SceneGrid`]: whether the grid is on
//!   and the distance between its lines) beside the surface camera bindings.
//! - Compute the spacing one frame's grid uses ([`frame_grid`]) and list the
//!   surfaces the grid is drawn on ([`editor_view_surfaces`]).
//!
//! # Design
//!
//! The grid belongs to the editor, not to the world. It is drawn on the
//! surfaces bound to a view camera ([`CameraBinding::View`]), which are the
//! editor's own viewports, and such a camera is plain data, never an entity.
//! A surface drawn through a world camera never shows the grid. So an editor
//! gets the grid by turning one setting on, and a game's window stays
//! untouched however a frontend binds its cameras.
//!
//! The renderer draws the grid as one fullscreen triangle. The fragment
//! shader casts a ray through each pixel and finds where it meets the ground
//! plane. So the grid has no size to run out of, and nothing to rebuild when
//! the view moves. The frame carries only the spacing; the fade and the
//! colours live in the shader.
//!
//! The frame names the grid by that spacing alone, and the surfaces the grid
//! is drawn on are the frame's editor views. So the grid and everything else
//! the editor draws over the scene share one list, and the two can never
//! disagree.

// Standard library
use std::collections::BTreeMap;

// Current crate
use crate::{CameraBinding, SurfaceCameras, SurfaceId};

// =============================================================================
// Constants
// =============================================================================

/// The cell size the grid uses when the frontend's value is unusable. The
/// shader divides world positions by the cell size, so it must be positive and
/// finite.
const DEFAULT_CELL_SIZE: f32 = 1.0;

// =============================================================================
// SceneGrid
// =============================================================================

/// The scene grid's on/off switch and the distance between its lines.
///
/// A frontend writes it through [`SurfaceCameras`], beside the surface camera
/// bindings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneGrid {
    /// Whether the surfaces bound to a view camera draw the grid.
    pub enabled: bool,
    /// How far apart the grid's minor lines sit, in world units.
    pub cell_size: f32,
}

impl Default for SceneGrid {
    fn default() -> Self {
        // The grid stays off until a frontend asks for it. Turning it on is
        // the editor's decision, so a game's window and any other consumer of
        // the renderer keeps the default.
        Self {
            enabled: false,
            cell_size: DEFAULT_CELL_SIZE,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the grid spacing one frame draws, or `None` when it draws no grid.
///
/// A surface draws the grid when the grid is on and the surface is bound to a
/// view camera. Those surfaces are the ones [`editor_view_surfaces`] returns,
/// so this function answers only the spacing. It returns `None` when the grid
/// is off or when no surface is bound to a view camera. A cell size that is
/// not a positive, finite number falls back to the default.
pub fn frame_grid(surface_cameras: &SurfaceCameras) -> Option<f32> {
    if !surface_cameras.grid.enabled {
        return None;
    }
    if view_bound_surfaces(&surface_cameras.bindings).is_empty() {
        return None;
    }
    let cell_size = surface_cameras.grid.cell_size;
    Some(if cell_size > 0.0 && cell_size.is_finite() {
        cell_size
    } else {
        DEFAULT_CELL_SIZE
    })
}

/// Returns every surface bound to a view camera, in ascending id order.
fn view_bound_surfaces(bindings: &BTreeMap<SurfaceId, CameraBinding>) -> Vec<SurfaceId> {
    bindings
        .iter()
        .filter(|(_, binding)| matches!(binding, CameraBinding::View { .. }))
        .map(|(surface, _)| *surface)
        .collect()
}

/// Returns every surface bound to a view camera, whether or not the grid is on.
///
/// These are the surfaces an editor's own viewports draw through. They show
/// the grid. They also carry what the editor draws over the scene: the gizmo
/// handles and the selection outlines. Turning the grid off must not remove
/// them from the list. The frame carries the list once, as its editor views.
pub fn editor_view_surfaces(surface_cameras: &SurfaceCameras) -> Vec<SurfaceId> {
    view_bound_surfaces(&surface_cameras.bindings)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // External crates
    use pill_engine::common_components::TransformComponent;

    /// Builds a binding to a view camera, the way an editor binds its own
    /// viewport.
    fn view_binding() -> CameraBinding {
        CameraBinding::View {
            camera: crate::CameraComponent::default(),
            transform: TransformComponent::default(),
        }
    }

    /// The grid is off by default, and a grid that is off gives no spacing.
    #[test]
    fn a_disabled_grid_gives_no_spacing() {
        assert!(!SceneGrid::default().enabled);
        let mut cameras = SurfaceCameras::default();
        cameras.set(SurfaceId(1), Some(view_binding()));
        assert_eq!(frame_grid(&cameras), None);
    }

    /// With the grid on, only the surfaces bound to a view camera draw it; a
    /// surface bound to an entity never does.
    #[test]
    fn only_view_bound_surfaces_draw_the_grid() {
        let mut world = pill_engine::World::new();
        let camera_entity = world.reserve_entity();
        let mut cameras = SurfaceCameras::default();
        cameras.set(SurfaceId(1), Some(view_binding()));
        cameras.set(SurfaceId(2), Some(view_binding()));
        cameras.set(SurfaceId(3), Some(CameraBinding::Entity(camera_entity)));
        cameras.grid = SceneGrid {
            enabled: true,
            cell_size: 0.5,
        };
        assert_eq!(frame_grid(&cameras), Some(0.5));
        assert_eq!(
            editor_view_surfaces(&cameras),
            vec![SurfaceId(1), SurfaceId(2)]
        );
    }

    /// The primary surface draws the grid when a frontend bound it to a view
    /// camera.
    #[test]
    fn a_view_bound_primary_draws_the_grid() {
        let mut cameras = SurfaceCameras::default();
        cameras.set(SurfaceId::PRIMARY, Some(view_binding()));
        cameras.grid = SceneGrid {
            enabled: true,
            cell_size: 1.0,
        };
        assert_eq!(frame_grid(&cameras), Some(1.0));
        assert_eq!(editor_view_surfaces(&cameras), vec![SurfaceId::PRIMARY]);
    }

    /// No view binding gives no grid, and a cell size the shader cannot use
    /// becomes the default.
    #[test]
    fn an_empty_binding_set_draws_nothing_and_a_bad_cell_falls_back() {
        let mut cameras = SurfaceCameras {
            grid: SceneGrid {
                enabled: true,
                cell_size: 1.0,
            },
            ..SurfaceCameras::default()
        };
        assert_eq!(frame_grid(&cameras), None);
        cameras.set(SurfaceId(1), Some(view_binding()));
        for cell_size in [0.0, -2.0, f32::NAN] {
            cameras.grid = SceneGrid {
                enabled: true,
                cell_size,
            };
            assert_eq!(frame_grid(&cameras), Some(DEFAULT_CELL_SIZE));
        }
    }
}
