//! The simulation's declared settings.
//!
//! # Responsibilities
//!
//! - Declares [`PhysicsSettings`], the values a project tunes under
//!   `modules.pill_physics.physics`.
//! - Applies a resolved value to the running manager, refusing values the
//!   simulation cannot run with.
//!
//! # Design
//!
//! Every value is a plain scalar, because a declared setting is one row: world
//! gravity is three of them, and [`PhysicsSettings::gravity`] composes the
//! vector the simulation reads. A value that would make the simulation
//! meaningless - a zero timestep, a gravity that is not a number - is refused by
//! name rather than clamped, and the manager keeps what it had.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{PillSettings, SettingsOutcome};

// Current crate
pub use crate::manager::PhysicsManager;
use crate::manager::{DEFAULT_FIXED_TIMESTEP_SECONDS, DEFAULT_GRAVITY, MAXIMUM_SUBSTEPS_PER_FRAME};

// =============================================================================
// PhysicsSettings
// =============================================================================

/// The tuning of the simulation.
#[derive(Debug, Clone, Copy, PartialEq, PillSettings)]
#[pill(module = "pill_physics", group = "physics")]
pub struct PhysicsSettings {
    /// World gravity along x, in world units per second squared.
    #[pill(
        default = 0.0,
        description = "World gravity along x, in world units per second squared."
    )]
    pub gravity_x: f32,
    /// World gravity along y, in world units per second squared.
    #[pill(
        default = -9.81,
        description = "World gravity along y, in world units per second squared. A negative value points down."
    )]
    pub gravity_y: f32,
    /// World gravity along z, in world units per second squared.
    #[pill(
        default = 0.0,
        description = "World gravity along z, in world units per second squared."
    )]
    pub gravity_z: f32,
    /// The length of one simulation step in seconds.
    #[pill(
        default = 0.016666668,
        min = 0.0001,
        description = "The length of one simulation step in seconds. Every frame advances the world by whole steps of exactly this length."
    )]
    pub fixed_timestep: f32,
    /// The most steps one frame may run.
    #[pill(
        default = 8,
        min = 1,
        description = "The most simulation steps one frame may run, so a long frame cannot spiral into a longer one."
    )]
    pub max_substeps: u32,
    /// Whether a body's drawn pose is blended between its last two steps.
    #[pill(
        default = false,
        description = "Whether a body's drawn pose is blended between its last two steps, which smooths motion at frame rates that are not a multiple of the step rate."
    )]
    pub interpolate: bool,
    /// Whether colliders are drawn as outlines.
    #[pill(
        default = false,
        description = "Whether every collider is drawn as an outline, for looking at what the simulation sees."
    )]
    pub debug_draw: bool,
}

impl PhysicsSettings {
    /// World gravity as the simulation reads it.
    pub fn gravity(&self) -> [f32; 3] {
        [self.gravity_x, self.gravity_y, self.gravity_z]
    }
}

impl std::default::Default for PhysicsSettings {
    /// World gravity, sixty steps a second, no interpolation and no outlines.
    fn default() -> Self {
        Self {
            gravity_x: DEFAULT_GRAVITY[0],
            gravity_y: DEFAULT_GRAVITY[1],
            gravity_z: DEFAULT_GRAVITY[2],
            fixed_timestep: DEFAULT_FIXED_TIMESTEP_SECONDS,
            max_substeps: MAXIMUM_SUBSTEPS_PER_FRAME,
            interpolate: false,
            debug_draw: false,
        }
    }
}

// =============================================================================
// ModuleSettings
// =============================================================================

impl pill_engine::ModuleSettings for PhysicsManager {
    type Values = PhysicsSettings;

    fn apply_settings(&mut self, settings: &PhysicsSettings) -> SettingsOutcome {
        // Step 1: Refuse values no simulation can run with, naming the field so
        // a typo in the file is easy to find.
        if !settings.fixed_timestep.is_finite() || settings.fixed_timestep <= 0.0 {
            pill_core::warn!(
                target: telemetry_target::ECS,
                manager = "pill_physics",
                error = "fixed_timestep must be greater than 0",
                "[settings] physics settings refused; the manager keeps its values"
            );
            return SettingsOutcome::Refused;
        }
        if settings.max_substeps == 0 {
            pill_core::warn!(
                target: telemetry_target::ECS,
                manager = "pill_physics",
                error = "max_substeps must be at least 1",
                "[settings] physics settings refused; the manager keeps its values"
            );
            return SettingsOutcome::Refused;
        }
        let gravity = settings.gravity();
        if gravity.iter().any(|value| !value.is_finite()) {
            pill_core::warn!(
                target: telemetry_target::ECS,
                manager = "pill_physics",
                error = "gravity must be finite",
                "[settings] physics settings refused; the manager keeps its values"
            );
            return SettingsOutcome::Refused;
        }

        // Step 2: Everything a running simulation can take, takes effect at
        // once: gravity, the step rate, the interpolation switch and the
        // outlines.
        self.set_gravity(gravity);
        self.set_fixed_timestep(settings.fixed_timestep, settings.max_substeps);
        let interpolate = settings.interpolate;
        let debug_draw = settings.debug_draw;
        self.set_interpolation(interpolate);
        self.set_debug_draw(debug_draw);
        pill_core::info!(
            target: telemetry_target::ECS,
            gravity_x = gravity[0],
            gravity_y = gravity[1],
            gravity_z = gravity[2],
            fixed_timestep = settings.fixed_timestep,
            max_substeps = settings.max_substeps,
            interpolate,
            debug_draw,
            "[settings] physics settings applied"
        );
        SettingsOutcome::Applied
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use pill_engine::ModuleSettings;

    use super::*;

    #[test]
    fn the_declared_defaults_are_the_manager_s() {
        let settings = PhysicsSettings::default();
        assert_eq!(settings.gravity(), DEFAULT_GRAVITY);
        assert_eq!(settings.fixed_timestep, DEFAULT_FIXED_TIMESTEP_SECONDS);
        assert_eq!(settings.max_substeps, MAXIMUM_SUBSTEPS_PER_FRAME);
        assert!(!settings.interpolate);
        assert!(!settings.debug_draw);
    }

    #[test]
    fn a_zero_timestep_is_refused() {
        let mut manager = PhysicsManager::new();
        let settings = PhysicsSettings {
            fixed_timestep: 0.0,
            ..PhysicsSettings::default()
        };
        assert_eq!(manager.apply_settings(&settings), SettingsOutcome::Refused);
        assert_eq!(manager.fixed_timestep(), DEFAULT_FIXED_TIMESTEP_SECONDS);
    }

    #[test]
    fn a_setting_moves_the_world_gravity_and_the_step_rate() {
        let mut manager = PhysicsManager::new();
        let settings = PhysicsSettings {
            gravity_x: 0.0,
            gravity_y: 0.0,
            gravity_z: 0.0,
            fixed_timestep: 1.0 / 30.0,
            max_substeps: 4,
            interpolate: true,
            debug_draw: true,
        };
        assert_eq!(manager.apply_settings(&settings), SettingsOutcome::Applied);
        assert_eq!(manager.gravity(), [0.0, 0.0, 0.0]);
        assert_eq!(manager.fixed_timestep(), 1.0 / 30.0);
        assert!(manager.interpolation_enabled());
        assert!(manager.debug_draw_enabled());
    }
}
