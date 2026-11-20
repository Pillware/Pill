//! An asset type declared with `#[asset]` is declared by the
//! artifact entry point, with no call from the artifact that owns it.
//!
//! # Responsibilities
//!
//! - Prove that a derived imported asset reaches the world's import registry,
//!   so a source file with its extension imports as that type.
//! - Prove that a derived standalone asset reaches it too, so its own document
//!   files load by extension.
//!
//! # Design
//!
//! The two declarations are read back from the import registry, which is the
//! engine's own answer to "what does this extension import as". Asking it
//! proves the registration ran; asserting on the entry point's return value
//! would only prove it did not fail. Both directions of the trait - imported
//! and standalone - are covered because a type is one or the other and the
//! derive has to pick the right registration call for each.

// External crates
use pill_engine::component_registry::{no_registration, run_artifact_registration};
use pill_engine::{
    asset, Asset, AssetLoadResult, AssetManager, Engine, ImportRegistry, ImportedAsset,
    StandaloneAsset,
};

// =============================================================================
// Declarations
// =============================================================================

/// An imported asset: its source file carries the type's contents.
#[asset(imported)]
#[derive(Debug, Default)]
struct Note {
    /// The name the importer was given.
    name: String,
}

impl Asset for Note {
    fn shared_name() -> Option<&'static str> {
        Some("automatic_assets::Note")
    }
}

impl ImportedAsset for Note {
    type ImportSettings = NoteSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["auto_note"];

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
struct NoteSettings {}

/// A standalone asset: it is its own document file, with no source beside it.
#[asset(standalone)]
#[derive(Debug, Default)]
struct Palette {
    /// The colour names the document lists.
    entries: Vec<String>,
}

impl Asset for Palette {
    fn shared_name() -> Option<&'static str> {
        Some("automatic_assets::Palette")
    }
}

impl StandaloneAsset for Palette {
    type Document = PaletteDocument;
    const FILE_EXTENSION: &'static str = "auto_palette";

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
struct PaletteDocument {
    /// The colour names the document lists.
    entries: Vec<String>,
}

// =============================================================================
// Tests
// =============================================================================

/// Both declared asset types reach the import registry through the entry
/// point, which is what lets their files load by extension.
#[test]
fn derived_asset_types_are_declared_by_the_entry_point() {
    let mut engine = Engine::new();

    let status = run_artifact_registration(&mut engine, no_registration);

    assert_eq!(status, 0);
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .expect("declaring an imported asset stores the import registry");
    assert_eq!(
        registry.type_for_extension("auto_note"),
        Some(<Note as ImportedAsset>::metadata_type_name()),
        "the imported asset must claim its source extension"
    );
    assert!(
        registry.is_standalone_extension("auto_palette"),
        "the standalone asset must claim its document extension"
    );
}

/// The two traits still do what the types were written to do: the imported
/// type decodes a name, the standalone one builds from its document. This is
/// what the declarations above point the registry at.
#[test]
fn the_declared_types_still_import_and_build() {
    let note = <Note as ImportedAsset>::import("res/note.auto_note", b"", &NoteSettings::default())
        .expect("an imported note reads its own bytes");
    assert_eq!(note.name, "res/note.auto_note");

    let assets = AssetManager::new();
    let palette = <Palette as StandaloneAsset>::from_document(
        "res/palette.auto_palette",
        PaletteDocument {
            entries: vec!["red".to_owned()],
        },
        &assets,
    )
    .expect("a palette builds from its own document");
    assert_eq!(palette.entries, ["red"]);
}
