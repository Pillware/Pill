//! The surfaces an editor draws its own views through.
//!
//! # Responsibilities
//!
//! - List the surfaces bound to a view camera ([`editor_view_surfaces`]), which
//!   are the ones an editor's own viewports draw through.
//!
//! # Design
//!
//! A view camera is plain data - a camera and a transform a frontend keeps
//! beside the surface binding - so it is never an entity. A surface bound to a
//! world camera draws the game and is never an editor view. Everything an editor
//! draws over the scene (the grid, the gizmo handles and the selection
//! outlines) is drawn on this one list, which the frame carries once as its
//! editor views, so no two of them can disagree about where they belong.

// Standard library
use std::collections::BTreeMap;

// Current crate
use crate::{CameraBinding, SurfaceCameras, SurfaceId};

// =============================================================================
// Free Functions
// =============================================================================

/// Returns every surface bound to a view camera, whether or not the grid is on.
///
/// These are the surfaces an editor's own viewports draw through. They show
/// the grid. They also carry what the editor draws over the scene: the gizmo
/// handles and the selection outlines. Turning the grid off must not remove
/// them from the list. The frame carries the list once, as its editor views.
pub fn editor_view_surfaces(surface_cameras: &SurfaceCameras) -> Vec<SurfaceId> {
    view_bound_surfaces(&surface_cameras.bindings)
}

/// Returns every surface bound to a view camera, in ascending id order.
pub(super) fn view_bound_surfaces(bindings: &BTreeMap<SurfaceId, CameraBinding>) -> Vec<SurfaceId> {
    bindings
        .iter()
        .filter(|(_, binding)| matches!(binding, CameraBinding::View { .. }))
        .map(|(surface, _)| *surface)
        .collect()
}
