//! Scoped style variants for stock egui widgets.
//!
//! # Responsibilities
//!
//! - [`primary`](crate::variants::primary), [`secondary`](crate::variants::secondary)
//!   and [`danger`](crate::variants::danger): scoped widget colours and button
//!   padding from the theme's tokens, for a block the caller wraps in
//!   `ui.scope`.
//!
//! # Design
//!
//! A variant changes only the `Ui` it is given. Callers wrap it in
//! `ui.scope(|ui| { variants::danger(ui, &theme); ... })`, so the theme's look
//! comes back when the block ends. `Button::fill` sets one colour for every
//! state and drops egui's hover effect; changing the scoped `Style` keeps the
//! idle, hover and press colours separate.

// External crates
use egui::{Color32, Ui};

// Current crate
use pill_ui_theme::{Rgba, Theme};

// =============================================================================
// Free Functions
// =============================================================================

/// Applies the `primary` variant: the accent fill.
pub fn primary(ui: &mut Ui, theme: &Theme) {
    set_state_fills_and_padding(
        ui,
        theme,
        theme.palette.accent,
        theme.palette.accent_hot,
        theme.palette.accent_hot,
    );
}

/// Applies the `secondary` variant: a raised neutral fill.
pub fn secondary(ui: &mut Ui, theme: &Theme) {
    set_state_fills_and_padding(
        ui,
        theme,
        theme.palette.panel_raised,
        theme.palette.line,
        theme.palette.panel,
    );
}

/// Applies the `danger` variant: a faint danger fill that becomes the full
/// danger colour on hover or press.
pub fn danger(ui: &mut Ui, theme: &Theme) {
    set_state_fills_and_padding(
        ui,
        theme,
        theme.palette.danger.with_alpha_factor(0.35),
        theme.palette.danger,
        theme.palette.danger,
    );
}

/// Sets the widget fill for each of the three interaction states, the border
/// colour used while hovered or pressed, and the theme's button padding, on
/// `ui`.
fn set_state_fills_and_padding(
    ui: &mut Ui,
    theme: &Theme,
    inactive: Rgba,
    hovered: Rgba,
    active: Rgba,
) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.weak_bg_fill = to_egui_color(inactive);
    widgets.inactive.bg_fill = to_egui_color(inactive);
    widgets.hovered.weak_bg_fill = to_egui_color(hovered);
    widgets.hovered.bg_fill = to_egui_color(hovered);
    widgets.active.weak_bg_fill = to_egui_color(active);
    widgets.active.bg_fill = to_egui_color(active);
    widgets.hovered.bg_stroke.color = to_egui_color(hovered);
    widgets.active.bg_stroke.color = to_egui_color(active);
    let [x, y] = theme.spacing.button_padding;
    ui.spacing_mut().button_padding = egui::vec2(x, y);
}

/// Returns the egui colour that matches a theme colour.
fn to_egui_color(theme_color: Rgba) -> Color32 {
    let [red, green, blue, alpha] = theme_color.0;
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use egui::{Context, RawInput};

    use super::{danger, primary, secondary};
    use pill_ui_theme::Theme;

    #[test]
    fn the_variants_set_distinct_fills() {
        let theme = Theme::brutalist();
        let context = Context::default();
        let mut full_output = context.run_ui(RawInput::default(), |ui| {
            primary(ui, &theme);
            let primary_fill = ui.visuals().widgets.inactive.weak_bg_fill;
            secondary(ui, &theme);
            let secondary_fill = ui.visuals().widgets.inactive.weak_bg_fill;
            danger(ui, &theme);
            let danger_fill = ui.visuals().widgets.inactive.weak_bg_fill;
            assert_ne!(primary_fill, secondary_fill);
            assert_ne!(secondary_fill, danger_fill);
            assert_ne!(primary_fill, danger_fill);
        });
        full_output.textures_delta.clear();
    }

    #[test]
    fn the_padding_comes_from_the_theme() {
        let theme = Theme::brutalist();
        let context = Context::default();
        let mut full_output = context.run_ui(RawInput::default(), |ui| {
            primary(ui, &theme);
            let [x, y] = theme.spacing.button_padding;
            assert_eq!(ui.spacing().button_padding, egui::vec2(x, y));
        });
        full_output.textures_delta.clear();
    }
}
