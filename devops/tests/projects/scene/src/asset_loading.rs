//! Loads the bundled model, its material and the sample map by path or
//! document.
//!
//! # Responsibilities
//!
//! - Mount the project's `res` and ask the world for the model by path, so its
//!   guid comes from the `.meta` file beside it.
//! - Import the committed material document, which names the renderer chain's
//!   shader.
//! - Load the sample map, which no material binds: it is what gives the host's
//!   asset watcher an import the asset suites can reimport in place.
//! - Hand the handles back, so the scene never looks an asset up by name.
//!
//! # Design
//!
//! The project runs the renderer's own chain, so every surface draws through
//! the material document rather than through a shader this file builds: the
//! chain draws materials built for its shader alone, and a committed document
//! is also what the editor lists and the asset suites edit.
//!
//! The material binds no map. The bundled model's texture coordinates are a
//! parametric patch grid rather than a surface layout, so no photograph lands
//! on it the way its author meant; the document draws the chain's own
//! constants instead, and only its parameters carry the look.
//!
//! A hot reload of the project calls `init` again on the same world, keeping
//! the asset manager; `World::load_asset` and `import_standalone` both hand
//! back the handle a previous run already holds, which is what makes the
//! reload idempotent.

// External crates
use pill_engine::{AssetManager, Handle, World};
use pill_master_renderer_data::{AssetLoader, Material, Mesh, Texture};
use std::path::{Path, PathBuf};

/// The handles [`load`] returns: the one mesh every model draws and the
/// material they all draw through.
#[derive(Clone, Copy)]
pub(crate) struct SceneAssets {
    pub mesh: Handle<Mesh>,
    pub material: Handle<Material>,
}

/// Mounts the bundled `res`, loads the model and the sample map by path, and
/// imports the committed material.
///
/// # Errors
///
/// Returns an error when a bundled asset fails to load or the manager is
/// missing. Every load returns the handle a previous run already holds, so a
/// project reload keeps its assets.
pub(crate) fn load(world: &mut World) -> Result<SceneAssets, Box<dyn std::error::Error>> {
    /// The model the scene draws at every size and position.
    const MODEL_PATH: &str = "models/teapot.obj";
    /// The standalone material document, which names the chain's shader.
    const MATERIAL_PATH: &str = "materials/teapot.material";
    /// The map no material binds. It is loaded by path because that is what
    /// registers an import with the host's asset watcher: the asset suites
    /// edit, corrupt and move it, and only a loaded asset is reimported in
    /// place.
    const SAMPLE_MAP_PATH: &str = "textures/reimport_sample.png";

    // Mounting the directory is what the path-based load resolves through.
    AssetLoader::set_root(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("res"));

    let mesh = world.load_asset::<Mesh>(MODEL_PATH)?;
    let _sample_map: Handle<Texture> = world.load_asset::<Texture>(SAMPLE_MAP_PATH)?;

    let assets = world
        .get_resource_mut::<AssetManager>()
        .ok_or_else(|| "the engine AssetManager resource is missing".to_owned())?;
    let material = assets
        .import_standalone::<Material>(Path::new(MATERIAL_PATH))?
        .handle;

    Ok(SceneAssets { mesh, material })
}
