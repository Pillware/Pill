//! The Assets pipeline: the state the create and rename dialogs edit, and the
//! recorded file batches the menus, the dialogs and the scripted channel
//! queue.
//!
//! # Responsibilities
//!
//! - [`AssetDialog`]: the state the create and rename dialogs edit while one
//!   is open, owned by the session so it survives shell reloads.
//! - The open, submit and cancel helpers every entry point shares - the
//!   menus, the dialog windows and the scripted channel - so each action
//!   takes one path and logs one token set.
//! - The enablement predicates [`can_delete`] and [`can_write_meta`], and the
//!   acting functions [`delete_selection`] and [`write_meta_selection`] that
//!   ask them the same question.
//! - The file actions that queue one recorded [`EditorCommand`] batch each:
//!   [`queue_delete`], [`queue_write_meta`] and [`queue_move`].
//! - [`reveal_target`] and [`reveal_target_in`]: the absolute path a "show in
//!   the file manager" request opens, refused when it would leave the root.
//!
//! # Design
//!
//! The dialogs are egui windows drawn by the shell. File edits are
//! [`EditorCommand`](crate::EditorCommand)'s asset commands: this module
//! queues them in [`PendingEditorCommands`], and the frame boundary applies
//! them and records them in the undo history. Nothing here changes the world
//! directly. A reveal is the only action that is not a command: it asks the
//! executable to open a folder, so the shell puts the path in the same queue
//! as a plain request.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::browser;
use crate::commands::PendingEditorCommands;
use crate::editor_state::{AssetListing, EditorCommand, EditorSelection, StandaloneTypeInfo};

// =============================================================================
// Types
// =============================================================================

/// The asset dialog that is open, if any.
///
/// The create variant holds the type, folder and name the dialog will write,
/// plus the answer to a file of that name already existing. The rename
/// variant holds the current path and the typed name. The state lives in
/// [`crate::session::EguiShellState::asset_dialog`], so a shell reload keeps
/// an open dialog exactly as it was.
#[derive(Clone, Debug, PartialEq)]
pub enum AssetDialog {
    /// The create dialog: a standalone type, a name and the destination
    /// folder (relative to the asset directory; empty for its root).
    Create {
        /// The destination folder, `/`-separated; empty for the root.
        folder: String,
        /// The chosen standalone type's registered name; empty until one is
        /// chosen.
        type_name: String,
        /// The typed name; the type's naming rules add the extension.
        name: String,
        /// What confirming writes.
        kind: CreateKind,
        /// The answer to a file of that name already existing: `Some(true)`
        /// keeps that file's guid, `Some(false)` writes a fresh one, and
        /// `None` means the person has not answered yet.
        overwrite: Option<bool>,
    },
    /// The rename dialog: the path being renamed and the typed new name.
    Rename {
        /// The asset's current path, as the listing reported it: relative to
        /// the project's asset directory for a project row. The recorded move
        /// resolves the new path below that directory.
        from: String,
        /// The typed new name; the file keeps its extension.
        name: String,
    },
}

/// What a create dialog writes when it is confirmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreateKind {
    /// A standalone asset's default document.
    Asset,
    /// An empty scene: the world is cleared and its empty document written.
    EmptyScene,
    /// The current world, written as a new scene (Save Scene As).
    SaveWorld,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the selected asset's listing row, or `None` when no asset is
/// selected or the snapshot no longer lists it.
pub fn selected_row<'a>(
    selection: &EditorSelection,
    assets: &'a [AssetListing],
) -> Option<&'a AssetListing> {
    let key = selection.asset.as_deref()?;
    assets.iter().find(|row| row.key() == key)
}

/// Returns whether `key` names an extension's resource tree: the extensions
/// root itself, one module's root row, or anything below them.
///
/// An extension's resources are listed and inspected, never written. The
/// create dialog and a folder's menu ask this before they offer to create
/// something there; the file actions ask the row's own `read_only` flag for
/// the same reason. Everything under the root counts as read-only whatever
/// rows the listing happens to hold, so a module that has just loaded or
/// unloaded still never offers a write.
pub fn is_read_only_key(assets: &[AssetListing], key: &str) -> bool {
    if crate::browser::root_of(key).read_only {
        return true;
    }
    assets
        .iter()
        .filter(|row| row.read_only)
        .map(AssetListing::key)
        .any(|read_only_key| read_only_key == key || key.starts_with(&format!("{read_only_key}/")))
}

/// Opens the create dialog with the first registered standalone type
/// selected.
///
/// The target folder follows the selection: a selected folder is the target, a
/// selected file's folder is, and with nothing selected it is the root. A
/// target inside a module's read-only resource tree is refused with a warning,
/// and the dialog stays closed.
pub fn open_create(
    dialog: &mut Option<AssetDialog>,
    selection: &EditorSelection,
    assets: &[AssetListing],
    types: &[StandaloneTypeInfo],
) {
    // Step 1: work out the target folder from the selection.
    let folder = match assets
        .iter()
        .find(|row| Some(row.key().as_str()) == selection.asset.as_deref())
    {
        Some(row) if browser::is_folder(row) => row.key(),
        Some(row) => {
            let key = row.key();
            key.rsplit_once('/')
                .map(|(parent, _)| parent.to_string())
                .unwrap_or_default()
        }
        None => String::new(),
    };
    // Step 2: refuse a target inside a module's resources, because the create
    // would be refused when it runs.
    if is_read_only_key(assets, &folder) {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            folder,
            "[editor] create refused: a module's resources are read-only"
        );
        return;
    }
    // Step 3: seed the dialog and report that it opened.
    let type_name = types
        .first()
        .map(|info| info.type_name.clone())
        .unwrap_or_default();
    *dialog = Some(AssetDialog::Create {
        folder: folder.clone(),
        type_name,
        name: String::new(),
        kind: CreateKind::Asset,
        overwrite: None,
    });
    pill_core::info!(
        target: telemetry_target::EDITOR,
        dialog = "create",
        folder = folder.as_str(),
        "[editor] asset dialog opened"
    );
}

/// Opens the create dialog for a scene, with the scene type already chosen.
///
/// It is the same window a material or a pass is created in, and the target
/// folder follows the selection the same way. `kind` says what confirming
/// writes: an empty scene (the Create menu's row) or the current world (Save
/// Scene As). Neither touches the world until the queued step runs, so
/// cancelling leaves the open scene as it was.
///
/// A project with no scene type registered gets a warning and no dialog.
pub fn open_create_scene(
    dialog: &mut Option<AssetDialog>,
    selection: &EditorSelection,
    assets: &[AssetListing],
    types: &[StandaloneTypeInfo],
    kind: CreateKind,
) {
    let Some(scene_type) = types
        .iter()
        .find(|info| info.extension.eq_ignore_ascii_case("scene"))
    else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] no scene type is registered"
        );
        return;
    };
    open_create(dialog, selection, assets, types);
    if let Some(AssetDialog::Create {
        type_name,
        kind: chosen_kind,
        ..
    }) = dialog.as_mut()
    {
        *type_name = scene_type.type_name.clone();
        *chosen_kind = kind;
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        dialog = "create scene",
        "[editor] asset dialog opened"
    );
}

/// Returns the path a create would write when a file is already there, so the
/// dialog can ask before it replaces one.
///
/// `None` when the dialog is not a create, when its type is missing, when the
/// name is missing or fails the type's naming rules, or when nothing is in the
/// way. Only the project's own tree is compared, because that is where the
/// create writes.
pub fn create_collision(
    dialog: &AssetDialog,
    types: &[StandaloneTypeInfo],
    assets: &[AssetListing],
) -> Option<String> {
    let AssetDialog::Create {
        folder,
        type_name,
        name,
        ..
    } = dialog
    else {
        return None;
    };
    if name.trim().is_empty() {
        return None;
    }
    let extension = types
        .iter()
        .find(|info| info.type_name == *type_name)
        .map(|info| info.extension.clone())?;
    let file_name = pill_engine::asset_metadata::standalone_file_name(name, &extension).ok()?;
    let path = if folder.is_empty() {
        file_name
    } else {
        format!("{folder}/{file_name}")
    };
    assets
        .iter()
        .any(|row| row.path == path && row.root.is_empty())
        .then_some(path)
}

/// Opens the rename dialog for the selected file, with the file's stem already
/// in the name field.
///
/// A folder, or a selection that names no file, gets a warning and no dialog.
pub fn open_rename(
    dialog: &mut Option<AssetDialog>,
    selection: &EditorSelection,
    assets: &[AssetListing],
) {
    let Some(row) = selected_row(selection, assets).filter(|row| !browser::is_folder(row)) else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] rename asset needs a selected file"
        );
        return;
    };
    let file_name = row.path.rsplit('/').next().unwrap_or(&row.path);
    let stem = file_name.split('.').next().unwrap_or(file_name);
    *dialog = Some(AssetDialog::Rename {
        from: row.path.clone(),
        name: stem.to_string(),
    });
    pill_core::info!(
        target: telemetry_target::EDITOR,
        dialog = "rename",
        from = row.path.as_str(),
        "[editor] asset dialog opened"
    );
}

/// Runs the create dialog's submit button: queues the recorded step and closes
/// the dialog.
///
/// A missing type or name warns and leaves the dialog open. What the step is
/// depends on what the dialog was opened for: a standalone asset's default
/// document, a new empty scene, or the current world saved as a scene. Every
/// kind queues commands, so the world and the files change only when they run.
pub fn submit_create(
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
) -> bool {
    // Step 1: the dialog must exist and name a type and a file.
    let Some(AssetDialog::Create {
        folder,
        type_name,
        name,
        kind,
        overwrite,
    }) = dialog.as_ref()
    else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] create asset without its dialog"
        );
        return false;
    };
    if type_name.is_empty() || name.trim().is_empty() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] create asset needs a type and a name"
        );
        return false;
    }
    // Step 2: queue the recorded step the dialog was opened for.
    let folder = folder.clone();
    let type_name = type_name.clone();
    let name = name.clone();
    let kind = *kind;
    let overwrite = *overwrite;
    if kind == CreateKind::Asset {
        pending.push_recorded(
            vec![EditorCommand::CreateStandaloneAsset {
                type_name: type_name.clone(),
                folder,
                name: name.clone(),
                overwrite,
            }],
            format!("create asset {name}"),
            None,
        );
    } else {
        let path = scene_path(&folder, &name);
        let (commands, label) = match kind {
            CreateKind::EmptyScene => (
                vec![EditorCommand::SceneNew, EditorCommand::SceneSaveAs { path }],
                format!("create scene {name}"),
            ),
            CreateKind::SaveWorld => (
                vec![EditorCommand::SceneSaveAs { path }],
                format!("save scene as {name}"),
            ),
            CreateKind::Asset => unreachable!("handled above"),
        };
        pending.push_recorded(commands, label, None);
    }
    // Step 3: report the command and close the dialog.
    pill_core::info!(
        target: telemetry_target::EDITOR,
        type_name = type_name.as_str(),
        name = name.as_str(),
        "[editor] asset create queued"
    );
    close_dialog(dialog, "create");
    true
}

/// Returns the path a scene create writes: the dialog's folder plus the typed
/// name with the scene extension.
fn scene_path(folder: &str, name: &str) -> String {
    let file_name = crate::scene_workflow::scene_file_name(name);
    if folder.is_empty() {
        file_name
    } else {
        format!("{folder}/{file_name}")
    }
}

/// Runs the rename dialog's submit button: builds the new path from the typed
/// name, queues the recorded move and closes the dialog.
///
/// The extension stays with the file unless the typed name already ends with
/// it. An empty name, or one containing a path separator, warns and leaves the
/// dialog open.
pub fn submit_rename(
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
) -> bool {
    // Step 1: the dialog must exist and name something.
    let Some(AssetDialog::Rename { from, name }) = dialog.as_ref() else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] rename asset without its dialog"
        );
        return false;
    };
    let from = from.clone();
    let typed_name = name.trim().to_string();
    if from.is_empty() || typed_name.is_empty() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] rename asset needs a name"
        );
        return false;
    }
    if typed_name.contains(['/', '\\']) {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            name = typed_name.as_str(),
            "[editor] rename asset refuses a path separator"
        );
        return false;
    }
    // Step 2: work out the new file name and the path it moves to.
    let file_name = from.rsplit('/').next().unwrap_or(&from);
    let extension = file_name
        .split_once('.')
        .map(|(_, extension)| extension.to_owned());
    let new_file_name = match &extension {
        Some(extension)
            if !typed_name
                .to_ascii_lowercase()
                .ends_with(&extension.to_ascii_lowercase()) =>
        {
            format!("{typed_name}.{extension}")
        }
        _ => typed_name.clone(),
    };
    let folder = from
        .rsplit_once('/')
        .map(|(folder, _)| folder.to_string())
        .unwrap_or_default();
    let to = if folder.is_empty() {
        new_file_name.clone()
    } else {
        format!("{folder}/{new_file_name}")
    };
    // Step 3: queue the move, report it and close the dialog.
    pending.push_recorded(
        vec![EditorCommand::MoveAssetFile {
            from: from.clone(),
            to: to.clone(),
        }],
        format!("rename asset {new_file_name}"),
        None,
    );
    pill_core::info!(
        target: telemetry_target::EDITOR,
        from = from.as_str(),
        to = to.as_str(),
        "[editor] asset rename queued"
    );
    close_dialog(dialog, "rename");
    true
}

/// Closes the open dialog, create or rename, and returns whether one was open.
pub fn cancel(dialog: &mut Option<AssetDialog>) -> bool {
    let Some(open_dialog) = dialog.as_ref() else {
        return false;
    };
    let dialog_kind = match open_dialog {
        AssetDialog::Create { .. } => "create",
        AssetDialog::Rename { .. } => "rename",
    };
    *dialog = None;
    pill_core::info!(
        target: telemetry_target::EDITOR,
        dialog = dialog_kind,
        "[editor] asset dialog closed"
    );
    true
}

/// Queues the recorded delete of `path`: the file and its `.meta` sidecar.
pub fn queue_delete(pending: &mut PendingEditorCommands, path: &str) {
    pending.push_recorded(
        vec![EditorCommand::DeleteAssetFile {
            path: path.to_string(),
        }],
        format!("delete asset {path}"),
        None,
    );
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path,
        "[editor] asset delete queued"
    );
}

/// Queues the recorded `.meta` write for `path`, which gives a sourced file the
/// sidecar it is missing.
pub fn queue_write_meta(pending: &mut PendingEditorCommands, path: &str) {
    pending.push_recorded(
        vec![EditorCommand::WriteAssetMeta {
            path: path.to_string(),
        }],
        format!("write meta {path}"),
        None,
    );
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path,
        "[editor] asset meta queued"
    );
}

/// Queues the recorded move of `from` to `to`.
///
/// The `.meta` sidecar travels with the file, so the loaded asset keeps its
/// handle and guid.
pub fn queue_move(pending: &mut PendingEditorCommands, from: &str, to: &str) {
    pending.push_recorded(
        vec![EditorCommand::MoveAssetFile {
            from: from.to_string(),
            to: to.to_string(),
        }],
        format!("move asset {from}"),
        None,
    );
    pill_core::info!(
        target: telemetry_target::EDITOR,
        from,
        to,
        "[editor] asset move queued"
    );
}

/// Returns whether the row is a file a `.meta` sidecar can be written for,
/// ignoring whether its tree is read-only.
///
/// A folder has no sidecar, a standalone asset carries its own header, and a
/// file that already has a sidecar needs no second one. This is the per-row
/// half of [`can_write_meta`] and of [`write_meta_selection`]'s refusal.
fn takes_meta_write(row: &AssetListing) -> bool {
    !browser::is_folder(row) && !row.standalone && !row.has_meta
}

/// Returns whether the File menu's Write Asset Meta row can act on the
/// selection: a sourced file without a sidecar, in a tree this editor may
/// write.
///
/// The menu row and [`write_meta_selection`] read this one answer, so a row
/// the menu enables is never a write that function refuses.
pub fn can_write_meta(selection: &EditorSelection, assets: &[AssetListing]) -> bool {
    selected_row(selection, assets).is_some_and(|row| takes_meta_write(row) && !row.read_only)
}

/// Returns whether a file tile's Delete Asset row can act on the selection: a
/// file, in a tree this editor may write.
pub fn can_delete(selection: &EditorSelection, assets: &[AssetListing]) -> bool {
    selected_row(selection, assets).is_some_and(|row| !browser::is_folder(row) && !row.read_only)
}

/// Runs Delete Asset: queues the recorded delete of the selected file and
/// returns whether it queued anything.
///
/// A selection that names no file is refused with a warning, and so is a file
/// in a module's read-only tree. The file tile's menu and the scripted
/// `toolbar delete` line both land here, so one rule covers them.
pub fn delete_selection(
    pending: &mut PendingEditorCommands,
    selection: &EditorSelection,
    assets: &[AssetListing],
) -> bool {
    let Some(row) = selected_row(selection, assets).filter(|row| !browser::is_folder(row)) else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] delete asset needs a selected file"
        );
        return false;
    };
    if !can_delete(selection, assets) {
        // The row is a file, so a read-only tree is the only refusal left.
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            path = row.key().as_str(),
            "[editor] delete refused: a module's resources are read-only"
        );
        return false;
    }
    queue_delete(pending, &row.key());
    true
}

/// Runs Write Asset Meta: queues the recorded sidecar write for the selected
/// sourced file and returns whether it queued anything.
///
/// The file must have no sidecar and no standalone header yet, because either
/// one already carries a guid. A selection that names no such file is refused
/// with a warning, and so is a file in a module's read-only tree. The File
/// menu's row and the scripted `toolbar meta` line both land here.
pub fn write_meta_selection(
    pending: &mut PendingEditorCommands,
    selection: &EditorSelection,
    assets: &[AssetListing],
) -> bool {
    let Some(row) = selected_row(selection, assets).filter(|row| takes_meta_write(row)) else {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "[editor] write asset meta needs a sourced file without one"
        );
        return false;
    };
    if !can_write_meta(selection, assets) {
        // The row takes a sidecar, so a read-only tree is the only refusal
        // left.
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            path = row.key().as_str(),
            "[editor] meta refused: a module's resources are read-only"
        );
        return false;
    }
    queue_write_meta(pending, &row.key());
    true
}

/// Returns where a file dragged onto `folder` goes: its own name below the
/// folder's path.
///
/// `None` when the file already lives there, so a drop that changes nothing
/// queues no command.
pub fn drop_destination(from: &str, folder: &str) -> Option<String> {
    let file_name = from.rsplit('/').next().unwrap_or(from);
    let to = if folder.is_empty() {
        file_name.to_string()
    } else {
        format!("{folder}/{file_name}")
    };
    (to != from).then_some(to)
}

/// Returns the absolute path a reveal opens, for a key that may name an
/// extension's resource.
///
/// A key under the extensions root (`extensions/<module>/<path>`) resolves
/// below that module's own directory, because a module's resources do not live
/// under the project's asset directory. Every other key resolves the way
/// [`reveal_target`] does. A module that is no longer loaded is refused with a
/// message, so a stale row never opens a folder of some other tree.
///
/// # Errors
///
/// Returns a message naming the refusal when the module is not loaded, when the
/// path is absolute or climbs out of the module's roots, or when no root holds
/// the file. The executable logs the message instead of opening anything.
pub fn reveal_target_in(
    asset_root: &Path,
    module_roots: &[(String, PathBuf)],
    path: &str,
) -> Result<PathBuf, String> {
    // Step 1: a key outside the extensions root is a project path.
    let relative = path.trim().replace('\\', "/");
    let extensions_prefix = format!("{}/", crate::editor_state::EXTENSIONS_ROOT);
    let Some(rest) = relative.strip_prefix(extensions_prefix.as_str()) else {
        return reveal_target(asset_root, path);
    };
    let (module, below) = rest.split_once('/').unwrap_or((rest, ""));
    // Step 2: collect the roots of the module the key names. A module may
    // declare several (its `res`, its shader sources, the images it ships),
    // and the listing merges them under the module's name, so the roots are
    // searched for the one that holds the file.
    let directories: Vec<&PathBuf> = module_roots
        .iter()
        .filter(|(name, _)| name == module)
        .map(|(_, directory)| directory)
        .collect();
    if directories.is_empty() {
        return Err(format!("`{path}` names no loaded extension"));
    }
    // Step 3: refuse a path that leaves the roots, then return the root that
    // holds the file.
    if below.starts_with('/') || Path::new(below).is_absolute() {
        return Err(format!("`{path}` is not a path below the extension's root"));
    }
    if below.split('/').any(|segment| segment == "..") {
        return Err(format!("`{path}` climbs out of the extension's root"));
    }
    if below.is_empty() {
        return Ok(directories[0].clone());
    }
    for directory in directories {
        let target = directory.join(below);
        if target.exists() {
            return Ok(target);
        }
    }
    Err(format!("`{path}` is not on disk"))
}

/// Returns the absolute path a reveal opens, for a project-relative asset
/// path.
///
/// The empty path is the asset root itself, which is the Assets panel's open
/// folder at the top; a nested path is joined below it. A path that climbs out
/// of the root, or one that names nothing on disk, is refused with a message
/// instead of being opened, so a stale row never makes the OS open something
/// else.
///
/// # Errors
///
/// Returns a message naming the refusal when the path is absolute, climbs out
/// of the asset directory, or is not on disk. The executable logs the message
/// instead of opening anything.
pub fn reveal_target(asset_root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = path.trim().replace('\\', "/");
    if relative.starts_with('/') || Path::new(&relative).is_absolute() {
        return Err(format!("`{path}` is not a path below the asset directory"));
    }
    if relative.split('/').any(|segment| segment == "..") {
        return Err(format!("`{path}` climbs out of the asset directory"));
    }
    let target = if relative.is_empty() {
        asset_root.to_path_buf()
    } else {
        asset_root.join(&relative)
    };
    if !target.exists() {
        return Err(format!("`{path}` is not on disk"));
    }
    Ok(target)
}

/// Closes the dialog and logs it under `dialog_kind`.
fn close_dialog(dialog: &mut Option<AssetDialog>, dialog_kind: &str) {
    *dialog = None;
    pill_core::info!(
        target: telemetry_target::EDITOR,
        dialog = dialog_kind,
        "[editor] asset dialog closed"
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::AssetKind;
    use crate::editor_state::EditorSelection;
    use crate::editor_state::EXTENSIONS_ROOT;

    /// A scratch folder of this test's own, with one file in it.
    fn scratch_folder(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "pill-reveal-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(directory.join("models")).unwrap();
        std::fs::write(directory.join("models/cube.obj"), b"o cube").unwrap();
        directory
    }

    /// A reveal resolves a path below the asset root, refuses a path that
    /// leaves it, and refuses a missing file instead of opening its parent
    /// folder.
    #[test]
    fn a_reveal_resolves_below_the_asset_root_and_refuses_escapes() {
        let root = scratch_folder("targets");
        // The empty path is the root itself: the panel's open folder at the top.
        assert_eq!(reveal_target(&root, "").unwrap(), root);
        // A nested file and folder both resolve.
        assert_eq!(
            reveal_target(&root, "models/cube.obj").unwrap(),
            root.join("models/cube.obj")
        );
        assert_eq!(reveal_target(&root, "models").unwrap(), root.join("models"));
        // Backslashes read as separators, the way the panels may hand them over.
        assert_eq!(
            reveal_target(&root, "models\\cube.obj").unwrap(),
            root.join("models/cube.obj")
        );
        // Escapes, absolute paths and missing files are all refusals.
        assert!(reveal_target(&root, "../outside").is_err());
        assert!(reveal_target(&root, "models/../../outside").is_err());
        assert!(reveal_target(&root, "/etc").is_err());
        assert!(reveal_target(&root, "models/missing.obj").is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// A reveal of an extension's resource finds the module root that holds it
    /// (a module may declare several), and refuses a module that is no longer
    /// loaded.
    #[test]
    fn an_extension_reveal_resolves_below_its_module_root() {
        let project = scratch_folder("project");
        let first = scratch_folder("extension_res");
        let second = scratch_folder("extension_sources");
        std::fs::create_dir_all(second.join("shaders")).unwrap();
        std::fs::write(second.join("shaders/a.hlsl"), b"// shader").unwrap();
        let roots = vec![
            ("pill_demo".to_string(), first.clone()),
            ("pill_demo".to_string(), second.clone()),
        ];

        // The second root of the module is the one that holds the shader.
        assert_eq!(
            reveal_target_in(&project, &roots, "extensions/pill_demo/shaders/a.hlsl").unwrap(),
            second.join("shaders/a.hlsl")
        );
        // A file below the first root resolves there instead.
        assert_eq!(
            reveal_target_in(&project, &roots, "extensions/pill_demo/models/cube.obj").unwrap(),
            first.join("models/cube.obj")
        );
        // The module's own row opens its first root.
        assert_eq!(
            reveal_target_in(&project, &roots, "extensions/pill_demo").unwrap(),
            first
        );
        // A project path still resolves below the project's asset directory.
        assert_eq!(
            reveal_target_in(&project, &roots, "models/cube.obj").unwrap(),
            project.join("models/cube.obj")
        );
        // An unloaded module, an escape and a missing file are refusals.
        let unloaded_module_reveal =
            reveal_target_in(&project, &roots, "extensions/pill_gone/a.hlsl");
        assert!(unloaded_module_reveal.is_err());
        assert!(reveal_target_in(&project, &roots, "extensions/pill_demo/../../outside").is_err());
        assert!(reveal_target_in(&project, &roots, "extensions/pill_demo/missing.hlsl").is_err());
        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    fn listing(path: &str, kind: &str, standalone: bool, has_meta: bool) -> AssetListing {
        AssetListing {
            root: String::new(),
            path: path.to_string(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone,
            has_meta,
            guid: None,
        }
    }

    /// A row of a module's read-only resource tree.
    fn module_row(root: &str, path: &str, kind: &str) -> AssetListing {
        AssetListing {
            root: root.to_string(),
            path: path.to_string(),
            read_only: true,
            kind: AssetKind::parse(kind),
            standalone: kind == "pill_rendering::Material",
            has_meta: kind == "pill_rendering::Material",
            guid: None,
        }
    }

    /// The enablement predicates and the acting functions answer the same
    /// question for the same rows: a project file with a sidecar, a sourced
    /// file without one, a module's read-only file, and a folder.
    #[test]
    fn the_asset_predicates_agree_with_the_actions_they_gate() {
        let rows = vec![
            listing("shaders/lit.hlsl", "Shader", false, true),
            listing("shaders/plain.hlsl", "Shader", false, false),
            module_row(
                "pill_rendering",
                "materials/pill.pbr.material",
                "pill_rendering::Material",
            ),
            listing("shaders", "folder", false, false),
        ];
        let select = |path: &str| {
            let mut selection = EditorSelection::default();
            selection.asset = Some(path.to_string());
            selection
        };
        let mut pending = PendingEditorCommands::default();
        let sidecar = select("shaders/lit.hlsl");
        assert!(
            !can_write_meta(&sidecar, &rows),
            "a sidecar is already there"
        );
        assert!(can_delete(&sidecar, &rows));
        let sourced = select("shaders/plain.hlsl");
        assert!(can_write_meta(&sourced, &rows));
        let module = select("extensions/pill_rendering/materials/pill.pbr.material");
        assert!(
            !can_write_meta(&module, &rows),
            "a module's resources are read-only, so the row must not offer a write"
        );
        assert!(!can_delete(&module, &rows));
        let folder = select("shaders");
        assert!(!can_write_meta(&folder, &rows));
        assert!(!can_delete(&folder, &rows));

        // The actions agree with every answer above.
        assert!(delete_selection(&mut pending, &sidecar, &rows));
        assert!(!write_meta_selection(&mut pending, &sidecar, &rows));
        assert!(write_meta_selection(&mut pending, &sourced, &rows));
        assert!(!delete_selection(&mut pending, &module, &rows));
        assert!(!write_meta_selection(&mut pending, &module, &rows));
        assert!(!delete_selection(&mut pending, &folder, &rows));
        assert!(!write_meta_selection(&mut pending, &folder, &rows));
    }

    /// Every asset action refuses a folder row, and refuses nothing else just
    /// for being a folder: a file whose type happens to spell `Folder` is a
    /// file. The actions and the panels share one predicate, so these answers
    /// all come from one rule.
    #[test]
    fn every_asset_action_refuses_a_folder_row() {
        let assets = vec![
            listing("materials", "folder", false, false),
            listing("materials/odd.folder", "demo::Folder", false, false),
        ];
        let select = |path: &str| {
            let mut selection = EditorSelection::default();
            selection.asset = Some(path.to_string());
            selection
        };
        let folder = select("materials");
        let folder_typed_file = select("materials/odd.folder");
        let mut dialog = None;
        let mut pending = PendingEditorCommands::default();

        open_rename(&mut dialog, &folder, &assets);
        assert!(dialog.is_none(), "a folder has no name to rename");
        assert!(!can_delete(&folder, &assets));
        assert!(!can_write_meta(&folder, &assets));
        assert!(!delete_selection(&mut pending, &folder, &assets));
        assert!(!write_meta_selection(&mut pending, &folder, &assets));

        open_rename(&mut dialog, &folder_typed_file, &assets);
        assert!(
            dialog.is_some(),
            "a type named Folder is not the folder sentinel"
        );
        assert!(can_delete(&folder_typed_file, &assets));
    }

    /// A module's resource tree is recognised by the extensions root and every
    /// key below it; a project key never is.
    #[test]
    fn a_module_tree_is_read_only() {
        let rows = vec![
            module_row("pill_rendering", "", "folder"),
            module_row("pill_rendering", "materials", "folder"),
            module_row(
                "pill_rendering",
                "materials/pill.pbr.material",
                "pill_rendering::Material",
            ),
            listing("textures/rock.png", "Texture", false, true),
            listing("pill_rendering", "folder", false, false),
        ];
        assert!(is_read_only_key(&rows, EXTENSIONS_ROOT));
        assert!(is_read_only_key(&rows, "extensions/pill_rendering"));
        assert!(is_read_only_key(
            &rows,
            "extensions/pill_rendering/materials"
        ));
        assert!(is_read_only_key(
            &rows,
            "extensions/pill_rendering/materials/pill.pbr.material"
        ));
        // A project folder of the same name is its own key, not the module's.
        assert!(!is_read_only_key(&rows, "textures/rock.png"));
        assert!(!is_read_only_key(&rows, "textures"));
        assert!(!is_read_only_key(&rows, "pill_rendering"));
    }

    /// A module's resource is found by its key, and neither the delete nor the
    /// meta write queues anything for it.
    #[test]
    fn a_module_resource_is_never_written() {
        let rows = vec![module_row(
            "pill_rendering",
            "materials/pill.pbr.material",
            "pill_rendering::Material",
        )];
        let mut selection = EditorSelection::default();
        selection.asset = Some("extensions/pill_rendering/materials/pill.pbr.material".to_string());
        assert!(selected_row(&selection, &rows).is_some(), "found by key");

        let mut pending = PendingEditorCommands::default();
        assert!(!delete_selection(&mut pending, &selection, &rows));
        assert!(
            pending.batches.is_empty(),
            "a refused delete queues nothing"
        );
        assert!(!write_meta_selection(&mut pending, &selection, &rows));
        assert!(pending.batches.is_empty());
    }

    fn types() -> Vec<StandaloneTypeInfo> {
        vec![
            StandaloneTypeInfo {
                type_name: "pill_rendering::Material".to_string(),
                display_name: "Material".to_string(),
                extension: "material".to_string(),
            },
            StandaloneTypeInfo {
                type_name: "pill_rendering::Skybox".to_string(),
                display_name: "Skybox".to_string(),
                extension: "skybox".to_string(),
            },
        ]
    }

    fn batches(pending: &PendingEditorCommands) -> &[crate::commands::PendingCommandBatch] {
        &pending.batches
    }

    #[test]
    fn opening_create_seeds_the_folder_and_first_type() {
        let assets = vec![
            listing("textures", "folder", false, false),
            listing("textures/rock.png", "texture", false, true),
        ];
        let mut dialog = None;
        let mut selection = EditorSelection::default();
        open_create(&mut dialog, &selection, &assets, &types());
        // Nothing selected: the root.
        assert_eq!(
            dialog,
            Some(AssetDialog::Create {
                folder: String::new(),
                type_name: "pill_rendering::Material".to_string(),
                name: String::new(),
                kind: CreateKind::Asset,
                overwrite: None,
            })
        );
        // A selected folder is the target.
        selection.asset = Some("textures".to_string());
        open_create(&mut dialog, &selection, &assets, &types());
        match dialog.as_ref() {
            Some(AssetDialog::Create { folder, .. }) => assert_eq!(folder, "textures"),
            other => panic!("expected a create dialog, got {other:?}"),
        }
        // A selected file targets its folder.
        selection.asset = Some("textures/rock.png".to_string());
        open_create(&mut dialog, &selection, &assets, &types());
        match dialog.as_ref() {
            Some(AssetDialog::Create { folder, .. }) => assert_eq!(folder, "textures"),
            other => panic!("expected a create dialog, got {other:?}"),
        }
    }

    #[test]
    fn submitting_create_queues_the_batch_and_closes() {
        let mut dialog = Some(AssetDialog::Create {
            folder: "textures".to_string(),
            type_name: "pill_rendering::Material".to_string(),
            name: "rock".to_string(),
            kind: CreateKind::Asset,
            overwrite: None,
        });
        let mut pending = PendingEditorCommands::default();
        assert!(submit_create(&mut dialog, &mut pending));
        assert!(dialog.is_none());
        let batch = &batches(&pending)[0];
        assert!(batch.record);
        assert_eq!(batch.label, "create asset rock");
        assert_eq!(
            batch.commands,
            vec![EditorCommand::CreateStandaloneAsset {
                type_name: "pill_rendering::Material".to_string(),
                folder: "textures".to_string(),
                name: "rock".to_string(),
                overwrite: None,
            }]
        );
    }

    /// A scene dialog opens on the scene type whatever the first type is, and
    /// refuses to open when no scene type is registered.
    #[test]
    fn a_scene_dialog_needs_the_scene_type_and_chooses_it() {
        let assets = vec![listing("scenes", "folder", false, false)];
        let mut selection = EditorSelection::default();
        selection.asset = Some("scenes".to_string());
        let mut scene_types = types();
        scene_types.push(StandaloneTypeInfo {
            type_name: "pill_engine::Scene".to_string(),
            display_name: "Scene".to_string(),
            extension: "scene".to_string(),
        });
        let mut dialog = None;
        open_create_scene(
            &mut dialog,
            &selection,
            &assets,
            &scene_types,
            CreateKind::SaveWorld,
        );
        assert_eq!(
            dialog,
            Some(AssetDialog::Create {
                folder: "scenes".to_string(),
                type_name: "pill_engine::Scene".to_string(),
                name: String::new(),
                kind: CreateKind::SaveWorld,
                overwrite: None,
            })
        );

        let mut dialog = None;
        open_create_scene(
            &mut dialog,
            &selection,
            &assets,
            &types(),
            CreateKind::EmptyScene,
        );
        assert_eq!(dialog, None, "without a scene type the dialog is refused");
    }

    /// A scene create is one recorded step that clears the world and writes it,
    /// and a save-as writes the world to the path the dialog chose.
    #[test]
    fn submitting_a_scene_create_writes_the_chosen_path() {
        let mut dialog = Some(AssetDialog::Create {
            folder: "scenes".to_string(),
            type_name: "pill_engine::Scene".to_string(),
            name: "level two".to_string(),
            kind: CreateKind::EmptyScene,
            overwrite: None,
        });
        let mut pending = PendingEditorCommands::default();
        assert!(submit_create(&mut dialog, &mut pending));
        let batch = &batches(&pending)[0];
        assert_eq!(batch.label, "create scene level two");
        assert_eq!(
            batch.commands,
            vec![
                EditorCommand::SceneNew,
                EditorCommand::SceneSaveAs {
                    path: "scenes/level two.scene".to_string()
                },
            ]
        );

        let mut dialog = Some(AssetDialog::Create {
            folder: String::new(),
            type_name: "pill_engine::Scene".to_string(),
            name: "copy".to_string(),
            kind: CreateKind::SaveWorld,
            overwrite: None,
        });
        let mut pending = PendingEditorCommands::default();
        assert!(submit_create(&mut dialog, &mut pending));
        assert_eq!(
            batches(&pending)[0].commands,
            vec![EditorCommand::SceneSaveAs {
                path: "copy.scene".to_string()
            }]
        );
    }

    #[test]
    fn submitting_create_needs_a_name_and_keeps_the_dialog() {
        let mut dialog = Some(AssetDialog::Create {
            folder: String::new(),
            type_name: "pill_rendering::Material".to_string(),
            name: "   ".to_string(),
            kind: CreateKind::Asset,
            overwrite: None,
        });
        let mut pending = PendingEditorCommands::default();
        assert!(!submit_create(&mut dialog, &mut pending));
        assert!(dialog.is_some());
        assert!(pending.batches.is_empty());
    }

    #[test]
    fn submitting_rename_builds_the_new_path() {
        let mut pending = PendingEditorCommands::default();
        let mut dialog = Some(AssetDialog::Rename {
            from: "textures/rock.png".to_string(),
            name: "stone".to_string(),
        });
        assert!(submit_rename(&mut dialog, &mut pending));
        assert!(dialog.is_none());
        let batch = &batches(&pending)[0];
        assert_eq!(batch.label, "rename asset stone.png");
        assert_eq!(
            batch.commands,
            vec![EditorCommand::MoveAssetFile {
                from: "textures/rock.png".to_string(),
                to: "textures/stone.png".to_string(),
            }]
        );
    }

    #[test]
    fn rename_keeps_a_typed_extension_and_refuses_separators() {
        let mut pending = PendingEditorCommands::default();
        let mut dialog = Some(AssetDialog::Rename {
            from: "rock.png".to_string(),
            name: "stone.PNG".to_string(),
        });
        assert!(submit_rename(&mut dialog, &mut pending));
        assert_eq!(
            batches(&pending)[0].commands,
            vec![EditorCommand::MoveAssetFile {
                from: "rock.png".to_string(),
                to: "stone.PNG".to_string(),
            }]
        );
        let mut dialog = Some(AssetDialog::Rename {
            from: "rock.png".to_string(),
            name: "a/b".to_string(),
        });
        assert!(!submit_rename(&mut dialog, &mut pending));
        assert_eq!(pending.batches.len(), 1);
        assert!(dialog.is_some());
    }

    #[test]
    fn toolbar_actions_check_the_selection() {
        let assets = vec![
            listing("textures", "folder", false, false),
            listing("textures/rock.png", "texture", false, true),
            listing("sourced.ini", "file", false, false),
        ];
        let mut pending = PendingEditorCommands::default();
        let mut selection = EditorSelection::default();
        // No selection: both refuse.
        assert!(!delete_selection(&mut pending, &selection, &assets));
        assert!(!write_meta_selection(&mut pending, &selection, &assets));
        // A folder refuses the delete.
        selection.asset = Some("textures".to_string());
        assert!(!delete_selection(&mut pending, &selection, &assets));
        // A file deletes; a file with a sidecar refuses the meta write.
        selection.asset = Some("textures/rock.png".to_string());
        assert!(delete_selection(&mut pending, &selection, &assets));
        assert!(!write_meta_selection(&mut pending, &selection, &assets));
        // A sourced file without a sidecar takes the meta write.
        selection.asset = Some("sourced.ini".to_string());
        assert!(write_meta_selection(&mut pending, &selection, &assets));
        let labels: Vec<&str> = pending
            .batches
            .iter()
            .map(|batch| batch.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["delete asset textures/rock.png", "write meta sourced.ini"]
        );
    }

    #[test]
    fn a_drop_destination_refuses_the_same_parent() {
        assert_eq!(
            drop_destination("rock.png", "textures"),
            Some("textures/rock.png".to_string())
        );
        assert_eq!(drop_destination("textures/rock.png", "textures"), None);
        assert_eq!(
            drop_destination("textures/rock.png", "models"),
            Some("models/rock.png".to_string())
        );
    }
}
