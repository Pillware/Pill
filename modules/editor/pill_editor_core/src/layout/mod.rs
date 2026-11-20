//! The egui editor's dock layout: an `egui_dock` state over [`EditorTab`]s.
//!
//! # Responsibilities
//!
//! - Re-export the layout pieces the shell and the executable share: the tab
//!   set ([`dock`]), the saved layout document ([`persistence`]) and the
//!   recovery document ([`recovery`]).
//!
//! # Design
//!
//! `egui_dock` owns the splits, tab geometry and drag behavior, so this
//! module only defines the editor's tabs, the default layout and where the
//! documents live. The dock value itself is a field of the shell's session
//! resource, [`crate::session::EguiShellState`], so nothing here is drawn or
//! kept alive between frames.

mod dock;
mod persistence;
mod recovery;

pub use dock::{activate_tab, EditorDock, EditorTab, BOTTOM_ROW_FRACTION, SIDE_COLUMN_FRACTION};
pub use persistence::{
    layout_directory, load_or_default, save, LayoutError, LAYOUT_SCHEMA_VERSION,
};
pub use recovery::{
    read as read_recovery, recovery_path, remove as remove_recovery, write as write_recovery,
    RecoveryDocument, RECOVERY_FILE, RECOVERY_VERSION,
};
