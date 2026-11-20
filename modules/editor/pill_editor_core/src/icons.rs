//! Phosphor icons for the editor's chrome: single glyphs from the regular
//! face and duotone icons drawn as a fill under the line art.
//!
//! # Responsibilities
//!
//! - [`append_font_definitions`]: the two vendored Phosphor families, added to
//!   the font definitions that [`crate::style::apply_theme`] installs through
//!   the UI kit.
//! - [`Icon`]: the icons the editor uses, with their codepoints in the fonts.
//! - [`rich`]: an icon and a label as one text run.
//! - [`paint_duotone`] and [`IconButton`]: Phosphor's duotone form, a fill
//!   under the line art, for the editor's buttons, menus and tool rows.
//!
//! # Design
//!
//! The vendored `Phosphor.ttf` (regular) and `Phosphor-Duotone.ttf` come from
//! the `@phosphor-icons/web` package. The duotone font holds each icon twice:
//! the fill shape at the codepoint and the line art one codepoint further.
//! The package's stylesheet overlays the pair at one fifth of the opacity;
//! [`paint_duotone`] draws the fill at [`DUOTONE_FILL_ALPHA`] from the same
//! origin as the line art and gets the same look with no image atlas. The
//! icons take the theme's colour and need no texture of their own.
//!
//! Both icon families list their Phosphor face first. The UI fonts also map
//! this private-use range, so an icon listed behind them would draw as a
//! stray letter. [`rich`] carries the rule for mixing an icon with letters.

// Standard library
use std::sync::Arc;

// External crates
use egui::{
    Color32, FontData, FontDefinitions, FontFamily, FontId, Painter, Rect, Response, Sense,
    StrokeKind, TextStyle, Ui, Vec2, Widget,
};

// =============================================================================
// Constants
// =============================================================================

/// The family of the regular-weight icons, falling back to the UI font.
pub const FAMILY: &str = "phosphor";

/// The family of the duotone icons, falling back to the UI font.
pub const DUOTONE_FAMILY: &str = "phosphor.duotone";

/// How strongly a duotone icon's fill shows under its line art.
pub const DUOTONE_FILL_ALPHA: f32 = 0.2;

/// An icon's height relative to the button text it sits beside.
const ICON_TEXT_RATIO: f32 = 1.15;

/// The gap between an icon and its label, in points.
const ICON_LABEL_GAP: f32 = 6.0;

/// How much the hovered fill of a row floating over a scene brightens.
const HOVER_BRIGHTEN_AMOUNT: u8 = 14;

/// The regular-weight font, as the package shipped it.
const REGULAR_FONT: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// The duotone font, as the package shipped it.
const DUOTONE_FONT: &[u8] = include_bytes!("../assets/Phosphor-Duotone.ttf");

/// The key the regular face is registered under.
pub const REGULAR_KEY: &str = "phosphor";
/// The key the duotone face is registered under.
pub const DUOTONE_KEY: &str = "phosphor.duotone";

// =============================================================================
// Icon
// =============================================================================

/// One Phosphor icon, named the way the package names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// A solid cube: the first scene view.
    Cube,
    /// A wireframe cube: the second scene view.
    CubeTransparent,
    /// A display with a play mark: the project's own view.
    MonitorPlay,
    /// A hierarchy of nodes: the entity list.
    TreeStructure,
    /// A cog: the system list.
    Gear,
    /// Three sliders: the selected entity's detail.
    SlidersHorizontal,
    /// An open folder: the asset browser.
    FolderOpen,
    /// The fader bank: the preferences.
    Faders,
    /// Arrows out of a centre: the translate tool.
    ArrowsOutCardinal,
    /// The clockwise arc: the rotate tool.
    ArrowsClockwise,
    /// Arrows out of a box: the scale tool.
    ArrowsOut,
    /// A globe: world space.
    GlobeHemisphereWest,
    /// A map pin: local space.
    MapPin,
    /// A horseshoe magnet: snapping.
    Magnet,
    /// A four-cell grid: the scene grid.
    GridFour,
    /// A plus: create.
    Plus,
    /// A bin: delete.
    Trash,
    /// A pencil: rename.
    PencilSimple,
    /// A file with a down arrow: write metadata.
    FileArrowDown,
    /// The power symbol: a system's on/off switch.
    Power,
    /// Two bars: pause the project.
    Pause,
    /// A triangle: resume the project.
    Play,
    /// A bar and a triangle: advance one frame.
    SkipForward,
    /// A circled plus: add an entity or a component.
    PlusCircle,
    /// A circled minus: remove a component.
    MinusCircle,
    /// A circled tick: confirm.
    CheckCircle,
    /// A circled cross: a problem or an error.
    XCircle,
    /// A floppy disk: save.
    FloppyDisk,
    /// A box with an arrow leaving it: pop a tab out into its own window.
    ArrowSquareOut,
    /// A box with an arrow entering it: move a popped-out tab back into the
    /// dock.
    ArrowSquareIn,
    /// A closed folder: a folder row of the project browser.
    Folder,
    /// A right-pointing chevron: a closed tree row.
    CaretRight,
    /// A down-pointing chevron: an open tree row.
    CaretDown,
    /// A blank sheet: a file with no more specific kind.
    File,
    /// A framed picture: a texture or an image file.
    Image,
    /// A shaded ball: a material.
    Sphere,
    /// Angle brackets: a shader or another source file.
    Code,
}

impl Icon {
    /// Every icon, in declaration order.
    pub const ALL: [Icon; 37] = [
        Icon::Cube,
        Icon::CubeTransparent,
        Icon::MonitorPlay,
        Icon::TreeStructure,
        Icon::Gear,
        Icon::SlidersHorizontal,
        Icon::FolderOpen,
        Icon::Faders,
        Icon::ArrowsOutCardinal,
        Icon::ArrowsClockwise,
        Icon::ArrowsOut,
        Icon::GlobeHemisphereWest,
        Icon::MapPin,
        Icon::Magnet,
        Icon::GridFour,
        Icon::Plus,
        Icon::Trash,
        Icon::PencilSimple,
        Icon::FileArrowDown,
        Icon::Power,
        Icon::Pause,
        Icon::Play,
        Icon::SkipForward,
        Icon::PlusCircle,
        Icon::MinusCircle,
        Icon::CheckCircle,
        Icon::XCircle,
        Icon::FloppyDisk,
        Icon::ArrowSquareOut,
        Icon::ArrowSquareIn,
        Icon::Folder,
        Icon::CaretRight,
        Icon::CaretDown,
        Icon::File,
        Icon::Image,
        Icon::Sphere,
        Icon::Code,
    ];

    /// The icon's codepoint in the vendored fonts: the regular font draws
    /// its line art there, and the duotone font its fill.
    pub fn codepoint(self) -> u32 {
        match self {
            Icon::Cube => 0xe1da,
            Icon::CubeTransparent => 0xec7c,
            Icon::MonitorPlay => 0xe58c,
            Icon::TreeStructure => 0xe67c,
            Icon::Gear => 0xe270,
            Icon::SlidersHorizontal => 0xe434,
            Icon::FolderOpen => 0xe256,
            Icon::Faders => 0xe228,
            Icon::ArrowsOutCardinal => 0xe0a4,
            Icon::ArrowsClockwise => 0xe094,
            Icon::ArrowsOut => 0xe0a2,
            Icon::GlobeHemisphereWest => 0xe28c,
            Icon::MapPin => 0xe316,
            Icon::Magnet => 0xe680,
            Icon::GridFour => 0xe296,
            Icon::Plus => 0xe3d4,
            Icon::Trash => 0xe4a6,
            Icon::PencilSimple => 0xe3b4,
            Icon::FileArrowDown => 0xe232,
            Icon::Power => 0xe3da,
            Icon::Pause => 0xe39e,
            Icon::Play => 0xe3d0,
            Icon::SkipForward => 0xe5a6,
            Icon::PlusCircle => 0xe3d6,
            Icon::MinusCircle => 0xe32c,
            Icon::CheckCircle => 0xe184,
            Icon::XCircle => 0xe4f8,
            Icon::FloppyDisk => 0xe248,
            Icon::ArrowSquareOut => 0xe5de,
            Icon::ArrowSquareIn => 0xe5dc,
            Icon::Folder => 0xe24a,
            Icon::CaretRight => 0xe13a,
            Icon::CaretDown => 0xe136,
            Icon::File => 0xe230,
            Icon::Image => 0xe2ca,
            Icon::Sphere => 0xee66,
            Icon::Code => 0xe1bc,
        }
    }

    /// The icon's Phosphor name, as the package spells it.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Cube => "cube",
            Icon::CubeTransparent => "cube-transparent",
            Icon::MonitorPlay => "monitor-play",
            Icon::TreeStructure => "tree-structure",
            Icon::Gear => "gear",
            Icon::SlidersHorizontal => "sliders-horizontal",
            Icon::FolderOpen => "folder-open",
            Icon::Faders => "faders",
            Icon::ArrowsOutCardinal => "arrows-out-cardinal",
            Icon::ArrowsClockwise => "arrows-clockwise",
            Icon::ArrowsOut => "arrows-out",
            Icon::GlobeHemisphereWest => "globe-hemisphere-west",
            Icon::MapPin => "map-pin",
            Icon::Magnet => "magnet",
            Icon::GridFour => "grid-four",
            Icon::Plus => "plus",
            Icon::Trash => "trash",
            Icon::PencilSimple => "pencil-simple",
            Icon::FileArrowDown => "file-arrow-down",
            Icon::Power => "power",
            Icon::Pause => "pause",
            Icon::Play => "play",
            Icon::SkipForward => "skip-forward",
            Icon::PlusCircle => "plus-circle",
            Icon::MinusCircle => "minus-circle",
            Icon::CheckCircle => "check-circle",
            Icon::XCircle => "x-circle",
            Icon::FloppyDisk => "floppy-disk",
            Icon::ArrowSquareOut => "arrow-square-out",
            Icon::ArrowSquareIn => "arrow-square-in",
            Icon::Folder => "folder",
            Icon::CaretRight => "caret-right",
            Icon::CaretDown => "caret-down",
            Icon::File => "file",
            Icon::Image => "image",
            Icon::Sphere => "sphere",
            Icon::Code => "code",
        }
    }

    /// The glyph the regular font draws for the icon.
    pub fn glyph(self) -> char {
        glyph(self.codepoint())
    }

    /// The glyphs the duotone font draws for the icon: its fill shape, and
    /// the line art one codepoint further.
    pub fn duotone(self) -> (char, char) {
        (glyph(self.codepoint()), glyph(self.codepoint() + 1))
    }
}

// =============================================================================
// IconButton
// =============================================================================

/// A button carrying a duotone icon and a label, drawn across the editor's
/// menus, tab headers and toolbars.
///
/// It uses the theme's button visuals: the hover and press feedback, and the
/// selected fill that marks the current choice. The icon sits before the
/// label. A row whose rectangle a panel has already fixed takes exactly that
/// rectangle through [`IconButton::fixed_size`].
pub struct IconButton<'a> {
    icon: Icon,
    label: &'a str,
    active: bool,
    fill_override: Option<Color32>,
    fixed_size: Option<Vec2>,
}

impl<'a> IconButton<'a> {
    /// Creates a button for `icon` and `label`. An empty `label` shows the
    /// icon on its own.
    pub fn new(icon: Icon, label: &'a str) -> Self {
        Self {
            icon,
            label,
            active: false,
            fill_override: None,
            fixed_size: None,
        }
    }

    /// Marks the button as the current choice: a checked menu item or a tool
    /// that is on.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Uses `fill` instead of the theme's button fill, so a panel floating
    /// over the scene can keep its own palette.
    pub fn fill(mut self, fill: Color32) -> Self {
        self.fill_override = Some(fill);
        self
    }

    /// Gives the button exactly `fixed_size` instead of the size its icon and
    /// label need.
    pub fn fixed_size(mut self, fixed_size: Vec2) -> Self {
        self.fixed_size = Some(fixed_size);
        self
    }
}

impl Widget for IconButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        // Step 1: Read the button text font, the padding and the text colour.
        let style = ui.style();
        let font = style
            .text_styles
            .get(&TextStyle::Button)
            .cloned()
            .unwrap_or_else(|| FontId::proportional(13.0));
        let padding = style.spacing.button_padding;
        let color = style.visuals.text_color();
        let icon_size = font.size * ICON_TEXT_RATIO;
        let label_galley = ui
            .painter()
            .layout_no_wrap(self.label.to_owned(), font, color);
        let gap = if self.label.is_empty() {
            0.0
        } else {
            ICON_LABEL_GAP
        };
        // Step 2: Measure the icon and label and work out the button's size.
        let content_size = Vec2::new(
            icon_size + gap + label_galley.size().x,
            label_galley.size().y.max(icon_size),
        );
        let button_size = self.fixed_size.unwrap_or(content_size + padding * 2.0);
        // Step 3: Allocate the rectangle and draw the button when it is visible.
        let (rect, response) = ui.allocate_exact_size(button_size, Sense::click());
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact_selectable(&response, self.active);
            let button_fill = self
                .fill_override
                .map_or(visuals.weak_bg_fill, |override_fill| {
                    if response.hovered() {
                        brighten(override_fill, HOVER_BRIGHTEN_AMOUNT)
                    } else {
                        override_fill
                    }
                });
            let corner_radius = ui.style().visuals.widgets.inactive.corner_radius;
            ui.painter().rect(
                rect,
                corner_radius,
                button_fill,
                visuals.bg_stroke,
                StrokeKind::Inside,
            );
            let icon_rect = Rect::from_center_size(
                egui::pos2(
                    rect.center().x - content_size.x * 0.5 + icon_size * 0.5,
                    rect.center().y,
                ),
                Vec2::splat(icon_size),
            );
            paint_duotone(ui.painter(), icon_rect, self.icon, visuals.fg_stroke.color);
            if !self.label.is_empty() {
                ui.painter().galley(
                    egui::pos2(
                        icon_rect.right() + gap,
                        rect.center().y - label_galley.size().y * 0.5,
                    ),
                    label_galley,
                    visuals.fg_stroke.color,
                );
            }
        }
        response
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Appends the Phosphor fonts to `fonts` as the [`FAMILY`] and
/// [`DUOTONE_FAMILY`] families.
///
/// Each family lists its icon face first and then every font of the
/// proportional family already built, so a label beside an icon still draws.
/// The icon face goes first because the UI fonts also map the icon range, so
/// a UI font listed first would draw an icon as a letter. The regular face
/// cannot lead a mixed run because it maps the lowercase letters with blank
/// glyphs, so [`rich`] takes its icon from the duotone face instead.
pub fn append_font_definitions(fonts: &mut FontDefinitions) {
    // Step 1: Register both font files under their keys.
    fonts.font_data.insert(
        REGULAR_KEY.to_owned(),
        Arc::new(FontData::from_static(REGULAR_FONT)),
    );
    fonts.font_data.insert(
        DUOTONE_KEY.to_owned(),
        Arc::new(FontData::from_static(DUOTONE_FONT)),
    );
    // Step 2: Build each icon family from its face and the proportional
    // fallbacks.
    let fallback_font_names = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let family_names = |icon_font_key: &str| {
        let mut font_names = Vec::with_capacity(fallback_font_names.len() + 1);
        font_names.push(icon_font_key.to_owned());
        font_names.extend(fallback_font_names.iter().cloned());
        font_names
    };
    fonts
        .families
        .insert(FontFamily::Name(FAMILY.into()), family_names(REGULAR_KEY));
    fonts.families.insert(
        FontFamily::Name(DUOTONE_FAMILY.into()),
        family_names(DUOTONE_KEY),
    );
}

/// The character at `codepoint`.
///
/// # Panics
///
/// Panics if `codepoint` is not a Unicode scalar value. Every Phosphor
/// codepoint is one, so the calls in this file cannot panic.
fn glyph(codepoint: u32) -> char {
    char::from_u32(codepoint).expect("a Phosphor codepoint is a scalar value")
}

/// Combines `icon` and `label` into one text run. The icon's line art comes
/// from the duotone face and the letters from the UI fonts behind it.
///
/// The duotone face leads its family and maps no ASCII letter, so a label's
/// letters reach the UI font. Its line art at `codepoint + 1` has the same
/// outline as the regular face. The regular face cannot carry a mixed run
/// because it maps the lowercase letters with blank glyphs.
pub fn rich(icon: Icon, label: &str) -> egui::RichText {
    let (_, line) = icon.duotone();
    egui::RichText::new(format!("{line}  {label}")).family(FontFamily::Name(DUOTONE_FAMILY.into()))
}

/// Returns `color` with its alpha channel scaled by `opacity_factor`.
///
/// The factor is clamped between 0 and 1.
pub fn with_alpha(color: Color32, opacity_factor: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * opacity_factor.clamp(0.0, 1.0)) as u8,
    )
}

/// Paints `icon`'s duotone form centred in `rect`, tinted `color`. The fill
/// shape is drawn at [`DUOTONE_FILL_ALPHA`] with the line art over it.
pub fn paint_duotone(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let icon_size = rect.width().min(rect.height());
    if icon_size <= 0.0 {
        return;
    }
    let font = FontId::new(icon_size, FontFamily::Name(DUOTONE_FAMILY.into()));
    let (fill, line) = icon.duotone();
    // Lay out the line art first and use its box for both layers. The two
    // glyphs share a baseline, so one origin keeps the fill directly under
    // the line, whatever shape each glyph has.
    let line_galley = painter.layout_no_wrap(line.to_string(), font.clone(), color);
    let origin = rect.center() - line_galley.size() * 0.5;
    let faded = with_alpha(color, DUOTONE_FILL_ALPHA);
    let fill_galley = painter.layout_no_wrap(fill.to_string(), font, faded);
    painter.galley(origin, fill_galley, faded);
    painter.galley(origin, line_galley, color);
}

/// Returns `color` with every RGB channel raised by `amount`, stopping at
/// 255. The alpha channel is unchanged.
fn brighten(color: Color32, amount: u8) -> Color32 {
    let lift = |channel: u8| channel.saturating_add(amount);
    Color32::from_rgba_unmultiplied(lift(color.r()), lift(color.g()), lift(color.b()), color.a())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that both vendored fonts map every icon's codepoint and that the
    /// duotone font also maps the line art one codepoint further. A typo or a
    /// package change then fails here instead of drawing a blank glyph.
    #[test]
    fn every_icon_is_in_the_vendored_fonts() {
        let regular = codepoints(REGULAR_FONT);
        let duotone = codepoints(DUOTONE_FONT);
        for icon in Icon::ALL {
            assert!(
                regular.contains(&icon.codepoint()),
                "the regular font has no glyph for {} (U+{:04X})",
                icon.name(),
                icon.codepoint()
            );
            assert!(
                duotone.contains(&icon.codepoint()),
                "the duotone font has no fill glyph for {} (U+{:04X})",
                icon.name(),
                icon.codepoint()
            );
            assert!(
                duotone.contains(&(icon.codepoint() + 1)),
                "the duotone font has no line glyph for {} (U+{:04X})",
                icon.name(),
                icon.codepoint() + 1
            );
        }
    }

    /// Checks that the inline form starts with the icon's line art and ends
    /// with the label.
    #[test]
    fn a_rich_label_starts_with_its_icon() {
        let grid = rich(Icon::GridFour, "Grid");
        let (_, grid_line) = Icon::GridFour.duotone();
        assert!(grid.text().starts_with(grid_line));
        assert!(grid.text().ends_with("Grid"));
        let magnet = rich(Icon::Magnet, "Grid");
        let (_, magnet_line) = Icon::Magnet.duotone();
        assert!(magnet.text().starts_with(magnet_line));
        assert_ne!(grid.text(), magnet.text());
    }

    /// Checks that an icon's codepoint is answered by its Phosphor face, not
    /// by the UI font that the family lists as a fallback. Inter maps many
    /// private-use codepoints, so the two layouts differ and a stray UI glyph
    /// would be visible.
    #[test]
    fn an_icon_is_drawn_by_its_phosphor_face() {
        let mut definitions = pill_ui_kit::style::font_definitions(
            &pill_ui_kit::style::ThemeFonts::default(),
            crate::style::append_fonts,
        );
        definitions.families.insert(
            FontFamily::Name("test.phosphor".into()),
            vec![REGULAR_KEY.to_owned()],
        );
        let mut fonts =
            egui::epaint::text::Fonts::new(egui::epaint::text::TextOptions::default(), definitions);

        for icon in [Icon::Cube, Icon::Image, Icon::File, Icon::FolderOpen] {
            let glyph_text = icon.glyph().to_string();
            let phosphor = ink(
                &mut fonts,
                &glyph_text,
                FontFamily::Name("test.phosphor".into()),
            );
            let icon_ink = ink(&mut fonts, &glyph_text, FontFamily::Name(FAMILY.into()));
            let ui = ink(&mut fonts, &glyph_text, FontFamily::Proportional);
            assert_eq!(icon_ink, phosphor, "{} came from the UI font", icon.name());
            assert_ne!(ui, phosphor, "the UI font does not claim {}", icon.name());
        }
    }

    /// Returns the rectangle `text` covers in `family`. Layout measures only
    /// the glyph, so the colour can come from the editor palette.
    fn ink(fonts: &mut egui::epaint::text::Fonts, text: &str, family: FontFamily) -> Rect {
        let color = crate::style::palette().text;
        fonts
            .with_pixels_per_point(1.0)
            .layout_no_wrap(text.to_owned(), FontId::new(24.0, family), color)
            .rect
    }

    /// Checks that both icon families list their Phosphor face first. The UI
    /// fonts they fall back to map the same private-use codepoints (Inter
    /// carries alternates there), so an icon listed behind them would draw as
    /// a stray letter.
    #[test]
    fn the_icon_families_lead_with_their_phosphor_face() {
        let mut fonts = FontDefinitions::default();
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "ui".to_owned());
        append_font_definitions(&mut fonts);
        for (family, key) in [(FAMILY, REGULAR_KEY), (DUOTONE_FAMILY, DUOTONE_KEY)] {
            let font_names = &fonts.families[&FontFamily::Name(family.into())];
            assert_eq!(
                font_names.first().map(String::as_str),
                Some(key),
                "{family}"
            );
            assert!(
                font_names.iter().any(|name| name == "ui"),
                "{family} lost its UI fallback"
            );
        }
    }

    /// Checks that the fade scales the colour's alpha and leaves a fully
    /// opaque colour unchanged.
    #[test]
    fn the_fill_fades_the_colour() {
        let solid = with_alpha(Color32::from_rgb(200, 100, 50), 1.0);
        assert_eq!(solid, Color32::from_rgb(200, 100, 50));
        let faded = with_alpha(Color32::from_rgb(200, 100, 50), 0.5);
        assert!(
            (faded.a() as i32 - 128).abs() <= 1,
            "half opacity gave {}",
            faded.a()
        );
        assert_ne!(faded, solid);
    }

    /// Returns every codepoint a font maps, read from its `cmap` table. Only
    /// the format 4 and format 12 subtables a TrueType font carries are read.
    fn codepoints(font: &[u8]) -> std::collections::HashSet<u32> {
        let mut found_codepoints = std::collections::HashSet::new();
        let table_count = read_u16(font, 4) as usize;
        for table_index in 0..table_count {
            let table_record_offset = 12 + table_index * 16;
            if &font[table_record_offset..table_record_offset + 4] != b"cmap" {
                continue;
            }
            let cmap_offset = read_u32(font, table_record_offset + 8) as usize;
            for subtable_index in 0..read_u16(font, cmap_offset + 2) as usize {
                let record_offset = cmap_offset + 4 + subtable_index * 8;
                let offset = cmap_offset + read_u32(font, record_offset + 4) as usize;
                match read_u16(font, offset) {
                    4 => read_format_4(font, offset, &mut found_codepoints),
                    12 => read_format_12(font, offset, &mut found_codepoints),
                    _ => {}
                }
            }
        }
        found_codepoints
    }

    /// Adds the codepoints of the `cmap` format 4 segments: one run of
    /// codepoints per segment.
    fn read_format_4(
        font: &[u8],
        offset: usize,
        found_codepoints: &mut std::collections::HashSet<u32>,
    ) {
        let segment_count = read_u16(font, offset + 6) as usize / 2;
        let ends_offset = offset + 14;
        let starts_offset = ends_offset + segment_count * 2 + 2;
        for segment_index in 0..segment_count {
            let end = read_u16(font, ends_offset + segment_index * 2);
            let start = read_u16(font, starts_offset + segment_index * 2);
            found_codepoints.extend((start..=end).map(u32::from));
        }
    }

    /// Adds the codepoints of the `cmap` format 12 groups: one run of
    /// codepoints per group.
    fn read_format_12(
        font: &[u8],
        offset: usize,
        found_codepoints: &mut std::collections::HashSet<u32>,
    ) {
        for group_index in 0..read_u32(font, offset + 12) as usize {
            let group_offset = offset + 16 + group_index * 12;
            let start = read_u32(font, group_offset);
            let end = read_u32(font, group_offset + 4);
            found_codepoints.extend(start..=end);
        }
    }

    /// Reads the big-endian `u16` at `offset` in `font`.
    fn read_u16(font: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([font[offset], font[offset + 1]])
    }

    /// Reads the big-endian `u32` at `offset` in `font`.
    fn read_u32(font: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            font[offset],
            font[offset + 1],
            font[offset + 2],
            font[offset + 3],
        ])
    }
}
