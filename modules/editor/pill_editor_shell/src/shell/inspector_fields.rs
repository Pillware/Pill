//! The Inspector's field rows: one row of a component section, with the
//! decorators and separators that row draws.
//!
//! # Responsibilities
//!
//! - `inspector_field_row`: draws one row the Inspector's row model produced -
//!   a `[Header]` or `[Space]` separator, one field with the widget its
//!   `[Tooltip]`, `[Range]` or `[Min]` asked for, a vector row, a colour row,
//!   or a value shown read-only.
//! - `inspector_asset_row` and `inspector_entity_row`: a reference row. It
//!   names the target it points at and offers a picker, a clear button and a
//!   drop target for an asset or entity dragged out of another tab.
//!   `AssetRowInput` and `EntityRowInput` carry what those rows draw from.
//! - `queue_field_edit` and `queue_element_edit`: the one recorded step a row's
//!   edit becomes, for every selected entity that carries the component.
//! - `inspector_row` and `is_scalar_field`: the shared label/control row, and
//!   whether a value has a widget of its own.
//! - `field_widget_name` and the row's `RowLogged` memory: the once-per-row log
//!   line the suites assert on.
//! - `remember_asset_row_text`, `remember_asset_row_rect` and their entity
//!   twins: a row's current text (logged when it changes) and its rectangle
//!   (logged once per placement), which the editor suite waits on.
//! - `record_row_placement`: the once-per-placement rule those rectangles share
//!   with the asset-settings rows.
//!
//! The two globs below spell the whole path, so a patch crate resolves them
//! too.

// Current crate
use pill_editor_core::hooks::FieldEditPhase;
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// Width of each small button beside a reference row's button (clear, pick).
pub const ASSET_ROW_CLEAR_WIDTH: f32 = 24.0;

/// The size every small button beside a reference row's button is drawn at,
/// so the room the row keeps for it is exactly the room it takes.
pub fn side_button_size() -> egui::Vec2 {
    egui::vec2(ASSET_ROW_CLEAR_WIDTH, ROW_HEIGHT)
}

/// The width a reference row's main button takes: what is left after
/// `side_buttons` small buttons and the gap before each, never less than
/// [`CONTROL_MIN_WIDTH`].
pub fn reference_button_width(ui: &egui::Ui, side_buttons: usize) -> f32 {
    let spacing = ui.spacing().item_spacing.x;
    (ui.available_width() - side_buttons as f32 * (ASSET_ROW_CLEAR_WIDTH + spacing))
        .max(CONTROL_MIN_WIDTH)
}

/// Height an asset or entity picker's list scrolls within.
pub const ASSET_PICKER_HEIGHT: f32 = 180.0;

// =============================================================================
// Types
// =============================================================================

/// What one asset reference row draws from: the field, its component and
/// the selection it writes to, and the project's assets the picker offers.
pub struct AssetRowInput<'a> {
    /// The primary entity the row describes.
    pub entity: Entity,
    /// The component section the row belongs to.
    pub component: &'a ComponentView,
    /// The field's position in the component's field list.
    pub index: usize,
    /// The field's registered name.
    pub field: &'a str,
    /// The registered asset type the field references, when recorded.
    pub asset_type: Option<&'a str>,
    /// The field's `[Tooltip]`, if it declared one.
    pub tooltip: Option<&'a str>,
    /// Every entity the edit reaches (the primary first).
    pub targets: &'a [Entity],
    /// The selected entities' details, for the mixed-value marker.
    pub details: &'a [EntityDetail],
    /// The project's assets, as the Assets tab lists them.
    pub assets: &'a [AssetListing],
    /// The store's own assets that no file backs: what the picker offers beside
    /// the listing's rows, badged as the engine's.
    pub engine_assets: &'a [EngineAsset],
}

/// What one entity reference row draws from: the field, its component and the
/// selection it writes to, and the scene's entities the picker offers.
pub struct EntityRowInput<'a> {
    /// The primary entity the row describes.
    pub entity: Entity,
    /// The component section the row belongs to.
    pub component: &'a ComponentView,
    /// The field's position in the component's field list.
    pub index: usize,
    /// The field's registered name.
    pub field: &'a str,
    /// The field's `[Tooltip]`, if it declared one.
    pub tooltip: Option<&'a str>,
    /// Every entity the edit reaches (the primary first).
    pub targets: &'a [Entity],
    /// The selected entities' details, for the mixed-value marker.
    pub details: &'a [EntityDetail],
    /// The scene's entities, as the Entities tab lists them.
    pub scene: &'a [EntityListEntry],
}

// =============================================================================
// Free Functions
// =============================================================================

/// Logs one drawn Inspector row's line, once per row.
///
/// The row's own memory is the gate: its place in the session's cached model
/// already identifies it, so nothing has to be keyed or allocated. It is a
/// macro rather than a function because the log fields are typed, and a
/// `pixels` number and a `text` string must reach the log as themselves.
macro_rules! log_row_once {
    ($logged:expr, $flag:expr, $type_name:expr, $kind:literal, $($field:tt)*) => {
        if !$logged.has($flag) {
            $logged.mark($flag);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                component = $type_name,
                $($field)*
                "[editor] inspector row kind={}",
                $kind
            );
        }
    };
}

// The parameters are the borrows one row draws from. A struct grouping them
// would have to be built again for every row of every component.
#[allow(clippy::too_many_arguments)]
/// Draws one row of a component section.
///
/// The row comes from the Inspector's row model: a separator line or gap a
/// field's decorators declared, one field with its widget, a vector row
/// (side-by-side fields with axis labels in the gizmo's colours), or a value
/// shown read-only.
pub fn inspector_field_row(
    ui: &mut egui::Ui,
    entity: Entity,
    component: &ComponentView,
    row: &inspector::FieldRow,
    logged: &mut inspector::RowLogged,
    targets: &[Entity],
    details: &[EntityDetail],
    assets: &[AssetListing],
    engine_assets: &[EngineAsset],
    scene: &[EntityListEntry],
    pending: &mut PendingEditorCommands,
    session: &mut InspectorSession,
    overlay_regions: &mut Vec<ShellRect>,
) {
    let type_name = component.type_name.as_str();
    match row {
        inspector::FieldRow::Header { text } => {
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "header",
                text = text.as_str(),
            );
            ui.add_space(6.0);
            ui.label(egui::RichText::new(text).strong());
            ui.add_space(2.0);
        }
        inspector::FieldRow::Space { pixels } => {
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "space",
                pixels = *pixels,
            );
            ui.add_space(*pixels);
        }
        inspector::FieldRow::Single {
            index,
            field,
            widget,
            tooltip,
        } => {
            let Some(value) = component.values.get(*index) else {
                return;
            };
            let tooltip = tooltip.as_deref();
            if row_is_mixed(details, type_name, *index, value) {
                log_row_once!(
                    logged,
                    inspector::RowLogged::MIXED,
                    type_name,
                    "mixed",
                    field = field.as_str(),
                    entities = targets.len(),
                );
            }
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "field",
                field = field.as_str(),
                widget = field_widget_name(widget),
                tooltip = tooltip.unwrap_or_default(),
            );
            match value {
                FieldValue::Text(current) => {
                    let mut edited = current.clone();
                    let lines = match widget {
                        inspector::FieldWidget::MultilineText { lines } => Some(*lines),
                        _ => None,
                    };
                    inspector_row(
                        ui,
                        &row_label(field, details, type_name, *index, value),
                        tooltip,
                        &mut |ui| {
                            let text_edit = match lines {
                                Some(lines) => egui::TextEdit::multiline(&mut edited)
                                    .desired_rows(lines)
                                    .desired_width(f32::INFINITY),
                                None => egui::TextEdit::singleline(&mut edited)
                                    .desired_width(f32::INFINITY),
                            };
                            let response = ui.add(text_edit);
                            if response.lost_focus() && edited != *current {
                                let committed = FieldValue::Text(edited.clone());
                                queue_field_edit(
                                    pending,
                                    targets,
                                    entity,
                                    type_name,
                                    field,
                                    committed,
                                    FieldEditPhase::Committed,
                                );
                            }
                        },
                    );
                }
                other => {
                    inspector_row(
                        ui,
                        &row_label(field, details, type_name, *index, other),
                        tooltip,
                        &mut |ui| {
                            // A field with an editor shows only the editor; its
                            // answer of `None` means "not edited this frame",
                            // not "no editor". Only a kind without one is shown
                            // as text.
                            if !has_field_editor(other) {
                                ui.label(format_field_value(other));
                                return;
                            }
                            // The control takes exactly the width left and
                            // crops a long value inside it, so a narrow panel
                            // squashes the field instead of the row growing.
                            if let Some(edited) = fixed_width_field(ui, other, Some(widget)) {
                                queue_field_edit(
                                    pending,
                                    targets,
                                    entity,
                                    type_name,
                                    field,
                                    edited.value,
                                    edited.phase,
                                );
                            }
                        },
                    );
                }
            }
        }
        inspector::FieldRow::Vector {
            index,
            field,
            elements,
            tooltip,
            ..
        } => {
            let Some(FieldValue::Array(values)) = component.values.get(*index) else {
                return;
            };
            if row_is_mixed(
                details,
                type_name,
                *index,
                &FieldValue::Array(values.clone()),
            ) {
                log_row_once!(
                    logged,
                    inspector::RowLogged::MIXED,
                    type_name,
                    "mixed",
                    field = field.as_str(),
                    entities = targets.len(),
                );
            }
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "vector",
                field = field.as_str(),
                tooltip = tooltip.as_deref().unwrap_or_default(),
            );
            vector_row(
                ui,
                field,
                tooltip.as_deref(),
                (*elements).min(values.len()),
                &mut |ui, element_index| {
                    if let Some(edited) = fixed_width_field(ui, &values[element_index], None) {
                        queue_element_edit(
                            pending,
                            targets,
                            entity,
                            type_name,
                            field,
                            element_index,
                            edited.value,
                            edited.phase,
                        );
                    }
                },
            );
        }
        inspector::FieldRow::Color {
            leaves,
            field,
            tooltip,
        } => {
            let values = leaf_values(component, leaves);
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "color",
                field = field.as_str(),
                channels = leaves.len(),
                tooltip = tooltip.as_deref().unwrap_or_default(),
            );
            inspector_row(ui, field, tooltip.as_deref(), &mut |ui| {
                let Some(mut channels) = color_channels(&values) else {
                    // The picker only reads floats. A colour held as integers is
                    // drawn one channel at a time, so it can still be edited.
                    for (axis_index, value) in values.iter().enumerate() {
                        ui.label(
                            egui::RichText::new(inspector::axis_label(axis_index))
                                .color(inspector::axis_color(axis_index))
                                .small(),
                        );
                        if let Some(edited) = scalar_edit(ui, value) {
                            let leaf = &leaves[axis_index];
                            match leaf.element {
                                Some(element) => queue_element_edit(
                                    pending,
                                    targets,
                                    entity,
                                    type_name,
                                    &leaf.field,
                                    element,
                                    edited.value,
                                    edited.phase,
                                ),
                                None => queue_field_edit(
                                    pending,
                                    targets,
                                    entity,
                                    type_name,
                                    &leaf.field,
                                    edited.value,
                                    edited.phase,
                                ),
                            }
                        }
                    }
                    return;
                };
                // The picker edits linear channels, and that is what the field
                // holds, so no colour conversion happens here.
                let response = ui.color_edit_button_rgba_unmultiplied(&mut channels);
                if response.changed() {
                    let edited: Vec<FieldValue> = leaves
                        .iter()
                        .zip(channels)
                        .map(|(leaf, channel)| match component.values.get(leaf.index) {
                            Some(FieldValue::F64(_)) => FieldValue::F64(channel as f64),
                            _ => FieldValue::F32(channel),
                        })
                        .collect();
                    let (commands, label, coalesce_key) =
                        inspector::selection_leaf_edit(targets, entity, type_name, leaves, &edited);
                    pending.push_recorded(commands, label, Some(coalesce_key));
                }
            });
        }
        inspector::FieldRow::VectorGroup {
            leaves,
            widget,
            field,
            tooltip,
        } => {
            // A grouped vector and a `[f32; 3]` field are two rows, and each
            // cached row carries its own logged flags, so both log a line.
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "vector",
                field = field.as_str(),
                axes = leaves.len(),
                tooltip = tooltip.as_deref().unwrap_or_default(),
            );
            // This draws the same row as a `[f32; 3]` field, but the three
            // values come from three scalar fields of their own.
            vector_row(
                ui,
                field,
                tooltip.as_deref(),
                leaves.len(),
                &mut |ui, axis_index| {
                    let leaf = &leaves[axis_index];
                    let Some(value) = component.values.get(leaf.index) else {
                        return;
                    };
                    if let Some(edited) = fixed_width_field(ui, value, Some(widget)) {
                        queue_field_edit(
                            pending,
                            targets,
                            entity,
                            type_name,
                            &leaf.field,
                            edited.value,
                            edited.phase,
                        );
                    }
                },
            );
        }
        inspector::FieldRow::Asset {
            index,
            field,
            asset_type,
            tooltip,
        } => {
            inspector_asset_row(
                ui,
                AssetRowInput {
                    entity,
                    component,
                    index: *index,
                    field,
                    asset_type: asset_type.as_deref(),
                    tooltip: tooltip.as_deref(),
                    targets,
                    details,
                    assets,
                    engine_assets,
                },
                pending,
                session,
                overlay_regions,
            );
        }
        inspector::FieldRow::Entity {
            index,
            field,
            tooltip,
        } => {
            inspector_entity_row(
                ui,
                EntityRowInput {
                    entity,
                    component,
                    index: *index,
                    field,
                    tooltip: tooltip.as_deref(),
                    targets,
                    details,
                    scene,
                },
                pending,
                session,
                overlay_regions,
            );
        }
        inspector::FieldRow::ReadOnly {
            index,
            field,
            tooltip,
        } => {
            let Some(value) = component.values.get(*index) else {
                return;
            };
            log_row_once!(
                logged,
                inspector::RowLogged::ROW,
                type_name,
                "read-only",
                field = field.as_str(),
                tooltip = tooltip.as_deref().unwrap_or_default(),
            );
            inspector_row(ui, field, tooltip.as_deref(), &mut |ui| {
                ui.label(egui::RichText::new(format_field_value(value)).weak());
            });
        }
    }
}

/// Whether a file dragged out of the Assets tab may be dropped on an asset row.
///
/// `dragged` is the listing key the Assets tab drags. It fits when it is one
/// of the files the row's own picker offers, which are the listed files of the
/// field's asset type. Without this check a mesh dropped one row too low, on a
/// material slot, would be queued and only refused by the engine.
pub fn asset_drop_fits(dragged: &str, row_input: &AssetRowInput<'_>) -> bool {
    let (files, _) = inspector::picker_candidates(
        row_input.assets,
        row_input.engine_assets,
        row_input.asset_type,
    );
    files.iter().any(|candidate| candidate.key() == dragged)
}

/// Draws one asset reference row.
///
/// The row is a button naming the referenced asset, which opens a picker of
/// every asset of the field's type, a clear button, and a drop target for a
/// file of the field's asset type dragged out of the Assets tab
/// ([`asset_drop_fits`]).
///
/// Every choice becomes one recorded `SetFieldAsset` step for the whole
/// selection. An asset that is not loaded yet is loaded when the step is
/// applied.
pub fn inspector_asset_row(
    ui: &mut egui::Ui,
    row_input: AssetRowInput<'_>,
    pending: &mut PendingEditorCommands,
    session: &mut InspectorSession,
    overlay_regions: &mut Vec<ShellRect>,
) {
    let type_name = row_input.component.type_name.as_str();
    let label = row_input
        .component
        .asset_labels
        .get(row_input.index)
        .cloned()
        .flatten()
        .unwrap_or(AssetReferenceLabel::Missing);
    let text = label.text().to_owned();
    // Step 1: log the asset this row draws, once per row and text.
    //
    // A suite can then wait for a pick to show up, and for the previous asset
    // to come back after an undo. The entity is part of the key: two entities
    // that hold the same asset are two rows.
    let key = format!("{}.{type_name}.{}", row_input.entity, row_input.field);
    if remember_asset_row_text(session, &key, &text) {
        pill_core::info!(
            target: telemetry_target::EDITOR,
            entity = %row_input.entity,
            component = type_name,
            field = row_input.field,
            asset_type = row_input.asset_type.unwrap_or_default(),
            asset = text.as_str(),
            "[editor] inspector row kind=asset"
        );
    }

    // Step 2: draw the row and collect the reference it picks.
    //
    // `Some(None)` clears the reference and `Some(Some(path))` points it at an
    // asset. The choice is queued once, after the row has drawn.
    let mut chosen: Option<Option<String>> = None;
    let mut button_rects = None;
    let row_label_text = match row_input.component.values.get(row_input.index) {
        Some(value) => row_label(
            row_input.field,
            row_input.details,
            type_name,
            row_input.index,
            value,
        ),
        None => row_input.field.to_owned(),
    };
    inspector_row(ui, &row_label_text, row_input.tooltip, &mut |ui| {
        let width = reference_button_width(ui, 1);
        let color = match label {
            AssetReferenceLabel::Named(_) => style::text(),
            _ => style::text_muted(),
        };
        let icon = assets_tiles::asset_type_icon(row_input.asset_type.unwrap_or_default());
        let response = ui.add_sized(
            egui::vec2(width, ROW_HEIGHT),
            egui::Button::new(icons::rich(icon, &text).color(color)).truncate(),
        );
        // A file dragged out of the Assets tab can be dropped on this button,
        // but only a file of the field's own asset type. While one hovers, the
        // row is outlined in the selection colour when the drop fits and in the
        // danger colour when it does not, so a near miss on the neighbouring
        // row is visible before the release.
        if let Some(dragged) = response.dnd_hover_payload::<String>() {
            let stroke = if asset_drop_fits(&dragged, &row_input) {
                ui.visuals().selection.stroke
            } else {
                egui::Stroke::new(ui.visuals().selection.stroke.width, style::danger())
            };
            ui.painter()
                .rect_stroke(response.rect, 2.0, stroke, egui::StrokeKind::Inside);
        }
        if let Some(dragged) = response.dnd_release_payload::<String>() {
            if asset_drop_fits(&dragged, &row_input) {
                chosen = Some(Some((*dragged).clone()));
            } else {
                pill_core::debug!(
                    target: telemetry_target::EDITOR,
                    entity = %row_input.entity,
                    field = row_input.field,
                    asset_type = row_input.asset_type.unwrap_or_default(),
                    asset = dragged.as_str(),
                    "[editor] asset drop refused: wrong asset type"
                );
            }
        }
        if response.clicked() {
            session.asset_picker = match session.asset_picker.as_deref() {
                Some(open) if open == key => None,
                _ => Some(key.clone()),
            };
            session.asset_search.clear();
        }
        button_rects = Some((response.rect, response.interact_rect));
        if ui
            .add_enabled_ui(!matches!(label, AssetReferenceLabel::Unset), |ui| {
                ui.add_sized(side_button_size(), egui::Button::new("x").small())
            })
            .inner
            .on_hover_text("Clear the reference")
            .clicked()
        {
            chosen = Some(None);
        }
    });
    if let Some((rect, interact_rect)) = button_rects {
        remember_asset_row_rect(session, &key, rect, interact_rect.contains(rect.center()));
    }

    // Step 3: draw the picker while it is open. It holds a search box and the
    // field's candidates, with `None` first. It floats over the rows below and
    // claims its rectangle, so a click in it is the picker's, not the scene's.
    if session.asset_picker.as_deref() == Some(key.as_str()) {
        if let Some((rect, _)) = button_rects {
            let picker_id = egui::Id::new(("pill_editor_shell.asset_picker", key.as_str()));
            let popup =
                floating_popup(
                    ui,
                    picker_id,
                    rect,
                    rect.width().max(180.0),
                    overlay_regions,
                    &mut |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut session.asset_search)
                                .hint_text("Search assets")
                                .desired_width(f32::INFINITY),
                        );
                        let search = session.asset_search.to_lowercase();
                        let (files, built) = inspector::picker_candidates(
                            row_input.assets,
                            row_input.engine_assets,
                            row_input.asset_type,
                        );
                        egui::ScrollArea::vertical()
                            .max_height(ASSET_PICKER_HEIGHT)
                            .show(ui, |ui| {
                                if ui.selectable_label(false, "None").clicked() {
                                    chosen = Some(None);
                                }
                                for candidate in files.into_iter().filter(|candidate| {
                                    candidate.path.to_lowercase().contains(&search)
                                }) {
                                    let current = matches!(&label, AssetReferenceLabel::Named(name)
                                    if *name == candidate.path);
                                    ui.horizontal(|ui| {
                                        // A row under a module root is a file the
                                        // engine ships, not one of the project's.
                                        // The badge is the only sign of that: a
                                        // module's resource path reads like any
                                        // other path.
                                        let from_module = !candidate.root.is_empty();
                                        if from_module {
                                            engine_badge(ui);
                                        }
                                        let mut row = ui.selectable_label(current, &candidate.path);
                                        if from_module {
                                            row = row.on_hover_text(format!(
                                                "Shipped with the {} module",
                                                candidate.root
                                            ));
                                        }
                                        if row.clicked() {
                                            chosen = Some(Some(candidate.path.clone()));
                                        }
                                        // The same "show in the Assets panel" button
                                        // the asset-settings picker offers. The
                                        // picker stays open, so looking an asset up
                                        // does not throw the edit away.
                                        if ui
                                            .small_button("o")
                                            .on_hover_text("Show this asset in the Assets panel")
                                            .clicked()
                                        {
                                            pending.focus_asset(candidate.key());
                                        }
                                    });
                                }
                                // The store's own assets, built in code, which no
                                // folder walk finds. A row is picked by its guid
                                // when it has one, and otherwise by the name it was
                                // added under; the field write resolves either one
                                // through the store.
                                for candidate in built.into_iter().filter(|candidate| {
                                    candidate.name.to_lowercase().contains(&search)
                                }) {
                                    let current = matches!(&label, AssetReferenceLabel::Named(name)
                                    if *name == candidate.name);
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
            // A press outside the picker closes it, a rule the popup helper
            // applies to every picker. The press that opened this one landed on
            // the row, so it does not close it again.
            if popup.pressed_outside {
                session.asset_picker = None;
            }
        }
    }

    // Step 4: queue the reference the row chose, as one recorded step.
    if let Some(reference) = chosen {
        session.asset_picker = None;
        let (commands, label, coalesce_key) = inspector::selection_asset_edit(
            row_input.targets,
            row_input.entity,
            type_name,
            row_input.field,
            reference,
        );
        pending.push_recorded(commands, label, Some(coalesce_key));
    }
}

/// Draws one entity reference row.
///
/// The row is a button naming the referenced entity, which opens a picker over
/// the scene, a target button that arms a scene click, a clear button, and a
/// drop target for an entity dragged out of the Entities tab.
///
/// Every choice becomes one recorded `SetFieldEntity` step for the whole
/// selection. A reference whose entity is gone reads `(missing)` rather than
/// disappearing: the field still holds the handle, and hiding it would leave
/// the row looking unset while the stored bytes say otherwise.
pub fn inspector_entity_row(
    ui: &mut egui::Ui,
    row_input: EntityRowInput<'_>,
    pending: &mut PendingEditorCommands,
    session: &mut InspectorSession,
    overlay_regions: &mut Vec<ShellRect>,
) {
    let type_name = row_input.component.type_name.as_str();
    let label = row_input
        .component
        .entity_labels
        .get(row_input.index)
        .cloned()
        .flatten()
        .unwrap_or(EntityReferenceLabel::Unset);
    let text = label.text().to_owned();
    // Step 1: log the entity this row draws, once per row and text, so a suite
    // can wait for a pick to show up and for an undo to restore the previous
    // one.
    let key = format!("{}.{type_name}.{}", row_input.entity, row_input.field);
    if remember_entity_row_text(session, &key, &text) {
        pill_core::info!(
            target: telemetry_target::EDITOR,
            entity = %row_input.entity,
            component = type_name,
            field = row_input.field,
            reference = text.as_str(),
            "[editor] inspector row kind=entity"
        );
    }

    // Step 2: draw the row and collect the reference it picks.
    //
    // `Some(None)` clears the reference and `Some(Some(entity))` points it at
    // one. The choice is queued once, after the row has drawn.
    let mut chosen: Option<Option<Entity>> = None;
    let mut scene_pick_armed = false;
    let mut button_rects = None;
    let row_label_text = match row_input.component.values.get(row_input.index) {
        Some(value) => row_label(
            row_input.field,
            row_input.details,
            type_name,
            row_input.index,
            value,
        ),
        None => row_input.field.to_owned(),
    };
    inspector_row(ui, &row_label_text, row_input.tooltip, &mut |ui| {
        let width = reference_button_width(ui, 2);
        let color = match label {
            EntityReferenceLabel::Live(_) => style::text(),
            _ => style::text_muted(),
        };
        let response = ui.add_sized(
            egui::vec2(width, ROW_HEIGHT),
            egui::Button::new(icons::rich(icons::Icon::TreeStructure, &text).color(color))
                .truncate(),
        );
        if response.dnd_hover_payload::<DraggedEntity>().is_some() {
            ui.painter().rect_stroke(
                response.rect,
                2.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        if let Some(dragged) = response.dnd_release_payload::<DraggedEntity>() {
            chosen = Some(Some(dragged.0));
        }
        if response.clicked() {
            session.entity_picker = match session.entity_picker.as_deref() {
                Some(open) if open == key => None,
                _ => Some(key.clone()),
            };
            session.entity_search.clear();
        }
        button_rects = Some((response.rect, response.interact_rect));
        if ui
            .add_sized(side_button_size(), egui::Button::new("+").small())
            .on_hover_text("Pick the reference by clicking it in the scene")
            .clicked()
        {
            scene_pick_armed = true;
        }
        if ui
            .add_enabled_ui(!matches!(label, EntityReferenceLabel::Unset), |ui| {
                ui.add_sized(side_button_size(), egui::Button::new("x").small())
            })
            .inner
            .on_hover_text("Clear the reference")
            .clicked()
        {
            chosen = Some(None);
        }
    });
    if let Some((rect, interact_rect)) = button_rects {
        remember_entity_row_rect(session, &key, rect, interact_rect.contains(rect.center()));
    }

    // Step 3: draw the picker while it is open: a search box over the scene's
    // entities, with `None` first.
    if session.entity_picker.as_deref() == Some(key.as_str()) {
        if let Some((rect, _)) = button_rects {
            let picker_id = egui::Id::new(("pill_editor_shell.entity_picker", key.as_str()));
            let popup = floating_popup(
                ui,
                picker_id,
                rect,
                rect.width().max(180.0),
                overlay_regions,
                &mut |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut session.entity_search)
                            .hint_text("Search entities")
                            .desired_width(f32::INFINITY),
                    );
                    let search = session.entity_search.to_lowercase();
                    egui::ScrollArea::vertical()
                        .max_height(ASSET_PICKER_HEIGHT)
                        .show(ui, |ui| {
                            if ui.selectable_label(false, "None").clicked() {
                                chosen = Some(None);
                            }
                            for candidate in inspector::entity_candidates(row_input.scene) {
                                if !candidate.display.to_lowercase().contains(&search) {
                                    continue;
                                }
                                let current = matches!(&label, EntityReferenceLabel::Live(name)
                                    if *name == candidate.display);
                                if ui.selectable_label(current, &candidate.display).clicked() {
                                    chosen = Some(Some(candidate.entity));
                                }
                            }
                        });
                },
            );
            // A press outside the picker closes it, the rule the popup helper
            // applies. The press that opened this one landed on the row.
            if popup.pressed_outside {
                session.entity_picker = None;
            }
        }
    }

    // Step 4: queue what the row asked for. A press on the target button arms
    // a scene click first; a reference the picker or a drop chose is queued
    // after it.
    if scene_pick_armed {
        // The scene's own click names the reference, so the request goes
        // through the command queue to the executable that reads scene picks.
        // The shell only draws rows; it does not own the scene.
        pending.push(vec![EditorCommand::ArmEntityPick {
            entity: row_input.entity,
            component: type_name.to_owned(),
            field: row_input.field.to_owned(),
        }]);
    }

    if let Some(reference) = chosen {
        session.entity_picker = None;
        let (commands, label, coalesce_key) = inspector::selection_entity_edit(
            row_input.targets,
            row_input.entity,
            type_name,
            row_input.field,
            reference,
        );
        pending.push_recorded(commands, label, Some(coalesce_key));
    }
}

/// Records the text an entity row draws and answers whether it changed since
/// the last frame that drew the row.
pub fn remember_entity_row_text(session: &mut InspectorSession, key: &str, text: &str) -> bool {
    session.entity_row_texts.remember(key.to_owned(), text)
}

/// Logs an entity row's rectangle once per placement, keyed by
/// `entity.component.field`, so a suite can drop an entity onto it.
///
/// The placement is recorded whether or not the line is logged, and the line
/// goes out only when the session logs rectangles. It exists to aim a scripted
/// drop and adds nothing to a person's console.
pub fn remember_entity_row_rect(
    session: &mut InspectorSession,
    key: &str,
    rect: egui::Rect,
    visible: bool,
) {
    if !visible {
        return;
    }
    let Some(rect) = record_row_placement(&mut session.entity_row_rects, key, rect) else {
        return;
    };
    if !session.log_rects {
        return;
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        row = key,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] inspector entity row rect"
    );
}

/// Records `rect` as `key`'s placement and returns it when the log has not
/// named that placement yet.
///
/// The comparison is [`LoggedRects::remember`]'s, which rounds to whole
/// points, and the returned rectangle is rounded the same way. The numbers in
/// the log are therefore the ones that were compared. A row that leaves the
/// visible area keeps its last placement, so scrolling back to it does not log
/// again.
pub fn record_row_placement(
    logged: &mut LoggedRects<String>,
    key: &str,
    rect: egui::Rect,
) -> Option<egui::Rect> {
    logged.remember(key.to_owned(), rect).then(|| {
        egui::Rect::from_min_max(
            egui::pos2(rect.min.x.round(), rect.min.y.round()),
            egui::pos2(rect.max.x.round(), rect.max.y.round()),
        )
    })
}

/// Records the text an asset row draws and answers whether it changed since
/// the last frame that drew the row.
///
/// The row logs its current asset on its first draw and on every change, so a
/// suite can wait for a pick to show up and for an undo to bring the previous
/// asset back.
pub fn remember_asset_row_text(session: &mut InspectorSession, key: &str, text: &str) -> bool {
    session.asset_row_texts.remember(key.to_owned(), text)
}

/// Logs an asset row's rectangle once per placement, keyed by
/// `entity.component.field`, so a suite can drop an asset onto it.
///
/// Only a row whose centre is inside its visible area reaches the log: a row
/// scrolled out of the panel cannot be clicked or dropped on. The line itself
/// belongs to a scripted run.
pub fn remember_asset_row_rect(
    session: &mut InspectorSession,
    key: &str,
    rect: egui::Rect,
    visible: bool,
) {
    if !visible {
        return;
    }
    let Some(rect) = record_row_placement(&mut session.asset_row_rects, key, rect) else {
        return;
    };
    if !session.log_rects {
        return;
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        row = key,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] inspector asset row rect"
    );
}

/// Returns the current value of each leaf of a grouped row, in the row's own
/// order. A leaf inside an array value reads that element.
pub fn leaf_values(component: &ComponentView, leaves: &[inspector::RowLeaf]) -> Vec<FieldValue> {
    leaves
        .iter()
        .filter_map(|leaf| match leaf.element {
            Some(element) => component
                .values
                .get(leaf.index)
                .and_then(|value| inspector::element_field(value, element)),
            None => component.values.get(leaf.index).cloned(),
        })
        .collect()
}

/// Returns the channels a colour row's values hold, as the unmultiplied linear
/// floats the picker takes, or `None` when a value is not a float.
///
/// Three channels stay opaque and four carry their alpha. Any other length is
/// not a colour the picker can draw.
pub fn color_channels(values: &[FieldValue]) -> Option<[f32; 4]> {
    if !(3..=4).contains(&values.len()) {
        return None;
    }
    let mut channels = [1.0f32; 4];
    for (channel, value) in channels.iter_mut().zip(values) {
        *channel = match value {
            FieldValue::F32(number) => *number,
            FieldValue::F64(number) => *number as f32,
            _ => return None,
        };
    }
    Some(channels)
}

/// Returns the label a row draws, with a `(mixed)` mark when the selection
/// disagrees on its value.
pub fn row_label(
    field: &str,
    details: &[EntityDetail],
    type_name: &str,
    index: usize,
    value: &FieldValue,
) -> String {
    if row_is_mixed(details, type_name, index, value) {
        format!("{field} (mixed)")
    } else {
        field.to_string()
    }
}

/// Queues one field edit for **every** selected entity that carries the
/// component, as one recorded step.
///
/// `targets` is [`super::tab_inspector::inspector_targets`]'s answer: the
/// primary entity first, then the other selected entities that carry the
/// component. One batch is one undo step however many entities the row wrote,
/// and the coalesce key comes from the primary, so a drag across a
/// multi-selection still collapses into a single step.
pub fn queue_field_edit(
    pending: &mut PendingEditorCommands,
    targets: &[Entity],
    entity: Entity,
    type_name: &str,
    field: &str,
    value: FieldValue,
    phase: FieldEditPhase,
) {
    let (commands, label, coalesce_key) =
        inspector::selection_field_edit(targets, entity, type_name, field, value);
    pending.push_recorded_with_phase(commands, label, Some(coalesce_key), phase);
}

/// Queues one array-element edit for every target, as one recorded step.
// The parameters mirror `inspector::selection_element_edit`; bundling them
// into a struct would only rename the same list.
#[allow(clippy::too_many_arguments)]
pub fn queue_element_edit(
    pending: &mut PendingEditorCommands,
    targets: &[Entity],
    entity: Entity,
    type_name: &str,
    field: &str,
    index: usize,
    value: FieldValue,
    phase: FieldEditPhase,
) {
    let (commands, label, coalesce_key) =
        inspector::selection_element_edit(targets, entity, type_name, field, index, value);
    pending.push_recorded_with_phase(commands, label, Some(coalesce_key), phase);
}

/// Returns whether the selected entities disagree on this row's value.
pub fn row_is_mixed(
    details: &[EntityDetail],
    type_name: &str,
    index: usize,
    value: &FieldValue,
) -> bool {
    details.iter().any(|detail| {
        detail
            .components
            .iter()
            .find(|view| view.type_name == type_name)
            .and_then(|view| view.values.get(index))
            .is_some_and(|other| other != value)
    })
}

/// Returns the name a row's widget logs under, so a suite can assert which
/// widget the model chose without reading pixels.
pub fn field_widget_name(widget: &inspector::FieldWidget) -> &'static str {
    match widget {
        inspector::FieldWidget::Checkbox => "checkbox",
        inspector::FieldWidget::Integer => "integer",
        inspector::FieldWidget::Float => "float",
        inspector::FieldWidget::Text => "text",
        inspector::FieldWidget::ReadOnly => "read-only",
        inspector::FieldWidget::FloatSlider { .. } => "float-slider",
        inspector::FieldWidget::IntegerSlider { .. } => "integer-slider",
        inspector::FieldWidget::FloatDrag { .. } => "float-drag",
        inspector::FieldWidget::IntegerDrag { .. } => "integer-drag",
        inspector::FieldWidget::MultilineText { .. } => "multiline",
        inspector::FieldWidget::Enum => "enum",
    }
}

/// Draws the Inspector's field row: the shared label/control row ([`row`]),
/// with the field's label on the left and its widget in the control column. A
/// `[Tooltip]` shows while the pointer rests on the label.
pub fn inspector_row(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: Option<&str>,
    widget: &mut dyn FnMut(&mut egui::Ui),
) {
    row(ui, label, tooltip, widget);
}

/// Returns whether a field's value has an editor in the Inspector: every
/// scalar kind and every declared enum. Any other kind is shown as text.
pub fn has_field_editor(value: &FieldValue) -> bool {
    is_scalar_field(value) || matches!(value, FieldValue::Isize(_) | FieldValue::Enum { .. })
}

/// Returns the width of one component's slot in a vector row: the control
/// column left after the label, shared equally by `components` slots and the
/// gaps between them. A narrow panel squashes every slot alike, down to the
/// axis letter and [`CONTROL_MIN_WIDTH`] of field, and the value is cropped
/// inside its field.
pub fn vector_slot_width(ui: &egui::Ui, components: usize) -> f32 {
    slot_width_within(
        ui.available_width(),
        components,
        ui.spacing().item_spacing.x,
    )
    .max(INSPECTOR_VECTOR_LABEL_WIDTH + CONTROL_MIN_WIDTH)
}

/// Draws a vector row: the label, then one axis letter and field per
/// component, the components sharing the control column equally.
///
/// When the control column is too narrow for every component to get
/// [`VECTOR_SLOT_MIN_WIDTH`], the components move to their own line under the
/// label and share the panel's whole width there, the way Unity lays out a
/// vector in a narrow Inspector. Nothing runs past the panel's edge either way.
/// `draw_component` draws component `index`'s field in the slot after its
/// letter.
pub fn vector_row(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: Option<&str>,
    components: usize,
    draw_component: &mut dyn FnMut(&mut egui::Ui, usize),
) {
    let spacing = ui.spacing().item_spacing.x;
    let control_width = ui.available_width() - label_column_width(ui) - ROW_LABEL_GAP - spacing;
    let beside_label =
        slot_width_within(control_width, components, spacing) >= VECTOR_SLOT_MIN_WIDTH;
    if beside_label {
        ui.horizontal(|ui| {
            label_cell(ui, label, tooltip);
            label_gap(ui);
            vector_slots(ui, components, draw_component);
        });
        return;
    }
    ui.horizontal(|ui| {
        label_cell(ui, label, tooltip);
    });
    ui.horizontal(|ui| {
        ui.add_space(VECTOR_OWN_LINE_INDENT);
        vector_slots(ui, components, draw_component);
    });
}

/// Draws `components` slots across the width left in the row, each an axis
/// letter and the field `draw_component` draws.
pub fn vector_slots(
    ui: &mut egui::Ui,
    components: usize,
    draw_component: &mut dyn FnMut(&mut egui::Ui, usize),
) {
    let slot_width = vector_slot_width(ui, components);
    for component_index in 0..components {
        ui.allocate_ui_with_layout(
            egui::vec2(slot_width, INSPECTOR_VECTOR_FIELD_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                axis_slot(ui, component_index);
                draw_component(ui, component_index);
            },
        );
        ui.add_space(VECTOR_SLOT_GAP);
    }
}

/// The width each of `components` slots gets in `width` points, after the gap
/// that follows every slot (the row's own [`VECTOR_SLOT_GAP`] plus egui's item
/// `spacing`).
pub fn slot_width_within(width: f32, components: usize, spacing: f32) -> f32 {
    let components = components.max(1) as f32;
    (width - (VECTOR_SLOT_GAP + spacing) * components) / components
}

/// Returns whether a value has a widget kind of its own.
pub fn is_scalar_field(value: &FieldValue) -> bool {
    matches!(
        value,
        FieldValue::F32(_)
            | FieldValue::F64(_)
            | FieldValue::I8(_)
            | FieldValue::I16(_)
            | FieldValue::I32(_)
            | FieldValue::I64(_)
            | FieldValue::U8(_)
            | FieldValue::U16(_)
            | FieldValue::U32(_)
            | FieldValue::U64(_)
            | FieldValue::Usize(_)
            | FieldValue::Bool(_)
    )
}
