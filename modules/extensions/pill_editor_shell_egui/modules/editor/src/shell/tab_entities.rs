//! The Entities tab: the entity rows and their menu.
//!
//! # Responsibilities
//!
//! - `DraggedEntity`: the drag payload a row carries to an Inspector
//!   entity-reference field.
//! - `entity_rows` and `entity_row_label`: the rows the tab lists, from the
//!   snapshot's entity list.
//! - `entity_row_height`: the one height every row has.
//! - `entities_tab`: the rows on screen with their selection and context
//!   menus, and the empty strip below the last one.
//! - `entity_menu_items`: the row menu's create and delete actions.
//! - `remember_hierarchy_row_rect`: each row's rectangle, logged for the suites.
//!
//! # Design
//!
//! Both globs below are absolute paths. A generated patch crate replays this
//! file's top-level `use` lines, and a `super::`-relative glob would have
//! nothing to point at there.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

/// The payload an Entities row drags onto an Inspector entity-reference
/// field.
///
/// A newtype, not the bare entity handle. egui matches a drop by the payload
/// type, so an entity and the path string an asset tile drags must not share
/// one.
#[derive(Clone, Copy, Debug)]
pub struct DraggedEntity(pub Entity);

/// Record an Entities row's rectangle in the shell's session.
///
/// The row logs its rectangle on its first draw and whenever it moves, so a
/// suite can drag from a row it cannot otherwise locate.
pub fn remember_hierarchy_row_rect(
    session: &mut InspectorSession,
    entity: Entity,
    rect: egui::Rect,
) {
    let entity_key = entity.to_string();
    session.hierarchy_seen.insert(entity_key.clone());
    if !session.hierarchy_row_rects.remember_exact(entity_key, rect) {
        return;
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        entity = %entity,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] hierarchy row rect"
    );
}

/// Returns one row per live entity, for the Entities tab.
///
/// Each row pairs the entity handle with its label: the display name and the
/// number of components attached.
pub fn entity_rows(snapshot: &EditorSnapshot) -> Vec<(Entity, String)> {
    snapshot
        .entities
        .iter()
        .map(|entry| (entry.entity, entity_row_label(entry)))
        .collect()
}

/// The label one Entities row shows: the display name and the number of
/// components attached.
pub fn entity_row_label(entry: &EntityListEntry) -> String {
    format!("{}  ({} components)", entry.display, entry.component_count)
}

/// The height of one Entities row, without the spacing between rows.
///
/// A row is a selectable button, which egui sizes to its text plus the button
/// padding, and never below the interact height. Every row has the same
/// height, which is what lets the tab lay out only the rows on screen.
pub fn entity_row_height(ui: &egui::Ui) -> f32 {
    let text_height = ui.text_style_height(&egui::TextStyle::Button);
    let padded_height = text_height + 2.0 * ui.spacing().button_padding.y;
    padded_height.max(ui.spacing().interact_size.y)
}

/// Adds the entity menu's rows to `ui` and queues the recorded edit of the one
/// that is clicked.
///
/// `entity` is the row the menu was opened on, or `None` for the strip below
/// the rows, where only New Entity is offered. Undo reverses either edit.
pub fn entity_menu_items(
    ui: &mut egui::Ui,
    entity: Option<Entity>,
    selection: &EditorSelection,
    pending: &mut PendingEditorCommands,
) {
    if ui
        .add(icons::IconButton::new(icons::Icon::Plus, "New Entity"))
        .clicked()
    {
        let (command, label) = entities::entity_create();
        pending.push_recorded(vec![command], label, None);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            "[editor] new entity queued"
        );
        ui.close();
    }
    if let Some(entity) = entity {
        if ui
            .add(icons::IconButton::new(icons::Icon::Trash, "Delete Entity"))
            .clicked()
        {
            let (commands, label) = entities::entity_delete(entity, selection.asset.clone());
            pending.push_recorded(commands, label, None);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                "[editor] delete entity queued"
            );
            ui.close();
        }
    }
}

/// Draws the Entities tab: one row per live entity, then an empty strip that
/// offers the same create action.
///
/// Only the rows inside the visible part of the list are laid out, so the
/// tab's cost follows the panel's height rather than the world's size. The
/// empty strip follows the last row, so it shows once the list is scrolled to
/// its end.
///
/// A left click selects the row, `Shift` extends the selection over a range
/// and `Ctrl` adds to it. A right click selects the row and opens its menu.
/// Dragging a row carries its entity to an Inspector reference field.
#[pill_hot_fn]
pub fn entities_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    selection: &mut EditorSelection,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    let total_rows = snapshot.entities.len();
    let row_height = entity_row_height(ui);
    egui::ScrollArea::vertical().show_rows(ui, row_height, total_rows, |ui, visible_rows| {
        let reaches_last_row = visible_rows.end == total_rows;
        // Step 1: the entity rows on screen.
        for entry in &snapshot.entities[visible_rows] {
            let entity = entry.entity;
            let selected = selection.contains(entity);
            let label_response = ui.selectable_label(selected, entity_row_label(entry));
            // The selected row: egui fills it with the theme's `selection`
            // colour, and the accent bar marks its left edge.
            if selected {
                style::selected_row_bar(ui, label_response.rect);
            }
            // One interaction layer senses both a click and a drag: a press
            // that moves carries the entity to a reference field, and a press
            // that does not is the row's click.
            let row_interaction = ui.interact(
                label_response.rect,
                label_response.id.with("dragged-entity"),
                egui::Sense::click_and_drag(),
            );
            row_interaction.dnd_set_drag_payload(DraggedEntity(entity));
            remember_hierarchy_row_rect(session, entity, row_interaction.rect);
            if row_interaction.clicked() {
                let selection_click = ui.input(|input| click_from_modifiers(&input.modifiers));
                // The list's own order, which a Shift-click reads to select a
                // range. Built only on a click, since it spans every row.
                let row_entities: Vec<Entity> =
                    snapshot.entities.iter().map(|entry| entry.entity).collect();
                select_entity_click(selection, entity, selection_click, &row_entities);
            }
            // A right click selects the row the menu opens for, replacing the
            // entity selection.
            if row_interaction.secondary_clicked() {
                select_entity(selection, entity);
            }
            context_menu("entities", &row_interaction, overlay_regions, &mut |ui| {
                entity_menu_items(ui, Some(entity), selection, pending)
            });
        }
        // Step 2: the empty strip below the rows, once the last row is on
        // screen. Its menu can only create an entity; there is no row to
        // delete.
        if reaches_last_row {
            let empty = ui.allocate_response(empty_area(ui), egui::Sense::click());
            context_menu("entities", &empty, overlay_regions, &mut |ui| {
                entity_menu_items(ui, None, selection, pending)
            });
        }
    });
}
