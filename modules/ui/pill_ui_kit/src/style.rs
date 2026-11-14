//! The token-to-egui translator and the font installer.
//!
//! # Responsibilities
//!
//! - [`build_style`]: builds a complete [`egui::Style`] from a
//!   [`Theme`](pill_ui_theme::Theme).
//! - [`font_definitions`]: builds the font definitions - egui's defaults, the
//!   theme's fonts in front of the caller's built-in families, and every
//!   family the translated styles name.
//! - [`install`]: puts the fonts on a context first, and only when their
//!   signature changed (the return says whether it did). Then it writes the
//!   style to both egui themes and pins the dark theme.
//! - [`FontSignature`]: the record the caller keeps, so that `set_fonts` runs
//!   only when the fonts changed.
//!
//! # Design
//!
//! Every entry point is a plain function that the caller's dylib compiles in,
//! so nothing this kit hands egui stores code from a reloadable image. The
//! context only ever receives plain data and owned font bytes, and the kit
//! holds no statics - the font signature, the font bytes and the theme all
//! belong to the caller. Fonts are installed before the style, because a style
//! that names a family nobody registered makes egui panic on the next pass.

// Standard library
use std::hash::{Hash, Hasher};
use std::sync::Arc;

// External crates
use egui::epaint::Shadow;
use egui::style::{ScrollStyle, Selection};
use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin,
    Stroke as EguiStroke, TextStyle, Theme as EguiTheme, ThemePreference, Vec2,
};

// Current crate
use pill_ui_theme::{FontFamilyName, Rgba, Stroke, Theme, WidgetStyle};

// =============================================================================
// Font keys
// =============================================================================

/// The family name a theme's `display` font is registered under.
pub const DISPLAY_FAMILY: &str = "display";

/// The font-definition key a theme's body font is registered under.
pub const THEME_BODY_KEY: &str = "theme.body";
/// The font-definition key a theme's display font is registered under.
pub const THEME_DISPLAY_KEY: &str = "theme.display";
/// The font-definition key a theme's monospace font is registered under.
pub const THEME_MONOSPACE_KEY: &str = "theme.monospace";

/// Whether `font_key` names one of the theme's own fonts.
///
/// A caller that adds its built-in families uses this to keep the theme's font
/// in front. The theme font is what a theme layer asked for; the caller's font
/// is only the fallback behind it.
pub fn is_theme_font(font_key: &str) -> bool {
    font_key.starts_with("theme.")
}

// =============================================================================
// Font records
// =============================================================================

/// The font bytes the theme's layers supplied.
///
/// The kit installs its own copy of each font. The game copies the bytes out of
/// its asset store; the editor's user layer reads them from the file it names.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeFonts {
    /// The body font's bytes.
    pub body: Option<Vec<u8>>,
    /// The display font's bytes.
    pub display: Option<Vec<u8>>,
    /// The monospace font's bytes.
    pub monospace: Option<Vec<u8>>,
}

/// The font set last installed, recorded by the caller.
///
/// [`install`] calls `set_fonts` when the signature is new - also when it is
/// fresh, as it is for a new context - and skips the call when the signature
/// has not changed. Skipping matters because every `set_fonts` call compares
/// the whole font data, which is expensive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontSignature(Option<u64>);

impl FontSignature {
    /// Computes the signature of `fonts` from the bytes of all three families.
    pub fn of(fonts: &ThemeFonts) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for bytes in [&fonts.body, &fonts.display, &fonts.monospace] {
            match bytes {
                Some(bytes) => {
                    1u8.hash(&mut hasher);
                    bytes.hash(&mut hasher);
                }
                None => 0u8.hash(&mut hasher),
            }
        }
        Self(Some(hasher.finish()))
    }
}

impl std::fmt::Display for FontSignature {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(hash) => write!(formatter, "{hash:016x}"),
            None => formatter.write_str("none"),
        }
    }
}

// =============================================================================
// Style and font installation
// =============================================================================

/// Builds egui's style from `theme`, starting from `base_style`.
///
/// Every field the theme owns is overwritten. Everything else keeps the value
/// `base_style` has.
pub fn build_style(theme: &Theme, base_style: &egui::Style) -> egui::Style {
    // Step 1: the colours, strokes, shadows and the five widget states.
    let mut style = base_style.clone();
    let visuals = &mut style.visuals;
    visuals.dark_mode = theme.dark_mode;
    // Each widget state gives its text colour in `fg_stroke`.
    // `override_text_color` would replace all of them with one colour, so it
    // is cleared.
    visuals.override_text_color = None;
    visuals.panel_fill = color(theme.palette.panel);
    visuals.window_fill = color(theme.palette.panel);
    visuals.window_stroke = stroke(theme.shape.window_stroke);
    visuals.window_corner_radius = CornerRadius::same(theme.shape.window_corner_radius);
    visuals.menu_corner_radius = CornerRadius::same(theme.shape.menu_corner_radius);
    visuals.window_shadow = shadow(theme.shape.window_shadow);
    visuals.popup_shadow = shadow(theme.shape.popup_shadow);
    visuals.extreme_bg_color = color(theme.palette.well);
    visuals.faint_bg_color = color(theme.palette.panel_raised);
    visuals.code_bg_color = color(theme.palette.well);
    visuals.selection = Selection {
        bg_fill: color(theme.palette.selection),
        stroke: stroke(theme.shape.selection_stroke),
    };
    visuals.hyperlink_color = color(theme.palette.accent_hot);
    visuals.warn_fg_color = color(theme.palette.warning);
    visuals.error_fg_color = color(theme.palette.danger);
    visuals.slider_trailing_fill = true;
    visuals.widgets.noninteractive = widget_visuals(theme.widgets.noninteractive);
    visuals.widgets.inactive = widget_visuals(theme.widgets.inactive);
    visuals.widgets.hovered = widget_visuals(theme.widgets.hovered);
    visuals.widgets.active = widget_visuals(theme.widgets.active);
    visuals.widgets.open = widget_visuals(theme.widgets.open);

    // Step 2: the spacing values.
    let spacing = &mut style.spacing;
    spacing.item_spacing = vec2(theme.spacing.item_spacing);
    spacing.button_padding = vec2(theme.spacing.button_padding);
    spacing.interact_size = vec2(theme.spacing.interact_size);
    spacing.window_margin = margin(theme.spacing.window_margin);
    spacing.menu_margin = margin(theme.spacing.menu_margin);
    spacing.indent = theme.spacing.indent;
    spacing.slider_width = theme.spacing.slider_width;
    spacing.slider_rail_height = theme.spacing.slider_rail_height;
    spacing.icon_width = theme.spacing.icon_width;
    spacing.icon_width_inner = theme.spacing.icon_width_inner;
    spacing.icon_spacing = theme.spacing.icon_spacing;
    spacing.menu_width = theme.spacing.menu_width;
    spacing.tooltip_width = theme.spacing.tooltip_width;
    spacing.combo_width = theme.spacing.combo_width;
    spacing.combo_height = theme.spacing.combo_height;
    spacing.text_edit_width = theme.spacing.text_edit_width;
    spacing.menu_spacing = theme.spacing.menu_spacing;
    // The theme chooses the scrollbar's width and whether it floats. Every
    // other scrollbar setting comes from egui's floating or solid preset.
    spacing.scroll = if theme.spacing.scroll_floating {
        ScrollStyle {
            bar_width: theme.spacing.scroll_bar_width,
            ..ScrollStyle::floating()
        }
    } else {
        ScrollStyle {
            bar_width: theme.spacing.scroll_bar_width,
            ..ScrollStyle::solid()
        }
    };

    // Step 3: the text styles, each by name and family.
    let mut text_styles = style.text_styles.clone();
    for (name, text_style) in &theme.text_styles {
        text_styles.insert(
            text_style_key(name),
            FontId::new(text_style.size, text_style_family(text_style.family)),
        );
    }
    style.text_styles = text_styles;
    style
}

/// Builds the font definitions from `fonts` and the caller's built-in families.
///
/// Within a family the order is: the theme's font first, then the caller's
/// built-ins, then egui's defaults. The theme's fonts are added before
/// `append_builtin_fonts` runs, so a family the caller builds by copying the
/// proportional list (an icon font's fallback chain, for example) carries the
/// theme's body font as well.
///
/// The `display` family always exists, so a style can name it even when the
/// theme ships no display font. In that case it holds whatever the proportional
/// family holds.
pub fn font_definitions(
    fonts: &ThemeFonts,
    append_builtin_fonts: impl FnOnce(&mut FontDefinitions),
) -> FontDefinitions {
    // Step 1: the theme's body and monospace fonts, ahead of everything else.
    let mut definitions = FontDefinitions::default();
    if let Some(bytes) = &fonts.body {
        definitions.font_data.insert(
            THEME_BODY_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes.clone())),
        );
        definitions
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, THEME_BODY_KEY.to_owned());
    }
    if let Some(bytes) = &fonts.monospace {
        definitions.font_data.insert(
            THEME_MONOSPACE_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes.clone())),
        );
        definitions
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, THEME_MONOSPACE_KEY.to_owned());
    }
    // Step 2: the caller's own families, behind the theme's fonts.
    append_builtin_fonts(&mut definitions);

    // Step 3: the `display` family - the theme's display font, then the
    // proportional family.
    let proportional = definitions
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut display_family = Vec::new();
    if let Some(bytes) = &fonts.display {
        definitions.font_data.insert(
            THEME_DISPLAY_KEY.to_owned(),
            Arc::new(FontData::from_owned(bytes.clone())),
        );
        display_family.push(THEME_DISPLAY_KEY.to_owned());
    }
    display_family.extend(proportional);
    definitions
        .families
        .insert(FontFamily::Name(DISPLAY_FAMILY.into()), display_family);
    definitions
}

/// Installs `theme` on `context`: the fonts first, then the style on both egui
/// themes, then the dark theme pinned.
///
/// `append_builtin_fonts` adds the caller's own families - the editor passes
/// its Inter, JetBrains Mono and Phosphor - and it runs only when the fonts are
/// installed. `signature` belongs to the context, so pass a new one for each
/// new context.
///
/// Returns whether `set_fonts` ran, so the caller can log the install once.
pub fn install(
    context: &Context,
    theme: &Theme,
    fonts: &ThemeFonts,
    append_builtin_fonts: impl FnOnce(&mut FontDefinitions),
    signature: &mut FontSignature,
) -> bool {
    // Step 1: replace the fonts, but only when the signature changed.
    let next_signature = FontSignature::of(fonts);
    let fonts_installed = *signature != next_signature;
    if fonts_installed {
        context.set_fonts(font_definitions(fonts, append_builtin_fonts));
        *signature = next_signature;
    }
    // Step 2: write the style to both egui themes and pin the dark one.
    let style = build_style(theme, &egui::Style::default());
    context.set_style_of(EguiTheme::Dark, style.clone());
    context.set_style_of(EguiTheme::Light, style);
    context.set_theme(ThemePreference::Dark);
    fonts_installed
}

// =============================================================================
// Value conversion
// =============================================================================

/// Returns the egui text-style key for a theme text style's name.
///
/// A name egui has no built-in key for becomes `TextStyle::Name`.
fn text_style_key(style_name: &str) -> TextStyle {
    match style_name {
        "small" => TextStyle::Small,
        "body" => TextStyle::Body,
        "button" => TextStyle::Button,
        "monospace" => TextStyle::Monospace,
        "heading" => TextStyle::Heading,
        other => TextStyle::Name(other.into()),
    }
}

/// Returns the egui font family for a theme font family.
///
/// `Display` maps to the named `display` family.
fn text_style_family(family: FontFamilyName) -> FontFamily {
    match family {
        FontFamilyName::Proportional => FontFamily::Proportional,
        FontFamilyName::Monospace => FontFamily::Monospace,
        FontFamilyName::Display => FontFamily::Name(DISPLAY_FAMILY.into()),
    }
}

/// Converts a theme colour to an egui colour. Both use straight alpha.
fn color(theme_color: Rgba) -> Color32 {
    let [red, green, blue, alpha] = theme_color.0;
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

/// Converts a theme stroke to an egui stroke.
fn stroke(theme_stroke: Stroke) -> EguiStroke {
    EguiStroke::new(theme_stroke.width, color(theme_stroke.color))
}

/// Converts a theme widget style to the egui look of one widget state.
fn widget_visuals(theme_widget: WidgetStyle) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: color(theme_widget.bg_fill),
        weak_bg_fill: color(theme_widget.weak_bg_fill),
        bg_stroke: stroke(theme_widget.bg_stroke),
        corner_radius: CornerRadius::same(theme_widget.corner_radius),
        fg_stroke: stroke(theme_widget.fg_stroke),
        expansion: theme_widget.expansion,
    }
}

/// Converts a theme shadow to an egui shadow. Both use whole-point fields.
fn shadow(theme_shadow: pill_ui_theme::Shadow) -> Shadow {
    Shadow {
        offset: theme_shadow.offset,
        blur: theme_shadow.blur,
        spread: theme_shadow.spread,
        color: color(theme_shadow.color),
    }
}

/// Converts a theme's `[x, y]` pair to an egui vector.
fn vec2(theme_pair: [f32; 2]) -> Vec2 {
    Vec2::new(theme_pair[0], theme_pair[1])
}

/// Converts one theme number to an egui margin.
///
/// egui stores a margin as an `i8`, so the value is rounded and clamped to
/// `0..=127`.
fn margin(theme_margin: f32) -> Margin {
    Margin::same(theme_margin.round().clamp(0.0, 127.0) as i8)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{RawInput, ThemePreference};
    use pill_ui_theme::{Spacing, Theme, ThemeOverrides};

    fn themed(overrides: &str) -> Theme {
        Theme::fallback()
            .merged_with(&ThemeOverrides::from_yaml_str(overrides).unwrap())
            .unwrap()
    }

    #[test]
    fn every_widget_state_maps_to_its_token() {
        let theme = themed(
            r##"{
                "widgets": {
                    "inactive": {"weak_bg_fill": "#112233"},
                    "hovered": {"bg_stroke": {"width": 3.0, "color": "#445566"}},
                    "active": {"fg_stroke": {"width": 2.0, "color": "#778899"}}
                }
            }"##,
        );
        let style = build_style(&theme, &egui::Style::default());
        assert_eq!(
            style.visuals.widgets.inactive.weak_bg_fill,
            color(Rgba::rgb(0x11, 0x22, 0x33))
        );
        assert_eq!(style.visuals.widgets.hovered.bg_stroke.width, 3.0);
        assert_eq!(
            style.visuals.widgets.hovered.bg_stroke.color,
            color(Rgba::rgb(0x44, 0x55, 0x66))
        );
        assert_eq!(style.visuals.widgets.active.fg_stroke.width, 2.0);
        assert_eq!(
            style.visuals.widgets.active.fg_stroke.color,
            color(Rgba::rgb(0x77, 0x88, 0x99))
        );
    }

    #[test]
    fn override_text_color_stays_none() {
        let style = build_style(&Theme::brutalist(), &egui::Style::default());
        assert!(style.visuals.override_text_color.is_none());
    }

    #[test]
    fn dark_mode_follows_the_token() {
        let theme = themed(r#"{"dark_mode": false}"#);
        let style = build_style(&theme, &egui::Style::default());
        assert!(!style.visuals.dark_mode);
    }

    #[test]
    fn switching_to_the_light_theme_still_yields_the_theme_visuals() {
        let theme = Theme::brutalist();
        let context = Context::default();
        let mut signature = FontSignature::default();
        install(
            &context,
            &theme,
            &ThemeFonts::default(),
            |_| {},
            &mut signature,
        );
        context.set_theme(ThemePreference::Light);
        let style = context.global_style();
        assert_eq!(style.visuals.panel_fill, color(theme.palette.panel));
    }

    #[test]
    fn the_display_family_is_always_registered() {
        let theme = Theme::brutalist();
        let definitions = font_definitions(&ThemeFonts::default(), |_| {});
        let display = definitions
            .families
            .get(&FontFamily::Name(DISPLAY_FAMILY.into()))
            .expect("the display family");
        assert!(!display.is_empty());

        let context = Context::default();
        let mut signature = FontSignature::default();
        install(
            &context,
            &theme,
            &ThemeFonts::default(),
            |_| {},
            &mut signature,
        );
        // A pass that lays out text in the named family must not panic.
        let mut pass_output = context.run_ui(RawInput::default(), |ui| {
            let text =
                egui::RichText::new("display").family(FontFamily::Name(DISPLAY_FAMILY.into()));
            ui.label(text);
        });
        pass_output.textures_delta.clear();
    }

    #[test]
    fn an_unchanged_font_signature_does_not_install_fonts_again() {
        let context = Context::default();
        let theme = Theme::fallback();
        let mut signature = FontSignature::default();
        let mut calls = 0;
        install(
            &context,
            &theme,
            &ThemeFonts::default(),
            |_| calls += 1,
            &mut signature,
        );
        assert_eq!(calls, 1);
        install(
            &context,
            &theme,
            &ThemeFonts::default(),
            |_| calls += 1,
            &mut signature,
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn a_theme_font_goes_first_in_its_family() {
        let fonts = ThemeFonts {
            body: Some(vec![0, 1, 2, 3]),
            ..ThemeFonts::default()
        };
        let definitions = font_definitions(&fonts, |_| {});
        let proportional = definitions
            .families
            .get(&FontFamily::Proportional)
            .expect("the proportional family");
        assert_eq!(proportional.first().map(String::as_str), Some("theme.body"));
    }

    /// The theme's fonts are added before the caller's, so a family the built-in
    /// callback copies the proportional list into carries them too. That is what
    /// lets an icon family draw a label beside its icon in the theme's body font.
    #[test]
    fn a_built_in_family_built_after_the_theme_font_carries_it() {
        let fonts = ThemeFonts {
            body: Some(vec![0, 1, 2, 3]),
            display: Some(vec![4, 5, 6, 7]),
            ..ThemeFonts::default()
        };
        let definitions = font_definitions(&fonts, |definitions| {
            // The shape an icon installer uses: the proportional family as it
            // stands, then its own font.
            let fallbacks = definitions
                .families
                .get(&FontFamily::Proportional)
                .cloned()
                .unwrap_or_default();
            definitions.font_data.insert(
                "builtin.icon".to_owned(),
                Arc::new(FontData::from_static(&[9, 9])),
            );
            let mut family_names = fallbacks;
            family_names.push("builtin.icon".to_owned());
            definitions
                .families
                .insert(FontFamily::Name("icons".into()), family_names);
        });

        let icons = definitions
            .families
            .get(&FontFamily::Name("icons".into()))
            .expect("the built-in family");
        assert_eq!(icons.first().map(String::as_str), Some(THEME_BODY_KEY));
        assert_eq!(icons.last().map(String::as_str), Some("builtin.icon"));
        // The display family starts with its own face, then falls back to the
        // proportional family, which begins with the theme's body font.
        let display = definitions
            .families
            .get(&FontFamily::Name(DISPLAY_FAMILY.into()))
            .expect("the display family");
        assert_eq!(display.first().map(String::as_str), Some(THEME_DISPLAY_KEY));
        assert_eq!(display.get(1).map(String::as_str), Some(THEME_BODY_KEY));
    }

    /// `install` reports whether it ran `set_fonts`: true on the first install
    /// and after a font changes, false when the font set is unchanged. Both UIs
    /// use the answer to log the install.
    #[test]
    fn install_reports_whether_the_fonts_were_installed() {
        let context = Context::default();
        let theme = Theme::fallback();
        let mut signature = FontSignature::default();
        let fonts = ThemeFonts {
            body: Some(vec![0, 1, 2, 3]),
            ..ThemeFonts::default()
        };
        assert!(install(&context, &theme, &fonts, |_| {}, &mut signature));
        assert!(!install(&context, &theme, &fonts, |_| {}, &mut signature));
        let other = ThemeFonts {
            body: Some(vec![3, 2, 1, 0]),
            ..ThemeFonts::default()
        };
        assert!(install(&context, &theme, &other, |_| {}, &mut signature));
        assert_ne!(signature.to_string(), "none");
    }

    #[test]
    fn the_light_and_dark_styles_are_the_same_theme() {
        let theme = Theme::brutalist();
        let context = Context::default();
        let mut signature = FontSignature::default();
        install(
            &context,
            &theme,
            &ThemeFonts::default(),
            |_| {},
            &mut signature,
        );
        assert_eq!(
            context.global_style().visuals.panel_fill,
            color(theme.palette.panel)
        );
        context.set_theme(ThemePreference::Light);
        assert_eq!(
            context.global_style().visuals.panel_fill,
            color(theme.palette.panel)
        );
        context.set_theme(ThemePreference::Dark);
        assert_eq!(
            context.global_style().visuals.panel_fill,
            color(theme.palette.panel)
        );
    }

    /// Checks that the theme's spacing values reach egui's style.
    #[test]
    fn spacing_maps_to_egui() {
        let theme = Theme::brutalist();
        let style = build_style(&theme, &egui::Style::default());
        let Spacing {
            item_spacing,
            button_padding,
            interact_size,
            ..
        } = theme.spacing;
        assert_eq!(style.spacing.item_spacing, vec2(item_spacing));
        assert_eq!(style.spacing.button_padding, vec2(button_padding));
        assert_eq!(style.spacing.interact_size, vec2(interact_size));
        // egui lays a tooltip out at exactly this width, so the theme's value
        // must arrive intact and must be positive.
        assert_eq!(style.spacing.tooltip_width, theme.spacing.tooltip_width);
        assert!(style.spacing.tooltip_width > 0.0);
    }

    #[test]
    fn text_styles_map_by_name_and_family() {
        let style = build_style(&Theme::brutalist(), &egui::Style::default());
        let heading = style.text_styles.get(&TextStyle::Heading).unwrap();
        assert_eq!(heading.size, 22.0);
        assert_eq!(heading.family, FontFamily::Proportional);
        let monospace = style.text_styles.get(&TextStyle::Monospace).unwrap();
        assert_eq!(monospace.family, FontFamily::Monospace);
    }
}
