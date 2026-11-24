//! The editor's egui look: one `Theme` built in code, and the palette the
//! shell reads.
//!
//! # Responsibilities
//!
//! - [`default_theme`]: the editor's look as theme tokens, built in code.
//! - [`ThemeLayer`] and [`UserThemeSelection`]: where a theme layer comes
//!   from - the code default, the user's file, the project's - and the merge
//!   that applies them in order.
//! - [`apply_theme`] and [`apply`]: the one entry point that installs a theme,
//!   and the re-install a pop-out window or the no-shell fallback needs.
//! - [`palette`] and the role accessors ([`text`], [`panel`], [`accent`], ...):
//!   the colours the shell needs beyond egui's own visuals.
//! - [`dock_style`]: the `egui_dock` styling (flat tabs merged with their
//!   bodies, thin separators) built from the same tokens.
//!
//! # Design
//!
//! Styling is egui `Style`/`Visuals` data, never CSS: the shell asks for a role
//! and holds no colour value of its own. Once the record is seeded,
//! [`apply_theme`] is the only code that writes it. The record lives in this
//! crate - a shared dylib that never reloads - so a shell reload leaves it
//! untouched, and the reloaded shell reads the same roles. Everything the
//! record holds is an owned copy - tokens, layer paths, font bytes - so nothing
//! a module image owns outlives it.

// Standard library
use std::sync::{Arc, OnceLock, RwLock};

// External crates
use egui::{Color32, Context, CornerRadius, Stroke};
use pill_core::telemetry::telemetry_target;
use pill_engine::RunMode;
use pill_ui_theme::{
    Rgba, Shadow as TokenShadow, Stroke as TokenStroke, Theme, WidgetStates, WidgetStyle,
};

// Current crate
use crate::kit;

// =============================================================================
// Constants
// =============================================================================

/// The UI font's bytes: Inter, vendored under `assets/` with its OFL text.
const BODY_FONT_BYTES: &[u8] = include_bytes!("../assets/Inter-Variable.ttf");
/// The code font's bytes: JetBrains Mono, vendored under `assets/` with its
/// OFL text.
const MONOSPACE_FONT_BYTES: &[u8] = include_bytes!("../assets/JetBrainsMono-Variable.ttf");
/// The names the two fonts are registered under in egui.
const BODY_FONT_KEY: &str = "editor.body";
const MONOSPACE_FONT_KEY: &str = "editor.monospace";

// =============================================================================
// The recorded palette
// =============================================================================

/// The editor's colour roles as egui colours, copied from the last installed
/// theme.
///
/// The shell reads them through the role accessors below and never names the
/// theme's own `Rgba` values, so changing the theme gives every call site a new
/// colour at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The window frame and tab-strip background.
    pub frame: Color32,
    /// A panel's and tab body's background.
    pub panel: Color32,
    /// A raised surface: menus, hovered rows, tooltips.
    pub panel_raised: Color32,
    /// A flat control's fill on a panel.
    pub control: Color32,
    /// Inputs and wells.
    pub well: Color32,
    /// Borders and separators.
    pub line: Color32,
    /// Normal text.
    pub text: Color32,
    /// Emphasized text: active tab titles, hovered labels.
    pub text_strong: Color32,
    /// Muted text: inactive tab titles, secondary labels.
    pub text_muted: Color32,
    /// The accent: the active tab's underline and other highlighted marks.
    pub accent: Color32,
    /// A brighter accent: the link colour and the face of an accented control
    /// while it is hovered or held.
    pub accent_hot: Color32,
    /// Success and running states.
    pub success: Color32,
    /// Warnings.
    pub warning: Color32,
    /// Errors and destructive actions.
    pub danger: Color32,
    /// A selected row's tint.
    pub selection: Color32,
    /// A selected row's left bar.
    pub selection_bar: Color32,
    /// A floating panel's fill over the scene.
    pub overlay: Color32,
    /// A floating panel's border.
    pub overlay_border: Color32,
    /// The ink a drop shadow is cast in.
    pub shadow: Color32,
    /// A log line at `info`.
    pub log_info: Color32,
    /// A log line at `debug`.
    pub log_debug: Color32,
    /// A log line at `warn`.
    pub log_warn: Color32,
    /// A log line at `error`.
    pub log_error: Color32,
}

impl Palette {
    /// The roles of `theme`.
    fn from_theme(theme: &Theme) -> Self {
        let palette = &theme.palette;
        Self {
            frame: to_egui_color(palette.frame),
            panel: to_egui_color(palette.panel),
            panel_raised: to_egui_color(palette.panel_raised),
            control: to_egui_color(palette.control),
            well: to_egui_color(palette.well),
            line: to_egui_color(palette.line),
            text: to_egui_color(palette.text),
            text_strong: to_egui_color(palette.text_strong),
            text_muted: to_egui_color(palette.text_muted),
            accent: to_egui_color(palette.accent),
            accent_hot: to_egui_color(palette.accent_hot),
            success: to_egui_color(palette.success),
            warning: to_egui_color(palette.warning),
            danger: to_egui_color(palette.danger),
            selection: to_egui_color(palette.selection),
            selection_bar: to_egui_color(palette.selection_bar),
            overlay: to_egui_color(palette.overlay),
            overlay_border: to_egui_color(palette.overlay_border),
            shadow: to_egui_color(theme.shape.window_shadow.color),
            log_info: to_egui_color(palette.log_info),
            log_debug: to_egui_color(palette.log_debug),
            log_warn: to_egui_color(palette.log_warn),
            log_error: to_egui_color(palette.log_error),
        }
    }
}

/// The theme the editor last resolved, with the palette it records.
struct ResolvedTheme {
    /// The tokens every context paints with.
    theme: Theme,
    /// [`ResolvedTheme::theme`] as egui colours.
    palette: Palette,
    /// The layers [`ResolvedTheme::theme`] was merged from, in apply order.
    layers: Vec<ThemeLayerInfo>,
    /// The font bytes the layers resolved.
    fonts: kit::style::ThemeFonts,
    /// The executable's layer generation. When it changes, there is a new
    /// theme to install.
    generation: u64,
    /// How many context installs have run. The log line's `contexts` field
    /// carries it.
    contexts: u64,
    /// Whether a theme has ever been installed, so the first one logs.
    installed: bool,
}

/// The one record every accessor reads.
static RESOLVED_THEME: OnceLock<RwLock<ResolvedTheme>> = OnceLock::new();

/// The record, seeded with the code default so a read before the first install
/// still answers.
fn resolved_theme() -> &'static RwLock<ResolvedTheme> {
    RESOLVED_THEME.get_or_init(|| {
        let theme = default_theme();
        let palette = Palette::from_theme(&theme);
        RwLock::new(ResolvedTheme {
            theme,
            palette,
            layers: vec![ThemeLayerInfo::code()],
            fonts: kit::style::ThemeFonts::default(),
            generation: 1,
            contexts: 0,
            installed: false,
        })
    })
}

/// Reads the record and recovers the data even if another thread panicked
/// while holding the write lock.
fn read_resolved_theme<T>(read: impl FnOnce(&ResolvedTheme) -> T) -> T {
    let record = resolved_theme()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read(&record)
}

/// The palette the last installed theme recorded.
pub fn palette() -> Palette {
    read_resolved_theme(|resolved| resolved.palette)
}

/// The theme the editor last installed.
pub fn current_theme() -> Theme {
    read_resolved_theme(|resolved| resolved.theme.clone())
}

/// The generation of the theme the editor last installed. A shell compares it
/// with the generation it already installed, so it knows when the executable
/// has resolved a new theme.
pub fn generation() -> u64 {
    read_resolved_theme(|resolved| resolved.generation)
}

/// The layers the installed theme was merged from.
pub fn current_layers() -> Vec<ThemeLayerInfo> {
    read_resolved_theme(|resolved| resolved.layers.clone())
}

/// The window frame and tab-strip background.
pub fn frame() -> Color32 {
    palette().frame
}

/// A panel's and tab body's background.
pub fn panel() -> Color32 {
    palette().panel
}

/// A raised surface: menus, hovered rows, tooltips.
pub fn panel_raised() -> Color32 {
    palette().panel_raised
}

/// A flat control's fill on a panel.
pub fn control() -> Color32 {
    palette().control
}

/// Inputs and wells.
pub fn well() -> Color32 {
    palette().well
}

/// Borders and separators.
pub fn line() -> Color32 {
    palette().line
}

/// Normal text.
pub fn text() -> Color32 {
    palette().text
}

/// Emphasized text.
pub fn text_strong() -> Color32 {
    palette().text_strong
}

/// Muted text.
pub fn text_muted() -> Color32 {
    palette().text_muted
}

/// The accent colour: the active tab's underline and other highlights.
pub fn accent() -> Color32 {
    palette().accent
}

/// A brighter accent, used for links and for the face of an accented control
/// while it is hovered or held.
pub fn accent_hot() -> Color32 {
    palette().accent_hot
}

/// Success and running states.
pub fn success() -> Color32 {
    palette().success
}

/// Warnings.
pub fn warning() -> Color32 {
    palette().warning
}

/// Errors and destructive actions.
pub fn danger() -> Color32 {
    palette().danger
}

/// A selected row's tint.
pub fn selection() -> Color32 {
    palette().selection
}

/// A selected row's left bar.
pub fn selection_bar() -> Color32 {
    palette().selection_bar
}

/// A floating panel's fill over the scene.
pub fn overlay() -> Color32 {
    palette().overlay
}

/// A floating panel's border.
pub fn overlay_border() -> Color32 {
    palette().overlay_border
}

/// The colour a drop shadow is drawn in: the theme's window-shadow colour. A
/// mark floating over the scene then casts the same shadow as every other
/// floating surface.
pub fn shadow() -> Color32 {
    palette().shadow
}

/// A log line at `info`.
pub fn log_info() -> Color32 {
    palette().log_info
}

/// A log line at `debug`.
pub fn log_debug() -> Color32 {
    palette().log_debug
}

/// A log line at `warn`.
pub fn log_warn() -> Color32 {
    palette().log_warn
}

/// A log line at `error`.
pub fn log_error() -> Color32 {
    palette().log_error
}

/// A colour a module published for its own command's dots.
///
/// The module owns these values, so the theme does not: it names a status in
/// RGBA and the editor paints it as given. The conversion stays here, beside
/// the token-to-egui one, so the shell never spells a colour itself.
pub fn published(rgba: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(rgba[0], rgba[1], rgba[2], rgba[3])
}

/// Paints the selected row's tinted background and its left accent bar over
/// `rect`.
///
/// The background is the `selection` colour and the bar the `selection_bar`
/// one, three points wide. A widget that already fills its row with the
/// `selection` colour calls [`selected_row_bar`] instead.
pub fn selected_row(ui: &egui::Ui, rect: egui::Rect) {
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, palette().selection);
    selected_row_bar(ui, rect);
}

/// Paints only the selected row's left accent bar.
///
/// Use it for a widget that already fills its row with the `selection` colour,
/// as the Entities list does.
pub fn selected_row_bar(ui: &egui::Ui, rect: egui::Rect) {
    let bar = egui::Rect::from_min_size(rect.left_top(), egui::vec2(3.0, rect.height()));
    ui.painter()
        .rect_filled(bar, egui::CornerRadius::ZERO, palette().selection_bar);
}

// =============================================================================
// The editor's theme
// =============================================================================

/// The editor's look as a complete theme: one flat, compact dark style.
///
/// The palette is near-black panels with a crimson accent for the active tab
/// and the selected row's bar. Controls are compact and draw their text in
/// Inter and JetBrains Mono.
pub fn default_theme() -> Theme {
    let mut theme = Theme::fallback();
    theme.name = "editor".to_owned();
    theme.dark_mode = true;
    theme.palette.frame = Rgba::rgb(18, 18, 20);
    theme.palette.panel = Rgba::rgb(26, 26, 29);
    theme.palette.panel_raised = Rgba::rgb(32, 32, 36);
    theme.palette.control = Rgba::rgb(36, 36, 41);
    theme.palette.well = Rgba::rgb(15, 15, 17);
    theme.palette.line = Rgba::rgb(46, 46, 52);
    theme.palette.text = Rgba::rgb(201, 201, 208);
    theme.palette.text_strong = Rgba::rgb(240, 240, 244);
    theme.palette.text_muted = Rgba::rgb(138, 138, 147);
    // The crimson accent: the active tab's underline and the selected row's
    // bar.
    theme.palette.accent = Rgba::rgb(200, 53, 63);
    theme.palette.accent_hot = Rgba::rgb(224, 69, 80);
    theme.palette.success = Rgba::rgb(111, 191, 115);
    theme.palette.warning = Rgba::rgb(224, 176, 64);
    theme.palette.danger = Rgba::rgb(229, 72, 77);
    // The selected row: a dark crimson tint under the accent bar.
    theme.palette.selection = Rgba::rgb(58, 28, 32);
    theme.palette.selection_bar = Rgba::rgb(200, 53, 63);
    theme.palette.overlay = Rgba::with_alpha(26, 26, 30, 236);
    theme.palette.overlay_border = Rgba::rgb(52, 52, 58);
    theme.palette.log_info = Rgba::rgb(106, 168, 224);
    theme.palette.log_debug = Rgba::rgb(138, 138, 147);
    theme.palette.log_warn = Rgba::rgb(224, 176, 64);
    theme.palette.log_error = Rgba::rgb(229, 72, 77);

    theme.shape.corner_radius = 3;
    theme.shape.window_corner_radius = 5;
    theme.shape.menu_corner_radius = 5;
    theme.shape.window_stroke = TokenStroke::new(1.0, Rgba::rgb(46, 46, 52));
    theme.shape.window_shadow = TokenShadow::new([0, 8], 24, 0, Rgba::with_alpha(0, 0, 0, 160));
    theme.shape.popup_shadow = TokenShadow::new([0, 6], 16, 0, Rgba::with_alpha(0, 0, 0, 160));
    theme.shape.selection_stroke = TokenStroke::new(1.0, Rgba::rgb(240, 240, 244));
    theme.shape.tab_underline_width = 2.0;
    theme.shape.selected_row_bar_width = 3.0;

    // The five widget states: the fill shows the state, the outline stays
    // subtle and the corners stay tight.
    theme.widgets = WidgetStates {
        noninteractive: widget_style(
            Rgba::rgb(26, 26, 29),
            Rgba::rgb(26, 26, 29),
            Some((1.0, Rgba::rgb(46, 46, 52))),
            Rgba::rgb(201, 201, 208),
        ),
        inactive: widget_style(
            Rgba::rgb(38, 38, 43),
            Rgba::rgb(38, 38, 43),
            None,
            Rgba::rgb(201, 201, 208),
        ),
        hovered: widget_style(
            Rgba::rgb(46, 46, 52),
            Rgba::rgb(46, 46, 52),
            Some((1.0, Rgba::rgb(70, 70, 78))),
            Rgba::rgb(240, 240, 244),
        ),
        active: widget_style(
            Rgba::rgb(54, 54, 61),
            Rgba::rgb(54, 54, 61),
            Some((1.0, Rgba::rgb(90, 90, 99))),
            Rgba::rgb(240, 240, 244),
        ),
        open: widget_style(
            Rgba::rgb(46, 46, 52),
            Rgba::rgb(46, 46, 52),
            Some((1.0, Rgba::rgb(70, 70, 78))),
            Rgba::rgb(240, 240, 244),
        ),
    };

    // Compact metrics: tight item spacing and a compact control size.
    theme.spacing.item_spacing = [6.0, 3.0];
    theme.spacing.button_padding = [8.0, 4.0];
    theme.spacing.interact_size = [24.0, 20.0];
    theme.spacing.window_margin = 8.0;
    theme.spacing.menu_margin = 6.0;
    theme.spacing.indent = 16.0;
    theme.spacing.slider_width = 120.0;
    theme.spacing.slider_rail_height = 4.0;
    theme.spacing.icon_width = 14.0;
    theme.spacing.icon_width_inner = 8.0;
    theme.spacing.icon_spacing = 6.0;
    theme.spacing.menu_width = 0.0;
    theme.spacing.tooltip_width = 500.0;
    theme.spacing.combo_width = 0.0;
    theme.spacing.combo_height = 220.0;
    theme.spacing.text_edit_width = 220.0;
    theme.spacing.menu_spacing = 2.0;
    theme.spacing.scroll_bar_width = 8.0;
    theme.spacing.scroll_floating = true;

    theme.text_styles = [
        ("small", 11.0, false),
        ("body", 12.5, false),
        ("button", 12.5, false),
        ("monospace", 12.0, true),
        ("heading", 14.0, false),
    ]
    .into_iter()
    .map(|(name, size, monospace)| {
        let family = if monospace {
            pill_ui_theme::FontFamilyName::Monospace
        } else {
            pill_ui_theme::FontFamilyName::Proportional
        };
        (name.to_owned(), pill_ui_theme::TextStyle::new(size, family))
    })
    .collect();
    theme
}

// =============================================================================
// The theme layers
// =============================================================================

/// Where one layer of the editor's theme came from.
///
/// The executable merges the layers in this order and the shell installs the
/// result. The generation number goes up whenever the merged theme changes,
/// so a shell that was reloaded can tell whether it still paints the theme the
/// executable resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThemeLayer {
    /// The editor's own default, built in code.
    Code,
    /// The user's file, named by the `theme` preference.
    User,
    /// The project's `res/editor/` theme, named by `editor.theme` in the
    /// project's settings.
    Project,
}

impl ThemeLayer {
    /// The layer's name, as the logs and `dump theme` spell it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

impl std::fmt::Display for ThemeLayer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// One layer of a resolved theme: where it came from and, for a file, which
/// path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeLayerInfo {
    /// Which layer this is.
    pub layer: ThemeLayer,
    /// The file the layer loaded, when it loaded one.
    pub path: Option<String>,
}

impl ThemeLayerInfo {
    /// The code default, which has no file.
    pub fn code() -> Self {
        Self {
            layer: ThemeLayer::Code,
            path: None,
        }
    }

    /// The user's file at `path`.
    pub fn user(path: impl Into<String>) -> Self {
        Self {
            layer: ThemeLayer::User,
            path: Some(path.into()),
        }
    }

    /// The project's file at `path`.
    pub fn project(path: impl Into<String>) -> Self {
        Self {
            layer: ThemeLayer::Project,
            path: Some(path.into()),
        }
    }

    /// The path this layer loaded, or `none` for the code default.
    pub fn path_text(&self) -> &str {
        self.path.as_deref().unwrap_or("none")
    }
}

/// The layer names joined as the logs and `dump theme` print them
/// (`code,user,project`).
pub fn layers_text(layers: &[ThemeLayerInfo]) -> String {
    layers
        .iter()
        .map(|info| info.layer.name())
        .collect::<Vec<_>>()
        .join(",")
}

/// What the user's `theme` preference selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UserThemeSelection {
    /// The preference is absent or blank: no user layer.
    #[default]
    None,
    /// A bare theme name such as `dark` or `light`, left over from a preference
    /// that named a theme instead of a file. The executable warns about it and
    /// adds no layer.
    Legacy(String),
    /// A theme file, absolute or relative to the editor's state directory.
    File(std::path::PathBuf),
}

impl UserThemeSelection {
    /// Reads the value of the `theme` preference.
    ///
    /// A value ending in `.theme`, `.yaml` or `.yml` names a file. Anything
    /// else is a bare theme name, which adds no layer. A relative path is
    /// joined to `state_directory`, the folder that holds the preferences
    /// file. A missing or blank value selects [`UserThemeSelection::None`].
    pub fn from_preference(
        preference_text: Option<&str>,
        state_directory: &std::path::Path,
    ) -> Self {
        let Some(preference_text) = preference_text
            .map(str::trim)
            .filter(|text| !text.is_empty())
        else {
            return Self::None;
        };
        if preference_text.ends_with(".theme")
            || preference_text.ends_with(".yaml")
            || preference_text.ends_with(".yml")
        {
            let path = std::path::Path::new(preference_text);
            return Self::File(if path.is_absolute() {
                path.to_path_buf()
            } else {
                state_directory.join(path)
            });
        }
        Self::Legacy(preference_text.to_string())
    }
}

/// The result of merging the layers over the code default.
#[derive(Clone, Debug)]
pub struct ThemeMerge {
    /// The merged theme.
    pub theme: Theme,
    /// The layers that were merged, in apply order; the code default is always
    /// first.
    pub layers: Vec<ThemeLayerInfo>,
    /// The layers a refused override dropped, with the reason.
    pub refused: Vec<(ThemeLayer, String)>,
}

/// Merges `user` and then `project` over `code` and refuses a bad layer on its
/// own.
///
/// A refused layer is skipped and named in [`ThemeMerge::refused`], so one
/// broken item never costs the other layer's values.
/// [`ThemeMerge::layers`] lists only the layers that were applied.
pub fn merge_layers(
    code: &Theme,
    user: Option<(&ThemeLayerInfo, &pill_ui_theme::ThemeOverrides)>,
    project: Option<(&ThemeLayerInfo, &pill_ui_theme::ThemeOverrides)>,
) -> ThemeMerge {
    let mut merge = ThemeMerge {
        theme: code.clone(),
        layers: vec![ThemeLayerInfo::code()],
        refused: Vec::new(),
    };
    for (info, overrides) in [user, project].into_iter().flatten() {
        match merge.theme.merged_with(overrides) {
            Ok(theme) => {
                merge.theme = theme;
                merge.layers.push(info.clone());
            }
            Err(error) => merge.refused.push((info.layer, error.to_string())),
        }
    }
    merge
}

/// One widget state's look: fills, an optional outline and a text colour.
fn widget_style(
    bg_fill: Rgba,
    weak_bg_fill: Rgba,
    outline: Option<(f32, Rgba)>,
    fg_color: Rgba,
) -> WidgetStyle {
    WidgetStyle {
        bg_fill,
        weak_bg_fill,
        bg_stroke: match outline {
            Some((width, outline_color)) => TokenStroke::new(width, outline_color),
            None => TokenStroke::new(0.0, Rgba::with_alpha(0, 0, 0, 0)),
        },
        fg_stroke: TokenStroke::new(1.0, fg_color),
        corner_radius: 3,
        expansion: 0.0,
    }
}

/// Converts a theme colour to an egui colour, keeping its straight (not
/// premultiplied) alpha.
fn to_egui_color(theme_color: Rgba) -> Color32 {
    let [red, green, blue, alpha] = theme_color.0;
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

// =============================================================================
// Applying the theme
// =============================================================================

/// Installs `theme` on `context` and records it as the editor's look.
///
/// The executable merges the layers and the shell installs the result, passing
/// the layers and the generation the theme came from. A context created later
/// (a pop-out window, the no-shell fallback) calls [`apply`] to install the
/// recorded theme. The install goes through the UI kit with `append_fonts` as
/// its callback: a font a theme layer names leads each UI family, the editor's
/// own fonts follow it, and each icon family leads with its Phosphor face.
pub fn apply_theme(
    context: &Context,
    theme: &Theme,
    layers: &[ThemeLayerInfo],
    generation: u64,
    fonts: &kit::style::ThemeFonts,
) {
    // Step 1: store the theme in the record when it changed, and count this
    // install.
    let (contexts, first) = {
        let mut record = resolved_theme()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let changed = record.theme != *theme
            || record.layers != layers
            || record.generation != generation
            || record.fonts != *fonts;
        if changed {
            record.theme = theme.clone();
            record.palette = Palette::from_theme(theme);
            record.layers = layers.to_vec();
            record.generation = generation;
            record.fonts = fonts.clone();
        }
        record.contexts += 1;
        let first = !record.installed || changed;
        record.installed = true;
        (record.contexts, first)
    };
    // Step 2: log once per resolved theme, not once per context, so a pop-out
    // that registers the same theme joins the count without logging again.
    if first {
        let user = layers
            .iter()
            .find(|info| info.layer == ThemeLayer::User)
            .map_or("none", ThemeLayerInfo::path_text);
        let project = layers
            .iter()
            .find(|info| info.layer == ThemeLayer::Project)
            .map_or("none", ThemeLayerInfo::path_text);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            layers = %layers_text(layers),
            user = %user,
            project = %project,
            generation,
            contexts,
            accent = %theme.palette.accent.hex(),
            corner = theme.shape.corner_radius,
            "[editor] theme applied"
        );
    }
    // Step 3: install `theme` and its fonts on this context. The signature is
    // new for each context. Fonts belong to the context, so a pop-out gets
    // them even when the main context already had them.
    let mut signature = kit::style::FontSignature::default();
    let fonts_installed = kit::style::install(context, theme, fonts, append_fonts, &mut signature);
    if fonts_installed {
        pill_core::info!(
            target: telemetry_target::EDITOR,
            signature = %signature,
            "[editor] fonts installed"
        );
    }
}

/// Appends the editor's own families to `fonts`: Inter for the UI, JetBrains
/// Mono for code and numbers, then the icon families. Phosphor joins no UI
/// family, because its regular font also maps the lowercase Latin letters,
/// with empty glyphs.
pub(crate) fn append_fonts(fonts: &mut egui::FontDefinitions) {
    fonts.font_data.insert(
        BODY_FONT_KEY.to_owned(),
        Arc::new(egui::FontData::from_static(BODY_FONT_BYTES)),
    );
    fonts.font_data.insert(
        MONOSPACE_FONT_KEY.to_owned(),
        Arc::new(egui::FontData::from_static(MONOSPACE_FONT_BYTES)),
    );
    for (family, key) in [
        (egui::FontFamily::Proportional, BODY_FONT_KEY),
        (egui::FontFamily::Monospace, MONOSPACE_FONT_KEY),
    ] {
        let names = fonts.families.entry(family).or_default();
        // A theme layer's font is what the layer asked for. Inter and
        // JetBrains Mono are the fallback the editor ships, so they go behind
        // it and ahead of egui's default fonts.
        let index = names
            .iter()
            .take_while(|name| kit::style::is_theme_font(name))
            .count();
        names.insert(index, key.to_owned());
    }
    crate::icons::append_font_definitions(fonts);
}

/// Installs the theme the editor last resolved on `context`.
///
/// A pop-out window and the no-shell fallback call this, so every context
/// paints with the same tokens as the main one.
pub fn apply(context: &Context) {
    let (theme, layers, generation, fonts) = read_resolved_theme(|resolved| {
        (
            resolved.theme.clone(),
            resolved.layers.clone(),
            resolved.generation,
            resolved.fonts.clone(),
        )
    });
    apply_theme(context, &theme, &layers, generation, &fonts);
}

// =============================================================================
// The dock's style
// =============================================================================

/// The tint the menu bar and the dock's frame take while a game runs.
///
/// `None` while the editor is being edited, so the editor looks as it always
/// did. A paused session takes the stronger colour: a game that is running but
/// not moving is the state that surprises people.
pub fn play_tint(theme: &Theme, run_mode: RunMode, paused: bool) -> Option<Color32> {
    if run_mode != RunMode::Playing {
        return None;
    }
    let [red, green, blue, alpha] = if paused {
        theme.palette.play_tint_paused.0
    } else {
        theme.palette.play_tint.0
    };
    Some(Color32::from_rgba_unmultiplied(red, green, blue, alpha))
}

/// The `egui_dock` styling for the shell's `DockArea`.
///
/// It starts from `egui_style` and changes it to the editor's look: flat tabs
/// that join their active bodies, a dark tab strip and thin separators between
/// panels. The colours come from `palette`; only an inactive tab's fill is
/// transparent.
pub fn dock_style(egui_style: &egui::Style, palette: &Palette) -> egui_dock::Style {
    let mut style = egui_dock::Style::from_egui(egui_style);
    style.main_surface_border_stroke = Stroke::NONE;
    style.dock_area_padding = None;
    // The tab strip is a darker band. The active tab takes the body's fill and
    // carries the accent underline, so it reads as one surface with its panel.
    // The line under the tabs is the neutral separator. The accent underline
    // belongs to the selected tab only, and the shell paints it, because the
    // dock uses one line colour for every tab.
    style.tab_bar.bg_fill = palette.frame;
    style.tab_bar.height = 26.0;
    style.tab_bar.corner_radius = CornerRadius::ZERO;
    style.tab_bar.hline_color = palette.line;
    style.tab_bar.inner_margin = egui::Margin::symmetric(6, 3);
    style.tab.active = tab_face(palette.panel, palette.text_strong);
    style.tab.focused = tab_face(palette.panel, palette.text_strong);
    style.tab.active_with_kb_focus = tab_face(palette.panel, palette.text_strong);
    style.tab.inactive = tab_face(Color32::TRANSPARENT, palette.text_muted);
    style.tab.inactive_with_kb_focus = tab_face(Color32::TRANSPARENT, palette.text_muted);
    style.tab.hovered = tab_face(palette.panel_raised, palette.text);
    style.tab.focused_with_kb_focus = tab_face(palette.panel_raised, palette.text);
    // The active tab joins its body. The shell paints the accent underline
    // there itself, so the dock must not draw the neutral line under it.
    style.tab.hline_below_active_tab_name = false;
    style.tab.spacing = 0.0;
    style.tab.tab_body.bg_fill = palette.panel;
    style.tab.tab_body.inner_margin = egui::Margin::same(6);
    style.tab.tab_body.corner_radius = CornerRadius::ZERO;
    style.tab.tab_body.stroke = Stroke::NONE;
    // Panel boundaries: one thin line, with a grab band several times its
    // width so the splitter is easy to catch. The line brightens while the
    // pointer is over it or it is dragged. It moves towards the text colours
    // instead of using a second pair of literal values.
    style.separator.width = 1.0;
    style.separator.extra_interact_width = 8.0;
    style.separator.color_idle = palette.line;
    style.separator.color_hovered = palette.line.lerp_to_gamma(palette.text_muted, 0.44);
    style.separator.color_dragged = palette.line.lerp_to_gamma(palette.text, 0.47);
    // The drag-and-drop preview uses the selection tint.
    style.overlay.selection_color = palette.selection;
    // The tab-strip and window buttons: muted, brighter on hover.
    let buttons = &mut style.buttons;
    buttons.add_tab_align = egui_dock::TabAddAlign::Left;
    for (color, active) in [
        (
            &mut buttons.close_tab_color,
            &mut buttons.close_tab_active_color,
        ),
        (
            &mut buttons.add_tab_color,
            &mut buttons.add_tab_active_color,
        ),
        (
            &mut buttons.close_all_tabs_color,
            &mut buttons.close_all_tabs_active_color,
        ),
        (
            &mut buttons.collapse_tabs_color,
            &mut buttons.collapse_tabs_active_color,
        ),
        (
            &mut buttons.minimize_window_color,
            &mut buttons.minimize_window_active_color,
        ),
    ] {
        *color = palette.text_muted;
        *active = palette.text_strong;
    }
    style
}

/// One `egui_dock` tab face: square corners and no outline, so the fill
/// alone separates the states.
fn tab_face(bg_fill: Color32, text_color: Color32) -> egui_dock::TabInteractionStyle {
    egui_dock::TabInteractionStyle {
        bg_fill,
        text_color,
        corner_radius: CornerRadius::ZERO,
        outline_color: Color32::TRANSPARENT,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The record is shared by the whole process, so the tests that install
    /// into it take turns. Otherwise a parallel test's install would change the
    /// state that another test is checking.
    static RECORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn record_guard() -> std::sync::MutexGuard<'static, ()> {
        RECORD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A fresh context with the code default installed.
    fn themed_context() -> Context {
        let context = Context::default();
        apply_theme(
            &context,
            &default_theme(),
            &[ThemeLayerInfo::code()],
            1,
            &kit::style::ThemeFonts::default(),
        );
        context
    }

    #[test]
    fn the_installed_style_carries_the_theme_tokens() {
        let _record_guard = record_guard();
        let context = themed_context();
        let theme = default_theme();
        let style = context.style_of(egui::Theme::Dark);
        let visuals = &style.visuals;
        assert_eq!(visuals.panel_fill, to_egui_color(theme.palette.panel));
        assert_eq!(visuals.window_fill, to_egui_color(theme.palette.panel));
        assert_eq!(visuals.window_stroke.width, 1.0);
        assert_eq!(
            visuals.window_stroke.color,
            to_egui_color(theme.palette.line)
        );
        assert_eq!(
            visuals.window_corner_radius,
            CornerRadius::same(theme.shape.window_corner_radius)
        );
        assert_eq!(visuals.extreme_bg_color, to_egui_color(theme.palette.well));
        assert_eq!(
            visuals.faint_bg_color,
            to_egui_color(theme.palette.panel_raised)
        );
        assert_eq!(
            visuals.selection.bg_fill,
            to_egui_color(theme.palette.selection)
        );
        assert_eq!(
            visuals.selection.stroke.color,
            to_egui_color(theme.shape.selection_stroke.color)
        );
        assert_eq!(
            visuals.hyperlink_color,
            to_egui_color(theme.palette.accent_hot)
        );
        assert!(visuals.slider_trailing_fill);
        assert_eq!(
            visuals.window_shadow.offset,
            theme.shape.window_shadow.offset
        );
        assert_eq!(visuals.window_shadow.blur, theme.shape.window_shadow.blur);
    }

    #[test]
    fn the_installed_style_carries_the_widget_states() {
        let _record_guard = record_guard();
        let context = themed_context();
        let theme = default_theme();
        let style = context.style_of(egui::Theme::Dark);
        let widgets = &style.visuals.widgets;
        assert_eq!(
            widgets.noninteractive.bg_fill,
            to_egui_color(theme.widgets.noninteractive.bg_fill)
        );
        assert_eq!(
            widgets.noninteractive.fg_stroke.color,
            to_egui_color(theme.widgets.noninteractive.fg_stroke.color)
        );
        assert_eq!(
            widgets.inactive.bg_fill,
            to_egui_color(theme.widgets.inactive.bg_fill)
        );
        assert_eq!(
            widgets.inactive.bg_stroke.width,
            theme.widgets.inactive.bg_stroke.width
        );
        assert_eq!(
            widgets.hovered.bg_fill,
            to_egui_color(theme.widgets.hovered.bg_fill)
        );
        assert_eq!(
            widgets.hovered.bg_stroke.color,
            to_egui_color(theme.widgets.hovered.bg_stroke.color)
        );
        assert_eq!(
            widgets.active.bg_fill,
            to_egui_color(theme.widgets.active.bg_fill)
        );
        assert_eq!(
            widgets.active.bg_stroke.color,
            to_egui_color(theme.widgets.active.bg_stroke.color)
        );
        assert_eq!(
            widgets.open.bg_fill,
            to_egui_color(theme.widgets.open.bg_fill)
        );
        for state in [
            &widgets.noninteractive,
            &widgets.inactive,
            &widgets.hovered,
            &widgets.active,
            &widgets.open,
        ] {
            assert_eq!(
                state.corner_radius,
                CornerRadius::same(theme.widgets.inactive.corner_radius)
            );
        }
    }

    #[test]
    fn the_installed_style_carries_the_metrics_and_text() {
        let _record_guard = record_guard();
        let context = themed_context();
        let theme = default_theme();
        let style = context.style_of(egui::Theme::Dark);
        assert_eq!(
            style.spacing.item_spacing,
            egui::vec2(theme.spacing.item_spacing[0], theme.spacing.item_spacing[1])
        );
        assert_eq!(
            style.spacing.button_padding,
            egui::vec2(
                theme.spacing.button_padding[0],
                theme.spacing.button_padding[1]
            )
        );
        assert_eq!(
            style.spacing.interact_size,
            egui::vec2(
                theme.spacing.interact_size[0],
                theme.spacing.interact_size[1]
            )
        );
        assert_eq!(style.spacing.scroll.floating, theme.spacing.scroll_floating);
        assert_eq!(
            style.spacing.scroll.bar_width,
            theme.spacing.scroll_bar_width
        );
        for (name, key) in [
            ("body", egui::TextStyle::Body),
            ("monospace", egui::TextStyle::Monospace),
            ("heading", egui::TextStyle::Heading),
            ("small", egui::TextStyle::Small),
        ] {
            let token = &theme.text_styles[name];
            assert_eq!(style.text_styles[&key].size, token.size, "{name}");
        }
        assert_eq!(context.theme(), egui::Theme::Dark);
    }

    /// The default theme's representative values: a crimson accent on
    /// near-black panels and tight corners.
    #[test]
    fn the_default_theme_has_the_representative_values() {
        let theme = default_theme();
        assert_eq!(theme.palette.accent, Rgba::rgb(200, 53, 63));
        assert_eq!(theme.palette.selection_bar, theme.palette.accent);
        assert!(theme.palette.frame.0[0] < 24, "the frame is near-black");
        assert_eq!(theme.shape.corner_radius, 3);
        assert!(
            theme.spacing.tooltip_width > 0.0,
            "a tooltip's text wraps at this width, so a zero would show one character per line"
        );
    }

    #[test]
    fn the_recorded_palette_is_the_installed_theme() {
        let _record_guard = record_guard();
        let context = Context::default();
        let theme = default_theme();
        apply_theme(
            &context,
            &theme,
            &[ThemeLayerInfo::code()],
            1,
            &kit::style::ThemeFonts::default(),
        );
        let palette = palette();
        assert_eq!(palette.panel, to_egui_color(theme.palette.panel));
        assert_eq!(text(), to_egui_color(theme.palette.text));
        assert_eq!(text_muted(), to_egui_color(theme.palette.text_muted));
        assert_eq!(accent(), to_egui_color(theme.palette.accent));
        assert_eq!(overlay(), to_egui_color(theme.palette.overlay));
        assert_eq!(shadow(), to_egui_color(theme.shape.window_shadow.color));
        assert_eq!(log_warn(), to_egui_color(theme.palette.log_warn));
    }

    /// The preference's value: a file path (absolute or relative to the state
    /// directory), a bare theme name, or nothing.
    #[test]
    fn the_user_layer_reads_a_preference_value() {
        let directory = std::path::Path::new("C:/state");
        assert_eq!(
            UserThemeSelection::from_preference(None, directory),
            UserThemeSelection::None
        );
        assert_eq!(
            UserThemeSelection::from_preference(Some("   "), directory),
            UserThemeSelection::None
        );
        assert_eq!(
            UserThemeSelection::from_preference(Some("dark"), directory),
            UserThemeSelection::Legacy("dark".to_string())
        );
        assert_eq!(
            UserThemeSelection::from_preference(Some("theme.yaml"), directory),
            UserThemeSelection::File(directory.join("theme.yaml"))
        );
        assert_eq!(
            UserThemeSelection::from_preference(Some("mine.theme"), directory),
            UserThemeSelection::File(directory.join("mine.theme"))
        );
        assert_eq!(
            UserThemeSelection::from_preference(Some("C:/elsewhere/mine.theme"), directory),
            UserThemeSelection::File(std::path::PathBuf::from("C:/elsewhere/mine.theme"))
        );
    }

    /// The layers are applied in order, user first and project last. The
    /// project's value wins where both name the same item, and the user's value
    /// stays where only the user named it.
    #[test]
    fn the_layers_merge_user_then_project() {
        let code = default_theme();
        let user = pill_ui_theme::ThemeOverrides::from_yaml_str(
            "palette:\n  accent: \"#ff488c\"\n  panel: \"#101820\"\n",
        )
        .expect("user document");
        let project =
            pill_ui_theme::ThemeOverrides::from_yaml_str("palette:\n  accent: \"#33d17a\"\n")
                .expect("project document");
        let user_info = ThemeLayerInfo::user("theme.json");
        let project_info = ThemeLayerInfo::project("editor/editor.theme");

        let merged = merge_layers(
            &code,
            Some((&user_info, &user)),
            Some((&project_info, &project)),
        );

        assert_eq!(merged.theme.palette.accent, Rgba::parse("#33d17a").unwrap());
        assert_eq!(merged.theme.palette.panel, Rgba::parse("#101820").unwrap());
        assert!(merged.refused.is_empty());
        assert_eq!(
            merged.layers,
            vec![
                ThemeLayerInfo::code(),
                ThemeLayerInfo::user("theme.json"),
                ThemeLayerInfo::project("editor/editor.theme"),
            ]
        );
        assert_eq!(layers_text(&merged.layers), "code,user,project");
    }

    /// A layer the merge refuses is named and skipped. The other layer still
    /// applies, and the code default it fell back to is still there.
    #[test]
    fn a_refused_layer_is_skipped_and_named() {
        let code = default_theme();
        let user =
            pill_ui_theme::ThemeOverrides::from_yaml_str("palette:\n  accent: not-a-colour\n")
                .expect("the document parses; the colour does not");
        let project =
            pill_ui_theme::ThemeOverrides::from_yaml_str("palette:\n  accent: \"#33d17a\"\n")
                .expect("project document");
        let user_info = ThemeLayerInfo::user("broken.theme");
        let project_info = ThemeLayerInfo::project("editor/editor.theme");

        let merged = merge_layers(
            &code,
            Some((&user_info, &user)),
            Some((&project_info, &project)),
        );

        assert_eq!(merged.theme.palette.accent, Rgba::parse("#33d17a").unwrap());
        assert_eq!(merged.refused.len(), 1);
        assert_eq!(merged.refused[0].0, ThemeLayer::User);
        assert!(merged.refused[0].1.contains("palette.accent"));
        assert_eq!(merged.layers.len(), 2);
    }

    /// The record follows the layer set. A new generation with the same tokens
    /// still counts as a theme to install, and `apply` re-installs the recorded
    /// theme.
    #[test]
    fn the_record_follows_the_layer_set() {
        let _record_guard = record_guard();
        let context = Context::default();
        let theme = default_theme();
        let layers = vec![ThemeLayerInfo::code(), ThemeLayerInfo::user("theme.json")];
        apply_theme(
            &context,
            &theme,
            &layers,
            7,
            &kit::style::ThemeFonts::default(),
        );
        let recorded =
            read_resolved_theme(|resolved| (resolved.generation, resolved.layers.clone()));
        assert_eq!(recorded.0, 7);
        assert_eq!(recorded.1, layers);
        // A second context installs the same record without moving it.
        let popout = Context::default();
        apply(&popout);
        let after = read_resolved_theme(|resolved| resolved.generation);
        assert_eq!(after, 7);
    }

    #[test]
    fn the_dock_style_reads_the_palette_roles() {
        let palette = Palette::from_theme(&default_theme());
        let dock = dock_style(&egui::Style::default(), &palette);
        assert_eq!(dock.tab_bar.bg_fill, palette.frame);
        assert_eq!(dock.tab_bar.hline_color, palette.line);
        assert!(
            !dock.tab.hline_below_active_tab_name,
            "the shell paints the active tab's accent underline itself"
        );
        assert_eq!(dock.tab.active.bg_fill, palette.panel);
        assert_eq!(dock.tab.active.text_color, palette.text_strong);
        assert_eq!(dock.tab.inactive.text_color, palette.text_muted);
        assert_eq!(dock.tab.hovered.bg_fill, palette.panel_raised);
        assert_eq!(dock.tab.tab_body.bg_fill, palette.panel);
        assert_eq!(dock.separator.color_idle, palette.line);
        assert_ne!(dock.separator.color_hovered, palette.line);
    }

    #[test]
    fn the_built_in_families_are_inter_jetbrains_mono_then_phosphor() {
        let definitions =
            kit::style::font_definitions(&kit::style::ThemeFonts::default(), append_fonts);
        let proportional = &definitions.families[&egui::FontFamily::Proportional];
        assert_eq!(
            proportional.first().map(String::as_str),
            Some(BODY_FONT_KEY),
            "Inter leads the proportional family"
        );
        let monospace = &definitions.families[&egui::FontFamily::Monospace];
        assert_eq!(
            monospace.first().map(String::as_str),
            Some(MONOSPACE_FONT_KEY),
            "JetBrains Mono leads the monospace family"
        );
        let duotone =
            &definitions.families[&egui::FontFamily::Name(crate::icons::DUOTONE_FAMILY.into())];
        assert_eq!(
            duotone.first().map(String::as_str),
            Some(crate::icons::DUOTONE_KEY),
            "Phosphor's duotone face leads its own family"
        );
        assert!(
            duotone.iter().any(|name| name == BODY_FONT_KEY),
            "Inter follows it as the label's fallback"
        );
    }

    /// A theme layer's font leads in each UI family, the editor's own fonts
    /// follow it as the fallback, and an icon family leads with its Phosphor
    /// face. The UI fonts also map the icons' private-use codepoints, so an
    /// icon listed behind them would draw as a letter.
    #[test]
    fn a_theme_font_leads_the_editors_own_families() {
        let fonts = kit::style::ThemeFonts {
            body: Some(vec![0, 1, 2, 3]),
            monospace: Some(vec![4, 5, 6, 7]),
            ..kit::style::ThemeFonts::default()
        };
        let definitions = kit::style::font_definitions(&fonts, append_fonts);

        let proportional = &definitions.families[&egui::FontFamily::Proportional];
        assert_eq!(
            proportional.first().map(String::as_str),
            Some(kit::style::THEME_BODY_KEY),
            "the theme's body font leads"
        );
        assert_eq!(
            proportional.get(1).map(String::as_str),
            Some(BODY_FONT_KEY),
            "Inter is the fallback behind it"
        );
        let monospace = &definitions.families[&egui::FontFamily::Monospace];
        assert_eq!(
            monospace.first().map(String::as_str),
            Some(kit::style::THEME_MONOSPACE_KEY)
        );
        assert_eq!(
            monospace.get(1).map(String::as_str),
            Some(MONOSPACE_FONT_KEY)
        );
        let icons = &definitions.families[&egui::FontFamily::Name(crate::icons::FAMILY.into())];
        assert_eq!(
            icons.first().map(String::as_str),
            Some(crate::icons::REGULAR_KEY),
            "Phosphor's regular face leads the icon family"
        );
        assert_eq!(
            icons.get(1).map(String::as_str),
            Some(kit::style::THEME_BODY_KEY),
            "the theme's body font follows it as the fallback"
        );
    }
    /// The play tint is absent while editing and present while a session runs,
    /// with the stronger colour once it is paused.
    #[test]
    fn the_play_tint_appears_only_while_a_session_runs() {
        let theme = default_theme();
        assert_eq!(play_tint(&theme, RunMode::Editing, false), None);
        assert_eq!(play_tint(&theme, RunMode::Editing, true), None);
        let playing = play_tint(&theme, RunMode::Playing, false).expect("a playing tint");
        let paused = play_tint(&theme, RunMode::Playing, true).expect("a paused tint");
        assert_ne!(playing, paused, "a paused session reads differently");
        let [red, green, blue, alpha] = theme.palette.play_tint.0;
        assert_eq!(
            playing,
            Color32::from_rgba_unmultiplied(red, green, blue, alpha)
        );
    }
}
