//! The module-settings probe: three declared knobs and the manager that reads
//! them.
//!
//! # Responsibilities
//!
//! - Declares the knobs this module reads, under `modules.pill_test.fixture` in
//!   `project_settings.yaml`, so the host has a real entry to resolve.
//! - Holds the manager that applies those values, and applies them again
//!   whenever the host re-applies the project's settings.
//! - Prints what it applied, and why it refused a value, so the module-settings
//!   suite can see the live state through the host log.
//!
//! # Design
//!
//! The probe's whole job is to be observable. Its applier keeps what it reads,
//! logs one line per apply, and refuses a value it cannot use - a negative gain
//! must be a refusal, not a silent no-op, which is the failure a person cannot
//! see.
//!
//! The manager is a shared resource, so it holds one world id across a reload
//! and the host re-applies the project's values to the regenerated value.

// Standard library
use std::sync::atomic::{AtomicU32, Ordering};

// External crates
use pill_engine::{resource, Engine, ModuleSettings, PillSettings, SettingsOutcome};

// Current crate
use crate::{SETTINGS_APPLIED_TOKEN, SETTINGS_REFUSED_TOKEN};

// =============================================================================
// Constants
// =============================================================================

/// Counts applied values in this process.
static APPLIED_COUNT: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// FixtureSettings
// =============================================================================

/// The knobs this module declares, under `modules.pill_test.fixture`.
#[derive(Clone, Debug, PartialEq, PillSettings)]
#[pill(module = "pill_test", group = "fixture")]
pub struct FixtureSettings {
    /// A flag the project sets.
    #[pill(default = true, description = "Whether the fixture does its work.")]
    pub enabled: bool,
    /// A number the project sets.
    #[pill(default = 1.0, description = "The gain the fixture applies.")]
    pub gain: f32,
    /// A text value the project sets.
    #[pill(default = "fixture", description = "The label the fixture reports.")]
    pub label: String,
}

impl Default for FixtureSettings {
    /// The values a project that sets none of the three gets.
    fn default() -> Self {
        Self {
            enabled: true,
            gain: 1.0,
            label: "fixture".to_owned(),
        }
    }
}

// =============================================================================
// FixtureManager
// =============================================================================

/// The probe's manager: three knobs and an applier that reports them.
///
/// It is `Clone` because it is game state: Play copies the world's resources and
/// puts them back at Stop, so a resource that cannot be copied refuses Play by
/// name. The manager is plain data, so a copy is what Play needs.
#[resource(shared = "pill_test::FixtureManager")]
#[derive(Debug, Clone)]
pub struct FixtureManager {
    /// A flag the project sets.
    pub enabled: bool,
    /// A number the project sets.
    pub gain: f32,
    /// A text value the project sets.
    pub label: String,
}

impl Default for FixtureManager {
    /// The values a project that sets none of the three gets.
    fn default() -> Self {
        let settings = FixtureSettings::default();
        Self {
            enabled: settings.enabled,
            gain: settings.gain,
            label: settings.label,
        }
    }
}

impl ModuleSettings for FixtureManager {
    type Values = FixtureSettings;

    fn apply_settings(&mut self, settings: &FixtureSettings) -> SettingsOutcome {
        // A negative or non-finite gain is a value no sound could use, so the
        // applier refuses it and keeps what it had: a suite needs a refusal it
        // can see, and a person needs a wrong value not to pass silently.
        if settings.gain < 0.0 || !settings.gain.is_finite() {
            pill_engine::tracing::info!(
                "{SETTINGS_REFUSED_TOKEN} gain={} reason=out_of_range",
                settings.gain
            );
            return SettingsOutcome::Refused;
        }
        self.enabled = settings.enabled;
        self.gain = settings.gain;
        self.label = settings.label.clone();
        let applied_count = APPLIED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        pill_engine::tracing::info!(
            "{SETTINGS_APPLIED_TOKEN} enabled={} gain={} label={} applied_total={applied_count}",
            self.enabled,
            self.gain,
            self.label
        );
        SettingsOutcome::Applied
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Stores the probe's manager and declares its settings surface.
///
/// The value is inserted only when the world has none, so the manager keeps one
/// id across a reload. The surface is declared on every generation: it holds
/// function pointers into this image, so a reload has to re-point them whether
/// or not the value was re-inserted.
pub fn register(engine: &mut Engine) {
    pill_engine::register_settings::<FixtureManager>(engine);
    if engine.world().get_resource::<FixtureManager>().is_none() {
        engine
            .world_mut()
            .insert_resource(FixtureManager::default());
    }
}
