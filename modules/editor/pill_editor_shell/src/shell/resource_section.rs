//! The Inspector's resource section: the rows of one editable world resource.
//!
//! # Responsibilities
//!
//! - [`resource_section`]: draw the resource's heading and note, then one row
//!   per editable field, through the row and scalar controls the component
//!   rows use.
//! - [`queue_edit`]: queue one field write as a recorded `SetResourceField`.
//!
//! # Design
//!
//! The snapshot builds the rows from the resource's curated field table,
//! listed in `pill_editor_core::resources`, which is the same table the write
//! path uses. The tab can therefore never show a field a write would refuse.
//! Each commit becomes one recorded command, which is what makes a resource
//! edit undoable like any other edit.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws one editable resource: the resource's label as a heading, a note that
/// every edit here is one undo step, then one row per editable field.
pub fn resource_section(
    ui: &mut egui::Ui,
    view: &ResourceView,
    pending: &mut PendingEditorCommands,
) {
    ui.heading(view.label.as_str());
    ui.label("A world resource. Every edit here is one undo step.");
    ui.separator();
    for field in &view.fields {
        row(ui, field.label.as_str(), None, &mut |ui| {
            // The shared scalar control picks the widget: a checkbox for a
            // boolean and a drag value for every number kind. An enum field
            // edits too, because the resource's table stores it as a `U32`
            // discriminant.
            if let Some(edited) = scalar_edit(ui, &field.value) {
                queue_edit(pending, view, field, edited.value);
                return;
            }
            // A kind with no control - text, an array, a list, an asset or
            // entity reference, an opaque value - shows its text form.
            ui.monospace(inspector::value_text(&field.value));
        });
    }
}

/// Queues one field write as a recorded `SetResourceField`.
pub fn queue_edit(
    pending: &mut PendingEditorCommands,
    view: &ResourceView,
    field: &ResourceFieldView,
    value: FieldValue,
) {
    let (command, label) = inspector::resource_edit(&view.name, &field.field, value);
    pending.push_recorded(
        vec![command],
        label,
        Some(inspector::resource_coalesce_key(&view.name, &field.field)),
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind the shared scalar control edits is editable in a resource,
    /// because each row hands its value to that control. A text field keeps
    /// the read-only fallback.
    #[test]
    fn every_numeric_kind_a_resource_carries_is_editable() {
        for value in [
            FieldValue::Bool(true),
            FieldValue::F32(1.0),
            FieldValue::F64(1.0),
            FieldValue::I8(1),
            FieldValue::I32(1),
            FieldValue::U16(1),
            FieldValue::U64(1),
            FieldValue::Isize(1),
        ] {
            assert_ne!(
                inspector::widget_for(&value),
                inspector::FieldWidget::ReadOnly,
                "{value:?} reaches a control, so the row routes it through `scalar_edit`"
            );
        }
        assert_eq!(
            inspector::widget_for(&FieldValue::Text("x".to_string())),
            inspector::FieldWidget::ReadOnly,
            "text keeps the painted fallback"
        );
    }
}
