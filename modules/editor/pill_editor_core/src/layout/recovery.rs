//! The recovery document: editor state a run saves as it goes, so the next
//! run can bring it back after an abrupt exit.
//!
//! # Responsibilities
//!
//! - [`RecoveryDocument`]: the dock, the selection and the version stamp,
//!   saved as a YAML document beside the layout file.
//! - [`write`] / [`read`] / [`remove`]: the three operations the editor
//!   executable calls on that file.
//!
//! # Design
//!
//! The layout file is saved when the editor quits cleanly, and it holds the
//! arrangement the person kept. The recovery file is written while the editor
//! runs and is deleted on a clean exit, so finding it at startup means the
//! last run did not finish. The editor then restores that document, so the
//! person gets back the arrangement they had instead of the default one.
//!
//! The selection is stored as text: entity display handles (`3v0`), the
//! selected asset's project-relative path and the selected resource's stable
//! name. No runtime handle or type id is written, because the document
//! outlives the process that wrote it. The clipboard payload follows the same
//! rule.

// External crates
use pill_core::telemetry::telemetry_target;
use serde::{Deserialize, Serialize};

// Current crate
use crate::editor_state::EditorSelection;
use crate::layout::EditorDock;

/// The recovery document's version. A document that carries any other version
/// is ignored instead of being partly applied.
pub const RECOVERY_VERSION: u32 = 1;

/// The recovery document's file name, in the same folder as the layout
/// document.
pub const RECOVERY_FILE: &str = "editor_egui_recovery.yaml";

/// The editor state a run saves as it goes, so the next run can restore it
/// after an abrupt exit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoveryDocument {
    /// The document's version ([`RECOVERY_VERSION`]).
    pub version: u32,
    /// The dock as it stood at the last autosave.
    pub dock: EditorDock,
    /// The selected entities' display handles, in selection order.
    pub entities: Vec<String>,
    /// The selected asset's project-relative path, when one was selected.
    pub asset: Option<String>,
    /// The selected resource's stable name, when one was selected.
    pub resource: Option<String>,
}

impl RecoveryDocument {
    /// Builds the document for the current state.
    ///
    /// The dock is settled first (its placeholder rectangles are replaced with
    /// finite ones). A node the frame has not laid out yet holds rectangles
    /// with non-finite floats, which would be written as YAML's `.nan` and
    /// read back as a float the layout cannot use. The layout document is
    /// written the same way.
    pub fn capture(dock: &EditorDock, selection: &EditorSelection) -> Self {
        Self {
            version: RECOVERY_VERSION,
            dock: dock.settled(),
            entities: selection
                .entities()
                .iter()
                .map(|entity| entity.to_string())
                .collect(),
            asset: selection.asset.clone(),
            resource: selection.resource.clone(),
        }
    }

    /// Whether this document's version is the one this build writes.
    pub fn is_current(&self) -> bool {
        self.version == RECOVERY_VERSION
    }
}

/// The recovery document's path: the layout directory, beside the layout
/// document.
pub fn recovery_path() -> std::path::PathBuf {
    crate::layout::layout_directory().join(RECOVERY_FILE)
}

/// Writes the document, replacing the previous one.
///
/// Returns whether it was written. A failure is logged and the editor keeps
/// running, because losing recoverability must not stop the editor.
pub fn write(document: &RecoveryDocument) -> bool {
    let path = recovery_path();
    let write_outcome =
        crate::editor_files::write_yaml(&path, document).map_err(|error| error.message());
    match write_outcome {
        Ok(()) => true,
        Err(message) => {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                reason = message.as_str(),
                "[editor] recovery write failed"
            );
            false
        }
    }
}

/// Reads the document a previous run left behind.
///
/// `None` when there is no document (a clean exit leaves none), or when this
/// build cannot use the one it finds. A document that does not parse, or that
/// carries another version, is deleted so the next run does not read it again.
pub fn read() -> Option<RecoveryDocument> {
    let path = recovery_path();
    let bytes = std::fs::read(&path).ok()?;
    match crate::editor_files::parse_yaml::<RecoveryDocument>(&bytes) {
        Ok(document) if document.is_current() => Some(document),
        Ok(document) => {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                version = document.version,
                "[editor] recovery document ignored"
            );
            let _ = std::fs::remove_file(&path);
            None
        }
        Err(error) => {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                reason = %error,
                "[editor] recovery document ignored"
            );
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// Removes the document. The run it described is ending cleanly, so there is
/// nothing left to recover.
pub fn remove() {
    let path = recovery_path();
    match std::fs::remove_file(&path) {
        Ok(()) => pill_core::info!(
            target: telemetry_target::EDITOR,
            "[editor] recovery document removed"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => pill_core::warn!(
            target: telemetry_target::EDITOR,
            reason = %error,
            "[editor] recovery document could not be removed"
        ),
    }
}
