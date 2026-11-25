//! The demo's two overlays: the egui tuning panel, drawn through the engine's
//! game UI, and the debug drawing.
//!
//! # Responsibilities
//!
//! - [`tuning_overlay_system`]: one egui pass per frame - the FPS readout, the
//!   tuning values and the button that resets them.
//! - [`debug_overlay_system`]: the field's box, the world axes and a status
//!   label, toggled with F3 in any build.
//!
//! # Design
//!
//! Both are ordinary project code, so an edit to either live-patches the
//! running demo. They are deliberately self-contained - no `crate::` paths and
//! no file-level constants - because a fast patch copies the file's imports
//! and the edited body into a small crate that links this library.
//!
//! The egui pass runs in the context that the [`GameUi`] resource owns, not in
//! a fresh one, and that is what lets the panel survive a project reload: its
//! fonts, its widget state and its textures live in a shared library, so the
//! reloaded project finds them exactly as they were. The tuning values live in
//! [`DemoState`]: the demo's input system changes them from the keys, the panel
//! displays them, and the panel's reset button restores the defaults.

// External crates
use pill_core::math::Vector2f;
use pill_engine::{pill_hot, system, Input, KeyCode, Res, ResMut, SystemError, Time};
use pill_rendering::{DebugDepth, DebugDraw, DebugStyle};
// The egui this engine uses, reached through the crate that re-exports it: a
// project never depends on egui directly, so every artifact shares one copy.
use pill_project_ui::{egui, GameUi, InputFrame, UiFrameSlot, PRIMARY_SURFACE};

// Current crate
use pills_rust::{DemoState, PILL_COUNT};

/// Draws the tuning panel: the FPS, the curl noise values and the reset button.
#[system(name = "tuning_overlay", order = 540)]
pub(crate) fn tuning_overlay_system(
    mut game_ui: ResMut<GameUi>,
    input: Res<Input>,
    mut ui_frame_slot: ResMut<UiFrameSlot>,
    time: Res<Time>,
    mut demo_state: ResMut<DemoState>,
) -> Result<(), SystemError> {
    let (Some(mut game_ui), Some(input), Some(mut ui_frame_slot), Some(time), Some(mut demo_state)) = (
        game_ui.get_mut(),
        input.get(),
        ui_frame_slot.get_mut(),
        time.get(),
        demo_state.get_mut(),
    ) else {
        return Ok(());
    };

    // Capture this frame's input as an owned snapshot. The UI pass takes an
    // `InputFrame` rather than the engine's `Input` resource, so the system
    // builds the snapshot once and hands it to the pass.
    let input_frame = InputFrame::capture(input);
    // The theme the panel's widgets paint with, cloned out of the resource
    // because the pass borrows the UI mutably. The theme can be re-applied
    // while the demo runs, so the panel reads it again every frame.
    let theme = game_ui.theme().clone();

    // The panel lays out against its own size and sits in the window's top-left
    // corner, so it needs no window-size input. A scale factor of 1.0 draws one
    // logical point as one screen pixel, so the panel keeps its 320 by 260 size.
    game_ui.run(
        &mut ui_frame_slot,
        &input_frame,
        Vector2f::new(320.0, 260.0),
        1.0,
        PRIMARY_SURFACE,
        |ui| {
            egui::Window::new("Pills")
                .default_pos(egui::pos2(12.0, 12.0))
                .resizable(false)
                .collapsible(false)
                .show(ui.ctx(), |window| {
                    window.label(format!(
                        "{:.0} FPS | {} pills",
                        1.0 / time.delta_seconds().max(1e-6),
                        PILL_COUNT
                    ));
                    window.separator();
                    window.collapsing("Curl noise", |section| {
                        section.label(format!("Scale {:.4}", demo_state.curl_scale));
                        section.label(format!("Attraction {:.0}", demo_state.curl_attraction));
                        section.label(format!("Damping {:.2}", demo_state.curl_damping));
                        section.label(format!("Epsilon {:.4}", demo_state.curl_epsilon));
                    });
                    window.separator();
                    window.label("O/P scale  L/K attraction  M/N damping  V/B epsilon");
                    if window
                        .add(
                            pill_project_ui::kit::ChunkyButton::new("Reset curls", &theme)
                                .accent(true),
                        )
                        .clicked()
                    {
                        *demo_state = DemoState::default();
                    }
                });
        },
    );
    Ok(())
}

/// Draws the field's bounds, the world axes and the tuning values, and toggles
/// debug drawing with F3.
#[system(name = "debug_overlay", order = 550)]
pub(crate) fn debug_overlay_system(
    input: Res<Input>,
    mut debug_draw: ResMut<DebugDraw>,
    demo_state: Res<DemoState>,
) -> Result<(), SystemError> {
    // A world without the resource has nothing to draw into.
    let Some(mut debug_draw) = debug_draw.get_mut() else {
        return Ok(());
    };
    if input
        .get()
        .is_some_and(|input| input.key_pressed(KeyCode::F3))
    {
        let enabled = !debug_draw.is_enabled();
        debug_draw.set_enabled(enabled);
    }
    if !debug_draw.is_enabled() {
        return Ok(());
    }

    // The volume the pills spawn in.
    let (field_center, field_half_extents) = ([0.0, 20.0, 0.0], [20.0, 20.0, 20.0]);
    debug_draw.aabb(
        field_center,
        field_half_extents,
        DebugStyle::colored([0.3, 0.8, 1.0, 1.0]),
    );

    // The axes and labels draw over the scene, so they stay visible in the swarm.
    let on_top_style = |color| DebugStyle {
        depth: DebugDepth::OnTop,
        ..DebugStyle::colored(color)
    };
    debug_draw
        .arrow(
            [0.0; 3],
            [3.0, 0.0, 0.0],
            on_top_style([1.0, 0.2, 0.2, 1.0]),
        )
        .arrow(
            [0.0; 3],
            [0.0, 3.0, 0.0],
            on_top_style([0.2, 1.0, 0.2, 1.0]),
        )
        .arrow(
            [0.0; 3],
            [0.0, 0.0, 3.0],
            on_top_style([0.3, 0.5, 1.0, 1.0]),
        )
        .label_world([0.0; 3], "origin", on_top_style([1.0; 4]))
        .label_world(
            [
                field_center[0] - field_half_extents[0],
                field_center[1] + field_half_extents[1],
                field_center[2] - field_half_extents[2],
            ],
            "curl noise field",
            on_top_style([0.3, 0.8, 1.0, 1.0]),
        );

    if let Some(demo_state) = demo_state.get() {
        let status = format!(
            "pills - F3: debug draw\ncurl scale {:.4}  attraction {:.2}\ndamping {:.4}  epsilon {:.4}",
            demo_state.curl_scale,
            demo_state.curl_attraction,
            demo_state.curl_damping,
            demo_state.curl_epsilon
        );
        debug_draw.label_screen(16.0, 16.0, &status, on_top_style([1.0, 0.9, 0.4, 1.0]));
    }
    Ok(())
}
