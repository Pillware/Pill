//! Loading and saving the dock layout, with a schema version.
//!
//! # Responsibilities
//!
//! - Load the saved dock, falling back to the default for a document this
//!   build cannot read.
//! - Write without destroying the previous file when a save goes wrong.
//! - Own [`layout_directory`], where the editor's state files live.
//!
//! # Design
//!
//! The document is the whole [`EditorDock`] with a schema version field.
//! Loading upgrades a document the migration list has an entry for and
//! replaces every other one with the default layout, so a stale or unreadable
//! file never stops the editor from opening. Writes go through
//! [`crate::editor_files::write_bytes`], which stages a temporary file and
//! renames it over the target.

// Standard library
use std::io;
use std::path::{Path, PathBuf};

// External crates
use pill_core::error::EngineMessage;
use pill_core::telemetry::telemetry_target;
use thiserror::Error;

// Current crate
use super::EditorDock;
use crate::error::LayoutPersistenceError;

// =============================================================================
// Constants
// =============================================================================

/// The schema of the dock-layout document this build writes and accepts.
///
/// Raise it whenever the tab set changes, or whenever the document carries
/// something new beside the dock tree: `EditorTab` is written into the
/// document, so a reader has to understand the tab values, the `panel_ids`
/// list and the `popouts` list.
///
/// A document that `migrate` has a rule for is upgraded in place and written
/// back with this version. Any other one - older than the oldest rule, or
/// newer than this build - is replaced by the default layout, with one log
/// line.
pub const LAYOUT_SCHEMA_VERSION: u32 = 13;

/// The application directory inside the per-user state directory.
const APPLICATION_DIRECTORY: &str = "RustHybridEcs";

/// The dock document's file name in the layout directory.
const LAYOUT_FILE: &str = "editor_egui_layout.yaml";

// =============================================================================
// Types
// =============================================================================

/// A saved dock document the layout model refuses.
///
/// [`LayoutPersistenceError`] keeps this type as the source of its `Validation`
/// variant, and that error's namespace is `editor::layout`, so the type lives
/// here with the layout code.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LayoutError {
    /// The document carries a schema version this build does not write.
    #[error("invalid dock layout: UnsupportedVersion({0})")]
    UnsupportedVersion(u32),
}

// =============================================================================
// Free Functions
// =============================================================================

/// Load the saved dock, falling back to the default on any invalid document.
pub fn load_or_default() -> EditorDock {
    load_from_path(&layout_directory().join(LAYOUT_FILE))
}

/// Load the dock document at `path`, falling back to the default on any
/// document this build cannot read.
///
/// An older document that [`migrate`] has a rule for is upgraded and written
/// back before it is returned. Any other document this build cannot use - a
/// schema it does not know, a file that does not parse - is replaced by the
/// default layout, so the editor always opens.
///
/// A replacement is logged once, at info, as `[editor] layout reset`, with the
/// reason in its own field: the message is the stable token a suite can match,
/// and the reason is what a person reads.
fn load_from_path(path: &Path) -> EditorDock {
    // A missing file is the normal first-run state and stays silent.
    let Ok(bytes) = std::fs::read(path) else {
        return EditorDock::new();
    };
    match crate::editor_files::parse_yaml::<EditorDock>(&bytes) {
        Ok(dock) if dock.is_current() => dock,
        Ok(mut dock) => match migrate(&mut dock) {
            Some(previous_schema_version) => {
                // A document this build can upgrade in place: keep the layout
                // the user arranged and write it back with the current version.
                save(&dock);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    path = %path.display(),
                    from = previous_schema_version,
                    to = LAYOUT_SCHEMA_VERSION,
                    "[editor] layout migrated"
                );
                dock
            }
            None => {
                reset_layout(
                    path,
                    &format!(
                        "schema {} where this build writes {}",
                        dock.schema_version(),
                        LAYOUT_SCHEMA_VERSION
                    ),
                );
                EditorDock::new()
            }
        },
        Err(source) => {
            reset_layout(path, &format!("unreadable document: {source}"));
            EditorDock::new()
        }
    }
}

/// Upgrade an older document in place, returning the version it came from;
/// `None` when this build has no rule for that version.
///
/// Each entry in `TAB_ADDITIONS` names the newest schema that still lacks a
/// tab, the tab the new one is placed beside, and the tab itself. A document
/// several versions old gets every missing tab in one pass, in tab-strip
/// order, and no tab the user arranged moves.
fn migrate(dock: &mut EditorDock) -> Option<u32> {
    const TAB_ADDITIONS: [(u32, super::EditorTab, super::EditorTab); 3] = [
        (7, super::EditorTab::Assets, super::EditorTab::Console),
        (8, super::EditorTab::Console, super::EditorTab::Problems),
        (9, super::EditorTab::Problems, super::EditorTab::Statistics),
    ];
    let previous_schema_version = dock.schema_version();
    if !(7..LAYOUT_SCHEMA_VERSION).contains(&previous_schema_version) {
        return None;
    }
    for (version, anchor, tab) in TAB_ADDITIONS {
        if previous_schema_version <= version && !dock.contains(tab) {
            append_beside(dock, anchor, tab);
        }
    }
    dock.set_schema_version(LAYOUT_SCHEMA_VERSION);
    Some(previous_schema_version)
}

/// Append `tab` to the leaf that holds `anchor` (the tab group it is docked
/// in), or to the focused leaf when the anchor is not docked, so an unusual
/// layout keeps the rest of its tabs.
fn append_beside(dock: &mut EditorDock, anchor: super::EditorTab, tab: super::EditorTab) {
    let leaf_path = dock
        .state_mut()
        .find_tab(&anchor)
        .map(|tab_path| tab_path.node_path());
    match leaf_path {
        Some(leaf_path) => {
            if let Ok(node) = dock.state_mut().leaf_mut(leaf_path) {
                node.append_tab(tab);
            }
        }
        None => dock.state_mut().push_to_focused_leaf(tab),
    }
}

/// Log that the document at `path` was replaced, and the reason it was.
fn reset_layout(path: &Path, reason: &str) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path = %path.display(),
        reason,
        "[editor] layout reset"
    );
}

/// Save `dock` to the layout directory.
///
/// A failed write leaves the previous document untouched and is logged as a
/// warning, since the caller has nothing to do about it.
pub fn save(dock: &EditorDock) {
    let path = layout_directory().join(LAYOUT_FILE);
    if let Err(error) = save_to_path(dock, path) {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            "Cannot save dock layout: {}",
            error.to_plain_message()
        );
    }
}

/// Write `dock` as a YAML document at `path`, replacing the file there.
///
/// The document is written from [`EditorDock::settled`], which replaces the
/// infinite placeholder rectangles of a dock that was never laid out with
/// zeros, since no document format can hold them and the next show recomputes
/// them.
///
/// # Errors
///
/// [`LayoutPersistenceError::Serialization`] when the dock cannot be turned
/// into YAML, and [`LayoutPersistenceError::Filesystem`] when the directory,
/// the temporary file or the target cannot be written.
fn save_to_path(dock: &EditorDock, path: PathBuf) -> Result<(), LayoutPersistenceError> {
    crate::editor_files::write_yaml(&path, &dock.settled()).map_err(|error| match error {
        crate::editor_files::FileError::Serialize { message, .. } => {
            LayoutPersistenceError::Serialization { detail: message }
        }
        crate::editor_files::FileError::Write { path, source } => {
            LayoutPersistenceError::Filesystem { path, source }
        }
        other_file_error => LayoutPersistenceError::Filesystem {
            path: path.display().to_string(),
            source: io::Error::other(other_file_error.message()),
        },
    })
}

/// The directory the editor keeps its state files in: the dock layout, the
/// preferences (which also hold the gizmo settings), the keybindings and the
/// recovery snapshot.
pub fn layout_directory() -> PathBuf {
    editor_state_directory().join(APPLICATION_DIRECTORY)
}

/// The per-user directory that editor state belongs in.
///
/// Windows: `%LOCALAPPDATA%`, or the temp directory when it is not set.
/// Elsewhere: `$XDG_STATE_HOME`, then `~/.local/state` when that is unset (as
/// the XDG base directory spec says), then the temp directory when `$HOME` is
/// unset too.
///
/// A browser has no environment and no filesystem, so this function is
/// compiled out there. The web posture answers a fixed relative directory, and
/// every load fails into the default dock.
#[cfg(not(target_arch = "wasm32"))]
fn editor_state_directory() -> PathBuf {
    if cfg!(windows) {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local_app_data);
        }
    } else {
        if let Some(state_home) = std::env::var_os("XDG_STATE_HOME").filter(|path| !path.is_empty())
        {
            return PathBuf::from(state_home);
        }
        if let Some(home) = std::env::var_os("HOME").filter(|path| !path.is_empty()) {
            return PathBuf::from(home).join(".local").join("state");
        }
    }
    std::env::temp_dir()
}

/// The web posture's editor state directory; see the native function above.
#[cfg(target_arch = "wasm32")]
fn editor_state_directory() -> PathBuf {
    PathBuf::from("editor-state")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::EditorTab;

    /// A dock-layout document saved by `egui_dock` 0.17, whose node model
    /// differs from this build's: a real saved file, not a hand-written
    /// sketch. Its schema is 6, older than the oldest upgrade rule.
    const V6_LAYOUT: &str = include_str!("fixtures/layout_v6.yaml");

    /// A directory of this test's own under the temp directory.
    fn scratch_directory(label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("pill_layout_test_{label}"));
        std::fs::create_dir_all(&directory).expect("the temp directory is writable");
        directory
    }

    #[test]
    fn a_stored_v6_document_is_replaced_by_the_default_layout() {
        // The two node models differ, so the document may fail to parse, or
        // may parse and then fail the schema check. Either way it must never
        // be taken for this build's own file.
        if let Ok(dock) = crate::editor_files::parse_yaml::<EditorDock>(V6_LAYOUT.as_bytes()) {
            assert_eq!(dock.schema_version(), 6);
            assert!(!dock.is_current());
        }
        let directory = scratch_directory("v6");
        let path = directory.join(LAYOUT_FILE);
        std::fs::write(&path, V6_LAYOUT).expect("the fixture writes");

        let dock = load_from_path(&path);

        assert!(dock.is_current());
        for tab in EditorTab::ALL {
            assert!(dock.contains(tab), "{tab:?} is missing from the default");
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_layout_written_by_this_build_loads_back_unchanged() {
        let directory = scratch_directory("round_trip");
        let path = directory.join(LAYOUT_FILE);
        // A dock that differs from the default, so a load that fell back to
        // `EditorDock::new()` could not pass this test.
        let mut saved = EditorDock::new();
        saved.pop_out(EditorTab::Console);
        save_to_path(&saved, path.clone()).expect("the layout saves");

        let loaded = load_from_path(&path);

        assert_eq!(loaded.schema_version(), LAYOUT_SCHEMA_VERSION);
        assert!(
            loaded.is_popped_out(EditorTab::Console),
            "the loaded layout lost the popped-out tab"
        );
        for tab in EditorTab::ALL {
            assert_eq!(
                saved.state().find_tab(&tab),
                loaded.state().find_tab(&tab),
                "{tab:?} moved between the file and the load"
            );
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    /// Saving twice replaces the first document: the shared state-file writer
    /// renames its temporary file over the target, so no `.bak` file is left
    /// beside the layout.
    #[test]
    fn a_second_save_replaces_the_first_layout() {
        let directory = scratch_directory("second_save");
        let path = directory.join(LAYOUT_FILE);
        save_to_path(&EditorDock::new(), path.clone()).expect("the first save");
        let mut popped_out_dock = EditorDock::new();
        popped_out_dock.pop_out(EditorTab::Console);
        save_to_path(&popped_out_dock, path.clone()).expect("the second save");

        let loaded = load_from_path(&path);

        assert!(
            loaded.is_popped_out(EditorTab::Console),
            "the second save did not land"
        );
        assert!(
            !path.with_extension("yaml.bak").exists(),
            "the writer kept a backup file"
        );
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn a_document_from_a_newer_schema_is_replaced_too() {
        let directory = scratch_directory("newer");
        let path = directory.join(LAYOUT_FILE);
        let mut document: serde_json::Value =
            serde_json::to_value(EditorDock::new()).expect("the dock serializes to a value");
        document["schema_version"] = serde_json::Value::from(LAYOUT_SCHEMA_VERSION + 1);
        std::fs::write(
            &path,
            pill_engine::asset_document::render(&document).expect("the document renders"),
        )
        .expect("the document writes");

        let dock = load_from_path(&path);

        assert!(dock.is_current());
        for tab in EditorTab::ALL {
            assert!(dock.contains(tab), "{tab:?} is missing from the default");
        }
        std::fs::remove_dir_all(&directory).ok();
    }
}
