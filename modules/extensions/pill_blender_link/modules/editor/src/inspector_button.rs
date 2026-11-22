//! The Blender link's buttons on the mesh renderer's Inspector header.
//!
//! # Responsibilities
//!
//! - [`blender_button`]: the Blender logo, with a dot for the link's state
//!   (waiting, connected, streaming, disconnected), a dot for unsaved
//!   changes, and hover text that says what the link is doing.
//! - [`save_button`]: the save action, shown once the mesh is linked and
//!   usable while it holds unsaved changes.
//! - [`paint_blender_logo`]: the logo itself, painted from shapes so the
//!   extension ships no image.
//!
//! # Design
//!
//! A draw function cannot read the world, so everything it shows comes from
//! the [`CommandState`] the link's system published for the entity this frame.

// External crates
use pill_editor_core::egui;
use pill_editor_core::icons;
use pill_editor_core::placements::{CommandDrawContext, CommandState};
use pill_editor_core::style;

// Current crate
use crate::link_update::OPEN_BUTTON_TOOLTIP;

// =============================================================================
// Constants
// =============================================================================

/// The logo's orange.
const LOGO_ORANGE: egui::Color32 = egui::Color32::from_rgb(0xEA, 0x76, 0x00);
/// The logo's blue.
const LOGO_BLUE: egui::Color32 = egui::Color32::from_rgb(0x26, 0x57, 0x87);

/// The side of each button, in points; the Inspector reserves this much.
const BUTTON_SIZE: f32 = 16.0;

/// The radius of a status dot, in points.
const DOT_RADIUS: f32 = 2.6;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the Blender button and reports a click on it.
///
/// Registered as the command's draw function, so the shell calls it while it
/// draws the header of every `MeshRendererComponent`.
pub fn blender_button(ui: &mut egui::Ui, context: &mut CommandDrawContext<'_>) {
    let state = context.state;
    let enabled = state.is_none_or(|state| state.enabled);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), egui::Sense::click());
    if response.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    paint_blender_logo(ui.painter(), rect.shrink(1.0));
    if let Some(state) = state {
        paint_status_dots(ui.painter(), rect, state);
    }
    let response = with_status_tooltip(response, state, OPEN_BUTTON_TOOLTIP);
    if response.clicked() && enabled {
        context.invoke();
    }
}

/// Draws the save button for a linked mesh and reports a click on it.
///
/// A mesh that was never linked publishes a hidden state, so the shell never
/// calls this for it.
pub fn save_button(ui: &mut egui::Ui, context: &mut CommandDrawContext<'_>) {
    let state = context.state;
    let enabled = state.is_none_or(|state| state.enabled);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(BUTTON_SIZE, BUTTON_SIZE), egui::Sense::click());
    if response.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon_color = if enabled {
        style::text_strong()
    } else {
        style::text_muted()
    };
    icons::paint_duotone(
        ui.painter(),
        rect.shrink(1.0),
        icons::Icon::FloppyDisk,
        icon_color,
    );
    if let Some(state) = state {
        paint_status_dots(ui.painter(), rect, state);
    }
    let response = with_status_tooltip(response, state, "Save");
    if response.clicked() && enabled {
        context.invoke();
    }
}

/// Paints a state's dots: the status in the lower-right corner, unsaved work
/// in the upper-right one.
pub fn paint_status_dots(painter: &egui::Painter, rect: egui::Rect, state: &CommandState) {
    let ring_color = style::panel();
    let paint_dot = |center: egui::Pos2, rgba: [u8; 4]| {
        painter.circle_filled(center, DOT_RADIUS + 1.0, ring_color);
        painter.circle_filled(
            center,
            DOT_RADIUS,
            egui::Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3]),
        );
    };
    if let Some(indicator) = state.indicator {
        paint_dot(
            rect.right_bottom() - egui::vec2(DOT_RADIUS, DOT_RADIUS),
            indicator,
        );
    }
    if let Some(attention) = state.attention {
        paint_dot(
            egui::pos2(rect.max.x - DOT_RADIUS, rect.min.y + DOT_RADIUS),
            attention,
        );
    }
}

/// Adds the state's hover text, one line per entry, or `fallback` when the
/// module published no state.
pub fn with_status_tooltip(
    response: egui::Response,
    state: Option<&CommandState>,
    fallback: &str,
) -> egui::Response {
    let enabled = state.is_none_or(|state| state.enabled);
    let response = if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    };
    match state {
        Some(state)
            if state
                .tooltip
                .as_ref()
                .is_some_and(|lines| !lines.is_empty()) =>
        {
            let text = state
                .tooltip
                .as_ref()
                .map(|lines| lines.join("\n"))
                .unwrap_or_default();
            response.on_hover_text(text)
        }
        _ => response.on_hover_text(fallback),
    }
}

/// Paints the Blender logo into `rect`: the orange body with its three
/// prongs reaching to the upper left, and the white and blue eye.
pub fn paint_blender_logo(painter: &egui::Painter, rect: egui::Rect) {
    let size = rect.width().min(rect.height());
    let radius = size * 0.3;
    let eye_center = rect.center() + egui::vec2(size * 0.16, size * 0.1);
    let point = |x_factor: f32, y_factor: f32| {
        eye_center + egui::vec2(x_factor * radius, y_factor * radius)
    };
    let prong_stroke = egui::Stroke::new(radius * 0.55, LOGO_ORANGE);
    // The prongs: a long one over the top, a middle one, a short one below.
    painter.line_segment([point(-0.2, -0.75), point(-1.75, -1.55)], prong_stroke);
    painter.line_segment([point(-0.6, -0.2), point(-1.6, -0.2)], prong_stroke);
    painter.line_segment([point(-0.55, 0.45), point(-1.1, 0.95)], prong_stroke);
    // The body and the eye.
    painter.circle_filled(eye_center, radius, LOGO_ORANGE);
    painter.circle_filled(eye_center, radius * 0.62, egui::Color32::WHITE);
    painter.circle_filled(eye_center, radius * 0.38, LOGO_BLUE);
}
