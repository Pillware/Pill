//! A manager whose tuning comes from a settings asset of its own type.
//!
//! # Responsibilities
//!
//! - Declares a standalone settings asset (`.fixture_settings`) whose document
//!   is the three knobs the manager reads, so the host has a real asset to
//!   create from the defaults and re-apply on an edit.
//! - Declares the manager that applies that document.
//! - Prints the values it is asked to apply, and why it refused a document, so
//!   the manager-settings suite can see the live state through the host log.
//!
//! # Design
//!
//! The fixture's whole job is to be observable. Its applier keeps what it reads,
//! logs one line per apply, and refuses a value it cannot use - a negative gain
//! must be a refusal, not a silent no-op, which is the failure a person cannot
//! see.
//!
//! It is a shared resource, so the value survives a reload: the host then
//! re-applies the settings asset to the regenerated manager, which is what the
//! suite checks after an unrelated source edit.

// Standard library
use std::sync::atomic::{AtomicU32, Ordering};

// External crates
use pill_engine::{asset, resource, AssetLoadResult, AssetManager, Engine, SettingsOutcome};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// Printed by the applier, once per applied document.
pub const APPLIED_TOKEN: &str = "[test] fixture manager applied";

/// Printed by the applier when it refuses a document.
pub const REFUSED_TOKEN: &str = "[test] fixture manager refused";

/// Counts applied documents in this process.
static APPLIED_COUNT: AtomicU32 = AtomicU32::new(0);

// =============================================================================
// FixtureSettings
// =============================================================================

/// The document a fixture settings asset holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FixtureSettings {
    /// A flag the document sets.
    pub enabled: bool,
    /// A number the document sets.
    pub gain: f32,
    /// A text value the document sets.
    pub label: String,
}

impl Default for FixtureSettings {
    /// The values a newly created file holds.
    fn default() -> Self {
        Self {
            enabled: true,
            gain: 1.0,
            label: "fixture".to_owned(),
        }
    }
}

// =============================================================================
// FixtureSettingsAsset
// =============================================================================

/// The standalone asset a project tunes the fixture manager through.
///
/// Its document is [`FixtureSettings`], so the file at
/// `res/settings/fixture_manager.fixture_settings` names the same three knobs
/// the manager applies. The loaded value carries nothing: a settings file is
/// applied from its document, and the asset type is what gives the file its
/// identity in the store.
#[asset(standalone)]
#[derive(Debug, Default)]
pub struct FixtureSettingsAsset;

impl pill_engine::Asset for FixtureSettingsAsset {
    fn shared_name() -> Option<&'static str> {
        Some("native::FixtureSettingsAsset")
    }
}

impl pill_engine::StandaloneAsset for FixtureSettingsAsset {
    type Document = FixtureSettings;
    const FILE_EXTENSION: &'static str = "fixture_settings";

    fn from_document(
        _name: &str,
        _document: FixtureSettings,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(Self)
    }
}

// =============================================================================
// FixtureManager
// =============================================================================

/// The fixture's manager: three knobs and an applier that reports them.
///
/// It keeps a shared name, so it holds one world id across a reload: an id
/// built from `TypeId` differs in every generation, and the value would be
/// retired instead of kept.
#[resource(shared = "native::FixtureManager")]
pub struct FixtureManager {
    /// A flag the document sets.
    pub enabled: bool,
    /// A number the document sets.
    pub gain: f32,
    /// A text value the document sets.
    pub label: String,
}

impl FixtureManager {
    /// The values a new settings asset starts from.
    pub fn new() -> Self {
        let settings = FixtureSettings::default();
        Self {
            enabled: settings.enabled,
            gain: settings.gain,
            label: settings.label,
        }
    }
}

impl Default for FixtureManager {
    fn default() -> Self {
        Self::new()
    }
}

impl pill_engine::SettingsAssetManager for FixtureManager {
    type Settings = FixtureSettingsAsset;

    fn settings_name() -> &'static str {
        "fixture_manager"
    }

    fn apply_settings(&mut self, settings: &FixtureSettings) -> SettingsOutcome {
        // A negative or non-finite gain is a value no sound could use, so the
        // applier refuses it and keeps what it had: a suite needs a refusal it
        // can see, and a person needs a wrong value not to pass silently.
        if settings.gain < 0.0 || !settings.gain.is_finite() {
            pill_engine::tracing::info!(
                "{REFUSED_TOKEN} gain={} reason=out_of_range",
                settings.gain
            );
            return SettingsOutcome::Refused;
        }
        self.enabled = settings.enabled;
        self.gain = settings.gain;
        self.label = settings.label.clone();
        let applied_count = APPLIED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        pill_engine::tracing::info!(
            "{APPLIED_TOKEN} enabled={} gain={} label={} applied_total={applied_count}",
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

/// Stores the fixture manager, keeping a live value across a reload.
///
/// The value is inserted only when the world has none, so a kept manager
/// survives a reload. The settings-asset surface is declared on every
/// generation: it holds function pointers into this image, so a reload has to
/// re-point them whether or not the value was re-inserted.
pub fn register(engine: &mut Engine) {
    engine
        .world_mut()
        .register_settings_asset_manager::<FixtureManager>();
    if engine.world().get_resource::<FixtureManager>().is_none() {
        engine
            .world_mut()
            .insert_settings_asset_manager(FixtureManager::new());
    }
}
