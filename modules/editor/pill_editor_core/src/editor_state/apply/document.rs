//! Applies the document command family: the scene document commands and the
//! preferences.
//!
//! # Responsibilities
//!
//! - Apply the scene document commands (new, open, save, save as), clearing
//!   the unsaved marker when one succeeds.
//! - Apply a preferences change as one read-modify-write of the preferences
//!   file.
//!
//! # Design
//!
//! One `match` over the family's own variants, called from
//! [`EditorCommand::apply_one`](super::EditorCommand); a command of another
//! family hits `unreachable!`. Every successful arm answers
//! [`CommandOutcome::plain()`], because none of these commands can be undone.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::Engine;

// Current crate
use super::CommandOutcome;
use super::*;

/// Applies one document command.
///
/// # Errors
///
/// Returns a message naming the command and the reason when:
///
/// - the preferences file cannot be read or written, a key name is empty, or
///   a dotted key's parent already holds a value that is not a JSON object;
/// - a scene cannot be imported, written or read back, or no guid can be read
///   or made for it;
/// - a save finds no open scene, or the engine has no asset directory,
///   registered asset types or `AssetManager` to write with.
///
/// # Panics
///
/// Panics when `command` belongs to another family; `EditorCommand::apply_one`
/// routes only document commands here.
pub(super) fn apply(
    engine: &mut Engine,
    command: &EditorCommand,
) -> Result<CommandOutcome, String> {
    match command {
        EditorCommand::SetPreferences { values } => {
            // Read the whole file, change only the given keys and write it
            // back, so keys another writer owns (the per-project gizmo entry)
            // survive. A blank field writes nothing, so a key that was never
            // set stays absent and its reader keeps the default.
            let directory = crate::layout::layout_directory();
            let mut document = crate::preferences::read_document(&directory)
                .map_err(|message| format!("{command:?}: {message}"))?;
            let mut written_key_count = 0usize;
            for (key, text) in values {
                if text.trim().is_empty() {
                    continue;
                }
                written_key_count += 1;
                crate::preferences::set_key(
                    &mut document,
                    key,
                    crate::preferences::parse_value(text),
                )
                .map_err(|message| format!("{command:?}: {message}"))?;
            }
            crate::preferences::write_document(&directory, &document)
                .map_err(|message| format!("{command:?}: {message}"))?;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                keys = written_key_count,
                "[editor] preferences saved"
            );
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SceneNew => {
            let removed_entity_count = crate::scene_workflow::new_scene(engine);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                entities = removed_entity_count,
                "[editor] scene new"
            );
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SceneOpen { path } => {
            crate::scene_workflow::open_scene(engine, path)
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SceneSave => {
            crate::scene_workflow::write_scene(engine, None)
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::plain())
        }

        EditorCommand::SceneSaveAs { path } => {
            let path = crate::scene_workflow::scene_file_name(path);
            crate::scene_workflow::write_scene(engine, Some(&path))
                .map_err(|error| format!("{command:?}: {error}"))?;
            Ok(CommandOutcome::plain())
        }

        other => unreachable!("{:?} is not a document command", other.family()),
    }
}
