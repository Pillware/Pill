//! The demo's debug overlay. It draws the field's bounds, the world axes and
//! the tuning values through the engine's `DebugDraw`.
//!
//! # Responsibilities
//!
//! - Draw the volume the pills spawn in, the axes at the origin, and a label
//!   for the origin and for the field, every frame ([`debug_overlay_system`]).
//! - Let F3 turn debug drawing on and off at runtime, in any build.

// External crates
use pill_engine::{pill_hot_fn, Input, KeyCode, Res, ResMut, SystemError};
use pill_rendering::{DebugDepth, DebugDraw, DebugStyle};

// Current crate
use crate::game::DemoState;

/// The key that turns debug drawing on and off.
const DEBUG_DRAW_TOGGLE_KEY: KeyCode = KeyCode::F3;

/// Center of the volume the pills spawn in.
const FIELD_CENTER: [f32; 3] = [0.0, 20.0, 0.0];
/// Half the volume's size on each axis.
const FIELD_HALF_EXTENTS: [f32; 3] = [20.0, 20.0, 20.0];

/// How long each axis drawn at the origin is.
const AXIS_LENGTH_WORLD_UNITS: f32 = 3.0;

/// Draws the field's bounds, the world axes and the tuning values, and lets F3
/// turn debug drawing on and off.
#[pill_hot_fn]
pub(crate) fn debug_overlay_system(
    input: Res<Input>,
    mut debug_draw: ResMut<DebugDraw>,
    demo_state: Res<DemoState>,
) -> Result<(), SystemError> {
    // Step 1: Get the resource, and toggle debug drawing when F3 is pressed.
    // A world without the resource has nothing to draw into.
    let Some(mut debug_draw) = debug_draw.get_mut() else {
        return Ok(());
    };
    if input
        .get()
        .is_some_and(|input| input.key_pressed(DEBUG_DRAW_TOGGLE_KEY))
    {
        let enabled = !debug_draw.is_enabled();
        debug_draw.set_enabled(enabled);
    }
    if !debug_draw.is_enabled() {
        return Ok(());
    }

    // Step 2: Draw the field's bounds, the axes and the two labels.
    debug_draw.aabb(
        FIELD_CENTER,
        FIELD_HALF_EXTENTS,
        DebugStyle::colored([0.3, 0.8, 1.0, 1.0]),
    );
    // The axes and labels draw over everything, so the pills never hide them.
    let on_top = |color| DebugStyle {
        depth: DebugDepth::OnTop,
        ..DebugStyle::colored(color)
    };
    debug_draw
        .arrow(
            [0.0; 3],
            [AXIS_LENGTH_WORLD_UNITS, 0.0, 0.0],
            on_top([1.0, 0.2, 0.2, 1.0]),
        )
        .arrow(
            [0.0; 3],
            [0.0, AXIS_LENGTH_WORLD_UNITS, 0.0],
            on_top([0.2, 1.0, 0.2, 1.0]),
        )
        .arrow(
            [0.0; 3],
            [0.0, 0.0, AXIS_LENGTH_WORLD_UNITS],
            on_top([0.3, 0.5, 1.0, 1.0]),
        )
        .label_world([0.0, 0.0, 0.0], "origin", on_top([1.0; 4]))
        .label_world(
            [
                FIELD_CENTER[0] - FIELD_HALF_EXTENTS[0],
                FIELD_CENTER[1] + FIELD_HALF_EXTENTS[1],
                FIELD_CENTER[2] - FIELD_HALF_EXTENTS[2],
            ],
            "curl noise field",
            on_top([0.3, 0.8, 1.0, 1.0]),
        );

    // Step 3: Build the status line and draw it in the corner.
    let status_text = match demo_state.get() {
        Some(state) => format!(
            "pills - F3: debug draw\ncurl scale {:.4}  attraction {:.2}\ndamping {:.4}  epsilon {:.4}",
            state.curl_scale, state.curl_attraction, state.curl_damping, state.curl_epsilon
        ),
        None => String::from("pills - F3: debug draw"),
    };
    debug_draw.label_screen(16.0, 16.0, &status_text, on_top([1.0, 0.9, 0.4, 1.0]));
    Ok(())
}
