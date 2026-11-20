//! Loads the mesh, the texture and the material the pills are drawn with.
//!
//! # Responsibilities
//!
//! - Import the pill mesh, using the settings from its metadata file.
//! - Import the texture the pill material uses, then load the material from
//!   its `.material` file.
//! - Return the handles the scene draws with, as [`SceneAssets`].
//!
//! # Design
//!
//! The material is a file in `res/materials`. It names its shader and its
//! texture by guid, so it can be edited in the editor, and the development
//! host reimports it when the file changes. There is no skybox and no level
//! geometry to load: the pills are the whole scene.
//!
//! Importing a path that is already loaded returns the existing asset instead
//! of a second copy, so this code can run again on every reload.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::{AssetImport, AssetImportError, AssetManager, Handle, MetadataPolicy, World};
use pill_master_renderer_data::{
    config::pbr_pipeline, AssetLoader, Material, Mesh, Texture, TextureImportSettings, TextureType,
};

// =============================================================================
// Constants
// =============================================================================

/// The mesh, relative to `res`. The path is also the asset's name.
const PILL_MESH: &str = "models/pill.obj";

/// The textures the pill material uses, and the type each one is imported as
/// when it has no metadata file. Once the file exists, its `texture_type` is
/// used instead.
const PILL_TEXTURES: &[(&str, TextureType)] = &[("textures/grid.png", TextureType::Color)];

/// The pill material's file, relative to `res`.
const PILL_MATERIAL: &str = "materials/pill.material";

// =============================================================================
// Types
// =============================================================================

/// The asset handles the scene needs to draw the pills.
pub(crate) struct SceneAssets {
    /// The mesh every floating pill draws.
    pub pill_mesh: Handle<Mesh>,
    /// The glowing material every floating pill draws with.
    pub pill_material: Handle<Material>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns this project's `res` directory.
///
/// `file!()` gives this source file's path: relative to the project's manifest
/// in an ordinary build, and absolute when the development host compiles the
/// project as a generated workspace member. `CARGO_MANIFEST_DIR` completes a
/// relative path and leaves an absolute one unchanged, so the join finds this
/// file either way. Two parent steps give the project directory, and `res`
/// lives inside it.
pub(crate) fn res_directory() -> PathBuf {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file!());
    source_path
        .parent()
        .and_then(Path::parent)
        .expect("this file lives in the project's `src`")
        .join("res")
}

/// Loads the assets the pills are drawn with.
///
/// # Errors
///
/// Returns an error when the asset manager is missing, when a file or its
/// metadata fails to load, or when the PBR chain does not install.
pub(crate) fn load(world: &mut World) -> Result<SceneAssets, Box<dyn std::error::Error>> {
    // Step 1: Point the loader at this project's `res` directory and take the
    // asset manager from the world.
    AssetLoader::set_root(res_directory());
    let assets = world
        .get_resource_mut::<AssetManager>()
        .ok_or("the engine AssetManager resource is missing")?;

    // Step 2: Import the mesh, then the textures and the PBR chain. The
    // material names both by guid, so it can only load after them.
    let pill_mesh = import_mesh(assets, PILL_MESH)?;
    for (path, texture_type) in PILL_TEXTURES {
        let request = AssetImport::<Texture>::new(*path, MetadataPolicy::CreateIfMissing)
            .with_initial_settings(TextureImportSettings {
                texture_type: *texture_type,
            });
        assets.import(request)?;
    }

    pbr_pipeline::install(assets)?;

    // Step 3: Import the material.
    let pill_material = assets
        .import_standalone::<Material>(Path::new(PILL_MATERIAL))?
        .handle;

    Ok(SceneAssets {
        pill_mesh,
        pill_material,
    })
}

/// Imports the mesh at `path`, creating its metadata file when it has none.
///
/// # Errors
///
/// Returns [`AssetImportError`] when the path does not normalize or names a
/// metadata file, when the source or its metadata file cannot be read or
/// decoded, when the metadata file belongs to another asset type or a newer
/// format, or when the guid is already used by another asset.
fn import_mesh(assets: &mut AssetManager, path: &str) -> Result<Handle<Mesh>, AssetImportError> {
    let request = AssetImport::<Mesh>::new(path, MetadataPolicy::CreateIfMissing);
    Ok(assets.import(request)?.handle)
}
