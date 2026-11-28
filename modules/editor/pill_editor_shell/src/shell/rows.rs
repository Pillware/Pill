//! The editor's shared row layout: a fixed label column, then the row's
//! control.
//!
//! # Responsibilities
//!
//! - [`label_column_width`]: the Inspector's label column, a share of the
//!   panel's width, so every field's control starts at one x however long its
//!   label is, and a wider panel gives the labels more room.
//! - [`ROW_LABEL_WIDTH`] and [`WIDE_ROW_LABEL_WIDTH`]: the fixed columns of the
//!   tabs whose labels do not scale with the panel, the same in every tab that
//!   uses them.
//! - [`row`] and [`row_in`]: one label and the control beside it, the default
//!   way a field is drawn.
//! - [`label_cell`] and [`label_cell_in`]: the label alone, for a row whose
//!   control is a nested run the tab builds by hand.
//! - [`label_gap`]: the rest of the space after that label cell, which a tab
//!   drawing such a row adds itself.
//! - [`value_row`] and [`value_row_in`]: a read-only row whose control is the
//!   value in monospace text.
//! - [`control_column`]: the column a row's control is drawn in, which never
//!   lets the control widen the row past the panel's edge.
//!
//! # Design
//!
//! A row is a layout, never a fill: no row paints a background, so the panel's
//! own surface shows through and rows are never striped.
//!
//! The label cell has a fixed width and starts its text at the cell's left
//! edge. That keeps a panel's labels in one column, and a label too long for
//! its column is cut off inside it instead of pushing its neighbours aside.
//!
//! A row leaves at least [`ROW_LABEL_GAP`] points between the label and its
//! control, whatever item spacing the panel uses. Rows place standard widgets
//! at their usual heights.
//!
//! The Inspector's rows - component fields and asset settings alike - take
//! the proportional column, so its controls form one column. The tabs that
//! list whole setting names (Preferences, Systems, Statistics) keep the fixed
//! wide one ([`WIDE_ROW_LABEL_WIDTH`]).
//!
//! The globs below spell absolute paths on purpose: a generated patch crate
//! replays this file's top-level `use` lines, and a `super::`-relative glob
//! would have nothing to point at there.

// Current crate
// Unused in this build, but kept on purpose: a generated patch replays this
// file's opening, and any shell sibling its body names resolves through this
// glob.
#[allow(unused_imports)]
use pill_editor_shell::shell::prelude::*;
#[allow(unused_imports)]
use pill_editor_shell::shell::*;

/// The fixed label column of the tabs that name fields without scaling with
/// the panel.
pub const ROW_LABEL_WIDTH: f32 = 80.0;

/// The narrowest the Inspector's label column gets, in points. A narrowing
/// panel takes its room from the fields, never from the labels below this.
pub const LABEL_COLUMN_MIN_WIDTH: f32 = 100.0;

/// The narrowest a field's control gets, in points, once the panel has no
/// room left beyond the label column. The control is squashed down to this and
/// its content cropped inside it.
pub const CONTROL_MIN_WIDTH: f32 = 16.0;

/// The share of a panel's width the Inspector's label column takes. The rest
/// is the control column, where every field's control starts at one x.
pub const LABEL_COLUMN_FRACTION: f32 = 0.3;

/// The widest the Inspector's label column grows, in points, so a very wide
/// panel gives the extra room to the controls rather than to the labels.
pub const LABEL_COLUMN_MAX_WIDTH: f32 = 200.0;

/// The width in points of a row's label column where labels are whole setting
/// names instead of fields: the Preferences, Systems and Statistics tabs and
/// the Inspector's asset-settings rows. The wide column fits their longest
/// label and still leaves the narrow Preferences panel room for its controls.
pub const WIDE_ROW_LABEL_WIDTH: f32 = 140.0;

/// The height in points of every row: its label cell and the slot its control
/// is centred in. It is a few points taller than a standard control, so every
/// field has the same room above and below it and none is cut at its edge.
pub const ROW_HEIGHT: f32 = 22.0;

/// The least space in points a row keeps between its label and its control:
/// a little more than the panel's own item spacing, so the label reads as
/// naming the control beside it without drifting away from it.
pub const ROW_LABEL_GAP: f32 = 8.0;

/// The label column of a panel `ui` lays out: [`LABEL_COLUMN_FRACTION`] of
/// the width it has left, between [`LABEL_COLUMN_MIN_WIDTH`] and
/// [`LABEL_COLUMN_MAX_WIDTH`]. Rows read it before they place anything, so
/// every row of one panel gets the same column. Below the minimum the column
/// stays put and the fields take the squeeze.
pub fn label_column_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() * LABEL_COLUMN_FRACTION)
        .clamp(LABEL_COLUMN_MIN_WIDTH, LABEL_COLUMN_MAX_WIDTH)
}

/// Draws one row's label in the panel's label column
/// ([`label_column_width`]), left-aligned and cut off at its edge. `tooltip`
/// is shown while the pointer rests on it.
pub fn label_cell(ui: &mut egui::Ui, label: &str, tooltip: Option<&str>) -> egui::Response {
    let width = label_column_width(ui);
    label_cell_in(ui, width, label, tooltip)
}

/// Draws one row's label in a label column `label_column_width` points wide.
///
/// The cell takes its own rectangle and the label is laid out from that
/// rectangle's left edge. A label given a fixed-size slot by the row itself
/// would be centered in it, so a short name would drift away from the column
/// it belongs to.
pub fn label_cell_in(
    ui: &mut egui::Ui,
    label_column_width: f32,
    label: &str,
    tooltip: Option<&str>,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(label_column_width, ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let mut label_response = ui
        .new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        )
        .add(egui::Label::new(egui::RichText::new(label).color(style::text_muted())).truncate());
    if let Some(tooltip) = tooltip {
        label_response = kit::tooltip::on_hover_text(label_response, tooltip);
    }
    label_response
}

/// Adds the rest of the gap after a label cell, so the label and its control
/// stand at least [`ROW_LABEL_GAP`] points apart. egui already adds its item
/// spacing between the two widgets, so this adds only the part still missing.
pub fn label_gap(ui: &mut egui::Ui) {
    let spacing = ui.spacing().item_spacing.x;
    ui.add_space((ROW_LABEL_GAP - spacing).max(0.0));
}

/// Draws one label-and-control row in the panel's label column
/// ([`label_column_width`]): `label` on the left, then whatever `draw_control`
/// draws in the control column, which runs to the panel's right edge.
pub fn row(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: Option<&str>,
    draw_control: &mut dyn FnMut(&mut egui::Ui),
) {
    let width = label_column_width(ui);
    row_in(ui, width, label, tooltip, draw_control);
}

/// Draws one label-and-control row whose label column is `label_column_width`
/// points wide.
pub fn row_in(
    ui: &mut egui::Ui,
    label_column_width: f32,
    label: &str,
    tooltip: Option<&str>,
    draw_control: &mut dyn FnMut(&mut egui::Ui),
) {
    ui.horizontal(|ui| {
        label_cell_in(ui, label_column_width, label, tooltip);
        label_gap(ui);
        control_column(ui, draw_control);
    });
}

/// Draws a row's control in a column exactly as wide as the room left in the
/// row, and claims exactly that width.
///
/// A control laid out wider than the column is cut at the column's edge, but
/// it never widens the row: a row that grew would widen the whole panel, every
/// row after it would be laid out to the wider width, and the panel's own edge
/// would cut them all. The column is as tall as its control, and at least
/// [`ROW_HEIGHT`].
pub fn control_column(ui: &mut egui::Ui, draw_control: &mut dyn FnMut(&mut egui::Ui)) {
    let width = ui.available_width().max(0.0);
    // The control is centred in one row's height; a taller one (a text box of
    // several lines) grows the column downwards. Only the sides are clipped.
    let column_rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, ROW_HEIGHT));
    let mut control = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(column_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let parent_clip = ui.clip_rect();
    control.set_clip_rect(egui::Rect::from_x_y_ranges(
        column_rect.x_range().intersection(parent_clip.x_range()),
        parent_clip.y_range(),
    ));
    draw_control(&mut control);
    let height = control.min_rect().height().max(ROW_HEIGHT);
    ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
}

/// Draws one read-only row: the label, then `value` in monospace text.
pub fn value_row(ui: &mut egui::Ui, label: &str, value: &str) {
    let width = label_column_width(ui);
    value_row_in(ui, width, label, value);
}

/// Draws one read-only row whose label column is `label_column_width` points
/// wide.
pub fn value_row_in(ui: &mut egui::Ui, label_column_width: f32, label: &str, value: &str) {
    row_in(ui, label_column_width, label, None, &mut |ui| {
        ui.monospace(value);
    });
}
