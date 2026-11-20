//! Asset previews on the editor's side: the preference that turns them on, and
//! the preview each listing row asks for.
//!
//! # Responsibilities
//!
//! - [`RENDER_PREVIEWS_KEY`] and [`render_previews`]: the preference a person
//!   turns on or off in the Preferences tab.
//! - [`preview_asset`]: which renderer preview a listing row asks for, from its
//!   asset type, its file and its guid.
//! - [`preview_asset_for`]: the same choice for the key the panel names a row
//!   by.
//!
//! # Design
//!
//! Nothing here renders. The renderer draws the preview and the executable
//! turns the returned pixels into an editor texture, so this module only
//! decides *what* to ask for.

// Standard library
use std::path::Path;

// External crates
use serde_json::{Map, Value};

// Current crate
use crate::editor_state::AssetListing;

// =============================================================================
// Constants
// =============================================================================

/// The preferences key that holds whether rendered asset previews are on.
pub const RENDER_PREVIEWS_KEY: &str = "editor.render_asset_previews";

// =============================================================================
// Free Functions
// =============================================================================

/// Whether the preferences document asks for rendered asset previews.
///
/// A stored `false` turns them off, whether it is a boolean or the text
/// `false` (letter case and surrounding spaces are ignored). Any other value
/// leaves them on, and a missing key does too, so a fresh preferences file
/// renders previews.
pub fn render_previews(preferences_document: &Map<String, Value>) -> bool {
    match crate::preferences::lookup(preferences_document, RENDER_PREVIEWS_KEY) {
        Some(Value::Bool(enabled)) => *enabled,
        Some(Value::String(text)) => !text.trim().eq_ignore_ascii_case("false"),
        _ => true,
    }
}

/// The preview a listing row asks for, when its type is a texture, a material
/// or a mesh.
///
/// Returns `None` for every other type.
///
/// A texture and a mesh are named by their file, below the root they come
/// from. A project may load such an asset under a name of its own or not load
/// it at all, so the file is what the renderer decodes. A material is named by
/// the guid the listing holds for it.
pub fn preview_asset(
    listing_row: &AssetListing,
    asset_root: &Path,
) -> Option<pill_renderer_api::PreviewAsset> {
    match listing_row.kind.type_leaf() {
        Some("Texture") => Some(pill_renderer_api::PreviewAsset::Texture {
            path: preview_file_path(listing_row, asset_root)?
                .to_string_lossy()
                .into_owned(),
        }),
        Some("Material") => Some(pill_renderer_api::PreviewAsset::Material {
            guid: listing_row.guid.clone()?,
        }),
        Some("Mesh") => Some(pill_renderer_api::PreviewAsset::Mesh {
            path: preview_file_path(listing_row, asset_root)?
                .to_string_lossy()
                .into_owned(),
        }),
        _ => None,
    }
}

/// The file a listing row's preview uses: its path below the project's asset
/// root, or below its own module's directory. A module's resources do not
/// live in the project at all.
///
/// Returns `None` when the row names a module that is not loaded. The
/// project's tree cannot stand in for it.
fn preview_file_path(listing_row: &AssetListing, asset_root: &Path) -> Option<std::path::PathBuf> {
    if listing_row.root.is_empty() {
        return Some(asset_root.join(&listing_row.path));
    }
    pill_engine::asset_store::module_roots()
        .into_iter()
        .find(|module_root| module_root.module == listing_row.root)
        .map(|module_root| module_root.directory.join(&listing_row.path))
}

/// The preview of the listing row that `listing_key` names.
///
/// `listing_key` is what the panel names a row by. It is the path for the
/// project's own tree, prefixed with the extensions root for a module's file.
/// That way, a module's texture and a project file that share a relative path
/// cannot be confused.
pub fn preview_asset_for<'a>(
    asset_listings: impl IntoIterator<Item = &'a AssetListing>,
    listing_key: &str,
    asset_root: &Path,
) -> Option<pill_renderer_api::PreviewAsset> {
    asset_listings
        .into_iter()
        .find(|listing_row| listing_row.key() == listing_key)
        .and_then(|listing_row| preview_asset(listing_row, asset_root))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::AssetKind;

    fn listing_row(path: &str, kind: &str, guid: Option<&str>) -> AssetListing {
        AssetListing {
            root: String::new(),
            path: path.to_string(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone: false,
            has_meta: guid.is_some(),
            guid: guid.map(str::to_string),
        }
    }

    /// The preference reads a boolean or a string. A missing key renders the
    /// previews, and only a stored `false` - as a boolean or as the text
    /// `false` - turns them off.
    #[test]
    fn the_preview_preference_reads_booleans_and_strings_and_defaults_on() {
        let mut preferences_document = Map::new();
        assert!(render_previews(&preferences_document));
        crate::preferences::set_key(
            &mut preferences_document,
            RENDER_PREVIEWS_KEY,
            Value::Bool(true),
        )
        .unwrap();
        assert!(render_previews(&preferences_document));
        crate::preferences::set_key(
            &mut preferences_document,
            RENDER_PREVIEWS_KEY,
            Value::Bool(false),
        )
        .unwrap();
        assert!(!render_previews(&preferences_document));
        crate::preferences::set_key(
            &mut preferences_document,
            RENDER_PREVIEWS_KEY,
            Value::String(" TRUE ".to_string()),
        )
        .unwrap();
        assert!(render_previews(&preferences_document));
        crate::preferences::set_key(
            &mut preferences_document,
            RENDER_PREVIEWS_KEY,
            Value::String("false".to_string()),
        )
        .unwrap();
        assert!(!render_previews(&preferences_document));
        // Anything that is not the text `false`, ignoring case and spaces,
        // leaves the previews on. A typo must not take the feature away from
        // someone who asked for it.
        crate::preferences::set_key(
            &mut preferences_document,
            RENDER_PREVIEWS_KEY,
            Value::String("yes".to_string()),
        )
        .unwrap();
        assert!(render_previews(&preferences_document));
    }

    /// A row's preview comes from its type's last segment. A texture and a
    /// mesh name their file below the asset root. A material names its guid,
    /// and a row whose type has no preview asks for nothing.
    #[test]
    fn a_row_names_the_preview_its_type_asks_for() {
        let asset_root = Path::new("C:/project/res");
        let asset_listings = vec![
            listing_row("textures/stone.png", "demo::Texture", Some("aa")),
            listing_row("materials/red.material", "demo::Material", Some("bb")),
            listing_row("models/cube.obj", "demo::Mesh", Some("cc")),
            listing_row("models/loose.obj", "demo::Mesh", None),
            listing_row("scenes/main.scene", "demo::Scene", Some("dd")),
        ];
        assert_eq!(
            preview_asset_for(&asset_listings, "textures/stone.png", asset_root),
            Some(pill_renderer_api::PreviewAsset::Texture {
                path: asset_root
                    .join("textures/stone.png")
                    .to_string_lossy()
                    .into_owned()
            })
        );
        assert_eq!(
            preview_asset_for(&asset_listings, "materials/red.material", asset_root),
            Some(pill_renderer_api::PreviewAsset::Material {
                guid: "bb".to_string()
            })
        );
        assert_eq!(
            preview_asset_for(&asset_listings, "models/cube.obj", asset_root),
            Some(pill_renderer_api::PreviewAsset::Mesh {
                path: asset_root
                    .join("models/cube.obj")
                    .to_string_lossy()
                    .into_owned()
            })
        );
        // A mesh needs no `.meta` sidecar. The renderer decodes the file itself.
        assert_eq!(
            preview_asset_for(&asset_listings, "models/loose.obj", asset_root),
            Some(pill_renderer_api::PreviewAsset::Mesh {
                path: asset_root
                    .join("models/loose.obj")
                    .to_string_lossy()
                    .into_owned()
            })
        );
        assert!(preview_asset_for(&asset_listings, "scenes/main.scene", asset_root).is_none());
        assert!(preview_asset_for(&asset_listings, "textures/missing.png", asset_root).is_none());
    }

    /// A module row's key starts with the extensions root. A row naming a
    /// module that is not loaded asks for nothing rather than reading the
    /// project's tree at the same path.
    #[test]
    fn a_modules_file_never_resolves_into_the_projects_tree() {
        let asset_root = Path::new("C:/project/res");
        let mut module_row = listing_row("textures/sky.png", "demo::Texture", Some("aa"));
        module_row.root = "a-module-that-is-not-loaded".to_owned();
        let listing_key = module_row.key();
        assert_eq!(
            listing_key,
            "extensions/a-module-that-is-not-loaded/textures/sky.png"
        );
        assert_eq!(preview_asset(&module_row, asset_root), None);
        assert_eq!(
            preview_asset_for(&[module_row], &listing_key, asset_root),
            None
        );
    }
}
