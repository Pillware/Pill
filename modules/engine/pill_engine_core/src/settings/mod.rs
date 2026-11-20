//! Manager settings: the settings asset a manager reads its tuning from.
//!
//! # Responsibilities
//!
//! - Declare the settings surface a manager resource implements, the applier
//!   that moves a document into the live value, and the outcome it reports
//!   ([`settings`]).
//! - Hold the settings *asset* side: the bindings that point a manager at one,
//!   the file a host creates when the project has none, and the change
//!   detection that re-applies an edited file ([`manager_settings`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports both
//! (`pub use settings::manager_settings` in `lib.rs`), so moving a file into
//! this group changes no caller.
//!
//! This group and the settings module inside it share a name, so the items of
//! [`settings`] are re-exported here as well: `crate::settings::SettingsOutcome`
//! is the path callers have always used, and `crate::settings::settings::…` is
//! the same item by its new path.

/// Settings assets that configure manager resources: the bindings that point a
/// manager at one, the file a host creates when the project has none, and the
/// change detection that re-applies an edited file.
pub mod manager_settings;

// The group is named after the module callers look for, and
// `crate::settings::SettingsOutcome` must resolve while the module sits in
// `settings/`. Clippy reads the repetition as an accident; it is deliberate.
#[allow(clippy::module_inception)]
/// Manager settings: the settings asset a manager resource reads, its
/// conventional path, and the applier that moves its document into the live
/// value.
pub mod settings;

// Current crate
pub use self::settings::*;
