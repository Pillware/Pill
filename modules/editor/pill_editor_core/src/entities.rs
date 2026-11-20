//! The Entities tab's shared actions, used by the menus, the palette, the
//! Cut command and the scripted channel. They are the recorded steps that
//! create one empty entity and delete the selected one.
//!
//! # Responsibilities
//!
//! - [`entity_create`]: one empty entity, under the `new entity` label.
//! - [`entity_delete`]: the selection clear and the destroy as one recorded
//!   step, under the `delete entity` label, so undo restores the entity and
//!   the selection that pointed at it.
//!
//! # Design
//!
//! The callers queue these commands in the shared
//! [`PendingEditorCommands`](crate::commands::PendingEditorCommands)
//! resource, and they apply at the frame boundary like every other editor
//! edit. A restore gives the entity a new handle. From then on,
//! [`EditorCommand::apply_recorded`] rewrites the rest of the batch, and
//! the inverses already collected, to that handle.

// Current crate
use crate::editor_state::EditorCommand;

/// Returns the recorded create: one empty entity, with the history label
/// `new entity`.
pub fn entity_create() -> (EditorCommand, String) {
    (
        EditorCommand::CreateEntity {
            components: Vec::new(),
        },
        "new entity".to_string(),
    )
}

/// Returns the recorded delete: clearing the selection and destroying the
/// entity, as one history step under the `delete entity` label.
///
/// `asset` is the selected asset's path. The clear keeps it, so deleting an
/// entity does not drop the selected file as well. The clear comes first on
/// purpose: undo runs a step's commands backwards, so the restore runs
/// first, gives the entity a new handle, and the selection command after it
/// is moved to that handle (see [`EditorCommand::apply_recorded`]).
pub fn entity_delete(
    entity: pill_engine::Entity,
    asset: Option<String>,
) -> (Vec<EditorCommand>, String) {
    (
        vec![
            EditorCommand::SetSelection {
                entities: Vec::new(),
                asset,
            },
            EditorCommand::DestroyEntity { entity },
        ],
        "delete entity".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_create_is_one_empty_entity_under_the_new_entity_label() {
        let (command, label) = entity_create();
        assert_eq!(
            command,
            EditorCommand::CreateEntity {
                components: Vec::new()
            }
        );
        assert_eq!(label, "new entity");
    }

    #[test]
    fn the_delete_destroys_and_clears_the_selection_as_one_step() {
        let mut engine = pill_engine::Engine::new();
        let entity = engine
            .world_mut()
            .create_entity()
            .build()
            .expect("entity builds");
        let (commands, label) = entity_delete(entity, Some("textures/x.png".to_string()));
        assert_eq!(label, "delete entity");
        assert_eq!(
            commands,
            vec![
                EditorCommand::SetSelection {
                    entities: Vec::new(),
                    asset: Some("textures/x.png".to_string())
                },
                EditorCommand::DestroyEntity { entity }
            ]
        );
    }
}
