//! Echoes the input a frontend delivers and zooms the camera with the wheel.
//!
//! # Responsibilities
//!
//! - Log every key that went down this frame, every mouse button that did, and
//!   the camera distance after a scroll.
//! - Move the camera along its viewing axis when the wheel turns, clamped to a
//!   range the scene still fits in.
//!
//! # Design
//!
//! The lines are the observation point the web suite matches on: an input that
//! never reached the game leaves one of them absent, and no frame-based check
//! can tell "the frontend forwards input" from "the scene is static". They are
//! written here, in the project, because that is what proves the event reached
//! the world rather than the frontend's own log.

use pill_engine::{system, Input, MouseButton, Query, Res, SystemError};
use pill_rendering::{CameraComponent, TransformComponent};

/// How far one wheel line moves the camera, in world units.
const ZOOM_STEP: f32 = 0.5;

/// Physical pixels that count as one wheel line.
///
/// A windowed frontend reports notched wheels in lines and a page reports them
/// in pixels; the zoom reads both so the same gesture moves the camera by the
/// same amount in either frontend.
const PIXELS_PER_LINE: f32 = 50.0;

/// The closest the camera may come to the scene.
const MINIMUM_DISTANCE: f32 = 1.5;

/// The farthest the camera may go.
const MAXIMUM_DISTANCE: f32 = 24.0;

/// Logs the keys and mouse buttons that went down since the previous frame.
#[system(name = "input_echo", order = 520)]
pub(crate) fn input_echo_system(input: Res<Input>) -> Result<(), SystemError> {
    let Some(input) = input.get() else {
        return Ok(());
    };
    for key in input.keys_pressed() {
        pill_engine::tracing::debug!(target: "project", "key pressed: {key:?}");
    }
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        if input.mouse_button_pressed(button) {
            pill_engine::tracing::debug!(target: "project", "mouse button pressed: {button:?}");
        }
    }
    Ok(())
}

/// Zooms the camera by the wheel and reports the distance it settled at.
///
/// The distance is the camera's translation along Z, which is the axis the
/// camera looks down: the scene's camera is placed by its Z and rotated only
/// afterwards by whatever tool the editor uses.
#[system(name = "camera_zoom", order = 530)]
pub(crate) fn camera_zoom_system(
    input: Res<Input>,
    mut cameras: Query<(&mut TransformComponent, &CameraComponent)>,
) -> Result<(), SystemError> {
    let Some(input) = input.get() else {
        return Ok(());
    };
    let lines = input.scroll_lines().y + input.scroll_pixels().y / PIXELS_PER_LINE;
    if lines == 0.0 {
        return Ok(());
    }
    for (mut transform, _) in cameras.iter_mut() {
        let distance = (transform.translation[2] - lines * ZOOM_STEP)
            .clamp(MINIMUM_DISTANCE, MAXIMUM_DISTANCE);
        transform.translation[2] = distance;
        pill_engine::tracing::debug!(target: "project", "scrolled; camera distance {distance:.2}");
    }
    Ok(())
}
