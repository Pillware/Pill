//! Whether the frame's shadows are traced with hardware ray tracing.
//!
//! # Responsibilities
//!
//! - Carry the game's answer as one resource ([`RayTracedShadows`]).
//!
//! # Design
//!
//! The switch is renderer-neutral: it says what the game wants, not how it is
//! done. A renderer that cannot trace rays reads the flag and ignores it, which
//! is the same posture [`crate::editor::SceneGrid`] takes for the editor's grid. The
//! flag travels in the frame as [`crate::RenderFrame::ray_traced_shadows`], so a
//! renderer never reaches back into the world for it.
//!
//! Whether a build can trace at all is the build's own answer: a renderer
//! compiled with its ray tracing option opens a device that can trace whenever
//! the adapter supports it, and the game decides per frame with this flag. No
//! environment variable takes part in the decision.

// =============================================================================
// RayTracedShadows
// =============================================================================

/// Whether the game wants the frame's shadows traced with ray tracing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayTracedShadows {
    /// Whether ray-traced shadows are asked for.
    pub enabled: bool,
}

impl Default for RayTracedShadows {
    /// Shadows start traced: a build that can trace draws them, and one that
    /// cannot ignores the flag.
    fn default() -> Self {
        Self { enabled: true }
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
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadows_start_traced() {
        assert!(RayTracedShadows::default().enabled);
    }

    #[test]
    fn the_switch_can_be_set_by_the_game() {
        let switch = RayTracedShadows { enabled: false };
        assert!(!switch.enabled);
    }
}
