//! The development host's runtime switches: the settings that decide which
//! file watchers a run starts.
//!
//! # Responsibilities
//!
//! - Reads `host.hot_reloading`, `host.function_hot_patching`,
//!   `host.assets_hot_reloading` and `host.renderer` from the project's
//!   resolved settings once, at startup, and holds the four answers.
//! - Writes the startup line that says which posture the run is in.
//!
//! The switches only stop watchers. Nothing about the host is compiled out by
//! them: a run with every switch off still builds the project at startup, still
//! loads it as a module, and still answers an explicit reload.

// =============================================================================
// Imports
// =============================================================================

// Standard library

// External crates

// Current crate
use crate::project_settings::ProjectSettings;
use pill_core::info;
use pill_core::telemetry::telemetry_target;

// =============================================================================
// Switch names
// =============================================================================

/// The setting that turns the source watchers on or off.
///
/// Off means no edit is ever noticed: no reload, no patch, no rebuild
/// trigger.
pub const HOT_RELOADING_SETTING: &str = "host.hot_reloading";

/// The setting that turns the per-function patch fast path on or off.
///
/// Off means a watched edit takes the ordinary rebuild-and-reload path
/// instead of being compiled into the running module.
pub const FUNCTION_HOT_PATCHING_SETTING: &str = "host.function_hot_patching";

/// The setting that turns the asset and shader watchers on or off.
///
/// One setting covers the project's `res` folder and both shader trees, so a
/// run that wants a quiet filesystem switches them off together.
pub const ASSETS_HOT_RELOADING_SETTING: &str = "host.assets_hot_reloading";

/// The setting that names the renderer a standalone run loads.
///
/// Empty follows the project's enabled modules. The editor follows them too
/// and says that it ignored this setting.
pub const RENDERER_SETTING: &str = "host.renderer";

/// The settings as one line, for the startup log.
pub const SWITCH_SETTINGS: &str = "host.hot_reloading, host.function_hot_patching, \
     host.assets_hot_reloading, host.renderer";

// =============================================================================
// The switches
// =============================================================================

/// Which watchers this run starts, and which renderer it was asked to load,
/// read once when the host starts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HostSwitches {
    /// Whether the project's and the extensions' source trees are watched.
    pub(crate) hot_reloading: bool,
    /// Whether an edit that arrives may be compiled into the running module.
    pub(crate) function_hot_patching: bool,
    /// Whether the project's assets and both shader trees are watched.
    pub(crate) assets_hot_reloading: bool,
    /// The renderer the settings name, or `None` when the enabled modules
    /// decide.
    pub(crate) renderer: Option<String>,
}

impl HostSwitches {
    /// Reads the switches from the project's resolved settings.
    pub(crate) fn from_settings(settings: &ProjectSettings) -> Self {
        Self {
            hot_reloading: settings.hot_reloading(),
            function_hot_patching: settings.function_hot_patching(),
            assets_hot_reloading: settings.assets_hot_reloading(),
            renderer: settings.requested_renderer().map(str::to_string),
        }
    }

    /// Writes the one line that says which posture this run is in.
    ///
    /// The values are log fields, so a test can read the boolean it switched
    /// off without matching the whole sentence.
    pub(crate) fn log(&self) {
        info!(
            target: telemetry_target::ENGINE,
            hot_reloading = self.hot_reloading,
            function_hot_patching = self.function_hot_patching,
            assets_hot_reloading = self.assets_hot_reloading,
            renderer = self.renderer.as_deref().unwrap_or("the enabled modules"),
            settings = SWITCH_SETTINGS,
            "the hot reload switches"
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The startup line names every setting, so a person reading a log knows
    /// what to change to get the other posture.
    #[test]
    fn the_setting_list_names_all_four() {
        assert!(SWITCH_SETTINGS.contains(HOT_RELOADING_SETTING));
        assert!(SWITCH_SETTINGS.contains(FUNCTION_HOT_PATCHING_SETTING));
        assert!(SWITCH_SETTINGS.contains(ASSETS_HOT_RELOADING_SETTING));
        assert!(SWITCH_SETTINGS.contains(RENDERER_SETTING));
    }
}
