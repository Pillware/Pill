//! The manager's tuning document and its settings asset.
//!
//! # Responsibilities
//!
//! - Declares [`PhysicsSettings`], the values a project tunes in one file.
//! - Declares [`PhysicsSettingsAsset`], the `.physics_settings` file the values
//!   are read from and written to.
//! - Applies a loaded document to the running manager, refusing values the
//!   simulation cannot run with.
//!
//! # Design
//!
//! The document holds plain numbers, as every value on this crate's boundary
//! does. A file is hand-editable, so a value that would make the simulation
//! meaningless - a zero timestep, a negative gravity scale - is refused by name
//! rather than clamped, and the manager keeps what it had.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::*;
use serde::{Deserialize, Serialize};

// Current crate
pub use crate::manager::PhysicsManager;
use crate::manager::{DEFAULT_FIXED_TIMESTEP_SECONDS, DEFAULT_GRAVITY, MAXIMUM_SUBSTEPS_PER_FRAME};

// =============================================================================
// PhysicsSettings
// =============================================================================

/// The tuning of the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PhysicsSettings {
    /// World gravity in world units per second squared. The default is the
    /// earth's, pointing down.
    pub gravity: [f32; 3],
    /// The length of one simulation step in seconds. Every step advances the
    /// world by exactly this much, so the same scene settles the same way at
    /// any frame rate.
    pub fixed_timestep: f32,
    /// The most steps one frame may run, so a long frame cannot spiral into a
    /// longer one.
    pub max_substeps: u32,
    /// Whether a body's drawn pose is blended between its last two steps. It
    /// smooths motion at frame rates that are not a multiple of the step rate.
    pub interpolate: bool,
    /// Whether the colliders are drawn as outlines, for looking at what the
    /// simulation actually sees.
    pub debug_draw: bool,
}

impl std::default::Default for PhysicsSettings {
    /// World gravity, sixty steps a second, no interpolation and no outlines.
    fn default() -> Self {
        Self {
            gravity: DEFAULT_GRAVITY,
            fixed_timestep: DEFAULT_FIXED_TIMESTEP_SECONDS,
            max_substeps: MAXIMUM_SUBSTEPS_PER_FRAME,
            interpolate: false,
            debug_draw: false,
        }
    }
}

// =============================================================================
// PhysicsSettingsAsset
// =============================================================================

/// The `.physics_settings` document a project tunes the simulation through.
///
/// The file lives at `res/settings/pill_physics.physics_settings`. The loaded
/// value is only a carrier: the manager reads the document, not the asset.
///
/// The attribute declares the type, so every generation of the module registers
/// it with the world's asset store without a call in `register`.
#[asset(standalone)]
#[derive(Debug, Default)]
pub struct PhysicsSettingsAsset {
    /// The settings the file holds.
    pub settings: PhysicsSettings,
}

impl pill_engine::Asset for PhysicsSettingsAsset {
    fn shared_name() -> Option<&'static str> {
        Some("pill_physics::PhysicsSettingsAsset")
    }
}

impl pill_engine::StandaloneAsset for PhysicsSettingsAsset {
    type Document = PhysicsSettings;
    const FILE_EXTENSION: &'static str = "physics_settings";

    fn from_document(
        _name: &str,
        document: PhysicsSettings,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(Self { settings: document })
    }
}

// =============================================================================
// SettingsAssetManager
// =============================================================================

impl pill_engine::SettingsAssetManager for PhysicsManager {
    type Settings = PhysicsSettingsAsset;

    fn settings_name() -> &'static str {
        "pill_physics"
    }

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
        if settings.gravity.iter().any(|value| !value.is_finite()) {
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
        self.set_gravity(settings.gravity);
        self.set_fixed_timestep(settings.fixed_timestep, settings.max_substeps);
        let interpolate = settings.interpolate;
        let debug_draw = settings.debug_draw;
        self.set_interpolation(interpolate);
        self.set_debug_draw(debug_draw);
        pill_core::info!(
            target: telemetry_target::ECS,
            gravity_x = settings.gravity[0],
            gravity_y = settings.gravity[1],
            gravity_z = settings.gravity[2],
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
    use super::*;

    #[test]
    fn the_document_defaults_to_the_manager_s_defaults() {
        let settings = PhysicsSettings::default();
        assert_eq!(settings.gravity, DEFAULT_GRAVITY);
        assert_eq!(settings.fixed_timestep, DEFAULT_FIXED_TIMESTEP_SECONDS);
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
    fn a_document_moves_the_world_gravity_and_the_step_rate() {
        let mut manager = PhysicsManager::new();
        let settings = PhysicsSettings {
            gravity: [0.0, 0.0, 0.0],
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
