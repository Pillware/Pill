//! The embedded renderer's frame system: it gathers what the world holds and
//! hands it to `pill_rendering_core`, which builds the frame.
//!
//! # Responsibilities
//!
//! - Declare the post-update `rendering` system, and register it under the
//!   renderer's own owner.
//! - Read every resource and query the frame needs, turn each into a plain
//!   value, and call [`pill_rendering_core::frame::update_frame`].
//! - Ask the UI producer for its textures again the first time this renderer
//!   image finds a UI slot, because a rebuilt renderer starts empty while the
//!   producer outlives it.
//!
//! # Design
//!
//! This is the CPU renderer's half of the same contract the GPU renderer
//! implements: the frame building itself lives in `pill_rendering_core`, and
//! this module only knows which resources exist in the world and how a query
//! hands its rows over. Each renderer declares its own system, so no module
//! loads the other renderer's code.

// Standard library
use std::sync::atomic::{AtomicBool, Ordering};

// External crates
use pill_engine::{system, AssetManager, Entity, Query, Res, ResMut, SystemError};
use pill_renderer_api::{RenderFrame, SurfaceCameras};
use pill_rendering::RenderingManager;

// Current crate
use crate::components::{CameraComponent, MeshRendererComponent, TransformComponent};

// =============================================================================
// Constants
// =============================================================================

/// Whether this renderer image has already asked the UI producer to send its
/// textures again.
///
/// A reload gives the module a fresh image with fresh statics, so the flag
/// answers "has this image asked yet?".
static UI_TEXTURES_REQUESTED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Types
// =============================================================================

/// The resources [`rendering_system`] reads besides the frame, the assets and
/// the rendering manager.
///
/// They share one parameter because a system function takes at most seven.
type RenderingExtras<'w> = (
    Res<'w, pill_engine::Time>,
    Res<'w, SurfaceCameras>,
    ResMut<'w, pill_engine::DebugDraw>,
    Res<'w, pill_renderer_api::SelectionOutlines>,
    ResMut<'w, pill_renderer_api::PickRequests>,
    ResMut<'w, pill_ui_frame::UiFrameSlot>,
    ResMut<'w, pill_renderer_api::PreviewRequests>,
);

// =============================================================================
// Free Functions
// =============================================================================

/// Fills the frame from the current world, once per update.
///
/// It runs as the post-update `rendering` system, so gameplay has already
/// written the components it reads. A world missing the frame resource or the
/// asset store is not an error: the system returns and draws nothing.
#[system(
    name = "rendering",
    run_phase = "post_project",
    run_in_edit_mode = true
)]
pub fn rendering_system(
    mut frame: ResMut<RenderFrame>,
    assets: Res<AssetManager>,
    manager: Res<RenderingManager>,
    (
        time,
        surface_cameras,
        mut debug_draw,
        outlines,
        mut pick_requests,
        mut ui_slot,
        mut preview_requests,
    ): RenderingExtras<'_>,
    mut cameras: Query<(Entity, &TransformComponent, &CameraComponent)>,
    mut drawables: Query<(Entity, &TransformComponent, &MeshRendererComponent)>,
) -> Result<(), SystemError> {
    let Some(mut frame) = frame.get_mut() else {
        return Ok(());
    };
    let Some(assets) = assets.get() else {
        return Ok(());
    };

    // The debug geometry systems drew, with the timed items aged by this
    // frame's step.
    let delta_seconds = time.get().map(|time| time.delta_seconds());
    let debug = debug_draw
        .get_mut()
        .map(|mut debug_draw| {
            debug_draw.collect_frame(delta_seconds.unwrap_or(frame.delta_seconds))
        })
        .unwrap_or_default();
    let outlines = outlines
        .get()
        .cloned()
        .unwrap_or_else(pill_renderer_api::SelectionOutlines::default);
    // Pick requests are taken but never answered: a processor renderer has no
    // pick buffer, which is why it reports no pick results.
    let pick_requests = pick_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    let preview_requests = preview_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    // The frame's UI comes from the world's only producer. A rebuilt renderer
    // starts with an empty texture map while the producer has already sent
    // every image once: ask for all of them again the first time this image
    // finds a slot.
    let ui = match ui_slot.get_mut() {
        Some(mut slot) => {
            if !UI_TEXTURES_REQUESTED.swap(true, Ordering::Relaxed) {
                slot.request_texture_resend();
            }
            Some(slot.take())
        }
        None => None,
    };

    pill_rendering_core::frame::update_frame(
        &mut frame,
        pill_rendering_core::frame::FrameInput {
            assets,
            pipeline: manager.get().and_then(|manager| manager.pipeline()),
            delta_seconds,
            debug,
            outlines,
            pick_requests,
            preview_requests,
            surface_cameras: surface_cameras.get(),
            ui,
        },
        cameras
            .iter_mut()
            .map(|(entity, transform, camera)| (entity, *camera, *transform)),
        drawables
            .iter_mut()
            .map(|(entity, transform, renderer)| (entity, *transform, *renderer)),
    );
    Ok(())
}
