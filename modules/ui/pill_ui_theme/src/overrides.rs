//! The sparse theme document a `.theme` file holds.
//!
//! # Responsibilities
//!
//! - [`ThemeOverrides`]: every token a document may change, each one optional,
//!   so a file states only what it differs from the base theme.
//! - [`ThemeOverrides::from_yaml_str`]: accepts a bare override body or a
//!   complete standalone asset document (the body under `asset`).
//! - The nested override types the merge reads.
//!
//! # Design
//!
//! Every struct refuses unknown fields, and a field left out of a document
//! means "no change". A typo therefore fails loudly instead of silently doing
//! nothing, and a document never has to restate a value it keeps. Colours stay
//! strings here and are parsed during the merge, where the item's dotted path
//! is known, so an error can name `palette.accent` instead of "a colour".

// Standard library
use std::collections::BTreeMap;

// External crates
use serde::{Deserialize, Serialize};

// Current crate
use crate::theme::ThemeError;

// =============================================================================
// The document
// =============================================================================

/// The theme changes a `.theme` document lists.
///
/// A document names only the items it changes. Merging it onto a complete base
/// theme yields the theme a UI installs.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeOverrides {
    /// The theme's name. A document may set it, and the merge copies it onto
    /// the theme.
    pub name: Option<String>,
    /// Whether egui treats the theme as dark mode.
    pub dark_mode: Option<bool>,
    /// The colour roles this document changes.
    pub palette: PaletteOverrides,
    /// The named text styles this document replaces or adds.
    pub text_styles: BTreeMap<String, TextStyleOverride>,
    /// The layout metrics this document changes.
    pub spacing: SpacingOverrides,
    /// The corner radii, strokes, shadows and accent widths this document
    /// changes.
    pub shape: ShapeOverrides,
    /// The widget states this document changes.
    pub widgets: WidgetsOverrides,
    /// The font paths this document names.
    pub fonts: FontsOverrides,
}

impl ThemeOverrides {
    /// Reads an override document from its YAML text.
    ///
    /// The text is either the override body itself or a standalone asset file,
    /// which carries its body under `asset`. Both give the same value.
    ///
    /// # Errors
    ///
    /// [`ThemeError::Format`] with serde's message when the text is not YAML,
    /// does not hold a mapping, or names an unknown or wrongly typed item.
    pub fn from_yaml_str(yaml_text: &str) -> Result<Self, ThemeError> {
        let parsed_document = pill_engine_core::asset_document::parse(yaml_text.as_bytes())
            .map_err(ThemeError::Format)?;
        let body = match parsed_document {
            serde_json::Value::Object(mut map) => match map.remove("asset") {
                Some(asset) => asset,
                None => serde_json::Value::Object(map),
            },
            other => other,
        };
        serde_json::from_value(body).map_err(|error| ThemeError::Format(error.to_string()))
    }
}

// =============================================================================
// The palette
// =============================================================================

/// The colour roles [`ThemeOverrides`] can change, each written as `#rrggbb` or
/// `#rrggbbaa`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PaletteOverrides {
    /// See [`Palette::frame`](crate::theme::Palette::frame).
    pub frame: Option<String>,
    /// See [`Palette::panel`](crate::theme::Palette::panel).
    pub panel: Option<String>,
    /// See [`Palette::panel_raised`](crate::theme::Palette::panel_raised).
    pub panel_raised: Option<String>,
    /// See [`Palette::control`](crate::theme::Palette::control).
    pub control: Option<String>,
    /// See [`Palette::well`](crate::theme::Palette::well).
    pub well: Option<String>,
    /// See [`Palette::line`](crate::theme::Palette::line).
    pub line: Option<String>,
    /// See [`Palette::text`](crate::theme::Palette::text).
    pub text: Option<String>,
    /// See [`Palette::text_strong`](crate::theme::Palette::text_strong).
    pub text_strong: Option<String>,
    /// See [`Palette::text_muted`](crate::theme::Palette::text_muted).
    pub text_muted: Option<String>,
    /// See [`Palette::accent`](crate::theme::Palette::accent).
    pub accent: Option<String>,
    /// See [`Palette::accent_hot`](crate::theme::Palette::accent_hot).
    pub accent_hot: Option<String>,
    /// See [`Palette::success`](crate::theme::Palette::success).
    pub success: Option<String>,
    /// See [`Palette::warning`](crate::theme::Palette::warning).
    pub warning: Option<String>,
    /// See [`Palette::danger`](crate::theme::Palette::danger).
    pub danger: Option<String>,
    /// See [`Palette::selection`](crate::theme::Palette::selection).
    pub selection: Option<String>,
    /// See [`Palette::selection_bar`](crate::theme::Palette::selection_bar).
    pub selection_bar: Option<String>,
    /// See [`Palette::overlay`](crate::theme::Palette::overlay).
    pub overlay: Option<String>,
    /// See [`Palette::overlay_border`](crate::theme::Palette::overlay_border).
    pub overlay_border: Option<String>,
    /// See [`Palette::log_info`](crate::theme::Palette::log_info).
    pub log_info: Option<String>,
    /// See [`Palette::log_debug`](crate::theme::Palette::log_debug).
    pub log_debug: Option<String>,
    /// See [`Palette::log_warn`](crate::theme::Palette::log_warn).
    pub log_warn: Option<String>,
    /// See [`Palette::log_error`](crate::theme::Palette::log_error).
    pub log_error: Option<String>,
    /// See [`Palette::play_tint`](crate::theme::Palette::play_tint).
    pub play_tint: Option<String>,
    /// See [`Palette::play_tint_paused`](crate::theme::Palette::play_tint_paused).
    pub play_tint_paused: Option<String>,
}

// =============================================================================
// Text styles
// =============================================================================

/// One named text style's overridable fields.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextStyleOverride {
    /// The size in points. It must be a positive, finite number.
    pub size: Option<f32>,
    /// The family: `proportional`, `monospace` or `display`. The aliases
    /// `body` and `mono` are accepted too.
    pub family: Option<String>,
}

// =============================================================================
// Spacing and shape
// =============================================================================

/// The layout metrics [`ThemeOverrides`] can change.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpacingOverrides {
    /// See [`Spacing::item_spacing`](crate::theme::Spacing::item_spacing).
    pub item_spacing: Option<[f32; 2]>,
    /// See [`Spacing::button_padding`](crate::theme::Spacing::button_padding).
    pub button_padding: Option<[f32; 2]>,
    /// See [`Spacing::interact_size`](crate::theme::Spacing::interact_size).
    pub interact_size: Option<[f32; 2]>,
    /// See [`Spacing::window_margin`](crate::theme::Spacing::window_margin).
    pub window_margin: Option<f32>,
    /// See [`Spacing::menu_margin`](crate::theme::Spacing::menu_margin).
    pub menu_margin: Option<f32>,
    /// See [`Spacing::indent`](crate::theme::Spacing::indent).
    pub indent: Option<f32>,
    /// See [`Spacing::slider_width`](crate::theme::Spacing::slider_width).
    pub slider_width: Option<f32>,
    /// See [`Spacing::slider_rail_height`](crate::theme::Spacing::slider_rail_height).
    pub slider_rail_height: Option<f32>,
    /// See [`Spacing::icon_width`](crate::theme::Spacing::icon_width).
    pub icon_width: Option<f32>,
    /// See [`Spacing::icon_width_inner`](crate::theme::Spacing::icon_width_inner).
    pub icon_width_inner: Option<f32>,
    /// See [`Spacing::icon_spacing`](crate::theme::Spacing::icon_spacing).
    pub icon_spacing: Option<f32>,
    /// See [`Spacing::menu_width`](crate::theme::Spacing::menu_width).
    pub menu_width: Option<f32>,
    /// See [`Spacing::tooltip_width`](crate::theme::Spacing::tooltip_width).
    pub tooltip_width: Option<f32>,
    /// See [`Spacing::combo_width`](crate::theme::Spacing::combo_width).
    pub combo_width: Option<f32>,
    /// See [`Spacing::combo_height`](crate::theme::Spacing::combo_height).
    pub combo_height: Option<f32>,
    /// See [`Spacing::text_edit_width`](crate::theme::Spacing::text_edit_width).
    pub text_edit_width: Option<f32>,
    /// See [`Spacing::menu_spacing`](crate::theme::Spacing::menu_spacing).
    pub menu_spacing: Option<f32>,
    /// See [`Spacing::scroll_bar_width`](crate::theme::Spacing::scroll_bar_width).
    pub scroll_bar_width: Option<f32>,
    /// See [`Spacing::scroll_floating`](crate::theme::Spacing::scroll_floating).
    pub scroll_floating: Option<bool>,
}

/// The shape values [`ThemeOverrides`] can change.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShapeOverrides {
    /// See [`Shape::corner_radius`](crate::theme::Shape::corner_radius).
    pub corner_radius: Option<u8>,
    /// See [`Shape::window_corner_radius`](crate::theme::Shape::window_corner_radius).
    pub window_corner_radius: Option<u8>,
    /// See [`Shape::menu_corner_radius`](crate::theme::Shape::menu_corner_radius).
    pub menu_corner_radius: Option<u8>,
    /// See [`Shape::window_stroke`](crate::theme::Shape::window_stroke).
    pub window_stroke: Option<StrokeOverride>,
    /// See [`Shape::window_shadow`](crate::theme::Shape::window_shadow).
    pub window_shadow: Option<ShadowOverride>,
    /// See [`Shape::popup_shadow`](crate::theme::Shape::popup_shadow).
    pub popup_shadow: Option<ShadowOverride>,
    /// See [`Shape::selection_stroke`](crate::theme::Shape::selection_stroke).
    pub selection_stroke: Option<StrokeOverride>,
    /// See [`Shape::tab_underline_width`](crate::theme::Shape::tab_underline_width).
    pub tab_underline_width: Option<f32>,
    /// See [`Shape::selected_row_bar_width`](crate::theme::Shape::selected_row_bar_width).
    pub selected_row_bar_width: Option<f32>,
}

/// One stroke's overridable fields.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct StrokeOverride {
    /// The width in points.
    pub width: Option<f32>,
    /// The colour, written as `#rrggbb` or `#rrggbbaa`.
    pub color: Option<String>,
}

/// One shadow's overridable fields.
///
/// Each number is stored wider than egui's own field, so the merge can refuse
/// an out-of-range value and name the item it came from. serde alone would
/// reject it without that path.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShadowOverride {
    /// The offset in points. It must fit egui 0.36's `[i8; 2]`.
    pub offset: Option<[i32; 2]>,
    /// The blur radius in points. It must fit egui 0.36's `u8`.
    pub blur: Option<i32>,
    /// The spread in points. It must fit egui 0.36's `u8`.
    pub spread: Option<i32>,
    /// The colour, written as `#rrggbb` or `#rrggbbaa`.
    pub color: Option<String>,
}

// =============================================================================
// Widget states
// =============================================================================

/// The widget states [`ThemeOverrides`] can change.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WidgetsOverrides {
    /// Labels, separators and window frames.
    pub noninteractive: WidgetStyleOverride,
    /// An interactive widget at rest.
    pub inactive: WidgetStyleOverride,
    /// The pointer over a widget.
    pub hovered: WidgetStyleOverride,
    /// A pressed, dragged or focused widget.
    pub active: WidgetStyleOverride,
    /// A widget whose menu is open.
    pub open: WidgetStyleOverride,
}

/// One widget state's overridable fields.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WidgetStyleOverride {
    /// The fill for widgets that must have a background.
    pub bg_fill: Option<String>,
    /// The fill for widgets that may have a background (buttons).
    pub weak_bg_fill: Option<String>,
    /// The outline around the widget.
    pub bg_stroke: Option<StrokeOverride>,
    /// The text and glyph colour.
    pub fg_stroke: Option<StrokeOverride>,
    /// The corner radius in this state.
    pub corner_radius: Option<u8>,
    /// How much the frame grows in this state.
    pub expansion: Option<f32>,
}

// =============================================================================
// Fonts
// =============================================================================

/// The font paths [`ThemeOverrides`] can name.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontsOverrides {
    /// The body font's path.
    pub body: Option<String>,
    /// The display font's path.
    pub display: Option<String>,
    /// The monospace font's path.
    pub monospace: Option<String>,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_body_and_a_standalone_document_parse_equal() {
        let bare = ThemeOverrides::from_yaml_str(
            "palette:\n  accent: \"#33d17a\"\nspacing:\n  indent: 20.0\n",
        )
        .unwrap();
        let standalone = ThemeOverrides::from_yaml_str(
            "format_version: 1\nasset_type: pill_ui_theme::ThemeOverrides\nguid: \"00000000000000000000000000000001\"\nasset:\n  palette:\n    accent: \"#33d17a\"\n  spacing:\n    indent: 20.0\n",
        )
        .unwrap();
        assert_eq!(bare, standalone);
    }

    #[test]
    fn an_unknown_item_is_refused_by_name() {
        let error = ThemeOverrides::from_yaml_str("palette:\n  acccent: \"#ffffff\"\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("acccent"), "{error}");
    }

    #[test]
    fn an_empty_document_is_the_default() {
        assert_eq!(
            ThemeOverrides::from_yaml_str("{}").unwrap(),
            ThemeOverrides::default()
        );
    }
}
