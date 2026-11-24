//! The context menus the tabs open.
//!
//! # Responsibilities
//!
//! - The scene viewport's menu: the gizmo tools, snapping, the two spaces
//!   and the scene grid.
//! - The Inspector's component menu: remove one component, or add any addable
//!   one the entity does not carry.
//!
//! Both imports below use absolute paths, so a patch crate can resolve them
//! too.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

/// Draws the component menu. When `current` names the heading's component, a
/// Remove row detaches it. The Add Component submenu lists every registered
/// component marked addable that the entity does not carry. Both picks go
/// through the shared `inspector` builders and are labelled `add {short}` /
/// `remove {short}`. The commands modules placed in this component's menu come
/// last, with the entity and component as their target.
pub fn component_menu_items(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    detail: &EntityDetail,
    current: Option<&str>,
    pending: &mut PendingEditorCommands,
    placements: &mut PlacementHost<'_>,
) {
    // Step 1: the Remove row, when the menu was opened on a component heading.
    if let Some(component) = current {
        let label = format!("Remove {}", inspector::short_component_name(component));
        if ui
            .add(icons::IconButton::new(icons::Icon::MinusCircle, &label))
            .clicked()
        {
            let (command, label) = inspector::component_remove(detail.entity, component);
            pending.push_recorded(vec![command], label, None);
            ui.close();
        }
        ui.separator();
    }
    // Step 2: the Add Component submenu.
    ui.menu_button(
        icons::rich(icons::Icon::PlusCircle, "Add Component"),
        |ui| {
            // This is the same list the Inspector's Add popup shows. Both call
            // `addable_components`, so a single rule decides what can be added.
            // The menu and the popup cannot disagree about a component the
            // entity already carries.
            for type_name in addable_components(snapshot, detail) {
                let short = inspector::short_component_name(type_name);
                if ui
                    .add(icons::IconButton::new(icons::Icon::PlusCircle, short))
                    .clicked()
                {
                    let (command, label) = inspector::component_add(detail.entity, type_name);
                    pending.push_recorded(vec![command], label, None);
                    ui.close();
                }
            }
        },
    );
    // Step 3: the commands modules placed in this component's menu.
    if let Some(component) = current {
        let target = CommandTarget::Entity {
            entity: detail.entity,
            component: Some(component.to_string()),
        };
        let slot = UiSlot::component_context_menu(component);
        let rows = component_menu_rows(placements.commands, placements.states, component, &target);
        if !rows.is_empty() {
            ui.separator();
        }
        for command in &rows {
            placements.draw_row(ui, command, &slot, &target);
        }
    }
}

/// Draws the scene viewport's context menu: the three transform tools,
/// snapping, the two spaces and the scene grid. A row shows as active when the
/// snapshot's settings say so. A click queues a `SetGizmoTool` edit for a tool,
/// a space or snapping, and a `SetGrid` edit for the grid.
#[pill_hot_fn]
pub fn scene_context_menu(viewer: &mut EditorTabViewer<'_>, ui: &mut egui::Ui) {
    let settings = viewer.snapshot.gizmo;
    let state = toolbox::ToolboxState {
        gizmo: settings,
        grid: viewer.snapshot.grid_enabled,
        gizmos: viewer.snapshot.gizmos_enabled,
        frame: viewer.snapshot.gizmo_frame.as_deref(),
        component_tools: &viewer.snapshot.component_tools,
    };
    let pending = &mut viewer.pending;
    let element_menu = viewer.snapshot.element_menu.clone();
    let response = ui.allocate_response(ui.available_size_before_wrap(), egui::Sense::click());
    context_menu("scene", &response, &mut viewer.overlay_regions, &mut |ui| {
        // Step 0: the rows the active tool adds for the element selection,
        // before the editor's own rows: they act on the elements the menu was
        // opened over. A row queues the same `RunElementMenu` request the
        // scripted channel's `element menu` does, and the frame boundary asks
        // the tool what it means.
        if !element_menu.is_empty() {
            for entry in &element_menu {
                // The circled plus is the editor's "add something" mark, which
                // reads correctly for the tool rows a suite drives; a tool
                // that wants its own face can grow one when the menu reports
                // icons.
                if ui
                    .add(icons::IconButton::new(
                        icons::Icon::PlusCircle,
                        &entry.label,
                    ))
                    .clicked()
                {
                    pending.push_recorded(
                        vec![EditorCommand::RunElementMenu {
                            item: entry.id.clone(),
                        }],
                        format!("tool {}", entry.id),
                        None,
                    );
                    ui.close();
                }
            }
            ui.separator();
        }
        // Step 1: the three tool rows. They come from the toolbox's own list,
        // so a row here and on the panel share their icon, their active state
        // and the command a click queues. Each label comes from `menu_label`,
        // which holds the menu's own wording where it differs.
        for item in toolbox::TOOL_ROWS {
            let button = icons::IconButton::new(item.icon(&state), item.menu_label(&state))
                .active(item.active(&state));
            if ui.add(button).clicked() {
                pending.push(vec![item.command(&state)]);
                ui.close();
            }
        }
        ui.separator();
        // Step 2: the snapping row.
        let snapping = icons::IconButton::new(
            toolbox::ToolboxItem::Snap.icon(&state),
            toolbox::ToolboxItem::Snap.menu_label(&state),
        )
        .active(toolbox::ToolboxItem::Snap.active(&state));
        if ui.add(snapping).clicked() {
            pending.push(vec![toolbox::ToolboxItem::Snap.command(&state)]);
            ui.close();
        }
        // Step 3: the two space rows.
        for row in toolbox::SPACE_ROWS {
            let button =
                icons::IconButton::new(row.icon, row.menu).active(settings.space == row.space);
            if ui.add(button).clicked() {
                pending.push(vec![toolbox::space_command(row.space)]);
                ui.close();
            }
        }
        ui.separator();
        // Step 4: the scene grid's switch.
        let grid = icons::IconButton::new(
            toolbox::ToolboxItem::Grid.icon(&state),
            toolbox::ToolboxItem::Grid.menu_label(&state),
        )
        .active(toolbox::ToolboxItem::Grid.active(&state));
        if ui.add(grid).clicked() {
            pending.push(vec![toolbox::ToolboxItem::Grid.command(&state)]);
            ui.close();
        }
    });
}
