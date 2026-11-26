//! Tooltips that open above the widget they belong to.
//!
//! # Responsibilities
//!
//! - [`on_hover_text`], [`on_hover_ui`] and [`on_disabled_hover_text`]: egui's
//!   own tooltips, aimed above the widget.
//! - [`TOOLTIP_ALIGN`]: the alignment they all ask for.
//!
//! # Design
//!
//! egui places a tooltip below its widget - a popup's default alignment is
//! `RectAlign::BOTTOM_START` - and nothing on the context changes that, so a
//! UI that wants the tooltip above has to say so where the tooltip is asked
//! for. These functions are that place: each builds egui's own `Tooltip` and
//! re-aims its popup before showing it.
//!
//! The fallback is egui's own as well: it tries the alignment it was given,
//! then that alignment flipped in each direction, so a widget with no room
//! above - the menu bar, a tab strip along the top of a window - gets its
//! tooltip below instead. `TOP_START`'s flipped-in-y form is `BOTTOM_START`,
//! which is where every tooltip opened before this module existed.

// External crates
use egui::{Label, RectAlign, Response, Tooltip, Ui, WidgetText};

/// The alignment every tooltip here asks for: above the widget, left edges
/// aligned, which is egui's own default mirrored.
pub const TOOLTIP_ALIGN: RectAlign = RectAlign::TOP_START;

// =============================================================================
// Free Functions
// =============================================================================

/// Shows `text` above the widget `response` belongs to, while it is hovered.
///
/// Returns the response, so a call site can keep chaining.
pub fn on_hover_text(response: Response, text: impl Into<WidgetText>) -> Response {
    on_hover_ui(response, |ui| {
        // Keep the area from shrinking a tooltip whose content changes, the
        // guard egui's own text tooltips carry.
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(Label::new(text));
    })
}

/// Shows `contents` above the widget `response` belongs to, while it is
/// hovered.
pub fn on_hover_ui(response: Response, add_contents: impl FnOnce(&mut Ui)) -> Response {
    aligned(Tooltip::for_enabled(&response)).show(add_contents);
    response
}

/// Shows `text` above a disabled widget while the pointer rests on it.
pub fn on_disabled_hover_text(response: Response, text: impl Into<WidgetText>) -> Response {
    aligned(Tooltip::for_disabled(&response)).show(|ui| {
        ui.set_max_width(ui.spacing().tooltip_width);
        ui.add(Label::new(text));
    });
    response
}

/// The tooltip, aimed where this module's tooltips belong.
///
/// `Tooltip` hands out its popup as a public field and keeps the rest private,
/// so the popup is moved out, re-aimed and put back.
fn aligned(tooltip: Tooltip<'_>) -> Tooltip<'_> {
    let mut tooltip = tooltip;
    let popup = tooltip.popup.align(TOOLTIP_ALIGN);
    tooltip.popup = popup;
    tooltip
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2, Context, Event, Pos2, RawInput, Rect};

    /// The surface the test's passes draw on.
    const SURFACE: f32 = 400.0;
    /// The widget's size, and where its left edge sits on the surface.
    const WIDGET_SIZE: (f32, f32) = (100.0, 24.0);
    const WIDGET_LEFT: f32 = 60.0;

    /// How many passes a tooltip needs: one to move the pointer in, one to
    /// pass the delay, and one to place the tooltip with the size it measured.
    const PASSES: usize = 4;

    /// The widget's rectangle on the surface, at `top`.
    fn widget_rect(top: f32) -> Rect {
        Rect::from_min_size(pos2(WIDGET_LEFT, top), vec2(WIDGET_SIZE.0, WIDGET_SIZE.1))
    }

    /// Runs the passes a tooltip needs and answers with its rectangle, and the
    /// rectangle of the widget it belongs to.
    ///
    /// The pointer lands on the widget in the first pass and then rests there
    /// while the clock runs, because egui refuses a tooltip for a pointer that
    /// is still moving, and only opens one after its delay.
    fn tooltip_of(top: f32, enabled: bool) -> (Rect, Rect) {
        let context = Context::default();
        let target = widget_rect(top);
        let mut widget = None;
        for pass in 0..PASSES {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(SURFACE, SURFACE))),
                time: Some(pass as f64),
                ..RawInput::default()
            };
            if pass == 0 {
                input.events.push(Event::PointerMoved(target.center()));
            }
            let mut state = None;
            let mut output = context.run_ui(input, |ui| {
                let response = if enabled {
                    let response = ui.put(target, egui::Label::new("hover me"));
                    on_hover_text(response, "the tooltip")
                } else {
                    let mut placed = None;
                    ui.add_enabled_ui(false, |ui| {
                        placed = Some(ui.put(target, egui::Label::new("hover me")));
                    });
                    on_disabled_hover_text(placed.expect("the widget was placed"), "the tooltip")
                };
                state = Some((response.id, response.enabled()));
            });
            // Nothing paints here, so the atlas's changes have nowhere to go;
            // epaint refuses a delta dropped instead of applied.
            output.textures_delta.clear();
            widget = state;
            assert_eq!(
                widget.map(|(_, enabled)| enabled),
                Some(enabled),
                "the widget is not in the state it was made in"
            );
        }
        let id = Tooltip::tooltip_id(widget.expect("the passes ran").0, 0);
        let tooltip = context
            .memory(|memory| memory.area_rect(id))
            .expect("no tooltip was shown");
        (tooltip, target)
    }

    #[test]
    fn a_tooltip_opens_above_its_widget() {
        let (tooltip, widget) = tooltip_of(160.0, true);
        assert!(
            tooltip.bottom() <= widget.top(),
            "the tooltip {} sits below the widget {}",
            tooltip,
            widget
        );
    }

    #[test]
    fn a_tooltip_with_no_room_above_opens_below() {
        let (tooltip, widget) = tooltip_of(2.0, true);
        assert!(
            tooltip.top() >= widget.bottom(),
            "a widget against the top edge got its tooltip at {}, widget {}",
            tooltip,
            widget
        );
    }

    #[test]
    fn a_disabled_widget_tooltip_opens_above_too() {
        let (tooltip, widget) = tooltip_of(160.0, false);
        assert!(
            tooltip.bottom() <= widget.top(),
            "the tooltip {} sits below the widget {}",
            tooltip,
            widget
        );
    }
}
