//! The complete theme tokens and the merge that applies overrides to them.
//!
//! # Responsibilities
//!
//! - [`Theme`]: every value a UI's style needs, as plain owned data.
//! - [`Theme::fallback`] and [`Theme::brutalist`]: the neutral dark base and
//!   the brutalist recipe.
//! - [`Theme::merged_with`]: turns a sparse override document into a complete
//!   theme.
//! - The token types the document and the UI kit share ([`Rgba`], [`Stroke`],
//!   [`Shadow`], [`Spacing`], [`Shape`], [`WidgetStyle`], [`WidgetStates`]).
//!
//! # Design
//!
//! Everything here is plain data and needs no egui. This crate is a shared
//! `dylib`, so a project, `pill_project_ui`, the editor shell and
//! `pill_editor_core` all see the same `Theme` type, and each UI converts the
//! tokens inside its own link unit.
//!
//! A merge never fills gaps from egui's defaults. The base theme is complete,
//! so an item the document leaves out keeps the base value. One refused item
//! rejects the whole layer, so a UI never paints a half-applied theme.

// Standard library
use std::collections::BTreeMap;

// External crates
use thiserror::Error;

// Current crate
use crate::overrides::{
    FontsOverrides, PaletteOverrides, ShadowOverride, ShapeOverrides, SpacingOverrides,
    StrokeOverride, TextStyleOverride, ThemeOverrides, WidgetStyleOverride, WidgetsOverrides,
};

// =============================================================================
// Colours and strokes
// =============================================================================

/// An sRGB colour with straight (not premultiplied) alpha, stored as
/// `[red, green, blue, alpha]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    /// An opaque colour.
    pub const fn rgb(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue, 255])
    }

    /// A colour with an explicit alpha.
    pub const fn with_alpha(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self([red, green, blue, alpha])
    }

    /// Parse `#rrggbb` or `#rrggbbaa`.
    ///
    /// # Errors
    ///
    /// A message containing the refused text, when it lacks the `#`, has the
    /// wrong length, or has a channel that is not hexadecimal.
    pub fn parse(text: &str) -> Result<Self, String> {
        let digits = text
            .strip_prefix('#')
            .ok_or_else(|| format!("`{text}` is not a `#rrggbb` or `#rrggbbaa` colour"))?;
        if !digits.is_ascii() || (digits.len() != 6 && digits.len() != 8) {
            return Err(format!("`{text}` is not a `#rrggbb` or `#rrggbbaa` colour"));
        }
        let mut channels = [0u8; 4];
        channels[3] = 255;
        for index in 0..digits.len() / 2 {
            let pair = &digits[index * 2..index * 2 + 2];
            channels[index] = u8::from_str_radix(pair, 16)
                .map_err(|_| format!("`{text}` has a non-hex channel `{pair}`"))?;
        }
        Ok(Self(channels))
    }

    /// The colour as `#rrggbb`, or `#rrggbbaa` when it is not opaque.
    pub fn hex(self) -> String {
        let [red, green, blue, alpha] = self.0;
        if alpha == 255 {
            format!("#{red:02x}{green:02x}{blue:02x}")
        } else {
            format!("#{red:02x}{green:02x}{blue:02x}{alpha:02x}")
        }
    }

    /// The colour with its alpha scaled by `factor` (0.0..=1.0).
    pub fn with_alpha_factor(self, factor: f32) -> Self {
        let [red, green, blue, alpha] = self.0;
        let alpha = (f32::from(alpha) * factor.clamp(0.0, 1.0)).round() as u8;
        Self([red, green, blue, alpha])
    }
}

impl std::fmt::Display for Rgba {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.hex())
    }
}

/// A line stroke: a width in points and a colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    /// The width in points.
    pub width: f32,
    /// The stroke's colour.
    pub color: Rgba,
}

impl Stroke {
    /// A stroke of `width` points in `color`.
    pub const fn new(width: f32, color: Rgba) -> Self {
        Self { width, color }
    }
}

/// A window or popup shadow, in egui 0.36's integer fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shadow {
    /// The offset in points.
    pub offset: [i8; 2],
    /// The blur radius in points.
    pub blur: u8,
    /// The spread in points.
    pub spread: u8,
    /// The shadow's colour.
    pub color: Rgba,
}

impl Shadow {
    /// A shadow with the given fields.
    pub const fn new(offset: [i8; 2], blur: u8, spread: u8, color: Rgba) -> Self {
        Self {
            offset,
            blur,
            spread,
            color,
        }
    }
}

// =============================================================================
// Typography
// =============================================================================

/// The family a text style names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontFamilyName {
    /// The body font family.
    Proportional,
    /// The code font family.
    Monospace,
    /// The display font family.
    Display,
}

impl FontFamilyName {
    /// Parse a family name: `proportional` (or `body`), `monospace` (or
    /// `mono`) or `display`.
    ///
    /// # Errors
    ///
    /// A message naming the accepted families.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "proportional" | "body" => Ok(Self::Proportional),
            "monospace" | "mono" => Ok(Self::Monospace),
            "display" => Ok(Self::Display),
            _ => Err(format!(
                "`{text}` is not a font family (proportional, monospace or display)"
            )),
        }
    }

    /// The name this family is written as in a document.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proportional => "proportional",
            Self::Monospace => "monospace",
            Self::Display => "display",
        }
    }
}

/// One named text style: a size in points and a family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    /// The size in points.
    pub size: f32,
    /// The family the style draws in.
    pub family: FontFamilyName,
}

impl TextStyle {
    /// A style of `size` points in `family`.
    pub const fn new(size: f32, family: FontFamilyName) -> Self {
        Self { size, family }
    }
}

/// The font paths a theme names, resolved by each consumer.
///
/// A merge carries the paths and not the font bytes. The game loads a path as
/// a `FontAsset`, the editor's project layer does the same, and the editor's
/// user layer reads the file from disk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeFontPaths {
    /// The body font's path.
    pub body: Option<String>,
    /// The display font's path.
    pub display: Option<String>,
    /// The monospace font's path.
    pub monospace: Option<String>,
}

// =============================================================================
// Spacing and shape
// =============================================================================

/// The layout metrics a UI reads from the theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spacing {
    /// Gap between consecutive widgets.
    pub item_spacing: [f32; 2],
    /// Padding inside a button.
    pub button_padding: [f32; 2],
    /// Minimum interactive size.
    pub interact_size: [f32; 2],
    /// Padding inside a window.
    pub window_margin: f32,
    /// Padding inside a menu or popup.
    pub menu_margin: f32,
    /// Offset of an indented body.
    pub indent: f32,
    /// A slider's width.
    pub slider_width: f32,
    /// A slider rail's height.
    pub slider_rail_height: f32,
    /// The outer size of the icon a check box or radio button draws.
    pub icon_width: f32,
    /// The inner size of that icon: the mark inside the box.
    pub icon_width_inner: f32,
    /// The gap between an icon and its label.
    pub icon_spacing: f32,
    /// A menu's wrapping width. egui 0.36 never reads its `menu_width`, so
    /// this changes nothing today.
    pub menu_width: f32,
    /// The width a tooltip wraps its text at. Must be positive: egui gives a
    /// tooltip exactly this width, so zero collapses its text to one
    /// character per line.
    pub tooltip_width: f32,
    /// A combo box's minimum width; zero lets egui size it.
    pub combo_width: f32,
    /// The height at which a combo box starts to scroll.
    pub combo_height: f32,
    /// A text edit's default width.
    pub text_edit_width: f32,
    /// The horizontal gap between a menu and its submenu.
    pub menu_spacing: f32,
    /// A scroll bar's width.
    pub scroll_bar_width: f32,
    /// Whether scroll bars float over the content. A floating bar is thin
    /// until the pointer reaches it and takes no layout width.
    pub scroll_floating: bool,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            item_spacing: [8.0, 3.0],
            button_padding: [8.0, 3.0],
            interact_size: [24.0, 20.0],
            window_margin: 8.0,
            menu_margin: 6.0,
            indent: 18.0,
            slider_width: 100.0,
            slider_rail_height: 8.0,
            icon_width: 14.0,
            icon_width_inner: 8.0,
            icon_spacing: 4.0,
            menu_width: 0.0,
            tooltip_width: 500.0,
            combo_width: 0.0,
            combo_height: 200.0,
            text_edit_width: 280.0,
            menu_spacing: 0.0,
            scroll_bar_width: 8.0,
            scroll_floating: false,
        }
    }
}

/// The corner radii, strokes, shadows and two accent widths a UI reads from
/// the theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// The default corner radius.
    pub corner_radius: u8,
    /// A window's corner radius.
    pub window_corner_radius: u8,
    /// A menu's or popup's corner radius.
    pub menu_corner_radius: u8,
    /// The stroke around a window.
    pub window_stroke: Stroke,
    /// The window shadow.
    pub window_shadow: Shadow,
    /// The popup shadow.
    pub popup_shadow: Shadow,
    /// The stroke around selected text.
    pub selection_stroke: Stroke,
    /// The active tab's underline width.
    pub tab_underline_width: f32,
    /// The selected row's left bar width.
    pub selected_row_bar_width: f32,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            corner_radius: 4,
            window_corner_radius: 6,
            menu_corner_radius: 6,
            window_stroke: Stroke::new(1.0, Rgba::rgb(58, 58, 58)),
            window_shadow: Shadow::new([0, 6], 18, 0, Rgba::with_alpha(0, 0, 0, 140)),
            popup_shadow: Shadow::new([0, 4], 12, 0, Rgba::with_alpha(0, 0, 0, 180)),
            selection_stroke: Stroke::new(1.0, Rgba::rgb(255, 255, 255)),
            tab_underline_width: 2.0,
            selected_row_bar_width: 3.0,
        }
    }
}

// =============================================================================
// Widget states
// =============================================================================

/// One widget's look in one interaction state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidgetStyle {
    /// The fill for widgets that must have a background, such as a check box
    /// or a text edit.
    pub bg_fill: Rgba,
    /// The fill for widgets whose background is optional, such as buttons.
    pub weak_bg_fill: Rgba,
    /// The outline drawn around a widget that has one.
    pub bg_stroke: Stroke,
    /// The text and glyph colour.
    pub fg_stroke: Stroke,
    /// The corner radius in this state.
    pub corner_radius: u8,
    /// How much the frame grows in this state.
    pub expansion: f32,
}

/// The five interaction states egui picks from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WidgetStates {
    /// Labels, separators and window frames.
    pub noninteractive: WidgetStyle,
    /// An interactive widget at rest.
    pub inactive: WidgetStyle,
    /// The pointer over a widget.
    pub hovered: WidgetStyle,
    /// A pressed, dragged or focused widget.
    pub active: WidgetStyle,
    /// A widget whose menu is open.
    pub open: WidgetStyle,
}

impl Default for WidgetStates {
    fn default() -> Self {
        let line = Rgba::rgb(58, 58, 58);
        Self {
            noninteractive: WidgetStyle {
                bg_fill: Rgba::rgb(32, 32, 32),
                weak_bg_fill: Rgba::rgb(32, 32, 32),
                bg_stroke: Stroke::new(1.0, line),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(208, 208, 208)),
                corner_radius: 4,
                expansion: 0.0,
            },
            inactive: WidgetStyle {
                bg_fill: Rgba::rgb(17, 17, 17),
                weak_bg_fill: Rgba::rgb(42, 42, 42),
                bg_stroke: Stroke::new(1.0, line),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(208, 208, 208)),
                corner_radius: 4,
                expansion: 0.0,
            },
            hovered: WidgetStyle {
                bg_fill: Rgba::rgb(51, 51, 51),
                weak_bg_fill: Rgba::rgb(51, 51, 51),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(79, 139, 255)),
                fg_stroke: Stroke::new(1.5, Rgba::rgb(255, 255, 255)),
                corner_radius: 4,
                expansion: 1.0,
            },
            active: WidgetStyle {
                bg_fill: Rgba::rgb(47, 79, 127),
                weak_bg_fill: Rgba::rgb(47, 79, 127),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(122, 167, 255)),
                fg_stroke: Stroke::new(2.0, Rgba::rgb(255, 255, 255)),
                corner_radius: 4,
                expansion: 0.0,
            },
            open: WidgetStyle {
                bg_fill: Rgba::rgb(42, 42, 42),
                weak_bg_fill: Rgba::rgb(42, 42, 42),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(79, 139, 255)),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(79, 139, 255)),
                corner_radius: 4,
                expansion: 0.0,
            },
        }
    }
}

impl WidgetStates {
    /// The brutalist recipe's five states.
    fn brutalist() -> Self {
        let line = Rgba::rgb(70, 60, 64);
        let radius = 0;
        Self {
            noninteractive: WidgetStyle {
                bg_fill: Rgba::rgb(20, 17, 18),
                weak_bg_fill: Rgba::rgb(20, 17, 18),
                bg_stroke: Stroke::new(1.0, line),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(232, 228, 230)),
                corner_radius: radius,
                expansion: 0.0,
            },
            inactive: WidgetStyle {
                bg_fill: Rgba::rgb(10, 8, 9),
                weak_bg_fill: Rgba::rgb(34, 29, 31),
                bg_stroke: Stroke::new(1.0, line),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(232, 228, 230)),
                corner_radius: radius,
                expansion: 0.0,
            },
            hovered: WidgetStyle {
                bg_fill: Rgba::rgb(52, 44, 48),
                weak_bg_fill: Rgba::rgb(52, 44, 48),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(255, 72, 140)),
                fg_stroke: Stroke::new(1.5, Rgba::rgb(255, 255, 255)),
                corner_radius: radius,
                expansion: 1.0,
            },
            active: WidgetStyle {
                bg_fill: Rgba::rgb(255, 72, 140),
                weak_bg_fill: Rgba::rgb(255, 72, 140),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(255, 130, 180)),
                fg_stroke: Stroke::new(2.0, Rgba::rgb(0, 0, 0)),
                corner_radius: radius,
                expansion: 0.0,
            },
            open: WidgetStyle {
                bg_fill: Rgba::rgb(34, 29, 31),
                weak_bg_fill: Rgba::rgb(34, 29, 31),
                bg_stroke: Stroke::new(1.0, Rgba::rgb(255, 72, 140)),
                fg_stroke: Stroke::new(1.0, Rgba::rgb(255, 72, 140)),
                corner_radius: radius,
                expansion: 0.0,
            },
        }
    }
}

// =============================================================================
// The palette
// =============================================================================

/// The colour roles a theme paints with, named by role and not by hue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The window frame and tab-strip background.
    pub frame: Rgba,
    /// A panel's background.
    pub panel: Rgba,
    /// A surface one step lighter than the panel: a hovered tab, a neutral
    /// button's face, and striped rows.
    pub panel_raised: Rgba,
    /// A flat control's fill: buttons and rows drawn on a panel.
    pub control: Rgba,
    /// The background of text inputs and other sunken areas.
    pub well: Rgba,
    /// Borders and separators.
    pub line: Rgba,
    /// Normal text.
    pub text: Rgba,
    /// Emphasized text.
    pub text_strong: Rgba,
    /// Muted text.
    pub text_muted: Rgba,
    /// The accent: focus, active tabs, selection.
    pub accent: Rgba,
    /// A brighter accent for hovered accents.
    pub accent_hot: Rgba,
    /// Success and running states.
    pub success: Rgba,
    /// Warnings.
    pub warning: Rgba,
    /// Errors and destructive actions.
    pub danger: Rgba,
    /// A selected row's tint.
    pub selection: Rgba,
    /// A selected row's left bar.
    pub selection_bar: Rgba,
    /// A floating panel's fill over the scene.
    pub overlay: Rgba,
    /// A floating panel's border.
    pub overlay_border: Rgba,
    /// A log line at `info`.
    pub log_info: Rgba,
    /// A log line at `debug`.
    pub log_debug: Rgba,
    /// A log line at `warn`.
    pub log_warn: Rgba,
    /// A log line at `error`.
    pub log_error: Rgba,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            frame: Rgba::rgb(27, 27, 27),
            panel: Rgba::rgb(32, 32, 32),
            panel_raised: Rgba::rgb(42, 42, 42),
            control: Rgba::rgb(47, 49, 52),
            well: Rgba::rgb(17, 17, 17),
            line: Rgba::rgb(58, 58, 58),
            text: Rgba::rgb(208, 208, 208),
            text_strong: Rgba::rgb(255, 255, 255),
            text_muted: Rgba::rgb(138, 138, 138),
            accent: Rgba::rgb(79, 139, 255),
            accent_hot: Rgba::rgb(122, 167, 255),
            success: Rgba::rgb(122, 194, 82),
            warning: Rgba::rgb(255, 196, 80),
            danger: Rgba::rgb(255, 70, 70),
            selection: Rgba::rgb(47, 79, 127),
            selection_bar: Rgba::rgb(79, 139, 255),
            overlay: Rgba::with_alpha(18, 18, 18, 230),
            overlay_border: Rgba::rgb(68, 68, 68),
            log_info: Rgba::rgb(90, 160, 220),
            log_debug: Rgba::rgb(138, 138, 138),
            log_warn: Rgba::rgb(218, 170, 56),
            log_error: Rgba::rgb(217, 72, 62),
        }
    }
}

impl Palette {
    /// The brutalist recipe's colours.
    fn brutalist() -> Self {
        Self {
            frame: Rgba::rgb(20, 17, 18),
            panel: Rgba::rgb(20, 17, 18),
            panel_raised: Rgba::rgb(34, 29, 31),
            control: Rgba::rgb(34, 29, 31),
            well: Rgba::rgb(10, 8, 9),
            line: Rgba::rgb(70, 60, 64),
            text: Rgba::rgb(232, 228, 230),
            text_strong: Rgba::rgb(255, 255, 255),
            text_muted: Rgba::rgb(130, 122, 126),
            accent: Rgba::rgb(255, 72, 140),
            accent_hot: Rgba::rgb(255, 130, 180),
            success: Rgba::rgb(122, 194, 82),
            warning: Rgba::rgb(255, 196, 80),
            danger: Rgba::rgb(255, 70, 70),
            selection: Rgba::rgb(255, 72, 140),
            selection_bar: Rgba::rgb(255, 72, 140),
            overlay: Rgba::with_alpha(21, 22, 24, 226),
            overlay_border: Rgba::rgb(66, 68, 72),
            log_info: Rgba::rgb(90, 160, 220),
            log_debug: Rgba::rgb(130, 122, 126),
            log_warn: Rgba::rgb(255, 196, 80),
            log_error: Rgba::rgb(255, 70, 70),
        }
    }
}

// =============================================================================
// The theme
// =============================================================================

/// A complete set of UI tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// A name identifying this theme; a `.theme` document may override it.
    pub name: String,
    /// Whether the theme is dark, with light text on dark surfaces. egui
    /// reads it as a summary flag and builds no values from it.
    pub dark_mode: bool,
    /// The colours.
    pub palette: Palette,
    /// The named text styles, in stable order.
    pub text_styles: BTreeMap<String, TextStyle>,
    /// The layout metrics.
    pub spacing: Spacing,
    /// The radii, strokes, shadows and accents.
    pub shape: Shape,
    /// The five default widget states.
    pub widgets: WidgetStates,
    /// The font paths a consumer resolves.
    pub fonts: ThemeFontPaths,
}

impl Default for Theme {
    fn default() -> Self {
        Self::fallback()
    }
}

impl Theme {
    /// A neutral dark theme built in code.
    ///
    /// `install(engine)` installs this theme. It gives a game a usable look
    /// with no theme file and no project code.
    pub fn fallback() -> Self {
        Self {
            name: "fallback".to_owned(),
            dark_mode: true,
            palette: Palette::default(),
            text_styles: fallback_text_styles(),
            spacing: Spacing::default(),
            shape: Shape::default(),
            widgets: WidgetStates::default(),
            fonts: ThemeFontPaths::default(),
        }
    }

    /// The brutalist recipe, as a code default.
    ///
    /// It has square corners, shadows with no blur, a pink accent and light
    /// text on near-black surfaces. Its spacing and text sizes are the
    /// recipe's own.
    pub fn brutalist() -> Self {
        let mut text_styles = BTreeMap::new();
        text_styles.insert(
            "small".to_owned(),
            TextStyle::new(11.0, FontFamilyName::Proportional),
        );
        text_styles.insert(
            "body".to_owned(),
            TextStyle::new(14.0, FontFamilyName::Proportional),
        );
        text_styles.insert(
            "button".to_owned(),
            TextStyle::new(14.0, FontFamilyName::Proportional),
        );
        text_styles.insert(
            "monospace".to_owned(),
            TextStyle::new(13.0, FontFamilyName::Monospace),
        );
        text_styles.insert(
            "heading".to_owned(),
            TextStyle::new(22.0, FontFamilyName::Proportional),
        );
        Self {
            name: "brutalist".to_owned(),
            dark_mode: true,
            palette: Palette::brutalist(),
            text_styles,
            spacing: Spacing {
                item_spacing: [6.0, 6.0],
                button_padding: [10.0, 5.0],
                interact_size: [40.0, 26.0],
                window_margin: 10.0,
                menu_margin: 6.0,
                indent: 18.0,
                slider_width: 140.0,
                slider_rail_height: 4.0,
                icon_width: 16.0,
                icon_width_inner: 8.0,
                icon_spacing: 6.0,
                menu_width: 0.0,
                tooltip_width: 500.0,
                combo_width: 0.0,
                combo_height: 200.0,
                text_edit_width: 280.0,
                menu_spacing: 0.0,
                scroll_bar_width: 8.0,
                scroll_floating: false,
            },
            shape: Shape {
                corner_radius: 0,
                window_corner_radius: 0,
                menu_corner_radius: 0,
                window_stroke: Stroke::new(1.0, Rgba::rgb(70, 60, 64)),
                window_shadow: Shadow::new([4, 4], 0, 0, Rgba::rgb(0, 0, 0)),
                popup_shadow: Shadow::new([3, 3], 0, 0, Rgba::rgb(0, 0, 0)),
                selection_stroke: Stroke::new(1.0, Rgba::rgb(255, 255, 255)),
                tab_underline_width: 2.0,
                selected_row_bar_width: 3.0,
            },
            widgets: WidgetStates::brutalist(),
            fonts: ThemeFontPaths::default(),
        }
    }

    /// Apply the present items of `overrides` on top of this theme.
    ///
    /// # Errors
    ///
    /// A [`ThemeError`] naming the first item the document gets wrong. The
    /// theme this was called on is left unchanged.
    pub fn merged_with(&self, overrides: &ThemeOverrides) -> Result<Self, ThemeError> {
        let mut theme = self.clone();
        if let Some(name) = &overrides.name {
            theme.name = name.clone();
        }
        if let Some(dark_mode) = overrides.dark_mode {
            theme.dark_mode = dark_mode;
        }
        merge_palette(&mut theme.palette, &overrides.palette)?;
        merge_text_styles(&mut theme.text_styles, &overrides.text_styles)?;
        merge_spacing(&mut theme.spacing, &overrides.spacing)?;
        merge_shape(&mut theme.shape, &overrides.shape)?;
        merge_widgets(&mut theme.widgets, &overrides.widgets)?;
        merge_fonts(&mut theme.fonts, &overrides.fonts);
        Ok(theme)
    }
}

/// The neutral fallback's five named text styles.
fn fallback_text_styles() -> BTreeMap<String, TextStyle> {
    let mut styles = BTreeMap::new();
    styles.insert(
        "small".to_owned(),
        TextStyle::new(11.0, FontFamilyName::Proportional),
    );
    styles.insert(
        "body".to_owned(),
        TextStyle::new(13.0, FontFamilyName::Proportional),
    );
    styles.insert(
        "button".to_owned(),
        TextStyle::new(13.0, FontFamilyName::Proportional),
    );
    styles.insert(
        "monospace".to_owned(),
        TextStyle::new(12.0, FontFamilyName::Monospace),
    );
    styles.insert(
        "heading".to_owned(),
        TextStyle::new(15.0, FontFamilyName::Proportional),
    );
    styles
}

// =============================================================================
// Errors
// =============================================================================

/// A refused theme document or item.
#[derive(Debug, Error)]
pub enum ThemeError {
    /// The document did not parse as YAML, or did not fit the override shape.
    #[error("theme document: {0}")]
    Format(String),
    /// One item's value was refused.
    #[error("`{item}`: {detail}")]
    Item {
        /// The dotted path of the refused item.
        item: String,
        /// Why the value was refused.
        detail: String,
    },
}

impl ThemeError {
    /// Builds the error for one refused item, from its dotted path and the
    /// reason it was refused.
    fn item(item: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::Item {
            item: item.into(),
            detail: detail.into(),
        }
    }
}

// =============================================================================
// Merging
// =============================================================================

/// Parse a colour and name it on failure.
///
/// # Errors
///
/// [`ThemeError::Item`] naming `item` and the reason [`Rgba::parse`] refused
/// `text`.
fn parse_color(item: &str, text: &str) -> Result<Rgba, ThemeError> {
    Rgba::parse(text).map_err(|detail| ThemeError::item(item, detail))
}

/// Returns `value` when it is finite and non-negative, or an error naming
/// `item` otherwise.
///
/// # Errors
///
/// [`ThemeError::Item`] naming `item` when `value` is negative, infinite or
/// NaN.
fn check_number(item: &str, value: f32) -> Result<f32, ThemeError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(ThemeError::item(
            item,
            format!("`{value}` is not a finite, non-negative number"),
        ))
    }
}

/// Replace every colour role the document names.
///
/// # Errors
///
/// [`ThemeError::Item`] naming the first colour the document does not parse.
fn merge_palette(palette: &mut Palette, overrides: &PaletteOverrides) -> Result<(), ThemeError> {
    macro_rules! colours {
        ($( $field:ident => $item:literal ),* $(,)?) => {{
            $(
                if let Some(value) = overrides.$field.as_deref() {
                    palette.$field = parse_color($item, value)?;
                }
            )*
        }};
    }
    colours!(
        frame => "palette.frame",
        panel => "palette.panel",
        panel_raised => "palette.panel_raised",
        control => "palette.control",
        well => "palette.well",
        line => "palette.line",
        text => "palette.text",
        text_strong => "palette.text_strong",
        text_muted => "palette.text_muted",
        accent => "palette.accent",
        accent_hot => "palette.accent_hot",
        success => "palette.success",
        warning => "palette.warning",
        danger => "palette.danger",
        selection => "palette.selection",
        selection_bar => "palette.selection_bar",
        overlay => "palette.overlay",
        overlay_border => "palette.overlay_border",
        log_info => "palette.log_info",
        log_debug => "palette.log_debug",
        log_warn => "palette.log_warn",
        log_error => "palette.log_error",
    );
    Ok(())
}

/// Replace the named text styles the document names, adding new ones.
///
/// # Errors
///
/// [`ThemeError::Item`] naming a size that is not positive, or a family the
/// document does not accept.
fn merge_text_styles(
    styles: &mut BTreeMap<String, TextStyle>,
    overrides: &BTreeMap<String, TextStyleOverride>,
) -> Result<(), ThemeError> {
    for (name, overrides) in overrides {
        let item = format!("text_styles.{name}");
        let style = styles
            .entry(name.clone())
            .or_insert(TextStyle::new(14.0, FontFamilyName::Proportional));
        if let Some(size) = overrides.size {
            if !size.is_finite() || size <= 0.0 {
                return Err(ThemeError::item(
                    format!("{item}.size"),
                    format!("`{size}` is not a positive size"),
                ));
            }
            style.size = size;
        }
        if let Some(family) = overrides.family.as_deref() {
            style.family = FontFamilyName::parse(family)
                .map_err(|detail| ThemeError::item(format!("{item}.family"), detail))?;
        }
    }
    Ok(())
}

/// Replace the metrics the document names.
///
/// # Errors
///
/// [`ThemeError::Item`] naming the first number that is negative, infinite or
/// NaN, or a tooltip width that is not positive.
fn merge_spacing(spacing: &mut Spacing, overrides: &SpacingOverrides) -> Result<(), ThemeError> {
    macro_rules! numbers {
        ($( $field:ident => $item:literal ),* $(,)?) => {{
            $(
                if let Some(value) = overrides.$field {
                    spacing.$field = check_number($item, value)?;
                }
            )*
        }};
    }
    if let Some(value) = overrides.item_spacing {
        spacing.item_spacing = [
            check_number("spacing.item_spacing[0]", value[0])?,
            check_number("spacing.item_spacing[1]", value[1])?,
        ];
    }
    if let Some(value) = overrides.button_padding {
        spacing.button_padding = [
            check_number("spacing.button_padding[0]", value[0])?,
            check_number("spacing.button_padding[1]", value[1])?,
        ];
    }
    if let Some(value) = overrides.interact_size {
        spacing.interact_size = [
            check_number("spacing.interact_size[0]", value[0])?,
            check_number("spacing.interact_size[1]", value[1])?,
        ];
    }
    numbers!(
        window_margin => "spacing.window_margin",
        menu_margin => "spacing.menu_margin",
        indent => "spacing.indent",
        slider_width => "spacing.slider_width",
        slider_rail_height => "spacing.slider_rail_height",
        icon_width => "spacing.icon_width",
        icon_width_inner => "spacing.icon_width_inner",
        icon_spacing => "spacing.icon_spacing",
        menu_width => "spacing.menu_width",
        combo_width => "spacing.combo_width",
        combo_height => "spacing.combo_height",
        text_edit_width => "spacing.text_edit_width",
        menu_spacing => "spacing.menu_spacing",
        scroll_bar_width => "spacing.scroll_bar_width",
    );
    // A tooltip is laid out at exactly its named width. A zero or negative
    // width would wrap the text one character per line, so it is refused
    // rather than passed through.
    if let Some(value) = overrides.tooltip_width {
        if !value.is_finite() || value <= 0.0 {
            return Err(ThemeError::item(
                "spacing.tooltip_width",
                format!("`{value}` is not a positive width"),
            ));
        }
        spacing.tooltip_width = value;
    }
    if let Some(value) = overrides.scroll_floating {
        spacing.scroll_floating = value;
    }
    Ok(())
}

/// Replace one stroke's present fields.
///
/// # Errors
///
/// [`ThemeError::Item`] naming a width or colour the document states
/// incorrectly.
fn merge_stroke(
    stroke: &mut Stroke,
    item: &str,
    overrides: &StrokeOverride,
) -> Result<(), ThemeError> {
    if let Some(width) = overrides.width {
        stroke.width = check_number(&format!("{item}.width"), width)?;
    }
    if let Some(color) = overrides.color.as_deref() {
        stroke.color = parse_color(&format!("{item}.color"), color)?;
    }
    Ok(())
}

/// Replace one shadow's present fields.
///
/// # Errors
///
/// [`ThemeError::Item`] naming an offset, blur or spread that does not fit its
/// integer field, or a colour that does not parse.
fn merge_shadow(
    shadow: &mut Shadow,
    item: &str,
    overrides: &ShadowOverride,
) -> Result<(), ThemeError> {
    if let Some(offset) = overrides.offset {
        if offset.iter().any(|value| !(-128..=127).contains(value)) {
            return Err(ThemeError::item(
                format!("{item}.offset"),
                format!("`{offset:?}` does not fit egui's `[i8; 2]` offset"),
            ));
        }
        shadow.offset = [offset[0] as i8, offset[1] as i8];
    }
    if let Some(blur) = overrides.blur {
        if !(0..=255).contains(&blur) {
            return Err(ThemeError::item(
                format!("{item}.blur"),
                format!("`{blur}` does not fit egui's `u8` blur"),
            ));
        }
        shadow.blur = blur as u8;
    }
    if let Some(spread) = overrides.spread {
        if !(0..=255).contains(&spread) {
            return Err(ThemeError::item(
                format!("{item}.spread"),
                format!("`{spread}` does not fit egui's `u8` spread"),
            ));
        }
        shadow.spread = spread as u8;
    }
    if let Some(color) = overrides.color.as_deref() {
        shadow.color = parse_color(&format!("{item}.color"), color)?;
    }
    Ok(())
}

/// Replace the shape items the document names.
///
/// # Errors
///
/// [`ThemeError::Item`] from a stroke, a shadow, or a width that is negative,
/// infinite or NaN.
fn merge_shape(shape: &mut Shape, overrides: &ShapeOverrides) -> Result<(), ThemeError> {
    if let Some(value) = overrides.corner_radius {
        shape.corner_radius = value;
    }
    if let Some(value) = overrides.window_corner_radius {
        shape.window_corner_radius = value;
    }
    if let Some(value) = overrides.menu_corner_radius {
        shape.menu_corner_radius = value;
    }
    if let Some(value) = overrides.window_stroke.as_ref() {
        merge_stroke(&mut shape.window_stroke, "shape.window_stroke", value)?;
    }
    if let Some(value) = overrides.window_shadow.as_ref() {
        merge_shadow(&mut shape.window_shadow, "shape.window_shadow", value)?;
    }
    if let Some(value) = overrides.popup_shadow.as_ref() {
        merge_shadow(&mut shape.popup_shadow, "shape.popup_shadow", value)?;
    }
    if let Some(value) = overrides.selection_stroke.as_ref() {
        merge_stroke(&mut shape.selection_stroke, "shape.selection_stroke", value)?;
    }
    if let Some(value) = overrides.tab_underline_width {
        shape.tab_underline_width = check_number("shape.tab_underline_width", value)?;
    }
    if let Some(value) = overrides.selected_row_bar_width {
        shape.selected_row_bar_width = check_number("shape.selected_row_bar_width", value)?;
    }
    Ok(())
}

/// Replace one widget state's present fields.
///
/// # Errors
///
/// [`ThemeError::Item`] naming a colour, a stroke or an expansion the document
/// states incorrectly.
fn merge_widget(
    widget: &mut WidgetStyle,
    item: &str,
    overrides: &WidgetStyleOverride,
) -> Result<(), ThemeError> {
    if let Some(value) = overrides.bg_fill.as_deref() {
        widget.bg_fill = parse_color(&format!("{item}.bg_fill"), value)?;
    }
    if let Some(value) = overrides.weak_bg_fill.as_deref() {
        widget.weak_bg_fill = parse_color(&format!("{item}.weak_bg_fill"), value)?;
    }
    if let Some(value) = overrides.bg_stroke.as_ref() {
        merge_stroke(&mut widget.bg_stroke, &format!("{item}.bg_stroke"), value)?;
    }
    if let Some(value) = overrides.fg_stroke.as_ref() {
        merge_stroke(&mut widget.fg_stroke, &format!("{item}.fg_stroke"), value)?;
    }
    if let Some(value) = overrides.corner_radius {
        widget.corner_radius = value;
    }
    if let Some(value) = overrides.expansion {
        widget.expansion = check_number(&format!("{item}.expansion"), value)?;
    }
    Ok(())
}

/// Replace the fields the document names in each widget state.
///
/// # Errors
///
/// [`ThemeError::Item`] from the first state the document states incorrectly.
fn merge_widgets(
    widgets: &mut WidgetStates,
    overrides: &WidgetsOverrides,
) -> Result<(), ThemeError> {
    merge_widget(
        &mut widgets.noninteractive,
        "widgets.noninteractive",
        &overrides.noninteractive,
    )?;
    merge_widget(
        &mut widgets.inactive,
        "widgets.inactive",
        &overrides.inactive,
    )?;
    merge_widget(&mut widgets.hovered, "widgets.hovered", &overrides.hovered)?;
    merge_widget(&mut widgets.active, "widgets.active", &overrides.active)?;
    merge_widget(&mut widgets.open, "widgets.open", &overrides.open)?;
    Ok(())
}

/// Replace the font paths the document names.
fn merge_fonts(fonts: &mut ThemeFontPaths, overrides: &FontsOverrides) {
    if let Some(path) = &overrides.body {
        fonts.body = Some(path.clone());
    }
    if let Some(path) = &overrides.display {
        fonts.display = Some(path.clone());
    }
    if let Some(path) = &overrides.monospace {
        fonts.monospace = Some(path.clone());
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overrides::ThemeOverrides;

    /// A document naming only the accent.
    fn accent_only() -> ThemeOverrides {
        ThemeOverrides::from_yaml_str("palette:\n  accent: \"#33d17a\"\n").unwrap()
    }

    #[test]
    fn a_partial_document_changes_only_the_items_it_names() {
        let base = Theme::fallback();
        let merged = base.merged_with(&accent_only()).unwrap();

        assert_eq!(merged.palette.accent, Rgba::rgb(0x33, 0xd1, 0x7a));
        // Every other value stays exactly as the base has it.
        let mut expected = base.clone();
        expected.palette.accent = merged.palette.accent;
        assert_eq!(merged, expected);
    }

    #[test]
    fn an_empty_document_equals_the_base() {
        let base = Theme::brutalist();
        let merged = base.merged_with(&ThemeOverrides::default()).unwrap();
        assert_eq!(merged, base);
    }

    /// A layer that names one family keeps the other two. Font paths merge per
    /// family, so a project that names only `body` keeps the user's
    /// `monospace`.
    #[test]
    fn font_paths_merge_per_family() {
        let lower = ThemeOverrides::from_yaml_str(
            "fonts:\n  body: fonts/body.ttf\n  monospace: fonts/code.ttf\n",
        )
        .unwrap();
        let upper = ThemeOverrides::from_yaml_str("fonts:\n  body: fonts/other.ttf\n").unwrap();

        let merged = Theme::fallback()
            .merged_with(&lower)
            .unwrap()
            .merged_with(&upper)
            .unwrap();

        assert_eq!(merged.fonts.body.as_deref(), Some("fonts/other.ttf"));
        assert_eq!(merged.fonts.monospace.as_deref(), Some("fonts/code.ttf"));
        assert_eq!(merged.fonts.display, None);
    }

    #[test]
    fn two_documents_merge_in_order() {
        let base = Theme::fallback();
        let first = ThemeOverrides::from_yaml_str(
            "palette:\n  accent: \"#111111\"\nspacing:\n  indent: 20.0\n",
        )
        .unwrap();
        let second =
            ThemeOverrides::from_yaml_str("palette:\n  accent: \"#222222\"\n  text: \"#eeeeee\"\n")
                .unwrap();

        let merged = base
            .merged_with(&first)
            .unwrap()
            .merged_with(&second)
            .unwrap();

        // The later document wins on the item both name.
        assert_eq!(merged.palette.accent, Rgba::rgb(0x22, 0x22, 0x22));
        // Each document keeps the item only it names.
        assert_eq!(merged.palette.text, Rgba::rgb(0xee, 0xee, 0xee));
        assert_eq!(merged.spacing.indent, 20.0);
    }

    #[test]
    fn a_bad_colour_is_refused_with_its_item() {
        let document = ThemeOverrides::from_yaml_str("palette:\n  accent: nope\n").unwrap();
        let error = Theme::fallback().merged_with(&document).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("palette.accent"), "{message}");
    }

    #[test]
    fn an_out_of_range_shadow_is_refused_with_its_item() {
        let document =
            ThemeOverrides::from_yaml_str("shape:\n  window_shadow:\n    blur: 300\n").unwrap();
        let error = Theme::fallback().merged_with(&document).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("shape.window_shadow.blur"), "{message}");
    }

    #[test]
    fn a_non_positive_text_size_is_refused() {
        let document =
            ThemeOverrides::from_yaml_str("text_styles:\n  body:\n    size: 0.0\n").unwrap();
        let error = Theme::fallback().merged_with(&document).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("text_styles.body.size"), "{message}");
    }

    #[test]
    fn a_non_positive_tooltip_width_is_refused() {
        let document = ThemeOverrides::from_yaml_str("spacing:\n  tooltip_width: 0.0\n").unwrap();
        let error = Theme::fallback().merged_with(&document).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("spacing.tooltip_width"), "{message}");
    }

    #[test]
    fn the_code_themes_give_tooltips_a_usable_width() {
        assert!(Theme::fallback().spacing.tooltip_width > 0.0);
        assert!(Theme::brutalist().spacing.tooltip_width > 0.0);
    }

    #[test]
    fn a_refused_document_leaves_the_base_untouched() {
        let base = Theme::fallback();
        let document = ThemeOverrides::from_yaml_str("palette:\n  accent: nope\n").unwrap();
        assert!(base.merged_with(&document).is_err());
        assert_eq!(base, Theme::fallback());
    }

    #[test]
    fn brutalist_matches_the_recipe() {
        let theme = Theme::brutalist();
        assert_eq!(theme.palette.accent, Rgba::rgb(255, 72, 140));
        assert_eq!(theme.palette.frame, Rgba::rgb(20, 17, 18));
        assert_eq!(theme.palette.text, Rgba::rgb(232, 228, 230));
        assert_eq!(theme.shape.corner_radius, 0);
        assert_eq!(
            theme.shape.window_shadow,
            Shadow::new([4, 4], 0, 0, Rgba::rgb(0, 0, 0))
        );
        assert_eq!(theme.spacing.button_padding, [10.0, 5.0]);
        assert_eq!(theme.spacing.interact_size, [40.0, 26.0]);
        assert_eq!(theme.text_styles["heading"].size, 22.0);
        assert_eq!(theme.text_styles["body"].size, 14.0);
        assert_eq!(
            theme.text_styles["body"].family,
            FontFamilyName::Proportional
        );
        assert_eq!(
            theme.text_styles["monospace"].family,
            FontFamilyName::Monospace
        );
        assert!(theme.dark_mode);
    }

    #[test]
    fn rgb_parses_six_and_eight_digit_hex() {
        assert_eq!(Rgba::parse("#ff488c").unwrap(), Rgba::rgb(255, 72, 140));
        assert_eq!(
            Rgba::parse("#ff488c80").unwrap(),
            Rgba::with_alpha(255, 72, 140, 0x80)
        );
        assert!(Rgba::parse("ff488c").is_err());
        assert!(Rgba::parse("#ff48").is_err());
        assert!(Rgba::parse("#gggggg").is_err());
    }
}
