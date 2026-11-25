//! The renderer's frame system: it gathers what the world holds and hands it to
//! `pill_rendering_core`, which builds the frame.
//!
//! # Responsibilities
//!
//! - Declare the post-update `rendering` system, and register it under the
//!   renderer's own owner (decision 7: a renderer declares its own systems).
//! - Read every resource and query the frame needs, turn each into a plain
//!   value, and call [`pill_rendering_core::frame::update_frame`].
//! - Ask the UI producer for its textures again the first time this renderer
//!   image finds a UI slot, because a rebuilt painter starts empty while the
//!   producer outlives it.
//!
//! # Design
//!
//! Nothing in this module decides what the frame contains. The chain
//! resolution, the camera choice, the instances and the clocks live in
//! `pill_rendering_core`, shared with every other renderer; this module is the
//! half that knows about *this* world - which resources exist, and how a query
//! hands its rows over.
//!
//! [`RenderFrame`], [`RenderInstance`] and [`ResolvedPass`] are plain data from
//! `pill_renderer_api`, re-exported here.

// Standard library
use std::sync::atomic::{AtomicBool, Ordering};

// External crates
use pill_engine::{system, AssetManager, Entity, Query, Res, ResMut, SystemError};
pub use pill_renderer_api::frame::{
    CullMode, PassKind, PassTarget, RenderFrame, RenderInstance, ResolvedPass,
};

// Current crate
use crate::{
    components::{CameraComponent, MeshRendererComponent, TransformComponent},
    resources::RenderingManager,
};

// =============================================================================
// Constants
// =============================================================================

/// Whether this renderer image has already asked the UI producer to send its
/// textures again.
///
/// A reload gives the module a fresh image with fresh statics, so the flag
/// answers "has this image asked yet?". It starts false, so the first frame
/// that finds a UI slot asks for every texture again.
static UI_TEXTURES_REQUESTED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Types
// =============================================================================

/// The resources [`rendering_system`] reads besides the frame, the assets and
/// the rendering manager.
///
/// They share one parameter because a system function takes at most seven.
/// They include the world's only UI producer: the slot a game's own UI
/// publishes into ([`UiFrameSlot`](pill_ui_frame::UiFrameSlot)). A world that
/// never installed a slot has no UI.
type RenderingExtras<'w> = (
    Res<'w, pill_engine::Time>,
    Res<'w, pill_renderer_api::SurfaceCameras>,
    ResMut<'w, pill_renderer_api::DebugDraw>,
    Res<'w, pill_renderer_api::editor::SelectionOutlines>,
    ResMut<'w, pill_renderer_api::editor::PickRequests>,
    ResMut<'w, pill_ui_frame::UiFrameSlot>,
    (
        ResMut<'w, pill_renderer_api::editor::PreviewRequests>,
        Res<'w, pill_renderer_api::RayTracedShadows>,
    ),
);

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the shared names of the components this renderer's frame system
/// reads.
///
/// The names come from the component types themselves rather than from string
/// literals, so they cannot drift from the types. Keep the list in step with
/// the system's query parameters.
pub(crate) use pill_rendering_core::frame::consumed_component_names;

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
        (mut preview_requests, ray_traced_shadows),
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
    // frame's step. Empty while debug drawing is off, and when the world has no
    // debug drawing resource. A world with no clock keeps the step the frame
    // already carries.
    let delta_seconds = time.get().map(|time| time.delta_seconds());
    let debug = debug_draw
        .get_mut()
        .map(|mut debug_draw| {
            debug_draw.collect_frame(delta_seconds.unwrap_or(frame.delta_seconds))
        })
        .unwrap_or_default();
    // The selection outlines an editor asked for, copied from the resource. A
    // world without the resource outlines nothing.
    let outlines = outlines
        .get()
        .cloned()
        .unwrap_or_else(pill_renderer_api::editor::SelectionOutlines::default);
    // Pick requests queued since the last frame. The renderer answers them
    // after it draws.
    let pick_requests = pick_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    // Preview requests, taken the same way. The renderer draws at most one per
    // frame, oldest first, and refuses the rest.
    let preview_requests = preview_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    // The frame's UI comes from the world's only producer. A rebuilt painter
    // starts with an empty texture map, while the producer, which outlives this
    // image, has already sent every image once: ask for all of them again the
    // first time this image finds a slot.
    let ui = match ui_slot.get_mut() {
        Some(mut slot) => {
            if !UI_TEXTURES_REQUESTED.swap(true, Ordering::Relaxed) {
                slot.request_texture_resend();
            }
            Some(slot.take())
        }
        None => None,
    };

    // The pipelines owners appended after every surface's chain. A world with
    // no manager appends nothing, so the frame carries the chain alone.
    let append_pipelines: Vec<pill_rendering::AppendPipeline> = manager
        .get()
        .map(|manager| manager.append_pipelines().cloned().collect())
        .unwrap_or_default();

    pill_rendering_core::frame::update_frame(
        &mut frame,
        pill_rendering_core::frame::FrameInput {
            assets,
            pipeline: manager.get().and_then(|manager| manager.pipeline()),
            append_pipelines: &append_pipelines,
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
    // Whether the game wants ray-traced shadows. A world that never inserts the
    // switch takes its default, which is shadows traced; a renderer whose
    // device cannot trace draws today's shading whatever the flag says.
    frame.ray_traced_shadows = ray_traced_shadows.get().map_or_else(
        || pill_renderer_api::RayTracedShadows::default().enabled,
        |switch| switch.enabled,
    );
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_engine::Engine;

    /// Builds an engine the rendering system can run in: the frame and the
    /// asset store are inserted as resources, and the artifact entry point
    /// registers the declared `rendering` system.
    fn renderable_engine() -> Engine {
        let mut engine = Engine::new();
        engine.world_mut().insert_resource(RenderFrame::default());
        engine.world_mut().insert_resource(AssetManager::new());
        // The system is declared with `#[system]`, so it reaches the engine
        // through the artifact entry point. `register` is the half a reload
        // repeats, and it declares no system of its own.
        let status = pill_engine::component_registry::run_artifact_registration(
            &mut engine,
            crate::register,
        );
        assert_eq!(status, 0, "the renderer's registration reports success");
        engine
    }

    /// Builds a frame with one filled quad, the way a game's UI producer
    /// publishes it. It is made from `pill_ui_frame`'s draw vocabulary, which is
    /// the plain data the slot carries.
    fn drawn_frame() -> RenderFrame {
        let mut frame = RenderFrame::default();
        let mut list = pill_ui_frame::DrawList::new();
        list.push_quad(
            pill_ui_frame::TextureId(1),
            pill_ui_frame::Rect::from_min_size(
                pill_core::math::Vector2f::ZERO,
                pill_core::math::Vector2f::new(2.0, 2.0),
            ),
            pill_core::math::Vector2f::ZERO,
            pill_core::math::Vector2f::new(1.0, 1.0),
            pill_ui_frame::Color::WHITE,
        );
        frame.ui.draws.push(pill_ui_frame::UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list,
        });
        frame
    }

    #[test]
    fn a_published_frame_reaches_the_renderer() {
        let mut engine = renderable_engine();
        engine
            .world_mut()
            .insert_resource(pill_ui_frame::UiFrameSlot::new());
        {
            let slot = engine
                .world_mut()
                .get_resource_mut::<pill_ui_frame::UiFrameSlot>()
                .expect("inserted above");
            slot.publish(drawn_frame().ui);
        }
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert_eq!(
            frame.ui.draws.len(),
            1,
            "the published frame did not reach the renderer"
        );
        let slot = engine
            .world()
            .get_resource::<pill_ui_frame::UiFrameSlot>()
            .expect("inserted above");
        assert!(slot.is_empty(), "the frame was copied rather than drained");
    }

    #[test]
    fn a_slot_that_drew_nothing_still_owns_the_frame() {
        let mut engine = renderable_engine();
        engine
            .world_mut()
            .insert_resource(pill_ui_frame::UiFrameSlot::new());
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert!(
            frame.ui.is_empty(),
            "a game that drew nothing publishes an empty frame, not a fallback one"
        );
    }

    #[test]
    fn a_world_without_a_slot_carries_no_ui() {
        let mut engine = renderable_engine();
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert!(
            frame.ui.draws.is_empty(),
            "a world that never installed a slot has no UI to draw"
        );
        assert!(
            frame.ui.textures.textures.is_empty(),
            "and no textures either"
        );
    }
}
