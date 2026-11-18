//! The declaration attributes: `#[component]`, `#[resource]` and `#[asset]`.
//!
//! # Responsibilities
//!
//! - Prove that each attribute registers its type through the artifact entry
//!   point exactly as the derive it expands to does.
//! - Prove that `#[component(persistable)]` adds the serde derives and the
//!   schema-migration registration, and that a field decorator still rides the
//!   declaration.
//! - Prove that `#[resource(...)]` passes its arguments through, and that
//!   `#[asset(...)]` emits the accessor impl the type-erased store needs.
//!
//! # Design
//!
//! Every check reads an answer the engine itself gives - the declared field
//! layout, the world's resource ids, the import registry, the asset store - so
//! a test fails when the declaration stops reaching the engine, not when its
//! tokens change shape.

// External crates
use pill_engine::component_registry::{no_registration, register_all_components};
use pill_engine::{
    asset, component, resource, Asset, AssetLoadResult, AssetManager, Engine, ImportRegistry,
    ImportedAsset, PlayState, Resource, ResourceId, StandaloneAsset,
};

// =============================================================================
// Declarations
// =============================================================================

/// A persistable component: `persistable` is what asks for its serde derives,
/// and the field decorator below is what the Inspector draws.
#[component(persistable, shared = "attribute_declarations::PersistedProbe")]
#[derive(Debug, Clone, Copy, Default)]
struct PersistedProbe {
    /// A slider between the two values, declared the way the derive reads it.
    #[pill(range(0.0, 10.0))]
    intensity: f32,
}

/// A component that declares nothing at all, so the bare attribute compiles
/// and registers.
#[component]
#[derive(Debug, Clone, Copy)]
struct PlainProbe {
    /// A plain count.
    count: u32,
}

/// A resource whose arguments all reach the trait: a pinned shared name, the
/// file that defines its layout, and what a play session does with it.
#[resource(
    shared = "attribute_declarations::SessionProbe",
    layout_sources = ["src/attribute_declarations.rs"],
    play_state = "keep"
)]
#[derive(Clone)]
struct SessionProbe {
    /// Frames the session has run.
    frames: u64,
}

/// An imported asset: the attribute declares it and emits the accessor impl,
/// so nothing below repeats either.
#[asset(imported)]
#[derive(Debug, Default)]
struct NoteProbe {
    /// The name the importer was given.
    name: String,
}

impl Asset for NoteProbe {
    fn shared_name() -> Option<&'static str> {
        Some("attribute_declarations::NoteProbe")
    }
}

impl ImportedAsset for NoteProbe {
    type ImportSettings = NoteProbeSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["attribute_note"];

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
struct NoteProbeSettings {}

/// A standalone asset: its own document file, declared the same way.
#[asset(standalone)]
#[derive(Debug, Default)]
struct PaletteProbe {
    /// The colour names the document lists.
    entries: Vec<String>,
}

impl Asset for PaletteProbe {
    fn shared_name() -> Option<&'static str> {
        Some("attribute_declarations::PaletteProbe")
    }
}

impl StandaloneAsset for PaletteProbe {
    type Document = PaletteProbeDocument;
    const FILE_EXTENSION: &'static str = "attribute_palette";

    fn from_document(
        _name: &str,
        document: Self::Document,
        _assets: &AssetManager,
    ) -> AssetLoadResult<Self> {
        Ok(Self {
            entries: document.entries,
        })
    }
}

/// A palette's on-disk shape.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct PaletteProbeDocument {
    /// The colour names the document lists.
    entries: Vec<String>,
}

// =============================================================================
// Tests
// =============================================================================

/// Both attribute-declared components reach the registry, carrying the field
/// layout the derive records - including the decorator on the field.
#[test]
fn attribute_components_are_registered_by_the_entry_point() {
    let mut world = pill_engine::World::new();
    register_all_components(&mut world).expect("the attribute declarations register");

    let entity = world
        .create_entity()
        .with(PersistedProbe::default())
        .with(PlainProbe { count: 1 })
        .build()
        .expect("entity builds");
    let names = world
        .entity_component_names(entity)
        .expect("the entity reports the components it carries");
    assert!(names.iter().any(|name| name.ends_with("PersistedProbe")));
    assert!(names.iter().any(|name| name.ends_with("PlainProbe")));

    assert_eq!(PersistedProbe::FIELD_LAYOUT.len(), 1);
    assert_eq!(PersistedProbe::FIELD_LAYOUT[0].name, "intensity");
    assert_eq!(
        PersistedProbe::FIELD_LAYOUT[0].decorators.as_ref(),
        &[pill_engine::component_registry::FieldDecorator::Range {
            min: 0.0,
            max: 10.0,
        }]
    );
}

/// `persistable` is what feeds the schema fingerprint, so this binary's single
/// persistable component is exactly the name the fingerprint hashes.
#[test]
fn a_persistable_attribute_declaration_feeds_the_schema_fingerprint() {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "attribute_declarations::PersistedProbe".hash(&mut hasher);
    assert_eq!(
        pill_engine::component_registry::persistable_schema_fingerprint(),
        hasher.finish(),
        "the persistable attribute must reach the migration registry"
    );
}

/// The resource is declared by the entry point, and every argument reached the
/// trait the derive implements.
#[test]
fn an_attribute_resource_is_declared_and_carries_its_arguments() {
    let mut engine = Engine::new();
    let registration_sequence = engine.world().resource_registration_sequence();

    let status =
        pill_engine::component_registry::run_artifact_registration(&mut engine, no_registration);

    assert_eq!(status, 0);
    let declared = engine
        .world()
        .resource_ids_registered_since(registration_sequence);
    assert!(
        declared.contains(&ResourceId::of::<SessionProbe>()),
        "the entry point must declare the attribute's resource; declared: {declared:?}"
    );
    assert_eq!(
        SessionProbe::shared_name(),
        Some("attribute_declarations::SessionProbe")
    );
    assert_eq!(
        SessionProbe::layout_sources(),
        Some(["src/attribute_declarations.rs"].as_slice())
    );
    assert_eq!(SessionProbe::play_state(), PlayState::Keep);

    engine
        .world_mut()
        .insert_resource(SessionProbe { frames: 2 });
    assert_eq!(
        engine
            .world()
            .get_resource::<SessionProbe>()
            .map(|s| s.frames),
        Some(2)
    );
}

/// Both asset kinds reach the import registry through the entry point.
#[test]
fn attribute_asset_types_are_declared_by_the_entry_point() {
    let mut engine = Engine::new();

    let status =
        pill_engine::component_registry::run_artifact_registration(&mut engine, no_registration);

    assert_eq!(status, 0);
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .expect("declaring an imported asset stores the import registry");
    assert_eq!(
        registry.type_for_extension("attribute_note"),
        Some(<NoteProbe as ImportedAsset>::metadata_type_name()),
        "the imported asset must claim its source extension"
    );
    assert!(
        registry.is_standalone_extension("attribute_palette"),
        "the standalone asset must claim its document extension"
    );
}

/// The accessor impl the attribute emits is what lets the store hold the type:
/// every storing call is bound on `TraitAccessible<dyn Asset>`. The standalone
/// asset builds from its own document for the same reason.
#[test]
fn an_attribute_asset_is_stored_and_read_back_through_its_handle() {
    let mut assets = AssetManager::new();
    let handle = assets
        .add_named(
            "res/note.attribute_note",
            NoteProbe {
                name: "res/note.attribute_note".to_owned(),
            },
        )
        .expect("the name is free");

    let stored = assets
        .get(handle)
        .expect("the asset sits in the column its type registered");
    assert_eq!(stored.name, "res/note.attribute_note");
    assert!(assets.contains(handle));

    let palette = <PaletteProbe as StandaloneAsset>::from_document(
        "res/palette.attribute_palette",
        PaletteProbeDocument {
            entries: vec!["red".to_owned()],
        },
        &assets,
    )
    .expect("a palette builds from its own document");
    assert_eq!(palette.entries, ["red"]);
}
