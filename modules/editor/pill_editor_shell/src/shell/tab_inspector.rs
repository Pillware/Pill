//! The Inspector tab: the selection's components and their rows.
//!
//! # Responsibilities
//!
//! - `inspector_tab` and its header: the selected entity's name and the Add
//!   Component popup.
//! - `entity_inspector` and its section headers: one collapsible section per
//!   component, with the rows the inspector's row model asks for.
//! - `inspector_section_commands`: the commands modules place on a component's
//!   registered, drawn at the right end of their component's header bar.
//! - `addable_components`: the components the Add popup and the component menu
//!   offer.
//! - The vector row metrics (`INSPECTOR_VECTOR_FIELD_WIDTH`,
//!   `INSPECTOR_VECTOR_LABEL_WIDTH`, `INSPECTOR_VECTOR_FIELD_HEIGHT`) the
//!   field rows size each axis group with.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
#[allow(unused_imports)]
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// A vector element's group in a typical panel, in points: the axis letter and
/// its field together. The groups really share the control column equally
/// ([`vector_slot_width`]); this is the size the shell's tests lay one at.
pub const INSPECTOR_VECTOR_FIELD_WIDTH: f32 = 58.0;

/// The width of the slot that holds an axis letter inside its group, in
/// points. The slot is fixed, so every component's editor gets the same width
/// however wide the letter's glyph is.
pub const INSPECTOR_VECTOR_LABEL_WIDTH: f32 = 10.0;

/// The narrowest a vector component's slot (its axis letter and field) may be
/// beside the label, in points. A narrower panel moves the components to their
/// own line under the label.
pub const VECTOR_SLOT_MIN_WIDTH: f32 = 46.0;

/// How far a vector's own line of components is indented, in points, so it
/// reads as belonging to the label above it.
pub const VECTOR_OWN_LINE_INDENT: f32 = 12.0;

/// The space in points the row leaves after each vector component's slot.
pub const VECTOR_SLOT_GAP: f32 = 2.0;

/// The space in points between a component's separator line and its header.
pub const SECTION_LINE_GAP: f32 = 3.0;

/// The height of one vector field, in points: the shared row height, so a
/// vector row has the same padding as every other row. The field's editor and
/// the group that holds it are both this tall.
pub const INSPECTOR_VECTOR_FIELD_HEIGHT: f32 = ROW_HEIGHT;

/// The side of the square each Inspector header action gets on a component's
/// header bar, in points.
pub const INSPECTOR_ACTION_SIZE: f32 = 16.0;

// =============================================================================
// Types
// =============================================================================

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
    commands: &mut PlacementHost<'_>,
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
            entity_inspector(
                ui,
                snapshot,
                detail,
                pending,
                overlay_regions,
                session,
                commands,
            );
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
            component_menu_items(ui, snapshot, detail, None, pending, commands)
        });
    }
}

/// Draws the Inspector's top row: the entity's circle icon and name, and the
/// Add Component button aligned to the right. The button opens and closes the
/// popup that lists the components it can attach.
///
/// The row is the panel's subject rather than one more row, so the name takes
/// the theme's heading style - a project's `.theme` decides its size - and the
/// circle is drawn at that size beside it, in the same duotone form the
/// component sections use.
pub fn inspector_header(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
) {
    let mut row_geometry = None;
    ui.horizontal(|ui| {
        // Step 1: the entity's mark, sized to the heading so the two line up
        // when a theme changes the heading.
        let heading = egui::TextStyle::Heading.resolve(ui.style());
        let (icon_rect, _) =
            ui.allocate_exact_size(egui::vec2(heading.size, heading.size), egui::Sense::hover());
        icons::paint_duotone(
            ui.painter(),
            icon_rect,
            icons::Icon::Circle,
            ui.visuals().text_color(),
        );
        // Step 2: the name, in the heading style and strong enough to read as
        // the panel's title.
        let name = ui.label(
            egui::RichText::new(format!("entity {}", detail.entity))
                .text_style(egui::TextStyle::Heading)
                .strong(),
        );
        row_geometry = Some((icon_rect.union(name.rect), icon_rect.width(), heading.size));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let button = kit::tooltip::on_hover_text(
                ui.add(
                    icons::IconButton::new(icons::Icon::Plus, "")
                        .fixed_size(egui::vec2(24.0, 18.0))
                        .active(session.add_open),
                ),
                "Add Component",
            );
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
    // Step 3: report the row once per placement, so a suite can prove the
    // heading drew the circle at its own size without reading pixels.
    if let Some((rect, icon_width, font_size)) = row_geometry {
        if session
            .header_row
            .is_none_or(|(logged, logged_icon, logged_font)| {
                !LoggedRects::<String>::same_placement(logged, rect)
                    || logged_icon != icon_width
                    || logged_font != font_size
            })
        {
            session.header_row = Some((rect, icon_width, font_size));
            pill_core::debug!(
                target: telemetry_target::EDITOR,
                x = rect.min.x,
                y = rect.min.y,
                width = rect.width(),
                height = rect.height(),
                icon = icon_width,
                font = font_size,
                "[editor] inspector header"
            );
        }
    }
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
    let area = egui::Area::new(egui::Id::new("pill_editor_shell.inspector_add"))
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
/// the rows the inspector's row model asks for. The Inspector header actions
/// modules registered for a component sit at the right end of its bar.
pub fn entity_inspector(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    session: &mut InspectorSession,
    commands: &mut PlacementHost<'_>,
) {
    for component in &detail.components {
        let collapsed = session.collapsed.contains(&component.type_name);
        let command_count = header_commands(commands.commands, &component.type_name).count();
        let (header, commands_rect) =
            inspector_section_header(ui, component, collapsed, session, command_count);
        // Drawn after the bar's own click area, so a command's button sits on
        // top of it and a click on the button never toggles the section.
        if let Some(commands_rect) = commands_rect {
            inspector_section_commands(
                ui,
                commands_rect,
                detail.entity,
                &component.type_name,
                commands,
                session,
            );
        }
        context_menu("inspector", &header, overlay_regions, &mut |ui| {
            component_menu_items(
                ui,
                snapshot,
                detail,
                Some(component.type_name.as_str()),
                pending,
                commands,
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
/// Returns the header's response and, when `action_count` is above zero, the
/// space reserved at its right end for that many Inspector header actions.
/// The whole header is clickable, and the caller toggles the section when the
/// response reports a click. The header logs its rectangle once per placement.
pub fn inspector_section_header(
    ui: &mut egui::Ui,
    component: &ComponentView,
    collapsed: bool,
    session: &mut InspectorSession,
    action_count: usize,
) -> (egui::Response, Option<egui::Rect>) {
    let mut actions_rect = None;
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
                // The actions' space is only reserved here; the caller draws
                // them once the bar's click area exists.
                if action_count > 0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let width = action_count as f32 * INSPECTOR_ACTION_SIZE
                            + (action_count - 1) as f32 * ui.spacing().item_spacing.x;
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, INSPECTOR_ACTION_SIZE),
                            egui::Sense::hover(),
                        );
                        actions_rect = Some(rect);
                    });
                }
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
    let response = ui
        .interact(
            rect,
            ui.id()
                .with(("inspector-section", component.type_name.as_str())),
            egui::Sense::click(),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    (response, actions_rect)
}

/// Draws the commands the modules placed on one component's header into the
/// space its header bar reserved, right to left in registration order.
///
/// Each command draws from data - its icon, title and the state published for
/// this entity and component - or through the draw function its module
/// registered for that command and place. A click joins the pass's invocation
/// list, which the executable hands to the module that owns the id.
pub fn inspector_section_commands(
    ui: &mut egui::Ui,
    commands_rect: egui::Rect,
    entity: Entity,
    component_type_name: &str,
    commands: &mut PlacementHost<'_>,
    session: &mut InspectorSession,
) {
    // A child that allocates nothing in the parent, so the reserved space is
    // not counted twice in the Inspector's layout.
    let mut buttons_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(commands_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    // The commands the modules placed on this header, in registration order.
    let target = CommandTarget::Entity {
        entity,
        component: Some(component_type_name.to_string()),
    };
    let placed: Vec<(String, UiSlot)> = header_commands(commands.commands, component_type_name)
        .filter_map(|command| {
            command
                .placements
                .iter()
                .find(|slot| {
                    matches!(slot, UiSlot::InspectorComponentHeader { .. })
                        && slot.matches_component(component_type_name)
                })
                .map(|slot| (command.id.clone(), slot.clone()))
        })
        .collect();
    for (command_id, slot) in placed {
        let Some(command) = commands
            .commands
            .iter()
            .find(|command| command.id == command_id)
        else {
            continue;
        };
        let rect = draw_placed_command(
            &mut buttons_ui,
            command,
            &slot,
            &target,
            commands.states,
            commands.draws,
            commands.invocations,
        );
        let Some(rect) = rect else {
            continue;
        };
        let key = format!("{component_type_name}@{}", command.id);
        remember_command_rect(
            &mut session.command_rects,
            session.log_rects,
            &key,
            &command.id,
            &slot,
            rect,
        );
    }
}

/// The registered commands that place a button on `type_name`'s header.
pub fn header_commands<'a>(
    commands: &'a [CommandView],
    type_name: &'a str,
) -> impl Iterator<Item = &'a CommandView> + 'a {
    commands.iter().filter(move |command| {
        command.placements.iter().any(|slot| {
            matches!(slot, UiSlot::InspectorComponentHeader { .. })
                && slot.matches_component(type_name)
        })
    })
}
