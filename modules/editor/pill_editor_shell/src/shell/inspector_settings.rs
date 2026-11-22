//! The Inspector's asset import settings.
//!
//! # Responsibilities
//!
//! - `asset_settings_rows` and `asset_settings_row`: draw the selected asset's
//!   settings as a recursive walk. A container becomes a heading over its
//!   children one level in; a leaf becomes one label and control pair.
//! - `settings_row_label`, `settings_heading` and `settings_level`: the naming
//!   and the indent the walk shares with the reference rows, so
//!   `parameters.pbr_base.Color` draws a `parameters` heading, a `pbr_base`
//!   heading and a `Color` row.
//! - `reference_row` and `reference_picker`: one row per reference field the
//!   asset's type declares, and the popup that picks a new target. The same
//!   row serves a material's shader and its texture slots, driven by the
//!   snapshot's answers rather than by knowing a renderer type.
//! - `reference_list_rows`, `reference_list_entry_row` and `list_drag_update`:
//!   a reference family as a list a person reorders by dragging its rows, such
//!   as a chain's pass list.
//! - `settings_commit`: the recorded command a committed settings value
//!   queues.
//! - The colour parameter shape (`parameters.<slot>.Color`, an array of
//!   numbers) drawn as a swatch.
//!
//! # Design
//!
//! Every settings edit is one recorded [`EditorCommand::SetAssetSettings`]
//! step carrying the whole patched document, so undo and the file's round trip
//! stay the same whichever row wrote it. A row that references another asset
//! writes that asset's **guid**, which is the form the file holds; each
//! listing row carries its own guid for exactly this. The first write also
//! creates a slot the document does not hold yet.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The indent one nesting level of a settings document adds, in points.
///
/// The whole row moves right, so a container's children read as belonging to
/// it, while the label column keeps the width every level uses.
pub const SETTINGS_ROW_INDENT: f32 = 12.0;

/// The width the remove button takes at the end of a list row, in points.
pub const LIST_REMOVE_WIDTH: f32 = 18.0;

// =============================================================================
// Types
// =============================================================================

/// What the settings rows share while the recursive walk draws them.
///
/// One value travels down the recursion instead of a long argument list: the
/// pickers' listing and engine store, the writes the rows queue, the session's
/// collapse state and remembered rectangles, the frame's overlay regions, and
/// whether the asset may be written at all. Every row below the walk takes it
/// whole, so a borrow added here reaches the whole family instead of being
/// threaded through every signature.
pub struct SettingsWalk<'a> {
    /// The listing the pickers read.
    pub assets: &'a [AssetListing],
    /// The store's own assets the pickers offer.
    pub engine_assets: &'a [EngineAsset],
    /// The writes the rows queue.
    pub pending: &'a mut PendingEditorCommands,
    /// The session the collapse state and remembered rectangles live in.
    pub session: &'a mut InspectorSession,
    /// The frame's overlay regions, so a picker's rectangle is the UI's.
    pub overlay_regions: &'a mut Vec<ShellRect>,
    /// How the asset's type declared its document fields draw: a range as a
    /// slider, a tooltip as the label's hover text. Empty for a type that
    /// declares none.
    pub decorators: &'a [AssetFieldDecorator],
    /// Whether the asset belongs to a module, so nothing writes it.
    pub read_only: bool,
}

/// What a list entry carries while it is dragged: the family it belongs to and
/// the index it held when the drag began.
#[derive(Clone, Debug)]
pub struct ListEntryDrag {
    /// The settings key of the list it belongs to - `passes` for a chain's pass
    /// list.
    pub family: String,
    /// The entry's index in that list when the drag began.
    pub index: usize,
}

// =============================================================================
// Free Functions
// =============================================================================

/// The hover text one settings row carries: the tooltip its type declared, or
/// the whole key, which is what says where a short leaf name belongs.
pub fn settings_row_tooltip<'a>(decorators: &'a [AssetFieldDecorator], key: &'a str) -> &'a str {
    match AssetFieldDecorator::find(decorators, key) {
        Some(FieldDecorator::Tooltip(text)) => text.as_str(),
        _ => key,
    }
}

/// The range one settings row draws as a slider, when its type declared one.
pub fn settings_row_range(decorators: &[AssetFieldDecorator], key: &str) -> Option<(f64, f64)> {
    match AssetFieldDecorator::find(decorators, key) {
        Some(FieldDecorator::Range { min, max }) => Some((*min, *max)),
        _ => None,
    }
}

/// The label a settings row carries: the leaf of its key.
///
/// The headings above the row already spell the path out, and a truncated
/// `parameters...` names nothing.
pub fn settings_row_label(key: &str) -> String {
    key.rsplit_once('.')
        .map_or(key, |(_, leaf)| leaf)
        .to_owned()
}

/// Draw a settings container's heading: its leaf name, muted, with the whole
/// key as its tooltip.
///
/// The caller draws the container's children one level in, under the heading.
pub fn settings_heading(ui: &mut egui::Ui, key: &str) {
    let name = settings_row_label(key);
    ui.label(egui::RichText::new(name).color(style::text_muted()))
        .on_hover_text(key);
}

/// Draw `add_contents` one settings level in.
///
/// The step is this module's own, because the panel's indent of 16 points is
/// more than a short settings key needs.
pub fn settings_level(ui: &mut egui::Ui, add_contents: &mut dyn FnMut(&mut egui::Ui)) {
    let previous = ui.spacing().indent;
    ui.spacing_mut().indent = SETTINGS_ROW_INDENT;
    ui.indent("pill.settings", |ui| add_contents(ui));
    ui.spacing_mut().indent = previous;
}

/// Draw the settings document's rows: objects and arrays nest their children,
/// and every scalar leaf edits.
///
/// With `walk.read_only` set, every row reads its value instead of editing it,
/// because the asset belongs to a module. `references` are the reference
/// fields the asset's type declares: each gets a picker row, whether or not
/// the document binds it yet, and the document key behind it is left out of
/// the walk so one field has one row. A root that is not an object draws as
/// one read-only row.
pub fn asset_settings_rows(
    ui: &mut egui::Ui,
    asset_path: &str,
    settings: &JsonValue,
    references: &[AssetReferenceField],
    walk: &mut SettingsWalk<'_>,
) {
    match settings {
        JsonValue::Object(entries) => {
            for (key, value) in entries {
                // A key a reference field owns is drawn by that field's row.
                // That includes a list family, whose entries arrive as
                // `passes[0]`, `passes[1]`, ... and whose array would
                // otherwise be walked as raw JSON, guids and all.
                let reference_owns_key = references.iter().any(|field| {
                    field.key == *key
                        || field.key.starts_with(&format!("{key}."))
                        || reference_family(field) == Some(key.as_str())
                });
                if reference_owns_key {
                    continue;
                }
                asset_settings_row(ui, asset_path, settings, value, key.clone(), walk);
            }
            if walk.read_only {
                return;
            }
            for field in references {
                // A list entry is drawn by its family's block, which owns the
                // row's index, its drag handle and its remove button.
                if reference_family(field).is_some() {
                    continue;
                }
                reference_row(ui, asset_path, settings, field, walk);
            }
            reference_list_rows(ui, asset_path, settings, references, walk);
        }
        other => {
            read_only_row(ui, "settings", &other.to_string());
        }
    }
}

/// Draw every list family a reference field set names - `passes[0]`,
/// `passes[1]`, ... - as one editable list: a draggable row per entry, and the
/// picker that appends the next one.
///
/// A chain's `passes` list is the only family with bracketed entries; a
/// texture family expands to dotted keys such as `textures.base_color` and
/// draws as plain rows. A drag reorders the list, and a drag or a remove
/// rewrites the whole array in one recorded step, so undo takes back that one
/// change and not the whole file.
pub fn reference_list_rows(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    references: &[AssetReferenceField],
    walk: &mut SettingsWalk<'_>,
) {
    let mut families: Vec<(String, String)> = references
        .iter()
        .filter_map(|field| {
            let family = reference_family(field)?;
            Some((family.to_owned(), field.target.clone()))
        })
        .collect();
    families.sort();
    families.dedup();
    for (family, target) in families {
        let entries = reference_entries(root, &family);
        let mut rows: Vec<egui::Rect> = Vec::new();
        // The block a drop reads its insertion point from: the heading, the
        // list's rows and the row that appends one. A release anywhere on it
        // counts, including on a picker button.
        let block = ui
            .scope(|ui| {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{family} ({})", entries.len()))
                        .color(style::text_muted()),
                )
                .on_hover_text("Drag a row to reorder the list");
                for (index, entry) in entries.iter().enumerate() {
                    let field = list_entry_field(references, &family, index, entry, &target);
                    rows.push(reference_list_entry_row(
                        ui, asset_path, root, &family, index, &field, walk,
                    ));
                }
                // Appending uses the picker of the entry one past the end. The
                // inserting patch is what creates that entry.
                let append = AssetReferenceField {
                    key: format!("{family}[{}]", entries.len()),
                    target: target.clone(),
                    guid: None,
                    nested: true,
                    label: None,
                };
                list_append_row(ui, asset_path, root, &append, walk);
            })
            .response
            .rect;
        list_drag_update(ui, &family, &entries, &rows, block, asset_path, root, walk);
    }
}

/// Returns the entries a reference family's list holds, cloned out of the
/// settings document.
///
/// The family's block draws them, and an entry's remove rewrites the list
/// without the one that was dropped.
pub fn reference_entries(root: &JsonValue, family: &str) -> Vec<JsonValue> {
    root.get(family)
        .and_then(|list| list.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Returns the reference field one list entry draws.
///
/// That is the field the snapshot expanded for the entry's key. When the
/// document holds an entry the snapshot does not name, the field is built from
/// the entry itself, so the row still reads and writes.
pub fn list_entry_field(
    references: &[AssetReferenceField],
    family: &str,
    index: usize,
    entry: &JsonValue,
    target: &str,
) -> AssetReferenceField {
    let key = format!("{family}[{index}]");
    if let Some(field) = references.iter().find(|field| field.key == key) {
        return field.clone();
    }
    AssetReferenceField {
        key,
        target: target.to_owned(),
        guid: reference_guid(entry),
        nested: !matches!(entry, JsonValue::String(_)),
        label: None,
    }
}

/// Draw one entry of a reference list: its index behind a drag handle, its
/// picker, and the button that drops it from the list.
///
/// The whole row starts a drag, both the handle and the picker beside it,
/// because a person grabs an entry by its name more often than by the handle.
/// The row therefore moves wherever it is held, while a press that does not
/// move still opens the picker. A drop turns the row's rectangle into an
/// index.
pub fn reference_list_entry_row(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    family: &str,
    index: usize,
    field: &AssetReferenceField,
    walk: &mut SettingsWalk<'_>,
) -> egui::Rect {
    let key = field.key.as_str();
    let drag = ListEntryDrag {
        family: family.to_owned(),
        index,
    };
    let text = reference_text(field);
    // The row takes exactly the width it is given, so the rows below it keep
    // the panel's width. A run that sizes itself by its children comes back
    // one item spacing wider each time, and the list drifts out of the panel.
    let row_width = ui.available_width();
    let picker_width = list_picker_width(ui, row_width);
    let mut chosen: Option<Option<String>> = None;
    let mut button_rect = None;
    let mut remove_clicked = false;
    let response = ui
        .allocate_ui_with_layout(
            egui::vec2(row_width, ROW_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                // The handle shows the row's place and is what tells a person
                // the row can be dragged. The drag payload it carries is the
                // value the block reads back on release.
                let handle = ui
                    .add_sized(
                        egui::vec2(label_column_width(ui), ROW_HEIGHT),
                        egui::Label::new(
                            icons::rich(icons::Icon::ArrowsOutCardinal, &index.to_string())
                                .color(style::text_muted()),
                        )
                        .truncate(),
                    )
                    .interact(egui::Sense::drag());
                handle.dnd_set_drag_payload(drag.clone());
                handle.on_hover_text("Drag to reorder the list");
                label_gap(ui);
                let button = ui.add_sized(
                    egui::vec2(picker_width, ROW_HEIGHT),
                    egui::Button::new(icons::rich(
                        assets_tiles::asset_type_icon(&field.target),
                        &text,
                    ))
                    .truncate(),
                );
                let button = button.interact(egui::Sense::click_and_drag());
                button.dnd_set_drag_payload(drag);
                if let Some(dropped) = dropped_asset_path(&button) {
                    chosen = walk
                        .assets
                        .iter()
                        .find(|row| row.key() == dropped)
                        .and_then(|row| row.guid.clone())
                        .map(Some);
                }
                if button.clicked() {
                    walk.session.settings_picker = Some(key.to_string());
                    walk.session.settings_search.clear();
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        slot = key,
                        target = field.target.as_str(),
                        "[editor] settings reference picker"
                    );
                }
                button_rect = Some(button.rect);
                if ui
                    .add_sized(
                        egui::vec2(LIST_REMOVE_WIDTH, ROW_HEIGHT),
                        egui::Button::new("x").small(),
                    )
                    .on_hover_text("Remove this entry")
                    .clicked()
                {
                    remove_clicked = true;
                }
            },
        )
        .response;
    if walk.session.settings_picker.as_deref() == Some(key) {
        if let Some(rect) = button_rect {
            if let Some(reference) = reference_picker(ui, field, rect, walk) {
                walk.session.settings_picker = None;
                chosen = Some(reference);
            }
        }
    }
    if remove_clicked {
        let mut remaining_entries = reference_entries(root, family);
        remaining_entries.remove(index);
        settings_commit(
            walk.pending,
            asset_path,
            root,
            family,
            JsonValue::Array(remaining_entries),
        );
        pill_core::info!(
            target: telemetry_target::EDITOR,
            slot = key,
            "[editor] settings list entry removed"
        );
    }
    if let Some(reference) = chosen {
        let value = match reference {
            Some(guid) if field.nested => serde_json::json!({ "guid": guid }),
            Some(guid) => JsonValue::String(guid),
            None => JsonValue::Null,
        };
        settings_commit(walk.pending, asset_path, root, key, value);
    }
    remember_list_row_rect(walk.session, asset_path, family, index, response.rect);
    response.rect
}

/// Draw the row that appends an entry: the same picker, for the index one past
/// the end, and its choice is what creates the entry.
///
/// The label column stays empty so the button lines up with the rows above it,
/// and the row takes a file dragged from the Assets tab like any other picker.
pub fn list_append_row(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    field: &AssetReferenceField,
    walk: &mut SettingsWalk<'_>,
) {
    let mut chosen: Option<Option<String>> = None;
    let mut button_rect = None;
    let row_width = ui.available_width();
    let picker_width = list_picker_width(ui, row_width);
    ui.allocate_ui_with_layout(
        egui::vec2(row_width, ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            // An empty label column keeps the button under the entries' pickers.
            label_cell(ui, "", None);
            label_gap(ui);
            let button = ui.add_sized(
                egui::vec2(picker_width, ROW_HEIGHT),
                egui::Button::new(icons::rich(icons::Icon::Plus, "Add")),
            );
            if let Some(dropped) = dropped_asset_path(&button) {
                chosen = walk
                    .assets
                    .iter()
                    .find(|row| row.key() == dropped)
                    .and_then(|row| row.guid.clone())
                    .map(Some);
            }
            if button.clicked() {
                walk.session.settings_picker = Some(field.key.clone());
                walk.session.settings_search.clear();
            }
            button_rect = Some(button.rect);
        },
    );
    if walk.session.settings_picker.as_deref() == Some(field.key.as_str()) {
        if let Some(rect) = button_rect {
            if let Some(reference) = reference_picker(ui, field, rect, walk) {
                walk.session.settings_picker = None;
                chosen = Some(reference);
            }
        }
    }
    if let Some(reference) = chosen {
        let value = match reference {
            Some(guid) => serde_json::json!({ "guid": guid }),
            None => JsonValue::Null,
        };
        settings_commit(walk.pending, asset_path, root, &field.key, value);
    }
}

/// Turn a held list row into a reorder: while a row of this family is over the
/// block, the insertion point is marked, and on release the list is rewritten
/// in one recorded step.
///
/// The drop is read from the drag payload and the pointer, not from a drop
/// target, so a release anywhere over the block counts: on a picker button, in
/// a gap, or on the row that appends. That is what makes dropping between two
/// rows work.
// The parameters are one list's drawn state (`family`, `entries`, `rows`,
// `block`), the settings document a drop commits into, and the walk. Grouping
// them would add a type that exists only to satisfy the lint.
#[allow(clippy::too_many_arguments)]
pub fn list_drag_update(
    ui: &mut egui::Ui,
    family: &str,
    entries: &[JsonValue],
    rows: &[egui::Rect],
    block: egui::Rect,
    asset_path: &str,
    root: &JsonValue,
    walk: &mut SettingsWalk<'_>,
) {
    if !block.is_positive() {
        return;
    }
    let Some(dragged) =
        egui::DragAndDrop::payload::<ListEntryDrag>(ui.ctx()).filter(|it| it.family == family)
    else {
        return;
    };
    let Some(pointer) = ui
        .ctx()
        .pointer_latest_pos()
        .filter(|pointer| block.contains(*pointer))
    else {
        return;
    };
    let insert_at_index = insertion_index(rows, pointer.y);
    // The held row gets an outline of its own, so the row that is moving stays
    // readable while the line shows where it goes.
    if let Some(source) = rows.get(dragged.index) {
        ui.painter().rect_stroke(
            *source,
            2.0,
            egui::Stroke::new(1.0, style::accent()),
            egui::StrokeKind::Inside,
        );
    }
    let line_y = match rows.get(insert_at_index) {
        Some(row) => row.top(),
        None => rows.last().map_or(block.bottom(), |row| row.bottom()),
    };
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(block.left(), line_y - 1.0),
            egui::pos2(block.right(), line_y + 1.0),
        ),
        0.0,
        style::accent(),
    );
    if !ui.input(|input| input.pointer.any_released()) {
        return;
    }
    let Some(released) = egui::DragAndDrop::take_payload::<ListEntryDrag>(ui.ctx()) else {
        return;
    };
    let reordered_entries = moved_entry(entries, released.index, insert_at_index);
    if reordered_entries == entries {
        return;
    }
    settings_commit(
        walk.pending,
        asset_path,
        root,
        family,
        JsonValue::Array(reordered_entries),
    );
    pill_core::info!(
        target: telemetry_target::EDITOR,
        family,
        from = released.index,
        to = insert_at_index,
        "[editor] settings list reordered"
    );
}

/// Returns the insertion point a pointer at `y` names: the first row whose
/// middle the pointer has not passed, or the end of the list.
pub fn insertion_index(rows: &[egui::Rect], y: f32) -> usize {
    rows.iter()
        .position(|row| y < row.center().y)
        .unwrap_or(rows.len())
}

/// Returns the entries with the one at `from` moved to the insertion point
/// `to`, which is an index in `0..=len`.
///
/// A drop says where the entry goes *between* rows, so a move downwards takes
/// the entry out first and ends one slot earlier than the raw index. An index
/// that does not move the entry leaves the list as it was.
pub fn moved_entry(entries: &[JsonValue], from: usize, to: usize) -> Vec<JsonValue> {
    let mut reordered_entries = entries.to_vec();
    if from >= reordered_entries.len() {
        return reordered_entries;
    }
    let entry = reordered_entries.remove(from);
    let insert_at_index = if to > from { to - 1 } else { to }.min(reordered_entries.len());
    reordered_entries.insert(insert_at_index, entry);
    reordered_entries
}

/// Returns the width a list row's picker takes: the row's width less its label
/// column, the gap after it, its remove button and the spacing around them,
/// and never less than [`CONTROL_MIN_WIDTH`].
///
/// The trailing spacing is egui's own. A row's extent includes the gap its
/// last control leaves, so a row sized without it comes back one gap wider and
/// pushes every row below it out of the panel.
pub fn list_picker_width(ui: &egui::Ui, row_width: f32) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    (row_width - label_column_width(ui) - ROW_LABEL_GAP - LIST_REMOVE_WIDTH - 2.0 * spacing)
        .max(CONTROL_MIN_WIDTH)
}

/// Remembers a list row's rectangle and logs it once per placement, keyed
/// `asset:family[index]`, so a suite can aim a drag at it.
///
/// The placement is recorded even when logging is off, because the shell's own
/// tests read it. The log line belongs to a scripted run, like every other row
/// rectangle's.
pub fn remember_list_row_rect(
    session: &mut InspectorSession,
    asset_path: &str,
    family: &str,
    index: usize,
    rect: egui::Rect,
) {
    let key = format!("{asset_path}:{family}[{index}]");
    let Some(rect) =
        inspector_fields::record_row_placement(&mut session.settings_row_rects, &key, rect)
    else {
        return;
    };
    if !session.log_rects {
        return;
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        row = key.as_str(),
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] settings list row rect"
    );
}

/// Draw one settings row: a leaf gets the widget its JSON kind asks for, and
/// its commit carries the whole patched document. A container becomes a
/// heading over its children one level in.
///
/// A colour parameter is the one exception: it is claimed before the container
/// shapes below it and drawn as a swatch. A texture slot never arrives here,
/// because the walk leaves every key a reference field owns to that field's
/// row.
///
/// What the rows below need travels in [`SettingsWalk`], so the recursion
/// takes one shared value.
pub fn asset_settings_row(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    value: &JsonValue,
    key: String,
    walk: &mut SettingsWalk<'_>,
) {
    // The colour parameter is the one array drawn as a control of its own, so
    // it is claimed before the container shapes below take the value.
    if !walk.read_only {
        if let Some(channels) = colour_parameter(&key, value) {
            colour_parameter_row(ui, asset_path, root, &key, channels, walk.pending);
            return;
        }
    }
    match value {
        JsonValue::Object(entries) => {
            settings_heading(ui, &key);
            settings_level(ui, &mut |ui| {
                for (child_key, child) in entries {
                    asset_settings_row(
                        ui,
                        asset_path,
                        root,
                        child,
                        format!("{key}.{child_key}"),
                        walk,
                    );
                }
            });
            return;
        }
        JsonValue::Array(elements) => {
            settings_heading(ui, &key);
            settings_level(ui, &mut |ui| {
                for (index, child) in elements.iter().enumerate() {
                    asset_settings_row(
                        ui,
                        asset_path,
                        root,
                        child,
                        format!("{key}[{index}]"),
                        walk,
                    );
                }
            });
            return;
        }
        _ => {}
    }
    // A leaf becomes the shared row, labelled with the last part of its key.
    let label = settings_row_label(&key);
    if walk.read_only {
        read_only_row(ui, &label, &value.to_string());
        return;
    }
    match value {
        JsonValue::Bool(current) => {
            let mut edited = *current;
            let mut changed = false;
            row(
                ui,
                &label,
                Some(settings_row_tooltip(walk.decorators, &key)),
                &mut |ui| {
                    changed = ui.checkbox(&mut edited, "").changed();
                },
            );
            if changed {
                settings_commit(
                    walk.pending,
                    asset_path,
                    root,
                    &key,
                    JsonValue::Bool(edited),
                );
            }
        }
        JsonValue::Number(current) => {
            let mut current_integer = current
                .as_i64()
                .or_else(|| current.as_u64().map(|value| value as i64));
            match current_integer.as_mut() {
                Some(edited) => {
                    let mut changed = false;
                    row(
                        ui,
                        &label,
                        Some(settings_row_tooltip(walk.decorators, &key)),
                        &mut |ui| {
                            changed =
                                stretched_drag_value(ui, egui::DragValue::new(edited)).changed();
                        },
                    );
                    if changed {
                        settings_commit(
                            walk.pending,
                            asset_path,
                            root,
                            &key,
                            JsonValue::Number((*edited).into()),
                        );
                    }
                }
                None => {
                    let mut edited = current.as_f64().unwrap_or_default();
                    let mut changed = false;
                    row(
                        ui,
                        &label,
                        Some(settings_row_tooltip(walk.decorators, &key)),
                        &mut |ui| {
                            changed = stretched_drag_value(
                                ui,
                                egui::DragValue::new(&mut edited)
                                    .speed(0.05)
                                    .max_decimals(9),
                            )
                            .changed();
                        },
                    );
                    if changed {
                        if let Some(number) = serde_json::Number::from_f64(edited) {
                            settings_commit(
                                walk.pending,
                                asset_path,
                                root,
                                &key,
                                JsonValue::Number(number),
                            );
                        }
                    }
                }
            }
        }
        JsonValue::String(current) => {
            let mut edited = current.clone();
            let mut should_commit = false;
            row(
                ui,
                &label,
                Some(settings_row_tooltip(walk.decorators, &key)),
                &mut |ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut edited).desired_width(ui.available_width()),
                    );
                    should_commit = response.lost_focus() && edited != *current;
                },
            );
            if should_commit {
                settings_commit(
                    walk.pending,
                    asset_path,
                    root,
                    &key,
                    JsonValue::String(edited),
                );
            }
        }
        other => {
            read_only_row(ui, &label, &other.to_string());
        }
    }
}

/// Returns the channel values of a colour parameter, when `key` names one.
///
/// A colour parameter is a `Color` leaf holding three or four numbers, such as
/// the material document's `parameters.<slot>.Color`. Every other key and
/// every other value shape answers `None`.
pub fn colour_parameter(key: &str, value: &JsonValue) -> Option<Vec<f32>> {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    if !leaf.eq_ignore_ascii_case("Color") {
        return None;
    }
    let JsonValue::Array(elements) = value else {
        return None;
    };
    if !(3..=4).contains(&elements.len()) {
        return None;
    }
    elements
        .iter()
        .map(|element| element.as_f64().map(|number| number as f32))
        .collect()
}

/// Draw one colour parameter: a swatch whose popup holds the picker.
///
/// A commit writes the whole array back, as one recorded settings step.
pub fn colour_parameter_row(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    key: &str,
    channels: Vec<f32>,
    pending: &mut PendingEditorCommands,
) {
    let mut edited = [1.0f32; 4];
    for (channel, value) in edited.iter_mut().zip(&channels) {
        *channel = *value;
    }
    let label = settings_row_label(key);
    let mut picked = None;
    row(ui, &label, Some(key), &mut |ui| {
        if ui
            .color_edit_button_rgba_unmultiplied(&mut edited)
            .changed()
        {
            picked = Some(edited);
        }
    });
    if let Some(edited) = picked {
        let channel_numbers: Option<Vec<JsonValue>> = channels
            .iter()
            .enumerate()
            .map(|(index, _)| {
                serde_json::Number::from_f64(edited[index] as f64).map(JsonValue::Number)
            })
            .collect();
        if let Some(channel_numbers) = channel_numbers {
            settings_commit(
                pending,
                asset_path,
                root,
                key,
                JsonValue::Array(channel_numbers),
            );
        }
    }
}

/// Returns the path an asset from the Assets tab releases onto `response`,
/// when one is being dragged.
///
/// The type is checked first because egui's release helper *takes* the payload
/// it finds. Without the check, a picker the pointer happens to be over would
/// also consume a drag of another kind, such as a list row moving to a new
/// index.
pub fn dropped_asset_path(response: &egui::Response) -> Option<String> {
    if !egui::DragAndDrop::has_payload_of_type::<String>(&response.ctx) {
        return None;
    }
    response
        .dnd_release_payload::<String>()
        .map(|path| (*path).clone())
}

/// Returns the guid a settings reference holds, in either spelling.
///
/// That is the value itself when the document writes a bare guid, or the
/// `guid` field of the object form.
pub fn reference_guid(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(guid) => Some(guid.clone()),
        JsonValue::Object(entries) => entries
            .get("guid")
            .and_then(|guid| guid.as_str())
            .map(str::to_owned),
        _ => None,
    }
}

/// Returns the listing rows a reference's picker offers: the assets whose type
/// is the one the registry named for the field.
///
/// Only the listing's files are returned. [`reference_picker`] adds the store's
/// own assets beside them.
pub fn reference_candidates<'a>(assets: &'a [AssetListing], target: &str) -> Vec<&'a AssetListing> {
    inspector::picker_candidates(assets, &[], Some(target)).0
}

/// Returns the list family a reference field's key belongs to.
///
/// `passes[3]` is an entry of `passes`. A key without brackets is a plain
/// field, not a list entry, so it answers `None`.
pub fn reference_family(field: &AssetReferenceField) -> Option<&str> {
    field.key.split_once('[').map(|(family, _)| family)
}

/// Returns the text a reference row shows for its field.
///
/// That is the label the executable resolved: a listing path, or the name the
/// store loaded the asset under when it has no file. When neither knows the
/// guid, only the guid's first eight characters are shown.
pub fn reference_text(field: &AssetReferenceField) -> String {
    match (field.label.as_deref(), field.guid.as_deref()) {
        (Some(label), _) => label.to_owned(),
        (None, Some(guid)) => format!("{}...", guid.chars().take(8).collect::<String>()),
        (None, None) => "(none)".to_string(),
    }
}

/// Returns the listing path of the asset a reference names, when the listing
/// holds it: the row a picker marks as the current choice.
pub fn reference_path<'a>(
    field: &AssetReferenceField,
    assets: &'a [AssetListing],
) -> Option<&'a str> {
    let guid = field.guid.as_deref()?;
    assets
        .iter()
        .find(|row| row.guid.as_deref() == Some(guid))
        .map(|row| row.path.as_str())
}

/// Draw one reference field: the referenced asset's path (or its guid, when
/// the listing no longer holds it), a clear button, a drop target and a picker
/// over the rows of the type the field accepts.
///
/// Every choice writes the chosen asset's guid in the shape the document
/// already uses: the object form for a nested reference, the bare string for a
/// texture slot. A field the document does not hold yet writes the object
/// form. The same row serves a material's shader and its declared slots,
/// because what differs between them is data. A list entry draws through
/// [`reference_list_entry_row`], which keeps this row's picker.
pub fn reference_row(
    ui: &mut egui::Ui,
    asset_path: &str,
    root: &JsonValue,
    field: &AssetReferenceField,
    walk: &mut SettingsWalk<'_>,
) {
    let key = field.key.as_str();
    let label = settings_row_label(key);
    let text = reference_text(field);
    let mut chosen: Option<Option<String>> = None;
    let mut button_rect = None;
    row(ui, &label, Some(key), &mut |ui| {
        // The picker takes what the row has left beside its clear button, so
        // the two stay on one line however narrow the panel is. A wrapped run
        // would drop the picker, and then the clear button, onto rows of their
        // own.
        let clear_width = inspector_fields::ASSET_ROW_CLEAR_WIDTH;
        let spacing = ui.spacing().item_spacing.x;
        let button_width = (ui.available_width() - clear_width - spacing).max(CONTROL_MIN_WIDTH);
        let response = ui.add_sized(
            egui::vec2(button_width, ROW_HEIGHT),
            egui::Button::new(icons::rich(icons::Icon::Code, &text)).truncate(),
        );
        if let Some(dropped) = dropped_asset_path(&response) {
            chosen = walk
                .assets
                .iter()
                .find(|row| row.key() == dropped)
                .and_then(|row| row.guid.clone())
                .map(Some);
        }
        if response.clicked() {
            walk.session.settings_picker = Some(key.to_string());
            walk.session.settings_search.clear();
            pill_core::info!(
                target: telemetry_target::EDITOR,
                slot = key,
                target = field.target.as_str(),
                "[editor] settings reference picker"
            );
        }
        button_rect = Some(response.rect);
        let clear = ui
            .add_enabled_ui(field.guid.is_some(), |ui| {
                ui.add_sized(
                    egui::vec2(clear_width, ROW_HEIGHT),
                    egui::Button::new("x").small(),
                )
            })
            .inner
            .on_hover_text("Clear the reference");
        if clear.clicked() {
            chosen = Some(None);
        }
    });

    if walk.session.settings_picker.as_deref() == Some(key) {
        if let Some(rect) = button_rect {
            if let Some(reference) = reference_picker(ui, field, rect, walk) {
                walk.session.settings_picker = None;
                chosen = Some(reference);
            }
        }
    }

    if let Some(reference) = chosen {
        walk.session.settings_picker = None;
        let value = match reference {
            Some(guid) if field.nested => serde_json::json!({ "guid": guid }),
            Some(guid) => JsonValue::String(guid),
            None => JsonValue::Null,
        };
        settings_commit(walk.pending, asset_path, root, key, value);
    }
}

/// Draw the picker a reference opens under its row: every asset of the field's
/// target type that the listing holds, then the store's own entries, each with
/// the focus affordance that shows a candidate in the Assets panel.
///
/// Returns the choice made this frame: `Some(None)` clears the reference, and
/// `Some(Some(guid))` points it somewhere. The session's picker stays open
/// until the caller takes the choice. A list entry's picker offers no `None`,
/// because an entry leaves a list through its row's remove button, not by
/// being emptied in place.
pub fn reference_picker(
    ui: &mut egui::Ui,
    field: &AssetReferenceField,
    anchor: egui::Rect,
    walk: &mut SettingsWalk<'_>,
) -> Option<Option<String>> {
    let key = field.key.as_str();
    let current_path = reference_path(field, walk.assets);
    let clearable = reference_family(field).is_none();
    let mut chosen: Option<Option<String>> = None;
    let picker_id = egui::Id::new(("pill_editor_shell.reference_picker", key.to_string()));
    let popup = floating_popup(
        ui,
        picker_id,
        anchor,
        anchor.width().max(180.0),
        walk.overlay_regions,
        &mut |ui| {
            egui::ScrollArea::vertical()
                .max_height(inspector_fields::ASSET_PICKER_HEIGHT)
                .show(ui, |ui| {
                    if clearable && ui.selectable_label(field.guid.is_none(), "None").clicked() {
                        chosen = Some(None);
                    }
                    let (files, built) = inspector::picker_candidates(
                        walk.assets,
                        walk.engine_assets,
                        Some(&field.target),
                    );
                    for candidate in files {
                        let current = current_path == Some(candidate.path.as_str());
                        ui.horizontal(|ui| {
                            // A module's own resource wears the engine badge,
                            // so a person sees where it comes from. Only a row
                            // that carries a guid can be picked, and a row
                            // that cannot explains itself on hover instead of
                            // taking the click.
                            let from_module = !candidate.root.is_empty();
                            if from_module {
                                engine_badge(ui);
                            }
                            let pickable = candidate.guid.is_some();
                            let row = ui.add_enabled_ui(pickable, |ui| {
                                ui.selectable_label(current, &candidate.path).clicked()
                            });
                            if row.inner {
                                chosen = candidate.guid.clone().map(Some);
                            }
                            if !pickable {
                                row.response.on_hover_text(
                                    "Shipped with the engine, so its settings are read-only",
                                );
                            }
                            // The focus button: a single click shows the
                            // candidate in the Assets panel, which moves the
                            // selection and opens its folder, while the picker
                            // stays open, so browsing does not throw the edit
                            // away. A double click also makes the asset the
                            // Inspector's subject and closes the picker.
                            let focus = ui
                                .small_button("o")
                                .on_hover_text("Show this asset in the Assets panel");
                            if focus.double_clicked() {
                                walk.pending.focus_asset(candidate.key());
                                walk.session.settings_picker = None;
                                pill_core::info!(
                                    target: telemetry_target::EDITOR,
                                    path = candidate.path.as_str(),
                                    "[editor] asset focused"
                                );
                            } else if focus.clicked() {
                                walk.pending.focus_asset(candidate.key());
                            }
                        });
                    }
                    // The store's own assets have no file, so there is no
                    // folder to reveal. Choosing one writes its guid, or the
                    // name it was added under, which the field write resolves
                    // through the store.
                    for candidate in built {
                        let current = field.guid.as_deref() == candidate.guid.as_deref();
                        ui.horizontal(|ui| {
                            engine_badge(ui);
                            if ui.selectable_label(current, &candidate.name).clicked() {
                                chosen = Some(Some(
                                    candidate
                                        .guid
                                        .clone()
                                        .unwrap_or_else(|| candidate.name.clone()),
                                ));
                            }
                        });
                    }
                });
        },
    );
    // A press outside the picker and its button closes it, whichever button it
    // was: a picker that only closes by picking fights the person who changed
    // their mind. The press that opened it landed on the button, so it never
    // closes the picker it just opened.
    if popup.pressed_outside {
        walk.session.settings_picker = None;
    }
    chosen
}

/// Returns the guid of the listing row a path names.
///
/// The path is a listing row's key, so a module row is found the same way a
/// project row is. A path the listing does not hold, or a row without a guid,
/// answers `None`.
pub fn texture_guid_for(assets: &[AssetListing], path: &str) -> Option<String> {
    assets
        .iter()
        .find(|row| row.key() == path)
        .and_then(|row| row.guid.clone())
}

/// Queue the recorded edit that puts `value` at `key` in the settings, and
/// carry the whole patched document in the command.
///
/// A key that does not parse queues nothing and stays silent. A document the
/// patched path cannot reach queues nothing and logs a warning naming the
/// segment it could not reach.
pub fn settings_commit(
    pending: &mut PendingEditorCommands,
    asset_path: &str,
    root: &JsonValue,
    key: &str,
    value: JsonValue,
) {
    let Ok(path) = inspector::parse_settings_key_path(key) else {
        return;
    };
    let mut document = root.clone();
    // The inserting rule: a slot or a reference the type declares but the file
    // does not hold yet is created by its first write, which is exactly the
    // unbound-texture case.
    if let Err(message) = inspector::patch_settings_inserting(&mut document, &path, value) {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            key,
            error = message.as_str(),
            "[editor] asset settings edit refused"
        );
        return;
    }
    let (command, label) = inspector::settings_edit(asset_path, document, key);
    pending.push_recorded(
        vec![command],
        label,
        Some(inspector::settings_coalesce_key(asset_path, key)),
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn row(path: &str, kind: &str, guid: Option<&str>) -> AssetListing {
        AssetListing {
            root: String::new(),
            path: path.to_string(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone: true,
            has_meta: guid.is_some(),
            guid: guid.map(str::to_owned),
        }
    }

    /// A reference reads both spellings a document may use: the object form
    /// this build writes, and a bare guid.
    #[test]
    fn a_reference_reads_both_spellings() {
        assert_eq!(
            reference_guid(&serde_json::json!({ "guid": "abc" })).as_deref(),
            Some("abc")
        );
        assert_eq!(
            reference_guid(&serde_json::json!("abc")).as_deref(),
            Some("abc")
        );
        assert_eq!(reference_guid(&serde_json::Value::Null), None);
    }

    /// A picker offers the rows of the type its field accepts, and nothing
    /// else. The same filter serves a shader reference, a texture slot and a
    /// pass entry.
    #[test]
    fn a_reference_lists_the_type_its_field_accepts() {
        let assets = vec![
            row(
                "materials/a.material",
                "pill_rendering::Material",
                Some("m"),
            ),
            row("shaders/pbr.hlsl", "pill_rendering::Shader", Some("s")),
            row("textures/rock.png", "pill_rendering::Texture", Some("t")),
            row(
                "passes/opaque.render_pass",
                "pill_rendering::RenderPass",
                Some("p"),
            ),
        ];
        let shaders = |target: &str| -> Vec<String> {
            reference_candidates(&assets, target)
                .iter()
                .map(|row| row.path.clone())
                .collect()
        };
        assert_eq!(shaders("pill_rendering::Shader"), vec!["shaders/pbr.hlsl"]);
        assert_eq!(shaders("Texture"), vec!["textures/rock.png"]);
        assert_eq!(shaders("RenderPass"), vec!["passes/opaque.render_pass"]);
    }

    /// A drop between rows moves the entry there. A move downwards ends one
    /// slot earlier than the raw insertion index, a move upwards ends on it,
    /// and dropping an entry on either half of itself changes nothing.
    #[test]
    fn a_drop_moves_the_entry_to_the_insertion_point() {
        let entries = vec![
            serde_json::json!("a"),
            serde_json::json!("b"),
            serde_json::json!("c"),
        ];
        let order = |from: usize, to: usize| -> Vec<String> {
            moved_entry(&entries, from, to)
                .into_iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_owned())
                .collect()
        };
        assert_eq!(order(0, 0), vec!["a", "b", "c"]);
        assert_eq!(order(0, 1), vec!["a", "b", "c"]);
        assert_eq!(order(0, 2), vec!["b", "a", "c"]);
        assert_eq!(order(0, 3), vec!["b", "c", "a"]);
        assert_eq!(order(2, 0), vec!["c", "a", "b"]);
        assert_eq!(order(1, 1), vec!["a", "b", "c"]);
        assert_eq!(order(1, 2), vec!["a", "b", "c"]);
        assert_eq!(order(1, 3), vec!["a", "c", "b"]);
        // A drag that began on an entry the list no longer holds leaves it.
        assert_eq!(order(7, 0), vec!["a", "b", "c"]);
    }

    /// The insertion point is the first row whose middle the pointer has not
    /// passed. Above every row's middle is the list's head, and below every
    /// one is its end.
    #[test]
    fn the_insertion_point_follows_the_pointer() {
        let rows: Vec<egui::Rect> = (0..3)
            .map(|index| {
                egui::Rect::from_min_size(
                    egui::pos2(0.0, index as f32 * 20.0),
                    egui::vec2(80.0, 18.0),
                )
            })
            .collect();
        assert_eq!(insertion_index(&rows, 0.0), 0);
        assert_eq!(insertion_index(&rows, 8.0), 0);
        assert_eq!(insertion_index(&rows, 9.0), 1);
        assert_eq!(insertion_index(&rows, 29.0), 2);
        assert_eq!(insertion_index(&rows, 49.0), 3);
        assert_eq!(insertion_index(&[], 10.0), 0);
    }

    /// A settings row is named by its leaf, because the headings above it
    /// spell the path out and a truncated `parameters...` names nothing.
    #[test]
    fn a_settings_row_is_named_by_its_leaf() {
        assert_eq!(settings_row_label("shader"), "shader");
        assert_eq!(settings_row_label("textures.base_color"), "base_color");
        assert_eq!(settings_row_label("parameters.pbr_base.Color"), "Color");
        // An array element has no leaf of its own, so its own name stands.
        assert_eq!(settings_row_label("passes[2]"), "passes[2]");
    }

    /// A list row draws the field the snapshot expanded for its key. An entry
    /// the snapshot does not name still reads from the document itself.
    #[test]
    fn a_list_entry_reads_its_own_field() {
        let references = vec![AssetReferenceField {
            key: "passes[0]".to_string(),
            target: "pill_rendering::RenderPass".to_string(),
            guid: Some("first".to_string()),
            nested: true,
            label: None,
        }];
        let expanded = list_entry_field(&references, "passes", 0, &JsonValue::Null, "RenderPass");
        assert_eq!(expanded.key, "passes[0]");
        assert_eq!(expanded.guid.as_deref(), Some("first"));
        let parsed = list_entry_field(
            &references,
            "passes",
            1,
            &serde_json::json!({ "guid": "second" }),
            "RenderPass",
        );
        assert_eq!(parsed.key, "passes[1]");
        assert_eq!(parsed.guid.as_deref(), Some("second"));
        assert!(parsed.nested);
    }
}
