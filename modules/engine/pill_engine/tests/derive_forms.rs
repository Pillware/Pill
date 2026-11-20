//! The derive form of the three declaration macros still declares types.
//!
//! # Responsibilities
//!
//! - Keep one component, one resource and one asset on
//!   `#[derive(PillComponent)]` / `#[derive(PillResource)]` / `#[derive(PillAsset)]`,
//!   so the form the declaration attributes expand to stays a working choice.
//! - Prove that each one still registers through the artifact entry point,
//!   exactly as its attribute form does.
//!
//! # Design
//!
//! The derive form is the documented expansion of `#[component]`,
//! `#[resource]` and `#[asset]`, so it is also what a generated file - a patch
//! stub, a migration edit written by a suite - can rely on. Every other test
//! declares with the attribute; this file is the one that keeps the derive
//! covered.

// External crates
use pill_engine::component_registry::{no_registration, register_all_components};
use pill_engine::{
    Asset, AssetLoadResult, AssetManager, Component, Engine, ImportRegistry, ImportedAsset,
    PillAsset, PillComponent, PillResource, Resource, ResourceId,
};

// =============================================================================
// Declarations
// =============================================================================

/// A component on the derive form.
#[derive(Debug, Clone, Copy, Default, PillComponent)]
#[pill(shared = "derive_forms::DerivedProbe")]
struct DerivedProbe {
    /// A plain value.
    value: f32,
}

/// A resource on the derive form.
#[derive(Clone, PillResource)]
#[pill(shared = "derive_forms::DerivedSession")]
struct DerivedSession {
    /// Frames the session has run.
    frames: u64,
}

/// An imported asset on the derive form.
#[derive(Debug, Default, PillAsset)]
#[pill(imported)]
struct DerivedNote {
    /// The name the importer was given.
    name: String,
}

// Registers the derived asset type with the type-erased columns. The derive
// form writes this line itself; the `#[asset]` form emits it.
trait_type_map::impl_trait_accessible!(dyn Asset; DerivedNote);

impl Asset for DerivedNote {
    fn shared_name() -> Option<&'static str> {
        Some("derive_forms::DerivedNote")
    }
}

impl ImportedAsset for DerivedNote {
    type ImportSettings = DerivedNoteSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["derived_note"];

    fn import(
        name: &str,
        _source_bytes: &[u8],
        _settings: &Self::ImportSettings,
    ) -> AssetLoadResult<Self> {
        Ok(Self {
            name: name.to_owned(),
        })
    }
}

/// Everything a note's source bytes do not say for themselves.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct DerivedNoteSettings {}

// =============================================================================
// Tests
// =============================================================================

/// The derived component is still registered by `register_all_components`.
#[test]
fn a_derived_component_is_registered() {
    let mut world = pill_engine::World::new();
    register_all_components(&mut world).expect("the derive declarations register");

    let entity = world
        .create_entity()
        .with(DerivedProbe { value: 1.5 })
        .build()
        .expect("entity builds");
    let names = world
        .entity_component_names(entity)
        .expect("the entity reports the components it carries");
    assert!(names.iter().any(|name| name.ends_with("DerivedProbe")));
    assert_eq!(
        DerivedProbe::shared_name(),
        Some("derive_forms::DerivedProbe")
    );
}

/// The derived resource is still declared by the artifact entry point.
#[test]
fn a_derived_resource_is_declared_by_the_entry_point() {
    let mut engine = Engine::new();
    let registration_sequence = engine.world().resource_registration_sequence();

    let status =
        pill_engine::component_registry::run_artifact_registration(&mut engine, no_registration);

    assert_eq!(status, 0);
    let declared = engine
        .world()
        .resource_ids_registered_since(registration_sequence);
    assert!(
        declared.contains(&ResourceId::of::<DerivedSession>()),
        "the entry point must declare the derived resource; declared: {declared:?}"
    );
    assert_eq!(
        DerivedSession::shared_name(),
        Some("derive_forms::DerivedSession")
    );

    engine
        .world_mut()
        .insert_resource(DerivedSession { frames: 4 });
    assert_eq!(
        engine
            .world()
            .get_resource::<DerivedSession>()
            .map(|session| session.frames),
        Some(4)
    );
}

/// The derived asset is still declared, and still usable through the store.
#[test]
fn a_derived_asset_is_declared_and_stored() {
    let mut engine = Engine::new();
    let status =
        pill_engine::component_registry::run_artifact_registration(&mut engine, no_registration);
    assert_eq!(status, 0);
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .expect("declaring an imported asset stores the import registry");
    assert_eq!(
        registry.type_for_extension("derived_note"),
        Some(<DerivedNote as ImportedAsset>::metadata_type_name())
    );

    let mut assets = AssetManager::new();
    let handle = assets
        .add_named(
            "res/note.derived_note",
            DerivedNote {
                name: "res/note.derived_note".to_owned(),
            },
        )
        .expect("the name is free");
    let stored = assets
        .get(handle)
        .expect("the asset sits in the column its type registered");
    assert_eq!(stored.name, "res/note.derived_note");
}
