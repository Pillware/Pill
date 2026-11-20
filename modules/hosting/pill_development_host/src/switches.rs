//! The development host's runtime switches: the three environment variables
//! that decide which file watchers a run starts.
//!
//! # Responsibilities
//!
//! - Reads `PILL_HOT_RELOADING`, `PILL_FUNCTION_HOT_PATCHING` and
//!   `PILL_ASSETS_HOT_RELOADING` once, at startup, and holds the three answers.
//! - States the one rule that turns a variable's text into a yes or a no.
//! - Writes the startup line that says which posture the run is in.
//!
//! The switches only stop watchers. Nothing about the host is compiled out by
//! them: a run with every switch off still builds the project at startup, still
//! loads it as a module, and still answers an explicit reload.

// =============================================================================
// Imports
// =============================================================================

// Standard library
use std::env;

// External crates

// Current crate
use pill_core::info;
use pill_core::telemetry::telemetry_target;

// =============================================================================
// Switch names
// =============================================================================

/// The variable that turns the source watchers on or off.
///
/// Off means no edit is ever noticed: no reload, no patch, no rebuild
/// trigger.
pub const HOT_RELOADING_VARIABLE: &str = "PILL_HOT_RELOADING";

/// The variable that turns the per-function patch fast path on or off.
///
/// Off means a watched edit takes the ordinary rebuild-and-reload path
/// instead of being compiled into the running module.
pub const FUNCTION_HOT_PATCHING_VARIABLE: &str = "PILL_FUNCTION_HOT_PATCHING";

/// The variable that turns the asset and shader watchers on or off.
///
/// One variable covers the project's `res` folder and both shader trees, so a
/// run that wants a quiet filesystem switches them off together.
pub const ASSETS_HOT_RELOADING_VARIABLE: &str = "PILL_ASSETS_HOT_RELOADING";

/// The three variable names as one line, for the startup log.
pub const SWITCH_VARIABLES: &str =
    "PILL_HOT_RELOADING, PILL_FUNCTION_HOT_PATCHING, PILL_ASSETS_HOT_RELOADING";

// =============================================================================
// The switches
// =============================================================================

/// Which watchers this run starts, read once when the host starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HostSwitches {
    /// Whether the project's and the extensions' source trees are watched.
    pub(crate) hot_reloading: bool,
    /// Whether an edit that arrives may be compiled into the running module.
    pub(crate) function_hot_patching: bool,
    /// Whether the project's assets and both shader trees are watched.
    pub(crate) assets_hot_reloading: bool,
}

impl HostSwitches {
    /// Reads the three variables, treating an absent one as on.
    pub(crate) fn from_environment() -> Self {
        Self {
            hot_reloading: switch_value(HOT_RELOADING_VARIABLE),
            function_hot_patching: switch_value(FUNCTION_HOT_PATCHING_VARIABLE),
            assets_hot_reloading: switch_value(ASSETS_HOT_RELOADING_VARIABLE),
        }
    }

    /// Writes the one line that says which posture this run is in.
    ///
    /// The three values are log fields, so a test can read the boolean it
    /// switched off without matching the whole sentence.
    pub(crate) fn log(&self) {
        info!(
            target: telemetry_target::ENGINE,
            hot_reloading = self.hot_reloading,
            function_hot_patching = self.function_hot_patching,
            assets_hot_reloading = self.assets_hot_reloading,
            variables = SWITCH_VARIABLES,
            "the hot reload switches"
        );
    }
}

/// One switch's answer, read from the environment.
///
/// Absent means on, which is what makes every switch default to the behaviour
/// the host had before the variables existed.
pub(crate) fn switch_value(variable: &str) -> bool {
    parse_switch(env::var(variable).ok().as_deref())
}

/// The rule behind every switch: absent, empty or any unrecognized text means
/// on; `0`, `false`, `no` and `off` mean off, in any case.
///
/// A value nobody recognizes stays on deliberately. Off is the answer that
/// stops something from happening, so it is the one that has to be asked for.
pub(crate) fn parse_switch(value: Option<&str>) -> bool {
    const OFF_VALUES: [&str; 4] = ["0", "false", "no", "off"];

    let text = value.map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return true;
    }
    !OFF_VALUES
        .iter()
        .any(|off_value| text.eq_ignore_ascii_case(off_value))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule every switch shares, one row per spelling a person or a script
    /// may reasonably set.
    #[test]
    fn only_the_documented_spellings_turn_a_switch_off() {
        let off = [
            Some("0"),
            Some("false"),
            Some("FALSE"),
            Some("No"),
            Some("off"),
            Some(" OFF "),
        ];
        let on = [
            None,
            Some(""),
            Some(" "),
            Some("1"),
            Some("true"),
            Some("yes"),
            Some("on"),
            Some(" true "),
            Some("enabled"),
            Some("2"),
        ];

        for value in off {
            assert!(
                !parse_switch(value),
                "{value:?} should switch the watcher off"
            );
        }
        for value in on {
            assert!(parse_switch(value), "{value:?} should leave the watcher on");
        }
    }

    /// An environment with none of the variables set is the default posture:
    /// every watcher runs.
    #[test]
    fn an_unset_environment_starts_every_watcher() {
        // The names are read through `env::var`, so this test uses names no
        // other test or suite sets.
        let switches = HostSwitches {
            hot_reloading: switch_value("PILL_UNSET_SWITCH_FOR_TESTS"),
            function_hot_patching: switch_value("PILL_UNSET_SWITCH_FOR_TESTS"),
            assets_hot_reloading: switch_value("PILL_UNSET_SWITCH_FOR_TESTS"),
        };

        assert_eq!(
            switches,
            HostSwitches {
                hot_reloading: true,
                function_hot_patching: true,
                assets_hot_reloading: true,
            }
        );
    }

    /// The startup line names every variable, so a person reading a log knows
    /// what to set to get the other posture.
    #[test]
    fn the_variable_list_names_all_three() {
        assert!(SWITCH_VARIABLES.contains(HOT_RELOADING_VARIABLE));
        assert!(SWITCH_VARIABLES.contains(FUNCTION_HOT_PATCHING_VARIABLE));
        assert!(SWITCH_VARIABLES.contains(ASSETS_HOT_RELOADING_VARIABLE));
    }
}
