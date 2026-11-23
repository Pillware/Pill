//! Whether the frame's shadows are traced with hardware ray tracing.
//!
//! # Responsibilities
//!
//! - Carry the game's answer as one resource ([`RayTracedShadows`]).
//! - Read `PILL_RAY_TRACING` once, so a run that sets it starts with shadows on
//!   and a run that does not starts with today's shading.
//!
//! # Design
//!
//! The switch is renderer-neutral: it says what the game wants, not how it is
//! done. A renderer that cannot trace rays reads the flag and ignores it, which
//! is the same posture [`crate::SceneGrid`] takes for the editor's grid. The
//! flag travels in the frame as [`crate::RenderFrame::ray_traced_shadows`], so a
//! renderer never reaches back into the world for it.

// Standard library
use std::env;

// =============================================================================
// Constants
// =============================================================================

/// The variable that turns ray tracing on for a run (`PILL_RAY_TRACING`).
///
/// The GPU module reads the same variable to decide whether it opens a device
/// that can trace at all; a run where the device cannot trace draws today's
/// shading however this resource is set.
const RAY_TRACING_VARIABLE: &str = "PILL_RAY_TRACING";

// =============================================================================
// RayTracedShadows
// =============================================================================

/// Whether the game wants the frame's shadows traced with ray tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayTracedShadows {
    /// Whether ray-traced shadows are asked for.
    pub enabled: bool,
}

impl RayTracedShadows {
    /// Returns the switch a run starts with, from `PILL_RAY_TRACING`.
    ///
    /// `1` and `true`, in any case and trimmed, ask for it; anything else,
    /// including an unset variable, leaves it off. Off is the default, so a
    /// variable that only ever turns the path on cannot change a run by
    /// accident.
    pub fn from_environment() -> Self {
        Self {
            enabled: asked_for(env::var(RAY_TRACING_VARIABLE).ok().as_deref()),
        }
    }

    /// Returns the switch `text` - the variable's value - asks for.
    pub fn asked_for(text: Option<&str>) -> bool {
        asked_for(text)
    }
}

impl Default for RayTracedShadows {
    fn default() -> Self {
        // A world that inserts this resource without saying anything gets the
        // run's answer: ray tracing on when the environment asked for it.
        Self::from_environment()
    }
}

impl pill_engine::Resource for RayTracedShadows {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it, so a switch flipped mid-run survives a replay.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::RayTracedShadows")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Whether `text` asks for ray tracing.
fn asked_for(text: Option<&str>) -> bool {
    matches!(
        text.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("1" | "true")
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_that_says_one_or_true_asks_for_ray_tracing() {
        for value in ["1", "true", " TRUE ", "True", " 1 "] {
            assert!(asked_for(Some(value)), "{value:?} should ask for it");
        }
    }

    #[test]
    fn an_unset_or_unrecognized_variable_leaves_it_off() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("yes"),
            Some("on"),
        ] {
            assert!(!asked_for(value), "{value:?} should leave it off");
        }
    }

    #[test]
    fn the_switch_can_be_set_by_the_game() {
        let switch = RayTracedShadows { enabled: true };
        assert!(switch.enabled);
    }
}
