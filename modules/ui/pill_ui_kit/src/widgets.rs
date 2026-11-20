//! The brutalist chunky button: a flat face over a hard shadow with no blur.
//! While the button is held, the face moves onto the shadow, as if pressed.
//!
//! # Responsibilities
//!
//! - [`ChunkyButton`]: a button drawn from a [`Theme`](pill_ui_theme::Theme)'s
//!   tokens, in three variants - neutral, accent and selected - plus a
//!   disabled look.
//! - [`ChunkyButton::new`]: the label and the theme; `.accent(..)` and
//!   `.selected(..)` pick the variant.
//!
//! # Design
//!
//! egui's stock `Button` cannot draw a hard shadow with no blur, and
//! `Button::fill` also overrides the hover and pressed colours. This widget
//! therefore lays its rectangle out first, reads the state from the response,
//! and then paints the shadow and the face itself. A clipped button paints
//! nothing at all.
//!
//! The widget keeps nothing in egui's memory. Project drawing code is a
//! reloadable DLL image, so a value left in a context's memory could outlive
//! it. Everything the widget needs comes from the `Theme` the caller passes
//! and the `Response` it returns.

// External crates
use egui::{
    Align2, Color32, FontId, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextStyle,
    TextWrapMode, Ui, WidgetInfo, WidgetText, WidgetType,
};

// Current crate
use pill_ui_theme::{Rgba, Theme};

// =============================================================================
// Constants
// =============================================================================

/// The distance between the face and its shadow, in points.
///
/// The face is the allocated rectangle shrunk by this much. The shadow is the
/// face rectangle moved by the same amount, which puts it flush with the
/// rectangle's right and bottom edges. While the button is held, the face
/// moves onto the shadow instead.
pub const SHADOW_OFFSET: f32 = 3.0;

/// The width of the face's outline, in points.
const OUTLINE_WIDTH_POINTS: f32 = 2.0;

/// The width of the selection bar on a chosen face, in points.
const SELECTION_BAR_WIDTH_POINTS: f32 = 4.0;

// =============================================================================
// ChunkyButton
// =============================================================================

/// A button with a hard shadow, painted from a theme's tokens.
///
/// The button reports clicks like any other egui button: a click is a press
/// and a release on the same widget, and `add_enabled(false, ..)` reports no
/// click at all.
pub struct ChunkyButton<'a> {
    /// The label drawn on the face.
    label: WidgetText,
    /// The tokens the face is painted with.
    theme: &'a Theme,
    /// Whether the face uses the accent colours.
    uses_accent_face: bool,
    /// Whether the button is the chosen one of its group.
    selected: bool,
}

impl<'a> ChunkyButton<'a> {
    /// Creates a neutral chunky button with `label` on its face, painted from
    /// `theme`.
    pub fn new(label: impl Into<WidgetText>, theme: &'a Theme) -> Self {
        Self {
            label: label.into(),
            theme,
            uses_accent_face: false,
            selected: false,
        }
    }

    /// Chooses the accent face: the accent colour as the fill, the frame
    /// colour as the text colour, and the hot accent while the pointer is over
    /// the button or the button is held.
    pub fn accent(mut self, uses_accent_face: bool) -> Self {
        self.uses_accent_face = uses_accent_face;
        self
    }

    /// Marks the button as the chosen one of its group. A chosen button also
    /// uses the accent face and paints a bar of the selection colour along the
    /// shadow's left edge.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Returns the face's fill colour and text colour for the given state.
    ///
    /// Every colour comes from a theme token. The accent face uses the accent
    /// colour, or the hot accent while the pointer is over the button or the
    /// button is held. The neutral face uses the raised panel colour at rest,
    /// the control colour while hovered, and the line colour while held. A
    /// disabled face uses the panel colour and the muted text colour.
    fn face_colors(&self, enabled: bool, hovered: bool, held: bool) -> (Color32, Color32) {
        let palette = &self.theme.palette;
        if !enabled {
            return (
                to_egui_color(palette.panel),
                to_egui_color(palette.text_muted),
            );
        }
        if self.uses_accent_face || self.selected {
            let fill = if hovered || held {
                palette.accent_hot
            } else {
                palette.accent
            };
            (to_egui_color(fill), to_egui_color(palette.frame))
        } else {
            let fill = if held {
                palette.line
            } else if hovered {
                palette.control
            } else {
                palette.panel_raised
            };
            (to_egui_color(fill), to_egui_color(palette.text))
        }
    }
}

impl egui::Widget for ChunkyButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        // Step 1: Lay the button out. The size includes the room the shadow
        // needs, so the face never covers the next widget.
        let galley = self.label.clone().into_galley(
            ui,
            Some(TextWrapMode::Extend),
            f32::INFINITY,
            TextStyle::Button,
        );
        let padding = egui::vec2(
            self.theme.spacing.button_padding[0] + 4.0,
            self.theme.spacing.button_padding[1] + 3.0,
        );
        let size = galley.size() + padding * 2.0 + egui::vec2(SHADOW_OFFSET, SHADOW_OFFSET);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let label = galley.text().to_owned();
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &label));

        // Step 2: Paint, but only if the rectangle is on screen. A clipped
        // button draws nothing: no shadow and no outline.
        if ui.is_rect_visible(rect) {
            let radius = egui::CornerRadius::same(self.theme.shape.corner_radius);
            let held = response.is_pointer_button_down_on();
            let (fill, text_color) = self.face_colors(ui.is_enabled(), response.hovered(), held);
            // While held, the face moves onto the shadow, so the button looks
            // pressed down instead of just wearing another colour.
            let shadow_shift = if held { 0.0 } else { SHADOW_OFFSET };
            let face_rect = rect.shrink(SHADOW_OFFSET);
            let shadow_rect = face_rect.translate(egui::vec2(shadow_shift, shadow_shift));
            let painter = ui.painter();
            painter.add(Shape::rect_filled(
                shadow_rect,
                radius,
                to_egui_color(self.theme.palette.line),
            ));
            if self.selected {
                let selection_bar_rect = Rect::from_min_size(
                    shadow_rect.left_top(),
                    egui::vec2(SELECTION_BAR_WIDTH_POINTS, shadow_rect.height()),
                );
                painter.add(Shape::rect_filled(
                    selection_bar_rect,
                    radius,
                    to_egui_color(self.theme.palette.selection_bar),
                ));
            }
            painter.add(Shape::rect_filled(face_rect, radius, fill));
            painter.add(Shape::rect_stroke(
                face_rect,
                radius,
                Stroke::new(OUTLINE_WIDTH_POINTS, to_egui_color(self.theme.palette.line)),
                StrokeKind::Inside,
            ));
            let font_size = self
                .theme
                .text_styles
                .get("button")
                .map_or(12.0, |style| style.size);
            let family = ui.style().text_styles[&TextStyle::Button].family.clone();
            painter.text(
                face_rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::new(font_size, family),
                text_color,
            );
        }
        response
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the egui colour that matches a theme colour.
///
/// Both store straight sRGB alpha, so the four components pass through
/// unchanged.
fn to_egui_color(theme_color: Rgba) -> Color32 {
    let [red, green, blue, alpha] = theme_color.0;
    Color32::from_rgba_unmultiplied(red, green, blue, alpha)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput};

    /// The theme the tests paint with.
    fn theme() -> Theme {
        Theme::brutalist()
    }

    /// The screen every pass draws on.
    fn screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(240.0, 160.0))
    }

    /// Returns the filled rectangles a pass painted, each with its fill
    /// colour.
    ///
    /// A pass may paint a full-screen background, which is not part of the
    /// button, so only rectangles narrower than the screen are kept: the
    /// shadow, the face and, on a chosen button, the selection bar.
    fn filled_rectangles(pass_output: &egui::FullOutput) -> Vec<(Rect, Color32)> {
        pass_output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Rect(rect) if rect.stroke.width == 0.0 => Some((rect.rect, rect.fill)),
                _ => None,
            })
            .filter(|(rect, _)| rect.width() < screen().width())
            .collect()
    }

    /// Returns the fill colour of the widest rectangle, which is the button's
    /// face.
    ///
    /// The face and its shadow have the same width. `max_by` keeps the last of
    /// equally wide rectangles, and the face is painted after the shadow, so
    /// the face wins the tie.
    fn widest_fill(painted: &[(Rect, Color32)]) -> Option<Color32> {
        painted
            .iter()
            .max_by(|left, right| left.0.width().total_cmp(&right.0.width()))
            .map(|(_, fill)| *fill)
    }

    /// Runs one pass on `context` with `events`, calling `build` to add the
    /// widget.
    ///
    /// The texture changes are cleared before the output is returned: epaint
    /// panics when a texture change is dropped without being applied, and
    /// these tests apply none.
    fn pass(
        context: &egui::Context,
        events: Vec<Event>,
        pass_index: usize,
        mut build: impl FnMut(&mut Ui) -> Response,
    ) -> egui::FullOutput {
        let mut pass_output = context.run_ui(
            RawInput {
                screen_rect: Some(screen()),
                events,
                time: Some(pass_index as f64 * 0.1),
                ..Default::default()
            },
            |ui| {
                build(ui);
            },
        );
        pass_output.textures_delta.clear();
        pass_output
    }

    /// Returns the three event lists that press and release at `center`: a
    /// move there, a press, and a release, one list per pass.
    fn press_at(center: Pos2) -> Vec<Vec<Event>> {
        vec![
            vec![Event::PointerMoved(center)],
            vec![Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            vec![Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        ]
    }

    #[test]
    fn a_press_and_a_release_make_a_click() {
        let theme = theme();
        let context = egui::Context::default();
        // The first pass lays the button out. Its rectangle stays the same
        // afterwards, so the pointer can aim at the centre.
        let mut rect = Rect::ZERO;
        pass(&context, Vec::new(), 0, |ui| {
            let response = ui.add(ChunkyButton::new("Press", &theme).accent(true));
            rect = response.rect;
            response
        });
        assert!(rect.is_positive(), "the button was laid out");
        let mut clicked = false;
        for (index, events) in press_at(rect.center()).into_iter().enumerate() {
            pass(&context, events, index + 1, |ui| {
                let response = ui.add(ChunkyButton::new("Press", &theme).accent(true));
                clicked |= response.clicked();
                response
            });
        }
        assert!(clicked, "the press and release did not make a click");
    }

    #[test]
    fn a_disabled_button_reports_no_click() {
        let theme = theme();
        let context = egui::Context::default();
        let mut rect = Rect::ZERO;
        let pass_output = pass(&context, Vec::new(), 0, |ui| {
            let response = ui.add_enabled(false, ChunkyButton::new("Off", &theme));
            rect = response.rect;
            assert!(!response.enabled(), "the response knows it is disabled");
            response
        });
        assert!(rect.is_positive());
        let mut clicked = false;
        for (index, events) in press_at(rect.center()).into_iter().enumerate() {
            pass(&context, events, index + 1, |ui| {
                let response = ui.add_enabled(false, ChunkyButton::new("Off", &theme));
                clicked |= response.clicked();
                response
            });
        }
        assert!(!clicked, "a disabled button reported a click");
        // A disabled `Ui` fades whatever is painted in it, so the face shows
        // the panel fill at half alpha, not the colour of a live state.
        let painted = filled_rectangles(&pass_output);
        let face_fill = widest_fill(&painted).expect("the button painted a face");
        assert!(
            face_fill.a() < 255,
            "a disabled face is faded: {face_fill:?}"
        );
        assert!(
            face_fill.r() <= to_egui_color(theme.palette.panel).r(),
            "a disabled face is no brighter than the panel: {face_fill:?}"
        );
        assert_ne!(face_fill, to_egui_color(theme.palette.panel_raised));
        assert_ne!(face_fill, to_egui_color(theme.palette.accent));
    }

    /// The face uses the token that matches the state: raised at rest, control
    /// while the pointer is over it, and line while held.
    #[test]
    fn hover_and_press_use_the_state_tokens() {
        let theme = theme();
        let context = egui::Context::default();
        let mut rect = Rect::ZERO;
        let rest = pass(&context, Vec::new(), 0, |ui| {
            let response = ui.add(ChunkyButton::new("Idle", &theme));
            rect = response.rect;
            response
        });
        assert_eq!(
            widest_fill(&filled_rectangles(&rest)),
            Some(to_egui_color(theme.palette.panel_raised)),
            "the resting face is the raised control fill"
        );

        let center = rect.center();
        let hovered = pass(&context, vec![Event::PointerMoved(center)], 1, |ui| {
            ui.add(ChunkyButton::new("Idle", &theme))
        });
        assert_eq!(
            widest_fill(&filled_rectangles(&hovered)),
            Some(to_egui_color(theme.palette.control)),
            "hovering uses the control fill"
        );

        let held = pass(
            &context,
            vec![Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
            2,
            |ui| ui.add(ChunkyButton::new("Idle", &theme)),
        );
        assert_eq!(
            widest_fill(&filled_rectangles(&held)),
            Some(to_egui_color(theme.palette.line)),
            "holding the button uses the pressed fill"
        );
    }

    #[test]
    fn a_clipped_button_paints_nothing() {
        let theme = theme();
        // A one-pixel screen: the button is allocated below the visible area.
        let context = egui::Context::default();
        let mut pass_output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1.0, 1.0))),
                ..Default::default()
            },
            |ui| {
                ui.add_space(200.0);
                ui.add(ChunkyButton::new("Off screen", &theme).accent(true));
            },
        );
        pass_output.textures_delta.clear();
        assert!(
            pass_output.shapes.is_empty(),
            "a clipped button drew something: {:?}",
            pass_output.shapes.len()
        );
    }
}
