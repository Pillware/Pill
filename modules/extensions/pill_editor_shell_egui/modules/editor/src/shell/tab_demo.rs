//! The shell module's own editor panel: the example a module's panel is
//! written against, and the body the panel budget is tested with.
//!
//! # Responsibilities
//!
//! - [`demo_panel`]: draws the docked tab and, when the shell asks for it, the
//!   confirmation window. It reads only [`PanelContext`] and draws with egui,
//!   so it keeps nothing of this crate past the frame that drew it.
//! - The slow body: when `PILL_TEST_PANEL_SLOW` is set, the panel spends
//!   several times [`PANEL_BUDGET_MICROS`] spinning on purpose. The egui suite
//!   uses that to check the tab's badge and the budget toast.
//!
//! # Design
//!
//! This crate's `register` queues the panel through [`PanelRegistrations`]. The
//! executable then ties the request to a marker it owns, so the panel goes dead
//! when this module's image unloads. The panel keeps no state of its own: a
//! value that must survive a reload goes in a world resource, and never into
//! egui's context (see the reload-safety contract in `pill_editor_core`).

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Nothing in this file uses this import, but it has to stay: a patch crate
// copies this file's top-level `use` lines, and a sibling function the body
// names resolves through them.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

/// The environment variable that switches the panel to its deliberately slow
/// body.
pub const PANEL_SLOW_ENV: &str = "PILL_TEST_PANEL_SLOW";

/// The multiple of [`PANEL_BUDGET_MICROS`] the slow body spends spinning.
const SLOW_MULTIPLE: f32 = 3.0;

/// The demo panel's confirmation window id.
pub const DEMO_CONFIRM_WINDOW: &str = "demo.confirm";

/// The demo panel: the docked tab and the confirmation window it opens.
///
/// Public because the function inventory names it by path, and that inventory
/// is included at this crate's root, which cannot name a private function.
pub fn demo_panel(ui: &mut egui::Ui, context: &mut PanelContext<'_>) {
    // The confirmation window draws this body. The shell draws the window's
    // buttons and reads Enter and Escape, and the answer reaches the panel on
    // the next draw.
    if context.window == Some(DEMO_CONFIRM_WINDOW) {
        ui.label("Confirm the demo action?");
        ui.monospace("Enter applies it, Escape dismisses.");
        return;
    }
    ui.vertical(|ui| {
        ui.heading("Demo Panel");
        ui.label("Registered by the egui shell module itself.");
        ui.separator();
        ui.label(format!("frame rate: {:.1}", context.frames_per_second));
        match context.selected_entity {
            Some(entity) => ui.label(format!("selected entity: {entity}")),
            None => ui.label("selected entity: none"),
        };
        ui.separator();
        ui.label(
            "A module panel draws with egui and reads only plain data: the \
             dock keeps its tab, and a module that unloads reads ",
        );
        ui.monospace("module not loaded");
        ui.separator();
        if ui.button("Confirm...").clicked()
            || context.requested_window == Some(DEMO_CONFIRM_WINDOW)
        {
            context
                .commands
                .push(PanelCommand::OpenWindow(demo_confirm_window()));
        }
        if let Some(answer) = context.answer {
            if answer.window == DEMO_CONFIRM_WINDOW && answer.accepted {
                context
                    .commands
                    .push(PanelCommand::Log("demo confirmed".to_string()));
            }
        }
    });
    if slow_requested() {
        burn_micros(PANEL_BUDGET_MICROS * SLOW_MULTIPLE);
    }
}

/// Returns the confirmation window the demo panel asks for: a modal window.
pub fn demo_confirm_window() -> PanelWindowRequest {
    PanelWindowRequest {
        id: DEMO_CONFIRM_WINDOW.to_string(),
        title: "Demo confirmation".to_string(),
        width: 320.0,
        height: 120.0,
        modal: true,
    }
}

/// Returns whether `PILL_TEST_PANEL_SLOW` asks for the over-budget body. Any
/// value other than an empty string or `0` counts as on.
///
/// Public because the function inventory names it by path.
pub fn slow_requested() -> bool {
    std::env::var(PANEL_SLOW_ENV)
        .map(|value| !value.is_empty() && value != "0")
        .unwrap_or(false)
}

/// Spins on the wall clock for `micros` microseconds without doing any work,
/// which is how the slow body spends more than its frame budget.
///
/// Public because the function inventory names it by path.
pub fn burn_micros(micros: f32) {
    let started = pill_core::platform::Instant::now();
    let budget = std::time::Duration::from_micros(micros as u64);
    while started.elapsed() < budget {
        std::hint::spin_loop();
    }
}
