//! The Inspector tab: the selection's components and their rows.
//!
//! # Responsibilities
//!
//! - `inspector_tab` and its header: the selected entity's name and the Add
//!   Component popup.
//! - `entity_inspector` and its section headers: one collapsible section per
//!   component, with the rows the inspector's row model asks for.
//! - `addable_components`: the components the Add popup and the component menu
//!   offer.
//! - The vector row metrics (`INSPECTOR_VECTOR_FIELD_WIDTH`,
//!   `INSPECTOR_VECTOR_LABEL_WIDTH`, `INSPECTOR_VECTOR_FIELD_HEIGHT`) the
//!   field rows size each axis group with.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The narrowest a vector element's group gets, in points: the axis letter and
/// its field together. The groups share the control column equally and never
/// shrink below this; a value too long for its field is cropped.
pub const INSPECTOR_VECTOR_FIELD_WIDTH: f32 = 58.0;

/// The width of the slot that holds an axis letter inside its group, in
/// points. The slot is fixed, so every component's editor gets the same width
/// however wide the letter's glyph is.
pub const INSPECTOR_VECTOR_LABEL_WIDTH: f32 = 10.0;

/// The space in points between a component's separator line and its header.
pub const SECTION_LINE_GAP: f32 = 3.0;

/// The height of one vector field, in points. The field's editor and the group
/// that holds it are both this tall.
pub const INSPECTOR_VECTOR_FIELD_HEIGHT: f32 = 18.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the Inspector tab: the selected entity's components as collapsible
/// sections (a caret, the component's duotone icon and its label), the Add
/// Component button in the tab's top-right, and the selected asset's import
/// settings as editable rows below.
///
/// Every edit the tab queues is recorded as a `SetField`, `SetArrayElement` or
/// `SetAssetSettings` command. Each widget's edits share one coalesce key, so
/// one drag or one typing run becomes a single undo step.
#[pill_hot_fn]
pub fn inspector_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    selection: &EditorSelection,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    // This tab has no scroll area of its own. The dock already wraps the tab
    // body in one, and a nested area sizes itself to its content instead of
    // the body, so its rows would fall outside the scrolled region.
    // A selected resource takes the tab: one selection shows one thing.
    if let Some(view) = selection
        .resource
        .as_deref()
        .and_then(|name| snapshot.resources.iter().find(|view| view.name == name))
    {
        resource_section(ui, view, pending);
        return;
    }
    // The row log and the remembered rectangles belong to the entity on
    // screen. When another entity becomes the subject, forget both: a stale
    // row model could hide the new entity's rows, and a remembered rectangle
    // could point a click at a section that is no longer there.
    let subject = snapshot.detail.as_ref().map(|detail| detail.entity);
    if session.subject != subject {
        session.subject = subject;
        session.row_models.clear();
        session.clear_geometry();
    }
    match snapshot.detail.as_ref() {
        Some(detail) => {
            inspector_header(ui, snapshot, detail, pending, overlay_regions, session);
            entity_inspector(ui, snapshot, detail, pending, overlay_regions, session);
        }
        None => match selection.primary() {
            Some(entity) => {
                ui.label(format!("entity {entity} is not alive"));
            }
            None => {
                ui.label("Select an entity in the Entities tab.");
            }
        },
    }
    // The selected asset's section comes after the entity's. It shows the
    // asset's kind and its import settings, editable one leaf at a time.
    let selected_asset = selection.asset.as_deref();
    if let Some(detail) = snapshot
        .asset_detail
        .as_ref()
        .filter(|detail| selected_asset == Some(detail.key().as_str()))
    {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("asset {}", detail.key())).strong());
        read_only_row(ui, "kind", detail.row.kind.text());
        if detail.row.standalone {
            read_only_row(ui, "standalone", "yes");
        }
        // A module's resource is shown as it is. Every row only reads, and
        // nothing here can repoint it.
        if detail.row.read_only {
            // Log the read-only notice once per asset per session. The tab
            // redraws every frame, so the drawn notice is there all the time
            // and a suite has no single moment to wait for; this line marks
            // the first draw.
            if session.notices_logged.insert(detail.key()) {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    asset = detail.key().as_str(),
                    "[editor] inspector row kind=asset-read-only"
                );
            }
            ui.label(
                egui::RichText::new("provided by the engine - read-only")
                    .weak()
                    .italics(),
            );
        }
        ui.separator();
        match detail.settings.as_ref() {
            Some(settings) => asset_settings_rows(
                ui,
                &detail.key(),
                settings,
                &detail.references,
                &mut SettingsWalk {
                    assets: &snapshot.assets,
                    engine_assets: &snapshot.engine_assets,
                    pending,
                    session,
                    overlay_regions,
                    decorators: detail.decorators.as_slice(),
                    read_only: detail.row.read_only,
                },
            ),
            None => {
                ui.label(egui::RichText::new("no editable settings").weak().italics());
            }
        }
    }
    // The strip below the rows carries the component menu where no section
    // heading can hang it. A right-click there only offers the Add Component
    // submenu.
    if let Some(detail) = snapshot.detail.as_ref() {
        let empty = ui.allocate_response(empty_area(ui), egui::Sense::click());
        context_menu("inspector", &empty, overlay_regions, &mut |ui| {
            component_menu_items(ui, snapshot, detail, None, pending)
        });
    }
}

/// Draws the Inspector's top row: the selected entity's name, and the Add
/// Component button aligned to the right. The button opens and closes the
/// popup that lists the components it can attach.
pub fn inspector_header(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("entity {}", detail.entity)).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = ui
                .add(
                    icons::IconButton::new(icons::Icon::Plus, "")
                        .fixed_size(egui::vec2(24.0, 18.0))
                        .active(session.add_open),
                )
                .on_hover_text("Add Component");
            if session
                .add_rect
                .is_none_or(|logged| !LoggedRects::<String>::same_placement(logged, button.rect))
            {
                session.add_rect = Some(button.rect);
                pill_core::debug!(
                    target: telemetry_target::EDITOR,
                    x = button.rect.min.x,
                    y = button.rect.min.y,
                    width = button.rect.width(),
                    height = button.rect.height(),
                    "[editor] inspector add rect"
                );
            }
            if button.clicked() {
                session.add_open = !session.add_open;
                if session.add_open {
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        "[editor] inspector add opened"
                    );
                }
            }
        });
    });
    if session.add_open {
        inspector_add_popup(ui, snapshot, detail, pending, overlay_regions, session);
    }
}

/// Draws the Add Component popup, opened under the `+` button. It lists every
/// registered addable component the entity does not carry yet; a pick queues
/// the recorded `add {short}` step that the component menu and the scripted
/// channel also queue.
pub fn inspector_add_popup(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    let Some(anchor) = session.add_rect else {
        return;
    };
    let addable = addable_components(snapshot, detail);
    let mut picked = None;
    let area = egui::Area::new(egui::Id::new("pill_editor_shell_egui.inspector_add"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(anchor.max.x - 220.0, anchor.max.y + 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(220.0);
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        if addable.is_empty() {
                            ui.label(egui::RichText::new("every component is attached").weak());
                        }
                        for type_name in &addable {
                            let response = ui.selectable_label(
                                false,
                                icons::rich(
                                    inspector::component_icon(type_name),
                                    inspector::component_label(type_name),
                                ),
                            );
                            if remember_moved_rect(
                                &mut session.add_entries,
                                type_name,
                                response.rect,
                            ) {
                                pill_core::debug!(
                                    target: telemetry_target::EDITOR,
                                    component = type_name,
                                    x = response.rect.min.x,
                                    y = response.rect.min.y,
                                    width = response.rect.width(),
                                    height = response.rect.height(),
                                    "[editor] inspector add entry"
                                );
                            }
                            if response.clicked() {
                                picked = Some((*type_name).to_string());
                                ui.close();
                            }
                        }
                    });
            });
        });
    // The popup floats over the panels, so the frame must treat a press inside
    // it as the UI's and never as a scene pick.
    overlay_regions.push(shell_rect(area.response.rect));
    if let Some(type_name) = picked {
        let (command, label) = inspector::component_add(detail.entity, &type_name);
        pending.push_recorded(vec![command], label, None);
        session.add_open = false;
    } else if addable.is_empty() {
        // Nothing to pick. The popup stays open, and the next click on the
        // button or a press outside closes it.
    }
    // Any pointer press outside the button and outside the popup closes it,
    // whatever button it was.
    let outside = ui.ctx().input(|input| {
        input.pointer.any_pressed()
            && input.pointer.interact_pos().is_some_and(|position| {
                !anchor.contains(position) && !area.response.rect.contains(position)
            })
    });
    if outside {
        session.add_open = false;
    }
}

/// Returns the components the entity can still take: every registered one
/// that is marked addable and that the entity does not carry yet.
pub fn addable_components<'a>(snapshot: &'a EditorSnapshot, detail: &EntityDetail) -> Vec<&'a str> {
    snapshot
        .components
        .iter()
        .filter(|registered| registered.addable)
        .filter(|registered| {
            !detail
                .components
                .iter()
                .any(|carried| carried.type_name == registered.type_name)
        })
        .map(|registered| registered.type_name.as_str())
        .collect()
}

/// Draws one entity's components as collapsible sections. Each one has a
/// header bar (a caret, the component's duotone icon and its label) that
/// toggles the section and opens the component menu on a right-click, then
/// the rows the inspector's row model asks for.
pub fn entity_inspector(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    for component in &detail.components {
        let collapsed = session.collapsed.contains(&component.type_name);
        let header = inspector_section_header(ui, component, collapsed, session);
        context_menu("inspector", &header, overlay_regions, &mut |ui| {
            component_menu_items(
                ui,
                snapshot,
                detail,
                Some(component.type_name.as_str()),
                pending,
            )
        });
        if header.clicked() {
            if collapsed {
                session.collapsed.remove(&component.type_name);
            } else {
                session.collapsed.insert(component.type_name.clone());
            }
            pill_core::info!(
                target: telemetry_target::EDITOR,
                component = component.type_name.as_str(),
                collapsed = !collapsed,
                "[editor] inspector section"
            );
        }
        if collapsed {
            continue;
        }
        if !component.editable {
            ui.label(egui::RichText::new("no editable fields").weak().italics());
            ui.add_space(4.0);
            continue;
        }
        let (mut rows, targets) = inspector::take_component_rows(
            session,
            component,
            &snapshot.selected_details,
            detail.entity,
        );
        for cached in &mut rows {
            inspector_field_row(
                ui,
                detail.entity,
                component,
                &cached.row,
                &mut cached.logged,
                &targets,
                &snapshot.selected_details,
                &snapshot.assets,
                &snapshot.engine_assets,
                &snapshot.entities,
                pending,
                session,
                overlay_regions,
            );
        }
        inspector::store_component_rows(session, component.type_name.as_str(), rows, targets);
        ui.add_space(6.0);
    }
}

/// Returns the entities one component's edit reaches: the primary first, then
/// every other selected entity that carries the component.
pub fn inspector_targets(
    snapshot: &EditorSnapshot,
    primary: Entity,
    component: &str,
) -> Vec<Entity> {
    inspector::selection_targets(&snapshot.selected_details, primary, component)
}

/// Draws one component section's header: a separator line across the panel,
/// then the foldout caret, the component's duotone icon and its label, with no
/// fill behind them.
///
/// Returns the header's response. The whole header is clickable, and the
/// caller toggles the section when the response reports a click. The header
/// logs its rectangle once per placement.
pub fn inspector_section_header(
    ui: &mut egui::Ui,
    component: &ComponentView,
    collapsed: bool,
    session: &mut InspectorSession,
) -> egui::Response {
    let caret = if collapsed {
        icons::Icon::CaretRight
    } else {
        icons::Icon::CaretDown
    };
    // The line separates this component from the one above it, across the
    // whole panel.
    let line_y = ui.cursor().top();
    let panel_rect = ui.max_rect();
    ui.painter().hline(
        panel_rect.x_range(),
        line_y,
        egui::Stroke::new(1.0, style::line()),
    );
    ui.add_space(SECTION_LINE_GAP);
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(0, 2))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                let (caret_rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                icons::paint_duotone(ui.painter(), caret_rect, caret, style::text_muted());
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                icons::paint_duotone(
                    ui.painter(),
                    icon_rect,
                    inspector::component_icon(&component.type_name),
                    ui.visuals().text_color(),
                );
                ui.label(
                    egui::RichText::new(inspector::component_label(&component.type_name)).strong(),
                );
            });
        });
    let rect = frame.response.rect;
    if remember_moved_rect(&mut session.section_rects, &component.type_name, rect) {
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            component = component.type_name.as_str(),
            x = rect.min.x,
            y = rect.min.y,
            width = rect.width(),
            height = rect.height(),
            "[editor] inspector section rect"
        );
    }
    ui.interact(
        rect,
        ui.id()
            .with(("inspector-section", component.type_name.as_str())),
        egui::Sense::click(),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}
