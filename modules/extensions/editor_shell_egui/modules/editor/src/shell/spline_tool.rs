//! The Spline Tool panel.
//!
//! # Responsibilities
//!
//! - [`spline_tool_panel`]: list the entities that carry a spline, select one,
//!   show its values, and edit them through the same recorded commands the
//!   Inspector uses.
//! - [`reseed_window`]: the re-seed confirmation, a modal the shell owns and
//!   this panel draws.
//! - [`reseed_command`]: the write the confirmation applies when the person
//!   accepts the re-seed.
//!
//! # Design
//!
//! The panel is the example a module's editor crate is written against: it
//! names no type from another crate. A spline is found by the name of its
//! component (`archetype` holds the component names joined, as the Entities
//! tab shows them), and its values arrive as text through [`PanelContext`].
//! Nothing here can therefore outlive the module that registered it.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The panel's registry id.
pub const SPLINE_PANEL_ID: &str = "shell.spline_tool";
/// The spline component's short type name: the end of the name it registers
/// under.
pub const SPLINE_COMPONENT: &str = "Spline";
/// The re-seed confirmation's window id.
pub const RESEED_WINDOW: &str = "spline.reseed";
/// How many control points a re-seed leaves in use.
pub const RESEED_POINTS: usize = 4;

/// The component field the control points live in.
pub const SPLINE_POINTS_FIELD: &str = "control_points";

/// How many control-point rows the panel draws. A spline can hold more points
/// than fit on screen; the rest stay reachable in the Inspector.
pub const POINTS_SHOWN: usize = 8;

// =============================================================================
// Types
// =============================================================================

/// The data behind the re-seed write: the entity it edits and the
/// control-point count it sets.
pub struct Reseed {
    /// The entity the spline belongs to.
    pub entity: Entity,
    /// The control-point count the write sets.
    pub value: usize,
}

impl Reseed {
    /// Turns the confirmation into the recorded panel command that applies it.
    pub fn into_command(self) -> PanelCommand {
        PanelCommand::SetField {
            entity: self.entity,
            component: SPLINE_COMPONENT.to_string(),
            field: "control_point_count".to_string(),
            value: self.value.to_string(),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the panel: the docked tab and the re-seed modal it opens.
pub fn spline_tool_panel(ui: &mut egui::Ui, context: &mut PanelContext<'_>) {
    // Step 1: the re-seed confirmation draws its own body and nothing else.
    if context.window == Some(RESEED_WINDOW) {
        reseed_body(ui, context);
        return;
    }
    // Step 2: list the entities that carry a spline; a row selects one.
    ui.heading("Spline Tool");
    ui.label("Edits a spline through the editor's recorded commands.");
    ui.separator();
    let spline_rows: Vec<PanelEntityRow> = context
        .entities
        .iter()
        .filter(|row| row.archetype.contains(SPLINE_COMPONENT))
        .cloned()
        .collect();
    if spline_rows.is_empty() {
        ui.label("No entity carries a spline.");
        return;
    }
    for row in &spline_rows {
        let selected = context.selected_entity == Some(row.entity);
        if ui
            .selectable_label(selected, format!("{} ({})", row.display, row.entity))
            .clicked()
        {
            context
                .commands
                .push(PanelCommand::SelectEntity(row.entity));
        }
    }
    let Some(detail) = context.detail else {
        ui.separator();
        ui.label("Select a spline to edit it.");
        return;
    };
    let Some(component) = detail
        .components
        .iter()
        .find(|component| component.component == SPLINE_COMPONENT)
    else {
        ui.separator();
        ui.label("The selected entity carries no spline.");
        return;
    };
    ui.separator();
    // Step 3: draw the control points. The engine expands a struct array into
    // one row per leaf (`control_points[0].x`), so a point arrives as three
    // ordinary fields and the same recorded write path serves it. No struct
    // value has to cross this crate's boundary.
    let point_count = spline_point_count(component);
    if point_count > 0 {
        ui.label(format!("{point_count} control points"));
        for point_index in 0..point_count.min(POINTS_SHOWN) {
            row(ui, &format!("point {point_index}"), None, &mut |ui| {
                for axis_index in 0..3 {
                    let Some(field) = point_leaf(component, point_index, axis_index) else {
                        continue;
                    };
                    // The row holds its value as text, in the Inspector's form.
                    // A leaf that is not a float has no drag value.
                    let Ok(mut coordinate) = field.value.parse::<f32>() else {
                        continue;
                    };
                    let field_name = field.name.clone();
                    if ui
                        .add(egui::DragValue::new(&mut coordinate).speed(0.05))
                        .changed()
                    {
                        context.commands.push(point_write(
                            detail.entity,
                            field_name.as_str(),
                            coordinate,
                        ));
                    }
                }
            });
        }
    }
    // Step 4: offer the re-seed, then apply the answer the modal took. The
    // shell hands an answer over for one draw only.
    if ui.button("Re-seed...").clicked() {
        context
            .commands
            .push(PanelCommand::OpenWindow(reseed_window()));
    }
    if let Some(answer) = context.answer {
        if answer.window == RESEED_WINDOW && answer.accepted {
            context
                .commands
                .push(reseed_command(detail.entity).into_command());
        }
    }
}

/// Returns how many control points the selected spline's rows describe.
///
/// The count comes from the rows themselves: every element of the array is
/// named `control_points[i].<axis>`, so the highest index seen plus one is the
/// number the panel offers.
pub fn spline_point_count(component: &PanelComponentValues) -> usize {
    let mut point_count = 0;
    for field in &component.fields {
        if let Some(element_index) = field_element_index(field.name.as_str()) {
            point_count = point_count.max(element_index + 1);
        }
    }
    point_count
}

/// Returns the field row of one control point's axis, where 0 is x, 1 is y and
/// 2 is z, when the component describes it.
pub fn point_leaf(
    component: &PanelComponentValues,
    point_index: usize,
    axis_index: usize,
) -> Option<&PanelFieldValue> {
    let axis_suffix = ["x", "y", "z"].get(axis_index)?;
    component
        .fields
        .iter()
        .find(|field| field.name == format!("{SPLINE_POINTS_FIELD}[{point_index}].{axis_suffix}"))
}

/// Builds the write one control-point row queues when its drag value changes.
///
/// `field_name` is the row's name (`control_points[0].x`), so the write is an
/// ordinary recorded scalar edit - the same one an Inspector row queues.
pub fn point_write(entity: Entity, field_name: &str, coordinate: f32) -> PanelCommand {
    PanelCommand::SetField {
        entity,
        component: SPLINE_COMPONENT.to_string(),
        field: field_name.to_string(),
        value: format!("{coordinate}"),
    }
}

/// Returns the element index a `control_points[i]` row names:
/// `control_points[3].y` gives 3.
///
/// Public because the function inventory names it by path, and that inventory
/// is included at this crate's root, which cannot name a private function.
pub fn field_element_index(field_name: &str) -> Option<usize> {
    let element_text = field_name
        .strip_prefix(SPLINE_POINTS_FIELD)?
        .strip_prefix('[')?;
    let (element_index, _) = element_text.split_once(']')?;
    element_index.parse().ok()
}

/// Draws the re-seed confirmation's body.
///
/// The shell draws the window's buttons and reads Enter and Escape. This body
/// states the question and the entity it applies to.
pub fn reseed_body(ui: &mut egui::Ui, context: &mut PanelContext<'_>) {
    ui.label(format!(
        "Re-seed the spline to {} control points?",
        RESEED_POINTS
    ));
    match context.selected_entity {
        Some(entity) => ui.monospace(format!("entity {entity}")),
        None => ui.label("no spline selected"),
    };
}

/// Builds the request for the re-seed window. It is a modal, so the editor
/// waits for an answer.
pub fn reseed_window() -> PanelWindowRequest {
    PanelWindowRequest {
        id: RESEED_WINDOW.to_string(),
        title: "Re-seed spline".to_string(),
        width: 360.0,
        height: 140.0,
        modal: true,
    }
}

/// Builds the re-seed write for one entity.
pub fn reseed_command(entity: Entity) -> Reseed {
    Reseed {
        entity,
        value: RESEED_POINTS,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the component rows a panel receives for a two-point spline.
    fn points_component() -> PanelComponentValues {
        let mut fields = vec![PanelFieldValue {
            name: "control_point_count".to_string(),
            value: "2".to_string(),
        }];
        for index in 0..2 {
            for axis in ["x", "y", "z"] {
                fields.push(PanelFieldValue {
                    name: format!("control_points[{index}].{axis}"),
                    value: format!("{}.{}", index + 1, axis.len()),
                });
            }
        }
        PanelComponentValues {
            component: SPLINE_COMPONENT.to_string(),
            fields,
        }
    }

    /// Each point gets one row, and each row reads that point's own field
    /// leaves.
    #[test]
    fn control_points_become_one_row_per_point() {
        let component = points_component();
        assert_eq!(spline_point_count(&component), 2);
        let y_of_second = point_leaf(&component, 1, 1).expect("second point's y leaf");
        assert_eq!(y_of_second.name, "control_points[1].y");
        // A point the component does not describe is simply absent.
        assert!(point_leaf(&component, 2, 0).is_none());
    }

    /// A row's write is the recorded scalar edit an Inspector row queues.
    #[test]
    fn a_control_point_row_queues_a_recorded_field_write() {
        let entity = Entity::from_raw(7, 0);
        match point_write(entity, "control_points[0].x", 2.5) {
            PanelCommand::SetField {
                entity: written,
                component,
                field,
                value,
            } => {
                assert_eq!(written, entity);
                assert_eq!(component, SPLINE_COMPONENT);
                assert_eq!(field, "control_points[0].x");
                assert_eq!(value, "2.5");
            }
            other => panic!("a control-point row queued {other:?}"),
        }
    }

    /// Checks the element index the panel counts rows from.
    #[test]
    fn element_indices_come_from_the_expanded_leaf_names() {
        assert_eq!(field_element_index("control_points[3].y"), Some(3));
        assert_eq!(field_element_index("control_point_count"), None);
        assert_eq!(field_element_index("points[0].x"), None);
    }
}
