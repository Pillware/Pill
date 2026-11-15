//! A resource declared with `#[derive(PillResource)]` is declared by the
//! artifact entry point, with no call from the artifact.
//!
//! # Responsibilities
//!
//! - Prove that a derived resource is declared before the artifact's `register`
//!   half runs, and that storing a value afterwards still works.
//! - Prove that a manager resource declares its settings-asset surface from its
//!   own `register`, which runs on every generation.
//!
//! # Design
//!
//! The world stamps every resource registration with a sequence number, so the
//! test captures the sequence before the entry point runs and asks which ids
//! were declared since. That observes the declaration itself rather than the
//! value, which `insert_resource` would store either way.

// External crates
use pill_engine::component_registry::run_artifact_registration;
use pill_engine::{Engine, PillResource, ResourceId, SettingsAssetManager, SettingsOutcome};

// =============================================================================
// Declarations
// =============================================================================

/// A plain resource the artifact stores once the declaration is in place.
#[derive(PillResource)]
#[pill(shared = "automatic_resources::Countdown")]
struct Countdown {
    /// Frames left before the countdown ends.
    frames_left: u32,
}

/// A manager resource: the derive declares the value, and the registration
/// function below declares the settings asset it reads.
#[derive(PillResource)]
struct DeclaredManager {
    /// How many documents were applied to this value.
    applied_count: u32,
}

/// A game-state resource the derive makes copyable, so a play session can put
/// its edit-world value back at Stop.
#[derive(PillResource, Clone)]
#[pill(shared = "automatic_resources::SessionPoints")]
struct SessionPoints {
    /// The session's running total.
    points: u32,
}

/// The document a declared manager's settings asset holds.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct DeclaredManagerDocument {
    /// A number the applier reads.
    applied_count: u32,
}

/// The standalone asset a declared manager reads.
#[derive(Debug, Default)]
struct DeclaredManagerSettingsAsset {
    /// The settings the file holds.
    settings: DeclaredManagerDocument,
}

impl pill_engine::Asset for DeclaredManagerSettingsAsset {
    fn shared_name() -> Option<&'static str> {
        Some("automatic_resources::DeclaredManagerSettingsAsset")
    }
}

pill_engine::trait_type_map::impl_trait_accessible!(dyn pill_engine::Asset; DeclaredManagerSettingsAsset);

impl pill_engine::StandaloneAsset for DeclaredManagerSettingsAsset {
    type Document = DeclaredManagerDocument;
    const FILE_EXTENSION: &'static str = "declared_manager_settings";

    fn from_document(
        _name: &str,
        document: Self::Document,
        _assets: &pill_engine::AssetManager,
    ) -> pill_engine::AssetLoadResult<Self> {
        Ok(Self { settings: document })
    }
}

impl SettingsAssetManager for DeclaredManager {
    type Settings = DeclaredManagerSettingsAsset;

    fn settings_name() -> &'static str {
        "declared_manager"
    }

    fn apply_settings(&mut self, settings: &DeclaredManagerDocument) -> SettingsOutcome {
        self.applied_count = settings.applied_count;
        SettingsOutcome::Applied
    }
}

/// The asset builds from its document, which is what a load does.
#[test]
fn the_settings_asset_builds_from_its_document() {
    let asset = <DeclaredManagerSettingsAsset as pill_engine::StandaloneAsset>::from_document(
        "settings/declared_manager.declared_manager_settings",
        DeclaredManagerDocument { applied_count: 2 },
        &pill_engine::AssetManager::new(),
    )
    .expect("the document builds the asset");

    assert_eq!(asset.settings.applied_count, 2);
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

/// The half that declares a manager's settings-asset surface, as a module's own
/// `register` does on every generation.
fn declare_manager(engine: &mut Engine) -> u32 {
    engine
        .world_mut()
        .register_settings_asset_manager::<DeclaredManager>();
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

/// A manager resource declares its settings-asset surface from its own
/// `register`, which runs after the entry point's declarations.
#[test]
fn a_manager_declares_its_settings_asset_surface_from_register() {
    let mut engine = Engine::new();

    let status = run_artifact_registration(&mut engine, declare_manager);

    assert_eq!(status, 0);
    let surface = engine
        .world()
        .manager_settings_surface(ResourceId::of::<DeclaredManager>())
        .expect("a manager that declared its surface reports it");
    assert_eq!(surface.settings_name, "declared_manager");
    assert_eq!(
        surface.path,
        "settings/declared_manager.declared_manager_settings"
    );
    // The surface is reported without a value: a manager is only listed once
    // it is stored, which no declaration does on its own.
    assert!(engine.world().settings_asset_manager_ids().is_empty());
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
