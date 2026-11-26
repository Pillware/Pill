//! The caption strip: the title bar the editor draws for itself.
//!
//! # Responsibilities
//!
//! - The three window buttons at the strip's right: minimize, maximize or
//!   restore, and close.
//! - The drag zones on the parts of the strip nothing else draws, which move
//!   the window and toggle maximize on a double click.
//! - The eight resize strips along the window's own edge.
//!
//! # Design
//!
//! The editor's window has no system frame (`with_decorations(false)`), so the
//! menu bar sits where the icon and the title used to be and everything the
//! frame used to do is drawn here. Nothing here owns the window: a press or a
//! drag becomes one [`WindowControl`] on the frame's requests, and the
//! executable starts the platform's own move, resize or maximize loop. That
//! keeps the window's behaviour - snapping, the taskbar, the resize loop -
//! the platform's, and keeps the shell free of window handles.
//!
//! The strip is drawn in a foreground area above the menu bar, so the bar keeps
//! its own layout and the buttons sit over it. The drag zones are computed
//! around what the bar already draws rather than laid over it, so a press on a
//! menu title or a play control is never taken for a window drag.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The width of one window button, in points. The three together take the
/// right end of the strip.
pub const CAPTION_BUTTON_WIDTH: f32 = 46.0;

/// The side of the square the maximize and restore glyphs draw, in points.
pub const CAPTION_GLYPH_SIDE: f32 = 10.0;

/// How far the restore glyph's front square sits from the band behind it, in
/// points.
pub const CAPTION_RESTORE_OFFSET: f32 = 3.0;

/// How thick the band along the window's edge is, in points. A drag inside it
/// resizes the window instead of reaching the dock.
pub const CAPTION_RESIZE_BORDER: f32 = 4.0;

/// The narrowest drag zone worth registering, in points. A strip squeezed
/// until nothing is left of a gap registers no zone instead of a sliver.
pub const CAPTION_DRAG_ZONE_MIN: f32 = 8.0;

/// The gap between the project's name and the window buttons, in points.
pub const CAPTION_TITLE_GAP: f32 = 10.0;

/// How tall the accent line along the strip's bottom edge is, in points.
pub const CAPTION_ACCENT_HEIGHT: f32 = 5.0;

/// The fill the close button takes while the pointer is over it: the red the
/// platform's own close button uses.
pub const CAPTION_CLOSE_HOVER: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);

// =============================================================================
// Types
// =============================================================================

/// One window button of the caption strip, in the order the strip draws them
/// from its right edge: close, maximize or restore, minimize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionButton {
    /// Minimize the window.
    Minimize,
    /// Maximize the window, or restore it when it already is.
    Maximize,
    /// Close the window.
    Close,
}

impl CaptionButton {
    /// Every button, from the strip's right edge leftwards.
    pub const ALL: [Self; 3] = [Self::Close, Self::Maximize, Self::Minimize];

    /// Returns the wire name of this button, for log lines and widget ids.
    pub fn name(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }

    /// Returns the control a press on this button asks the executable for.
    pub fn control(self) -> WindowControl {
        match self {
            Self::Minimize => WindowControl::Minimize,
            Self::Maximize => WindowControl::ToggleMaximize,
            Self::Close => WindowControl::Close,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the strip the caption occupies: the top of the window's client
/// area, as tall as the menu bar under it.
#[must_use]
pub fn caption_strip(screen: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        screen.min,
        egui::pos2(screen.right(), screen.top() + menubar::MENU_BAR_HEIGHT),
    )
}

/// Returns each window button's cell in `strip`, in [`CaptionButton::ALL`]
/// order - the rightmost cell first.
#[must_use]
pub fn button_cells(strip: egui::Rect) -> [(CaptionButton, egui::Rect); 3] {
    let mut cells = [(CaptionButton::Close, egui::Rect::NOTHING); 3];
    let mut right = strip.right();
    for (slot, button) in cells.iter_mut().zip(CaptionButton::ALL) {
        let left = right - CAPTION_BUTTON_WIDTH;
        *slot = (
            button,
            egui::Rect::from_min_max(
                egui::pos2(left, strip.top()),
                egui::pos2(right, strip.bottom()),
            ),
        );
        right = left;
    }
    cells
}

/// Draws the caption over the menu bar: the window buttons at the strip's
/// right, then the drag zones on the parts of the strip nothing else drew.
///
/// The buttons go to the executable as [`WindowControl`] requests; a press
/// draws its wash and glyph here, in the frame it happened, because the window
/// acts on the request one stage later.
pub fn caption_bar(
    context: &egui::Context,
    session: &MenubarSession,
    input: &EguiShellInput,
    viewer: &mut EditorTabViewer<'_>,
) {
    let strip = caption_strip(context.content_rect());
    let cells = button_cells(strip);
    // The drag zones end where the buttons begin.
    let buttons_left = cells
        .iter()
        .map(|(_, rect)| rect.min.x)
        .fold(f32::INFINITY, f32::min);
    let zones = drag_zones(session, strip, buttons_left);
    // The line along the strip's bottom edge, which a running session alone
    // draws: it is how the window says a game is live.
    let line =
        pill_editor_core::style::caption_line(&viewer.snapshot.theme, viewer.snapshot.run_mode);
    egui::Area::new(egui::Id::new("pill_editor_shell.caption"))
        .fixed_pos(strip.min)
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            // Step 0: the accent line along the strip's bottom edge, first so
            // the buttons paint over it.
            caption_accent_line(ui, strip, line);
            // Step 1: the window buttons, right to left.
            for (button, rect) in cells {
                if window_button(ui, button, rect, input.maximized) {
                    viewer.requests.window_control(button.control());
                }
            }
            // Step 2: the empty strip moves the window, and a double click
            // maximizes or restores it.
            for (index, rect) in zones.iter().copied().enumerate() {
                let response = ui.interact(
                    rect,
                    egui::Id::new(("pill_editor_shell.caption_drag", index)),
                    egui::Sense::click_and_drag(),
                );
                if response.drag_started() {
                    viewer.requests.window_control(WindowControl::Drag);
                } else if response.double_clicked() {
                    viewer
                        .requests
                        .window_control(WindowControl::ToggleMaximize);
                }
            }
            // Step 3: the project's name, where a system title bar would put
            // it. It is painted over the last drag zone and senses nothing, so
            // a drag that starts on it still moves the window.
            if let Some(zone) = zones.last() {
                caption_title(
                    ui,
                    caption_title_rect(strip, zone.left(), buttons_left),
                    &input.project_name,
                );
            }
        });
}

/// Returns the band the accent line fills: the strip's whole width, its bottom
/// [`CAPTION_ACCENT_HEIGHT`] points.
#[must_use]
pub fn caption_accent_rect(strip: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(strip.left(), strip.bottom() - CAPTION_ACCENT_HEIGHT),
        strip.max,
    )
}

/// Paints the line along the strip's bottom edge, when `colour` asks for one.
///
/// The strip keeps the panel's own colour and is plain chrome while the editor
/// is being edited; `colour` is `Some` only while a session runs
/// ([`pill_editor_core::style::caption_line`]), which is how the window says a
/// game is live. The line is drawn before the buttons, so a hovered button's
/// wash covers its own end of it.
pub fn caption_accent_line(ui: &egui::Ui, strip: egui::Rect, colour: Option<egui::Color32>) {
    let Some(colour) = colour else {
        return;
    };
    ui.painter()
        .rect_filled(caption_accent_rect(strip), 0.0, colour);
}

/// Returns the rectangle the project's name is right-aligned in: the room
/// between what the strip already draws and the window buttons.
///
/// `drawn_until` is where the last thing the bar drew ends - the play controls,
/// or the last menu title when there are none - and the name takes the space
/// from there to the buttons, one gap short at each end. A strip too narrow for
/// the gap leaves an empty rectangle, and the name is not drawn.
#[must_use]
pub fn caption_title_rect(strip: egui::Rect, drawn_until: f32, buttons_left: f32) -> egui::Rect {
    let left = drawn_until + CAPTION_TITLE_GAP;
    let right = buttons_left - CAPTION_TITLE_GAP;
    if right <= left {
        return egui::Rect::NOTHING;
    }
    egui::Rect::from_min_max(
        egui::pos2(left, strip.top()),
        egui::pos2(right, strip.bottom()),
    )
}

/// Draws a window title's worth of text in `rect`, right-aligned and cut with
/// an ellipsis when the rectangle is too narrow for it.
///
/// Nothing about it is interactive: a window's title is part of the strip a
/// drag moves, not a control.
pub fn caption_title(ui: &mut egui::Ui, rect: egui::Rect, title: &str) {
    if title.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    // The name reads as a title, not as a label on a panel: the strip's own
    // text colour, one step quieter than a menu title's.
    let colour = ui.visuals().weak_text_color();
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.add(
        egui::Label::new(egui::RichText::new(title).color(colour))
            .selectable(false)
            .truncate(),
    );
}

/// Registers the resize strips along the window's edge.
///
/// The strips are drawn last in the pass and take no room inside the window:
/// they sit on the outermost [`CAPTION_RESIZE_BORDER`] points of the client
/// area. A maximized window cannot be resized, so it registers none, and the
/// strip under the window buttons keeps them clickable.
pub fn caption_edges(
    context: &egui::Context,
    input: &EguiShellInput,
    viewer: &mut EditorTabViewer<'_>,
) {
    if input.maximized {
        return;
    }
    let screen = context.content_rect();
    let strip = caption_strip(screen);
    let buttons_left = button_cells(strip)
        .iter()
        .map(|(_, rect)| rect.min.x)
        .fold(f32::INFINITY, f32::min);
    egui::Area::new(egui::Id::new("pill_editor_shell.caption_edges"))
        .fixed_pos(screen.min)
        .order(egui::Order::Foreground)
        .show(context, |ui| {
            for edge in WindowEdge::ALL {
                let rect = edge_strip(edge, screen, buttons_left);
                if rect.width() < CAPTION_RESIZE_BORDER || rect.height() < CAPTION_RESIZE_BORDER {
                    continue;
                }
                let response = ui.interact(
                    rect,
                    egui::Id::new(("pill_editor_shell.caption_edge", edge.name())),
                    egui::Sense::drag(),
                );
                if response.hovered() {
                    ui.ctx().set_cursor_icon(edge_cursor(edge));
                }
                if response.drag_started() {
                    pill_core::debug!(
                        target: telemetry_target::EDITOR,
                        edge = edge.name(),
                        "[editor] window resize"
                    );
                    viewer.requests.window_control(WindowControl::Resize(edge));
                }
            }
        });
}

/// Returns the band one edge or corner takes along the window's edge.
///
/// `buttons_left` is where the window buttons begin: the top band stops there
/// so the buttons keep the pointer, and the top-right corner has no band of
/// its own, which is where the platform's own close button sits too.
#[must_use]
pub fn edge_strip(edge: WindowEdge, screen: egui::Rect, buttons_left: f32) -> egui::Rect {
    let border = CAPTION_RESIZE_BORDER;
    let left = screen.left();
    let right = screen.right();
    let top = screen.top();
    let bottom = screen.bottom();
    let (x0, x1, y0, y1) = match edge {
        WindowEdge::West => (left, left + border, top, bottom),
        WindowEdge::East => (right - border, right, top, bottom),
        WindowEdge::North => (left, buttons_left.min(right), top, top + border),
        WindowEdge::South => (left, right, bottom - border, bottom),
        WindowEdge::NorthWest => (left, left + border, top, top + border),
        // The platform's own close button sits in the top-right corner too, so
        // the corner takes no band: the buttons keep the pointer.
        WindowEdge::NorthEast => (buttons_left, buttons_left, top, top + border),
        WindowEdge::SouthWest => (left, left + border, bottom - border, bottom),
        WindowEdge::SouthEast => (right - border, right, bottom - border, bottom),
    };
    egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1))
}

/// Returns the strip's drag zones: the parts of it left over once the menu
/// titles and the play controls are taken out.
///
/// The zones are the gaps, so a press on a title or a play control is never a
/// window drag. A gap narrower than [`CAPTION_DRAG_ZONE_MIN`] registers
/// nothing.
#[must_use]
pub fn drag_zones(
    session: &MenubarSession,
    strip: egui::Rect,
    buttons_left: f32,
) -> Vec<egui::Rect> {
    // Step 1: what the strip already draws, in the order it sits.
    let mut drawn: Vec<egui::Rect> = (0..menubar::MENU_TITLE_COUNT)
        .filter_map(|index| session.geometry.get(&MenubarSlot::Title(index)))
        .collect();
    drawn.extend(session.geometry.get(&MenubarSlot::Toolbar));
    drawn.sort_by(|a, b| a.min.x.total_cmp(&b.min.x));
    // Step 2: the gaps between them, up to the window buttons.
    let span = egui::Rect::from_min_max(
        strip.min,
        egui::pos2(buttons_left.min(strip.right()), strip.bottom()),
    );
    let mut zones = Vec::new();
    let mut cursor = span.left();
    for rect in drawn {
        let start = rect.left().max(span.left());
        let end = rect.right().min(span.right());
        if end <= start {
            continue;
        }
        if start - cursor >= CAPTION_DRAG_ZONE_MIN {
            zones.push(egui::Rect::from_min_max(
                egui::pos2(cursor, span.top()),
                egui::pos2(start, span.bottom()),
            ));
        }
        cursor = cursor.max(end);
    }
    if span.right() - cursor >= CAPTION_DRAG_ZONE_MIN {
        zones.push(egui::Rect::from_min_max(
            egui::pos2(cursor, span.top()),
            egui::pos2(span.right(), span.bottom()),
        ));
    }
    zones
}

/// Draws one window button and reports whether it was pressed.
pub fn window_button(
    ui: &egui::Ui,
    button: CaptionButton,
    rect: egui::Rect,
    maximized: bool,
) -> bool {
    let response = ui.interact(
        rect,
        egui::Id::new(("pill_editor_shell.caption_button", button.name())),
        egui::Sense::click(),
    );
    let visuals = ui.visuals();
    let text = visuals.text_color();
    // The wash stays on while the button is held, like the platform's own, so a
    // press dragged off the button still reads as pressed.
    let pointer = response.hovered() || response.is_pointer_button_down_on();
    // Step 1: the wash. Close turns the platform's red; the other two lift one
    // step of the theme's own button fill.
    let (wash, tint) = match (button, pointer) {
        (CaptionButton::Close, true) => (CAPTION_CLOSE_HOVER, egui::Color32::WHITE),
        (_, true) => (visuals.widgets.hovered.weak_bg_fill, text),
        (_, false) => (egui::Color32::TRANSPARENT, text),
    };
    if wash != egui::Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 0.0, wash);
    }
    // Step 2: the glyph, drawn rather than set in a font so it matches the
    // platform's own at any scale factor.
    paint_glyph(ui.painter(), button, rect.center(), maximized, tint);
    response
        .on_hover_cursor(egui::CursorIcon::Default)
        .clicked()
}

/// Paints one button's glyph centred on `centre`.
pub fn paint_glyph(
    painter: &egui::Painter,
    button: CaptionButton,
    centre: egui::Pos2,
    maximized: bool,
    tint: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.0, tint);
    let half = CAPTION_GLYPH_SIDE * 0.5;
    match button {
        CaptionButton::Minimize => {
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y),
                    egui::pos2(centre.x + half, centre.y),
                ],
                stroke,
            );
        }
        CaptionButton::Maximize if maximized => {
            // The restore glyph: the square of the window to come, with the
            // band of the one it replaces showing behind its top and right.
            let offset = CAPTION_RESTORE_OFFSET;
            let front = egui::Rect::from_min_size(
                egui::pos2(centre.x - half, centre.y - half + offset),
                egui::vec2(CAPTION_GLYPH_SIDE - offset, CAPTION_GLYPH_SIDE - offset),
            );
            painter.line_segment(
                [
                    egui::pos2(centre.x - half + offset, centre.y - half),
                    egui::pos2(centre.x + half, centre.y - half),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(centre.x + half, centre.y - half),
                    egui::pos2(centre.x + half, centre.y + half - offset),
                ],
                stroke,
            );
            painter.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Inside);
        }
        CaptionButton::Maximize => {
            let square = egui::Rect::from_center_size(
                centre,
                egui::vec2(CAPTION_GLYPH_SIDE, CAPTION_GLYPH_SIDE),
            );
            painter.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
        }
        CaptionButton::Close => {
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y - half),
                    egui::pos2(centre.x + half, centre.y + half),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(centre.x - half, centre.y + half),
                    egui::pos2(centre.x + half, centre.y - half),
                ],
                stroke,
            );
        }
    }
}

/// Returns the pointer the platform shows while an edge is under it.
pub fn edge_cursor(edge: WindowEdge) -> egui::CursorIcon {
    match edge {
        WindowEdge::North | WindowEdge::South => egui::CursorIcon::ResizeVertical,
        WindowEdge::West | WindowEdge::East => egui::CursorIcon::ResizeHorizontal,
        WindowEdge::NorthWest | WindowEdge::SouthEast => egui::CursorIcon::ResizeNwSe,
        WindowEdge::NorthEast | WindowEdge::SouthWest => egui::CursorIcon::ResizeNeSw,
    }
}
