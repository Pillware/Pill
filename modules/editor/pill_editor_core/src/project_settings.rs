//! The Project Settings tab's shared vocabulary: one row per declared setting,
//! with where each of its values comes from.
//!
//! # Responsibilities
//!
//! - [`ProjectSettingRow`]: one declared setting as a row - its entry, its
//!   dotted path, what it does, its kind and range, when a change takes effect,
//!   its default, the value each settings file holds, an environment override,
//!   and the value in effect.
//! - [`ProjectSettingsView`]: the rows, grouped in entry order, plus the
//!   project directory an edit writes to and the refusal a broken file leaves.
//! - [`SettingProvenance`]: which of those values is the one in effect, which
//!   is what the row's icon and tooltip say.
//! - [`describe_value`]: one value as the row's text.
//! - [`read_file`] and [`write_file`]: the disk half, so the tab's commands and
//!   the scripted verbs write the same way.
//!
//! # Design
//!
//! The rows are built by the editor executable from the host's resolution, and
//! this module only defines what a row is: the shell draws it and the core's
//! commands write it, so both sides have to agree on the type. Nothing here
//! reads a schema or a file on its own - a row carries the texts it shows.

// Standard library
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// External crates
use pill_engine::{ApplyTiming, SettingKind};

// Current crate
use crate::project_settings_file::{remove_setting, set_setting};

// =============================================================================
// Constants
// =============================================================================

/// The project's settings file, in the project root.
pub const PROJECT_SETTINGS_FILE: &str = "project_settings.yaml";

/// The local override beside it: the same shape, read over the project file,
/// and never committed.
pub const PROJECT_SETTINGS_LOCAL_FILE: &str = "project_settings.local.yaml";

/// The icon a row shows when no settings file holds its value.
pub const ICON_DEFAULT: &str = "default";

/// The icon a row shows when `project_settings.yaml` holds its value.
pub const ICON_PROJECT: &str = "project";

/// The icon a row shows when `project_settings.local.yaml` overrides it.
pub const ICON_LOCAL: &str = "local";

/// The icon a row shows when an environment variable overrides it.
pub const ICON_ENVIRONMENT: &str = "environment";

// =============================================================================
// Types
// =============================================================================

/// Where a setting's value in effect comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingProvenance {
    /// Nothing overrides the declaration.
    Default,
    /// `project_settings.yaml` holds it.
    Project,
    /// `project_settings.local.yaml` holds it.
    Local,
    /// An environment variable holds it.
    Environment,
}

impl SettingProvenance {
    /// The icon name the row reports in a log or a dump.
    pub fn icon(self) -> &'static str {
        match self {
            SettingProvenance::Default => ICON_DEFAULT,
            SettingProvenance::Project => ICON_PROJECT,
            SettingProvenance::Local => ICON_LOCAL,
            SettingProvenance::Environment => ICON_ENVIRONMENT,
        }
    }

    /// The glyph the row draws beside its value: a cog for the declaration, a
    /// file for the project's own, a folder for the local override beside it,
    /// and angle brackets for an environment variable.
    pub fn glyph(self) -> crate::icons::Icon {
        match self {
            SettingProvenance::Default => crate::icons::Icon::Gear,
            SettingProvenance::Project => crate::icons::Icon::File,
            SettingProvenance::Local => crate::icons::Icon::Folder,
            SettingProvenance::Environment => crate::icons::Icon::Code,
        }
    }

    /// The row's tooltip: the value's source, and whether a change takes effect
    /// without a restart.
    ///
    /// The two are one tooltip because a row can need both: a local override
    /// that only applies at startup says so in the same breath.
    pub fn tooltip(self, timing: ApplyTiming) -> String {
        let source = match self {
            SettingProvenance::Default => "defined by the engine, not in project settings",
            SettingProvenance::Project => "set in project_settings.yaml",
            SettingProvenance::Local => "overridden by project_settings.local.yaml",
            SettingProvenance::Environment => "overwritten by an environment variable",
        };
        match timing {
            ApplyTiming::Live => format!("{source}; applies live"),
            ApplyTiming::Startup => format!("{source}; needs a restart"),
            ApplyTiming::Build => format!("{source}; needs a rebuild"),
        }
    }
}

/// One declared setting, as the Project Settings tab lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectSettingRow {
    /// The entry the setting belongs to: a module's crate name, or `engine`,
    /// `host` or `editor`.
    pub entry: String,
    /// The setting's own group, empty when it sits directly under the entry.
    pub group: String,
    /// The setting's path inside its entry: `group.key`, or just the key.
    pub path: String,
    /// The setting's full dotted path, entry first: what an edit names.
    pub full_path: String,
    /// One line saying what the setting does.
    pub description: String,
    /// The kind of value it holds.
    pub kind: SettingKind,
    /// The smallest accepted value, for a number.
    pub minimum: Option<f64>,
    /// The largest accepted value, for a number.
    pub maximum: Option<f64>,
    /// When a changed value takes effect.
    pub timing: ApplyTiming,
    /// The declared default, as row text.
    pub default_text: String,
    /// What `project_settings.yaml` holds, when it holds anything.
    pub project_text: Option<String>,
    /// What `project_settings.local.yaml` holds, when it does.
    pub local_text: Option<String>,
    /// The environment variable that overrides it, with the value it carries,
    /// when one does.
    pub environment: Option<String>,
    /// The value in effect, as row text.
    pub effective_text: String,
    /// Where the value in effect comes from.
    pub provenance: SettingProvenance,
}

impl ProjectSettingRow {
    /// The row's label: the setting's path under the group it sits in.
    pub fn label(&self) -> &str {
        self.path.as_str()
    }

    /// The tooltip the row's icon shows.
    pub fn tooltip(&self) -> String {
        let mut text = self.provenance.tooltip(self.timing);
        if !self.description.is_empty() {
            text.push_str(": ");
            text.push_str(self.description.as_str());
        }
        if let Some(range) = self.range_text() {
            text.push(' ');
            text.push_str(range.as_str());
        }
        if let Some(environment) = self.environment.as_deref() {
            text.push_str(" (");
            text.push_str(environment);
            text.push(')');
        }
        text
    }

    /// The accepted range, in words, for a number; `None` for any other kind.
    pub fn range_text(&self) -> Option<String> {
        match (self.minimum, self.maximum) {
            (Some(minimum), Some(maximum)) => Some(format!("Range {minimum}..{maximum}.")),
            (Some(minimum), None) => Some(format!("At least {minimum}.")),
            (None, Some(maximum)) => Some(format!("At most {maximum}.")),
            (None, None) => None,
        }
    }
}

/// The tab's whole view: every row, the directory an edit writes to, and what
/// the last read refused.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectSettingsView {
    /// The project root the two settings files live in.
    pub directory: Option<String>,
    /// One row per declared setting, in entry order then declaration order.
    pub rows: Vec<ProjectSettingRow>,
    /// Why the files could not be read, when they could not.
    pub refusal: Option<String>,
}

impl ProjectSettingsView {
    /// The rows of one entry, in declaration order.
    pub fn rows_for<'a>(&'a self, entry: &'a str) -> impl Iterator<Item = &'a ProjectSettingRow> {
        self.rows.iter().filter(move |row| row.entry == entry)
    }

    /// The entries the view holds, in the order they first appear.
    pub fn entries(&self) -> Vec<&str> {
        let mut entries: Vec<&str> = Vec::new();
        for row in &self.rows {
            if !entries.contains(&row.entry.as_str()) {
                entries.push(row.entry.as_str());
            }
        }
        entries
    }

    /// The row at `full_path`, when the view holds one.
    pub fn row(&self, full_path: &str) -> Option<&ProjectSettingRow> {
        self.rows.iter().find(|row| row.full_path == full_path)
    }

    /// Whether either settings file holds a value for the row, which is what a
    /// reset removes.
    pub fn is_set(&self, full_path: &str) -> bool {
        self.row(full_path)
            .is_some_and(|row| row.project_text.is_some() || row.local_text.is_some())
    }
}

// =============================================================================
// Row text
// =============================================================================

/// One value as the row's text: a flag reads `on`/`off`, a number keeps its
/// digits, a list its items, a table its key count.
pub fn describe_value(value: &pill_engine::SettingsValue) -> String {
    value.describe()
}

// =============================================================================
// Disk
// =============================================================================

/// The path of one of a project's two settings files.
pub fn settings_path(directory: &Path, local: bool) -> PathBuf {
    directory.join(if local {
        PROJECT_SETTINGS_LOCAL_FILE
    } else {
        PROJECT_SETTINGS_FILE
    })
}

/// Reads a settings file, or an empty document when it does not exist.
///
/// # Errors
///
/// Returns a message naming the file when it cannot be read.
pub fn read_file(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Writes one setting into a project's settings file, keeping the file's
/// comments and layout.
///
/// The write goes to `project_settings.yaml` unless `local` asks for the local
/// override, which is created when it is not there yet.
///
/// # Errors
///
/// Returns a message naming the file when the setting cannot be written.
pub fn write_setting(
    directory: &Path,
    full_path: &str,
    value_text: &str,
    local: bool,
) -> Result<(), String> {
    let path = settings_path(directory, local);
    let text = read_file(&path)?;
    let written = set_setting(&text, full_path, value_text)
        .map_err(|message| format!("{}: {message}", path.display()))?;
    std::fs::write(&path, written).map_err(|error| format!("{}: {error}", path.display()))
}

/// Removes one setting from whichever of the project's settings files holds
/// it, and from the other when both do.
///
/// A file that does not exist is left alone, so a reset on a project with no
/// local override writes nothing.
///
/// # Errors
///
/// Returns a message naming the file when the setting cannot be removed.
pub fn remove_setting_everywhere(directory: &Path, full_path: &str) -> Result<(), String> {
    for local in [false, true] {
        let path = settings_path(directory, local);
        if !path.is_file() {
            continue;
        }
        let text = read_file(&path)?;
        let written = remove_setting(&text, full_path)
            .map_err(|message| format!("{}: {message}", path.display()))?;
        if written != text {
            std::fs::write(&path, written)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
    }
    Ok(())
}

/// Every file a reset of these rows would change, for a message.
pub fn files_holding(rows: &[ProjectSettingRow]) -> BTreeMap<&'static str, usize> {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for row in rows {
        if row.project_text.is_some() {
            *counts.entry(PROJECT_SETTINGS_FILE).or_default() += 1;
        }
        if row.local_text.is_some() {
            *counts.entry(PROJECT_SETTINGS_LOCAL_FILE).or_default() += 1;
        }
    }
    counts
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A row whose value comes from the project's file.
    fn project_row() -> ProjectSettingRow {
        ProjectSettingRow {
            entry: "pill_audio".to_string(),
            group: "audio".to_string(),
            path: "audio.ambient_sinks".to_string(),
            full_path: "pill_audio.audio.ambient_sinks".to_string(),
            description: "Ambient inputs the device opens.".to_string(),
            kind: SettingKind::Integer,
            minimum: Some(1.0),
            maximum: Some(64.0),
            timing: ApplyTiming::Startup,
            default_text: "16".to_string(),
            project_text: Some("32".to_string()),
            local_text: None,
            environment: None,
            effective_text: "32".to_string(),
            provenance: SettingProvenance::Project,
        }
    }

    #[test]
    fn a_row_names_its_source_and_its_restart() {
        let row = project_row();
        let tooltip = row.tooltip();
        assert!(tooltip.contains("set in project_settings.yaml"));
        assert!(tooltip.contains("needs a restart"));
        assert!(tooltip.contains("Ambient inputs the device opens."));
        assert!(tooltip.contains("Range 1..64."));
        assert_eq!(row.provenance.icon(), ICON_PROJECT);
    }

    #[test]
    fn a_local_value_says_so_before_its_timing() {
        let mut row = project_row();
        row.provenance = SettingProvenance::Local;
        row.timing = ApplyTiming::Live;
        row.environment = Some("PILL__PILL_AUDIO__AUDIO__AMBIENT_SINKS=64".to_string());
        let tooltip = row.tooltip();
        assert!(tooltip.contains("overridden by project_settings.local.yaml"));
        assert!(tooltip.contains("applies live"));
        assert!(tooltip.contains("PILL__PILL_AUDIO__AUDIO__AMBIENT_SINKS=64"));
        assert_eq!(row.provenance.icon(), ICON_LOCAL);
    }

    #[test]
    fn the_view_groups_rows_by_entry_in_first_seen_order() {
        let mut second = project_row();
        second.entry = "host".to_string();
        second.full_path = "host.hot_reloading".to_string();
        second.project_text = None;
        second.local_text = None;
        let view = ProjectSettingsView {
            rows: vec![project_row(), second, project_row()],
            ..ProjectSettingsView::default()
        };
        assert_eq!(view.entries(), vec!["pill_audio", "host"]);
        assert_eq!(view.rows_for("pill_audio").count(), 2);
        assert!(view.is_set("pill_audio.audio.ambient_sinks"));
        assert!(!view.is_set("host.hot_reloading"));
    }

    #[test]
    fn a_write_lands_in_the_file_and_a_reset_takes_it_out() {
        let directory = std::env::temp_dir().join("pill-editor-settings-view-test");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            settings_path(&directory, false),
            "# a comment worth keeping\nhost:\n  hot_reloading: true\n",
        )
        .unwrap();

        write_setting(&directory, "host.logging.level", "debug", false).unwrap();
        let written = std::fs::read_to_string(settings_path(&directory, false)).unwrap();
        assert!(written.contains("# a comment worth keeping"));
        assert!(written.contains("  logging:\n    level: debug\n"));

        write_setting(&directory, "host.logging.level", "trace", true).unwrap();
        let local = std::fs::read_to_string(settings_path(&directory, true)).unwrap();
        assert!(local.contains("level: trace"));

        remove_setting_everywhere(&directory, "host.logging.level").unwrap();
        assert!(!std::fs::read_to_string(settings_path(&directory, false))
            .unwrap()
            .contains("logging"));
        assert!(!std::fs::read_to_string(settings_path(&directory, true))
            .unwrap()
            .contains("logging"));

        let _ = std::fs::remove_dir_all(&directory);
    }
}
