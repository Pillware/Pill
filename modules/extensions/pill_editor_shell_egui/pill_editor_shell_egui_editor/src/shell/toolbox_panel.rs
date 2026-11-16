//! The floating toolbox that sits over a scene viewport.
//!
//! # Responsibilities
//!
//! - `toolbox_overlay`: one draggable panel per scene viewport, holding the
//!   gizmo's three tools, its space, its snapping and the scene grid's switch.
//!   The panel's rectangle and every row's rectangle are logged whenever the
//!   panel moves.
//!
//! # Design
//!
//! The globs below are absolute paths. A generated patch crate replays this
//! file's top-level `use` lines, so a `super::`-relative glob would not
//! resolve there.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

/// Draws the floating toolbox over one scene viewport: the gizmo's tools, its
/// space, its snapping and the scene grid's switch.
///
/// The panel carries the same settings as the viewport menu, always on screen.
/// A press on the handle along the panel's top drags it. A drag is kept inside
/// the viewport; a viewport too small to hold the whole panel keeps the
/// panel's top-left corner at its default spot, so the handle and the upper
/// rows stay reachable.
///
/// The panel floats over the scene's child window, and its rectangle joins the
/// frame's overlay regions, so a click anywhere on it - a row or the gap
/// between rows - belongs to the UI and never becomes a scene pick. The
/// panel's rectangle and every row's rectangle are logged whenever the panel
/// moves, and the scripted suite clicks the centre of the logged rows.
#[pill_hot_fn]
pub fn toolbox_overlay(viewer: &mut EditorTabViewer<'_>, ui: &mut egui::Ui, view: usize) {
    // Step 1: Read the settings the rows show, then place the panel at its
    // dragged origin or at the viewport's default corner.
    let viewport = ui.max_rect();
    let state = toolbox::ToolboxState {
        gizmo: viewer.snapshot.gizmo,
        grid: viewer.snapshot.grid_enabled,
        frame: viewer.snapshot.gizmo_frame.as_deref(),
    };
    let mut origin = toolbox::clamped_origin(
        viewer
            .toolbox
            .origin(view)
            .unwrap_or_else(|| toolbox::default_origin(viewport)),
        viewport,
    );
    // Step 2: Drag the panel by its handle, which is the one part of the panel
    // a press drags. A click on a row below it still reaches the row's button.
    let handle = toolbox::handle_rect(origin);
    let response = ui.interact(
        handle,
        ui.id().with(("scene-toolbox", view)),
        egui::Sense::drag(),
    );
    if response.dragged() {
        origin = toolbox::clamped_origin(origin + response.drag_delta(), viewport);
        viewer.toolbox.set_origin(view, origin);
    }
    if response.drag_stopped() {
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            view,
            x = origin.x,
            y = origin.y,
            "[editor] toolbox moved"
        );
    }
    // Step 3: Paint the panel's background and its title, then draw its rows.
    let panel = toolbox::panel_rect(origin);
    ui.painter().rect(
        panel,
        egui::CornerRadius::same(5),
        style::overlay(),
        egui::Stroke::new(1.0, style::overlay_border()),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        egui::pos2(panel.center().x, handle.center().y),
        egui::Align2::CENTER_CENTER,
        "Tools",
        egui::TextStyle::Small.resolve(ui.style()),
        style::text_muted(),
    );
    // The rows are ordinary buttons, each as wide as its row, so the panel
    // reads as one column of controls: the row of the current setting takes
    // the theme's selection colour, and the rest keep the widget's idle fill.
    // Each button's rectangle is recorded for the layout test, and the
    // scripted suite clicks the centre of each row's logged rectangle.
    let mut faces = Vec::with_capacity(toolbox::TOOLBOX_ITEMS.len());
    for item in toolbox::TOOLBOX_ITEMS {
        let rect = toolbox::item_rect(item, origin);
        let button = ui
            .scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                ui.add_sized(
                    rect.size(),
                    egui::Button::new(item.label(&state)).selected(item.active(&state)),
                )
            })
            .inner;
        faces.push(button.rect);
        if button.clicked() {
            viewer.pending.push(vec![item.command(&state)]);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                view,
                item = item.name(),
                "[editor] toolbox click"
            );
        }
    }
    // Step 4: Record the panel's rectangle as an overlay region and store the
    // rectangles the row buttons drew in the session. Each time the panel's
    // rectangle changes, the panel and every row are logged once.
    viewer.toolbox.set_faces(view, faces);
    viewer.overlay_regions.push(shell_rect(panel));
    if viewer.toolbox.remember_logged(view, panel) {
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            view,
            x = panel.min.x,
            y = panel.min.y,
            width = panel.width(),
            height = panel.height(),
            "[editor] toolbox rect"
        );
        for (item, rect) in std::iter::once(("handle", handle)).chain(
            toolbox::TOOLBOX_ITEMS
                .iter()
                .map(|item| (item.name(), toolbox::item_rect(*item, origin))),
        ) {
            pill_core::debug!(
                target: telemetry_target::EDITOR,
                view,
                item,
                x = rect.min.x,
                y = rect.min.y,
                width = rect.width(),
                height = rect.height(),
                "[editor] toolbox item"
            );
        }
    }
}
