//! A module's settings are declared once, and the entry point registers them
//! with the engine.
//!
//! # Responsibilities
//!
//! - Prove that `#[derive(PillSettings)]` writes the schema the engine reads:
//!   `enabled` first, then the decorated fields with their kinds and ranges.
//! - Prove that registering a surface twice keeps one entry, which is what a
//!   reload needs.
//! - Prove that resolved values reach a live manager, and that a value nobody
//!   sets keeps the declared default.
//!
//! # Design
//!
//! The test declares a small settings struct and a manager over it, registers
//! both by hand, and resolves a project document. Nothing here loads a DLL: the
//! declaration and the resolution are the same calls a module's registration
//! half makes.

// External crates
use pill_engine::{
    register_settings, resolve, resource, DeclaredSettings, Engine, ModuleSettings,
    ResolvedSettings, SettingDefault, SettingKind, SettingsDocument, SettingsEnvironment,
    SettingsOutcome, SettingsResolutionInput,
};

// =============================================================================
// Declarations
// =============================================================================

/// The settings a fixture module declares.
#[derive(Clone, Debug, pill_engine::PillSettings)]
#[pill(module = "pill_fixture", group = "experiment")]
struct FixtureSettings {
    /// A bounded whole number.
    #[pill(default = 4, min = 1, max = 8, description = "How loud the probe is.")]
    gain: u32,
    /// A line of text with a declared default.
    #[pill(default = "probe", description = "The name the probe reports.")]
    label: String,
    /// A flag that applies without a restart.
    #[pill(
        default = true,
        timing = "live",
        description = "Whether the probe runs."
    )]
    active: bool,
}

/// The manager that reads the settings.
#[resource]
#[derive(Debug, Default)]
struct FixtureManager {
    /// The applied gain.
    gain: u32,
    /// The applied label.
    label: String,
    /// The applied flag.
    active: bool,
}

impl ModuleSettings for FixtureManager {
    type Values = FixtureSettings;

    fn apply_settings(&mut self, values: &FixtureSettings) -> SettingsOutcome {
        self.gain = values.gain;
        self.label = values.label.clone();
        self.active = values.active;
        SettingsOutcome::Applied
    }
}

/// A project document that sets the gain to a number in range.
fn document() -> SettingsDocument {
    SettingsDocument::from_yaml_text(
        "modules:\n  pill_fixture:\n    enabled: true\n    experiment:\n      gain: 6\n",
    )
    .expect("the document reads")
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn a_declared_surface_carries_enabled_first_and_its_field_kinds() {
    let schema = FixtureSettings::schema();

    assert_eq!(schema.module, "pill_fixture");
    assert_eq!(schema.settings.len(), 4);
    assert_eq!(schema.settings[0].key, "enabled");
    assert_eq!(schema.settings[0].default, SettingDefault::Bool(false));
    assert_eq!(
        schema.spec("experiment.gain").map(|spec| spec.kind()),
        Some(SettingKind::Integer)
    );
    assert_eq!(
        schema.spec("experiment.gain").and_then(|spec| spec.minimum),
        Some(1.0)
    );
    assert_eq!(
        schema.spec("experiment.label").map(|spec| spec.kind()),
        Some(SettingKind::Text)
    );
    assert_eq!(
        schema.spec("experiment.active").map(|spec| spec.timing),
        Some(pill_engine::ApplyTiming::Live)
    );
}

#[test]
fn registering_a_surface_twice_keeps_one_entry() {
    let mut engine = Engine::new();
    register_settings::<FixtureManager>(&mut engine);
    register_settings::<FixtureManager>(&mut engine);

    assert_eq!(engine.module_settings_schemas().len(), 1);
    assert!(engine.has_module_settings("pill_fixture"));
}

#[test]
fn the_resolved_values_reach_the_live_manager() {
    let schemas = [FixtureSettings::schema()];
    let project = document();
    let resolved: ResolvedSettings = resolve(SettingsResolutionInput {
        schemas: &schemas,
        loaded_entries: &["pill_fixture"],
        disabled_entries: &[],
        deferred_entries: &[],
        project: Some(&project),
        local: None,
        environment: &SettingsEnvironment::empty(),
        strict: true,
    })
    .expect("the document declares only known settings");

    let mut engine = Engine::new();
    engine
        .world_mut()
        .insert_resource(FixtureManager::default());
    register_settings::<FixtureManager>(&mut engine);

    let outcomes = engine.apply_resolved_settings(&resolved);
    assert_eq!(outcomes, vec![("pill_fixture", SettingsOutcome::Applied)]);

    let manager = engine
        .world_mut()
        .get_resource_mut::<FixtureManager>()
        .expect("the manager is live");
    assert_eq!(manager.gain, 6);
    assert_eq!(manager.label, "probe");
    assert!(manager.active);
}
