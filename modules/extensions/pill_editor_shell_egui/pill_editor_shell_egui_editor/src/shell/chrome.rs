//! The chrome the shell's tabs share: tab icons, the selection moves, the
//! context menus and the rectangles the frame reports as the UI's.
//!
//! # Responsibilities
//!
//! - [`tab_icon`]: the icon a dock tab draws beside its title.
//! - [`paint_active_tab_underline`]: the accent line under the active tab's
//!   name, which the dock does not draw itself.
//! - The selection moves the tabs and the scene viewports make, each logging
//!   the token a suite watches for.
//! - [`apply_tab_redock`]: puts a floating surface's tabs back into the dock.
//! - [`empty_area`]: the empty strip under a panel's rows, where its menu
//!   opens.
//! - [`claim_pointer`], [`mark_menu_region`], [`context_menu`] and
//!   [`floating_popup`]: the rectangles a menu or a popup claims, so a click
//!   on it is never a scene pick.
//! - [`engine_badge`]: the mark on a picker row whose asset comes from the
//!   engine rather than the project.
//! - [`remember_moved_rect`]: the once-per-placement logging the Inspector's
//!   sections and the scene viewports' badges use.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Unused here on purpose: a patch crate repeats this file's opening lines, so
// the glob must be present for a patched body that names a sibling in the
// shell.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Types
// =============================================================================

/// A floating popup as its caller sees it.
///
/// It carries the area the popup drew and whether this frame's press landed
/// outside it.
pub struct FloatingPopup {
    /// The area's own response: its rectangle is the popup's.
    pub response: egui::Response,
    /// Whether this frame pressed outside both the anchor and the popup. A
    /// picker uses it to close itself.
    pub pressed_outside: bool,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the icon a dock tab draws beside its title: the view or panel the
/// tab shows.
pub fn tab_icon(tab: EditorTab) -> icons::Icon {
    match tab {
        EditorTab::Scene(0) => icons::Icon::Cube,
        EditorTab::Scene(_) => icons::Icon::CubeTransparent,
        EditorTab::Game => icons::Icon::MonitorPlay,
        EditorTab::Entities => icons::Icon::TreeStructure,
        EditorTab::Systems => icons::Icon::Gear,
        EditorTab::Inspector => icons::Icon::SlidersHorizontal,
        EditorTab::Assets => icons::Icon::FolderOpen,
        // The icon set has no terminal glyph, so the Console tab uses the
        // code brackets: the closest mark to a command prompt the set has.
        EditorTab::Console => icons::Icon::Code,
        EditorTab::Problems => icons::Icon::XCircle,
        EditorTab::Statistics => icons::Icon::Gear,
        EditorTab::Preferences => icons::Icon::Faders,
        // A module panel belongs to somebody else. The set has no puzzle-piece
        // glyph for an extension, so the panel uses the four-cell grid.
        EditorTab::Module(_) => icons::Icon::GridFour,
    }
}

/// Paints the accent underline under the active tab's name.
///
/// The dock draws its separator line under every inactive tab and skips the
/// active one, so the accent that marks the selected tab is left to the shell.
/// This line sits where the dock's own line would: the last pixel of the tab
/// button.
///
/// Public because [`tab_button`]'s generated hot patch reaches it through the
/// package path.
pub fn paint_active_tab_underline(response: &egui::Response) {
    let pixel = response.ctx.pixels_per_point().recip();
    response.ctx.layer_painter(response.layer_id).hline(
        response.rect.x_range(),
        response.rect.bottom() - pixel,
        (pixel, style::accent()),
    );
}

/// Returns the lowercase name of one gizmo mode, as the Preferences tab's rows
/// show it.
pub fn gizmo_mode_label(mode: GizmoMode) -> &'static str {
    match mode {
        GizmoMode::Translate => "translate",
        GizmoMode::Rotate => "rotate",
        GizmoMode::Scale => "scale",
    }
}

/// Returns the lowercase name of one gizmo space, as the Preferences tab's
/// rows show it.
pub fn gizmo_space_label(space: GizmoSpace) -> &'static str {
    match space {
        GizmoSpace::World => "world",
        GizmoSpace::Local => "local",
    }
}

/// What a click on an entity row means.
pub use pill_editor_core::editor_state::SelectionClick;

/// Returns what a click means with these modifiers held: Shift selects a
/// range, Ctrl or Command adds to the selection, and a plain click resets it.
///
/// The rule itself is the core's ([`SelectionClick::from_modifiers`]), so a
/// row click and a scene press cannot disagree about what a held key means.
pub fn click_from_modifiers(modifiers: &egui::Modifiers) -> SelectionClick {
    SelectionClick::from_modifiers(modifiers.shift, modifiers.command || modifiers.ctrl)
}

/// Moves the shared selection onto `entity` alone.
///
/// It logs the same `[editor] selection changed` token a scene pick logs, so a
/// suite can watch either path. Returns whether the selection moved.
pub fn select_entity(selection: &mut EditorSelection, entity: Entity) -> bool {
    select_entity_click(selection, entity, SelectionClick::Reset, &[])
}

/// Applies one click on an entity row to the shared selection.
///
/// `rows` is the list's own order, which only a range click reads: a range is
/// every row from the primary to the clicked one, so Shift-click selects what
/// the list shows between them. With no anchor in `rows`, a range click
/// behaves like a plain one. Returns whether the selection changed.
pub fn select_entity_click(
    selection: &mut EditorSelection,
    entity: Entity,
    click: SelectionClick,
    rows: &[Entity],
) -> bool {
    let before = selection.entities().to_vec();
    let clicked = selection.clicked(click, entity, rows);
    selection.set_entities(clicked);
    report_selection_change(selection, &before, entity)
}

/// Logs the `[editor] selection changed` token for a move the click helpers
/// made, and returns whether the entity set changed.
pub fn report_selection_change(
    selection: &EditorSelection,
    before: &[Entity],
    entity: Entity,
) -> bool {
    if selection.entities() == before {
        return false;
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %entity,
        count = selection.entities().len(),
        "[editor] selection changed"
    );
    true
}

/// Moves the shared selection onto the asset at `path` and logs the
/// `[editor] asset selected` token. Returns whether the selection moved.
pub fn select_asset(selection: &mut EditorSelection, path: &str) -> bool {
    if selection.asset.as_deref() == Some(path) {
        return false;
    }
    selection.asset = Some(path.to_string());
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path,
        "[editor] asset selected"
    );
    true
}

/// Brings a floating surface's tabs back into the dock: the Redock item of a
/// tab menu, run after the pass that asked.
///
/// Public because [`settle_pass`]'s generated hot patch calls it through the
/// package path.
pub fn apply_tab_redock(dock: &mut DockState<EditorTab>, surface: SurfaceIndex) {
    // Step 1: collect the tabs the floating surface holds.
    let tabs: Vec<EditorTab> = dock
        .iter_all_tabs()
        .filter(|(path, _)| path.surface == surface)
        .map(|(_, tab)| *tab)
        .collect();
    if tabs.is_empty() {
        return;
    }
    // Step 2: drop the empty surface and add each tab back to the dock's first
    // leaf.
    dock.remove_surface(surface);
    for tab in &tabs {
        dock.push_to_first_leaf(*tab);
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        count = tabs.len(),
        "[editor] tabs redocked"
    );
}

/// Returns the size of the empty strip under a panel's rows, where the panel's
/// own context menu opens away from any row.
pub fn empty_area(ui: &egui::Ui) -> egui::Vec2 {
    egui::Vec2::new(ui.available_width(), 32.0)
}

/// Logs a context menu opening as `[editor] context menu`, with `area` set to
/// the name given, so a suite can tell which menu a right-click opened.
///
/// Public because the dock's tab bar logs through it directly: the dock
/// reports a tab right-click from a hook that carries no [`context_menu`]
/// response.
pub fn log_context_menu(area: &str) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        area,
        "[editor] context menu"
    );
}

/// Claims one floating surface's rectangle for the UI.
///
/// The create and rename dialog, the About window, a picker popup under a
/// field and an open menu push their rectangle through here, so a press inside
/// them is the UI's and never a scene pick. One helper keeps the conversion
/// from an egui rectangle to a [`ShellRect`] in one place.
pub fn claim_pointer(rect: egui::Rect, regions: &mut Vec<ShellRect>) {
    regions.push(ShellRect {
        x: rect.min.x,
        y: rect.min.y,
        width: rect.width(),
        height: rect.height(),
    });
}

/// Adds an open context menu's rectangle to the frame's overlay regions.
///
/// A menu floats over whatever it was opened on, and a menu near the screen
/// edge can hang over a scene viewport, so a click inside it must never reach
/// the scene behind it. `opened` is `None` on the frames the menu is closed.
///
/// Public like every helper the shell re-exports, so a patch crate reaches it
/// through the package path.
pub fn mark_menu_region(opened: Option<egui::InnerResponse<()>>, regions: &mut Vec<ShellRect>) {
    if let Some(menu) = opened {
        claim_pointer(menu.response.rect, regions);
    }
}

/// Opens a floating popup under an anchor rectangle, claims its area and
/// answers whether a press landed outside it.
///
/// A picker draws in a foreground `Area` under the field it belongs to, so it
/// floats over the rows below instead of pushing them down. The popup's
/// rectangle joins the frame's overlay regions, so a click inside it never
/// becomes a scene pick. `width` is the popup's minimum width in points, and
/// `contents` draws the popup's body.
///
/// The rule that closes the popup lives here too, because only this code knows
/// the popup's rectangle: any press outside the popup and its anchor closes
/// it, whatever button it was. A list that only closes when an item is picked
/// fights the person who changed their mind. The press that opened a picker
/// landed on its anchor, so it never closes the popup it just opened.
pub fn floating_popup(
    ui: &egui::Ui,
    id: egui::Id,
    anchor: egui::Rect,
    width: f32,
    regions: &mut Vec<ShellRect>,
    contents: &mut dyn FnMut(&mut egui::Ui),
) -> FloatingPopup {
    // Step 1: draw the popup under the anchor and claim its area.
    let area = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor.left(), anchor.bottom() + 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(width);
                contents(ui);
            });
        });
    claim_pointer(area.response.rect, regions);
    // Step 2: decide whether this frame's press landed outside both the anchor
    // and the popup.
    let press = ui.input(|input| (input.pointer.any_pressed(), input.pointer.interact_pos()));
    let pressed_outside = match press {
        (true, Some(at)) => !anchor.contains(at) && !area.response.rect.contains(at),
        _ => false,
    };
    FloatingPopup {
        response: area.response,
        pressed_outside,
    }
}

/// Opens a panel's context menu on a secondary click.
///
/// The area is logged once per click, so a suite can see whose menu opened,
/// and `contents` builds the items. While the menu is open, its rectangle joins
/// the frame's overlay regions.
///
/// Every panel that has a menu goes through here - the scene viewport, the
/// Entities rows and strip, the Inspector's headings and strip, the Assets
/// rows, tiles and strip - so no panel can forget to mark its menu. The dock's
/// tab bar is the exception: it logs through [`log_context_menu`] and marks the
/// menu it draws itself.
pub fn context_menu(
    area: &str,
    response: &egui::Response,
    regions: &mut Vec<ShellRect>,
    contents: &mut dyn FnMut(&mut egui::Ui),
) {
    // Step 1: log the click that opened the menu, then let the surface build
    // the items.
    if response.secondary_clicked() {
        log_context_menu(area);
    }
    let opened = response.context_menu(|ui| contents(ui));
    // Step 2: claim the open menu's rectangle for the UI.
    mark_menu_region(opened, regions);
}

/// The badge a picker puts on a row whose asset is not one of the project's
/// own files: a store asset built in code (a chain's built-in pass, the
/// renderer's default material) or a file under a module root.
///
/// Public like every helper the shell re-exports, so a patch crate reaches it
/// through the package path.
pub fn engine_badge(ui: &mut egui::Ui) {
    let background = ui.visuals().extreme_bg_color;
    ui.label(
        egui::RichText::new("engine")
            .small()
            .color(style::text_muted())
            .background_color(background),
    );
}

/// Records one keyed rectangle and answers whether it is new or has moved, so
/// a section that shifts as a panel grows or collapses is logged again instead
/// of keeping its old place in the log.
///
/// The comparison is pixel-exact: a section header is an anchor a suite
/// clicks, so half a point of drift is a different target.
pub fn remember_moved_rect(logged: &mut LoggedRects<String>, key: &str, rect: egui::Rect) -> bool {
    logged.remember_exact(key.to_owned(), rect)
}
