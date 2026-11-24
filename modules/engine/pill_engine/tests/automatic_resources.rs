//! A resource declared with `#[resource]` is declared by the
//! artifact entry point, with no call from the artifact.
//!
//! # Responsibilities
//!
//! - Prove that a derived resource is declared before the artifact's `register`
//!   half runs, and that storing a value afterwards still works.
//! - Prove that a manager resource declares its settings surface from its own
//!   `register`, which runs on every generation.
//!
//! # Design
//!
//! The world stamps every resource registration with a sequence number, so the
//! test captures the sequence before the entry point runs and asks which ids
//! were declared since. That observes the declaration itself rather than the
//! value, which `insert_resource` would store either way.

// External crates
use pill_engine::component_registry::run_artifact_registration;
use pill_engine::{
    resource, DeclaredSettings, Engine, ModuleSettings, PillSettings, ResourceId, SettingsOutcome,
};

// =============================================================================
// Declarations
// =============================================================================

/// A plain resource the artifact stores once the declaration is in place.
#[resource(shared = "automatic_resources::Countdown")]
struct Countdown {
    /// Frames left before the countdown ends.
    frames_left: u32,
}

/// A manager resource: the derive declares the value, and the registration
/// function below declares the settings asset it reads.
#[resource]
struct DeclaredManager {
    /// How many documents were applied to this value.
    applied_count: u32,
}

/// A game-state resource the derive makes copyable, so a play session can put
/// its edit-world value back at Stop.
#[resource(shared = "automatic_resources::SessionPoints")]
#[derive(Clone)]
struct SessionPoints {
    /// The session's running total.
    points: u32,
}

/// The settings a declared manager reads.
#[derive(Clone, Debug, PillSettings)]
#[pill(module = "declared_manager", group = "probe")]
struct DeclaredManagerSettings {
    /// A number the applier reads.
    #[pill(default = 0, description = "How many documents the test applied.")]
    applied_count: u32,
}

impl Default for DeclaredManagerSettings {
    /// Nothing applied yet.
    fn default() -> Self {
        Self { applied_count: 0 }
    }
}

impl ModuleSettings for DeclaredManager {
    type Values = DeclaredManagerSettings;

    fn apply_settings(&mut self, settings: &DeclaredManagerSettings) -> SettingsOutcome {
        self.applied_count = settings.applied_count;
        SettingsOutcome::Applied
    }
}

// =============================================================================
// Tests
// =============================================================================

/// The registration half of the test artifact: it stores the value and checks
/// that the declaration arrived first.
fn store_countdown(engine: &mut Engine) -> u32 {
    engine
        .world_mut()
        .insert_resource(Countdown { frames_left: 3 });
    0
}

/// The half that declares a manager's settings surface, as a module's own
/// `register` does on every generation.
fn declare_manager(engine: &mut Engine) -> u32 {
    pill_engine::register_settings::<DeclaredManager>(engine);
    0
}

/// A derived resource is declared by the entry point, before `register` runs,
/// and `insert_resource` still stores a value for it.
#[test]
fn a_derived_resource_is_declared_by_the_entry_point() {
    let mut engine = Engine::new();
    let registration_sequence = engine.world().resource_registration_sequence();

    let status = run_artifact_registration(&mut engine, store_countdown);

    assert_eq!(status, 0);
    let declared = engine
        .world()
        .resource_ids_registered_since(registration_sequence);
    assert!(
        declared.contains(&ResourceId::of::<Countdown>()),
        "the entry point must declare the derived resource; declared: {declared:?}"
    );
    let stored = engine.world().get_resource::<Countdown>();
    assert_eq!(stored.map(|countdown| countdown.frames_left), Some(3));
}

/// A manager resource declares its settings surface from its own `register`,
/// which runs after the entry point's declarations.
#[test]
fn a_manager_declares_its_settings_surface_from_register() {
    let mut engine = Engine::new();

    let status = run_artifact_registration(&mut engine, declare_manager);

    assert_eq!(status, 0);
    let schemas = engine.module_settings_schemas();
    let schema = schemas
        .iter()
        .find(|schema| schema.module == "declared_manager")
        .expect("a manager that declared its surface reports it");
    assert_eq!(schema.settings.len(), 2, "`enabled` plus the declared row");
    assert_eq!(DeclaredManagerSettings::MODULE, "declared_manager");
    // The surface is declared without a value: a manager is only listed once
    // it is stored, which no declaration does on its own.
    assert!(engine.world().get_resource::<DeclaredManager>().is_none());
}

/// A derived resource that derives `Clone` records its copy hook at the type's
/// own site, so a play session copies it and Stop puts the edit-world value
/// back.
#[test]
fn a_derived_clone_resource_survives_a_play_round_trip() {
    let mut engine = Engine::new();
    engine.set_run_mode(pill_engine::RunMode::Editing);
    engine
        .world_mut()
        .insert_resource(SessionPoints { points: 3 });

    engine
        .enter_play()
        .expect("a derived `Clone` resource is copyable");

    engine
        .world_mut()
        .get_resource_mut::<SessionPoints>()
        .expect("the value is live")
        .points = 42;
    assert!(engine.exit_play(), "the session ends");

    assert_eq!(
        engine
            .world()
            .get_resource::<SessionPoints>()
            .map(|points| points.points),
        Some(3),
        "Stop puts the edit world's value back"
    );
    assert_eq!(engine.run_mode(), pill_engine::RunMode::Editing);
    assert_eq!(engine.world().restore_epoch(), 1);
}
