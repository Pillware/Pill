//! Applies the selection command family.
//!
//! These commands change the editor's shared selection, and `ArmEntityPick`
//! arms the request the next scene click answers.
//!
//! # Responsibilities
//!
//! - Apply the selection edits (`SetSelection`, `SetElementSelection`,
//!   `SelectResource`, `RestoreSelection`), each answering with the inverse
//!   that puts the previous selection back.
//! - Arm the next scene click (`ArmEntityPick`) without recording anything.
//!
//! # Design
//!
//! One `match` over the family's own variants, called from
//! [`EditorCommand::apply_one`](super::EditorCommand). Every arm for a
//! selection command ends in a [`CommandOutcome`]: the inverse the history
//! records, or `plain()` for a command that cannot be undone. A command of
//! another family hits `unreachable!`.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::Engine;

// Current crate
use super::CommandOutcome;
use super::*;

// =============================================================================
// Free Functions
// =============================================================================

/// Applies one selection command.
///
/// # Errors
///
/// Returns an error message when the selection resource cannot be claimed
/// after this function inserted it, or when `ArmEntityPick` finds no
/// `EntityPickRequest` resource in the world.
///
/// # Panics
///
/// Panics when `command` belongs to another family; `EditorCommand::apply_one`
/// routes only selection commands here.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::SetSelection { entities, asset } => {
            let world = engine.world_mut();
            if world.get_resource::<EditorSelection>().is_none() {
                world.insert_resource(EditorSelection::default());
            }
            // Entities the world no longer has are dropped from the
            // selection. The asset slot is still applied.
            let mut existing_entities = Vec::with_capacity(entities.len());
            for entity in entities {
                if world.is_entity_valid(*entity) {
                    existing_entities.push(*entity);
                } else {
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        entity = %entity,
                        "[editor] selection command dropped; the entity no longer exists"
                    );
                }
            }
            let Some(selection) = world.get_resource_mut::<EditorSelection>() else {
                return Err(format!("{command:?}: the selection could not be claimed"));
            };
            // A new entity set means a new set of elements: the old ones belong
            // to an entity the pick just deselected. The restore brings both
            // slots back.
            let previous_selection_command = EditorCommand::RestoreSelection {
                entities: selection.entities().to_vec(),
                asset: selection.asset.clone(),
                resource: selection.resource.clone(),
                elements: selection.elements().clone(),
            };
            let previous_elements = selection.elements().clone();
            selection.set_entities(existing_entities);
            selection.asset = asset.clone();
            selection.clear_elements();
            notify_element_change(world, &previous_elements);
            Ok(CommandOutcome::inverse(previous_selection_command))
        }

        EditorCommand::SetElementSelection { kind, elements } => {
            let world = engine.world_mut();
            if world.get_resource::<EditorSelection>().is_none() {
                world.insert_resource(EditorSelection::default());
            }
            // Only the elements of an entity the world still has are kept, the
            // same rule the entity slot follows.
            let existing: Vec<ElementRef> = elements
                .iter()
                .copied()
                .filter(|element| world.is_entity_valid(element.entity))
                .collect();
            let Some(selection) = world.get_resource_mut::<EditorSelection>() else {
                return Err(format!("{command:?}: the selection could not be claimed"));
            };
            let previous = EditorCommand::SetElementSelection {
                kind: selection.elements().kind().to_string(),
                elements: selection.elements().elements().to_vec(),
            };
            let previous_elements = selection.elements().clone();
            // An empty list is how a click on empty space clears the slot: the
            // kind goes with the elements, so nothing reads a kind nobody
            // selected.
            if existing.is_empty() {
                selection.clear_elements();
            } else if !selection.set_elements(kind.clone(), existing) {
                return Err(format!(
                    "{command:?}: a selection cannot mix elements of more than one entity"
                ));
            }
            // The suites read this line to see what a tool selected.
            pill_core::info!(
                target: telemetry_target::EDITOR,
                kind = kind.as_str(),
                selected = selection.elements().elements().len(),
                entity = selection
                    .elements()
                    .owner()
                    .map(|entity| entity.to_string())
                    .unwrap_or_else(|| "none".to_string())
                    .as_str(),
                "[editor] elements selected"
            );
            notify_element_change(world, &previous_elements);
            Ok(CommandOutcome::inverse(previous))
        }

        EditorCommand::SelectResource { resource } => {
            let world = engine.world_mut();
            if world.get_resource::<EditorSelection>().is_none() {
                world.insert_resource(EditorSelection::default());
            }
            let Some(selection) = world.get_resource_mut::<EditorSelection>() else {
                return Err(format!("{command:?}: the selection could not be claimed"));
            };
            let previous_resource = selection.resource.clone();
            // The Inspector shows one thing at a time. Choosing a resource
            // drops the entity, asset and element selections; the
            // `RestoreSelection` inverse brings all four slots back on undo.
            // Clearing the resource (`None`) leaves the other slots alone.
            let previous_entities = selection.entities().to_vec();
            let previous_asset = selection.asset.clone();
            let previous_elements = selection.elements().clone();
            selection.resource = resource.clone();
            if resource.is_some() {
                selection.clear_entities();
                selection.asset = None;
                selection.clear_elements();
            }
            notify_element_change(world, &previous_elements);
            // The egui editor's suite matches this line, so its token and
            // fields are a contract: it checks the selection a resource row
            // makes, and the one its undo restores.
            pill_core::info!(
                target: telemetry_target::EDITOR,
                resource = resource.as_deref().unwrap_or("none"),
                "[editor] resource selected"
            );
            Ok(CommandOutcome::inverse(EditorCommand::RestoreSelection {
                entities: previous_entities,
                asset: previous_asset,
                resource: previous_resource,
                elements: previous_elements,
            }))
        }

        EditorCommand::RestoreSelection {
            entities,
            asset,
            resource,
            elements,
        } => {
            let world = engine.world_mut();
            if world.get_resource::<EditorSelection>().is_none() {
                world.insert_resource(EditorSelection::default());
            }
            let Some(selection) = world.get_resource_mut::<EditorSelection>() else {
                return Err(format!("{command:?}: the selection could not be claimed"));
            };
            let previous_selection_command = EditorCommand::RestoreSelection {
                entities: selection.entities().to_vec(),
                asset: selection.asset.clone(),
                resource: selection.resource.clone(),
                elements: selection.elements().clone(),
            };
            let previous_elements = selection.elements().clone();
            selection.set_entities(entities.clone());
            selection.asset = asset.clone();
            selection.resource = resource.clone();
            let _ =
                selection.set_elements(elements.kind().to_string(), elements.elements().to_vec());
            // The egui editor's suite matches this line as well. After the
            // undo it requires an `entity` that is not `none`, because the
            // undo has to bring the entity slot back. Clearing only the
            // resource slot is not enough.
            pill_core::info!(
                target: telemetry_target::EDITOR,
                entity = selection
                    .primary()
                    .map(|entity| entity.to_string())
                    .unwrap_or_else(|| "none".to_string())
                    .as_str(),
                resource = resource.as_deref().unwrap_or("none"),
                entities = selection.entities().len(),
                "[editor] selection restored"
            );
            notify_element_change(world, &previous_elements);
            Ok(CommandOutcome::inverse(previous_selection_command))
        }

        EditorCommand::ArmEntityPick {
            entity,
            component,
            field,
        } => {
            let world = engine.world_mut();
            let entity_pick_request = world
                .get_resource_mut::<crate::editor_state::EntityPickRequest>()
                .ok_or("the world has no entity pick request slot")?;
            entity_pick_request.target = Some(crate::editor_state::EntityPickTarget {
                entity: *entity,
                component: component.clone(),
                field: field.clone(),
            });
            pill_core::info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                component = component.as_str(),
                field = field.as_str(),
                "[editor] inspector entity pick armed"
            );
            Ok(CommandOutcome::plain())
        }

        other => unreachable!("{:?} is not a selection command", other.family()),
    }
}

/// Tells the active tool that the element selection moved.
///
/// Every arm that can change the element slot calls this with the slot as it
/// was, so a tool hears about an undo, a marquee and a scripted command the
/// same way it hears about a click: one rule, whatever the source.
fn notify_element_change(world: &pill_engine::World, previous: &ElementSelection) {
    let current = world
        .get_resource::<EditorSelection>()
        .map(|selection| selection.elements().clone())
        .unwrap_or_default();
    let before: Vec<u64> = previous
        .elements()
        .iter()
        .map(|element| element.id)
        .collect();
    let after: Vec<u64> = current
        .elements()
        .iter()
        .map(|element| element.id)
        .collect();
    if before == after && previous.kind() == current.kind() {
        return;
    }
    let Some(entity) = current.owner().or_else(|| previous.owner()) else {
        return;
    };
    let Some(registry) = world.get_resource::<crate::tools::ToolRegistry>() else {
        return;
    };
    registry.notify_element_selection_changed(entity, &before, &after, world);
}
