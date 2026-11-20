//! The tuning overlay: the demo's in-game egui UI, drawn through the engine.
//!
//! # Responsibilities
//!
//! - [`tuning_overlay_system`]: one egui pass per frame, in a panel in the
//!   window's top-left corner - the FPS readout, the curl-noise tuning values
//!   and the button that resets them.
//!
//! # Design
//!
//! The overlay is ordinary project code: a `#[pill_hot_fn]` function behind an
//! annotated system, so an edit to this function live-patches the running demo.
//! It deliberately has no
//! `crate::` imports and no file-level constants. A generated fast patch
//! replays the file's imports into a synthetic crate and copies only the
//! function, so either shape would fail to compile there and cost the edit a
//! full reload.
//!
//! The pass runs inside the [`GameUi`] resource instead of building a fresh
//! context each frame, which is what makes the overlay survive a project
//! reload: the context, with its fonts, its widget state (collapsed sections,
//! focus) and its textures, lives in a shared library, so the reloaded project
//! finds them exactly as they were. `init` installs that resource and registers
//! its type; this system only draws.
//!
//! The tuning values live in [`DemoState`]. The demo's own input system changes
//! them from the tuning keys, and the overlay only displays them. The Reset
//! curls button is the one other writer: it puts the defaults back.
//!
//! The frame reaches the renderer as `pill_ui_frame` data: a draw list and a
//! texture set. Nothing in the rendering path names egui.

// External crates
use pill_core::math::Vector2f;
use pill_engine::{pill_hot_fn, Input, Res, ResMut, SystemError, Time};
// The egui version this engine pins, named through the crate that owns it. A
// project never depends on egui directly, so every artifact shares the one
// copy.
use pill_project_ui::{egui, GameUi, InputFrame, UiFrameSlot, PRIMARY_SURFACE};

// Current crate
// Named through the package name rather than `crate::`. A generated hot patch
// replays this file's imports into a synthetic crate, where only
// `pills_rust::...` resolves because it links this rlib.
use pills_rust::{DemoState, PILL_COUNT};

/// Draws the tuning overlay for this frame.
#[pill_hot_fn]
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

    // The engine's input snapshot, captured before the pass. `run` needs the UI
    // resource mutably, so the snapshot is taken first and the borrow of the
    // engine's input ends with it.
    let input_frame = InputFrame::capture(input);

    // The theme the panel's widgets paint with, copied out of the resource
    // because the pass borrows the UI mutably. A project reload installs the
    // theme again, so this copy only lives for the frame.
    let theme = game_ui.theme().clone();

    // The overlay lays itself out at its own size in the window's top-left
    // corner, so it needs no window size. The renderer applies the monitor's
    // scale factor, which is why 1.0 is passed here.
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
                    window.label(format!("{:.0}s uptime", time.elapsed_seconds()));
                    window.separator();
                    window.collapsing("Curl noise", |section| {
                        section.label(format!("Scale {:.4}", demo_state.curl_scale));
                        section.label(format!("Attraction {:.0}", demo_state.curl_attraction));
                        section.label(format!("Damping {:.2}", demo_state.curl_damping));
                        section.label(format!("Epsilon {:.4}", demo_state.curl_epsilon));
                    });
                    window.separator();
                    // The brutalist recipe's own button: a flat face over a
                    // hard shadow. While the button is held, the face moves
                    // down onto the shadow. The row shows one of each face -
                    // the accent one acts, the selected one marks the tuning
                    // as live, and the disabled one shows the inert look.
                    window.horizontal(|row| {
                        if row
                            .add(
                                pill_project_ui::kit::ChunkyButton::new("Reset curls", &theme)
                                    .accent(true),
                            )
                            .clicked()
                        {
                            let defaults = DemoState::default();
                            demo_state.curl_scale = defaults.curl_scale;
                            demo_state.curl_attraction = defaults.curl_attraction;
                            demo_state.curl_damping = defaults.curl_damping;
                            demo_state.curl_epsilon = defaults.curl_epsilon;
                        }
                        row.add(
                            pill_project_ui::kit::ChunkyButton::new("Tuning on", &theme)
                                .selected(true),
                        );
                        row.add_enabled(
                            false,
                            pill_project_ui::kit::ChunkyButton::new("Locked", &theme),
                        );
                    });
                    window.label("Tune: O/P, K/L, M/N, B/V");
                });
        },
    );
    Ok(())
}
