//! A spinning cube whose mesh and material are authored as files under `res`.
//!
//! # Responsibilities
//!
//! - Load `res/models/cube.obj` and `res/materials/cube.material` through the
//!   project's asset store, so the editor's Assets tab lists them and its
//!   Inspector edits the material's parameters.
//! - Place the camera and the cube, then spin it around X, Y and Z.
//!
//! # Design
//!
//! The file-authored twin of `cube_no_editor`: the same camera and the same
//! cube, spun around the same axes, but neither the mesh nor the material is
//! built in code. The material names the renderer's PBR shader by guid, so it
//! can only be imported after `pill_rendering::register` has
//! installed that chain.
//!
//! `init` runs again on every hot reload, against the live world. Both setup
//! steps can run twice without adding a second copy: the asset store hands
//! back an already-loaded path, and each entity is only added when the world
//! has none of that kind yet. So a reload finds the running scene.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::{
    pill_project, system, AssetLoader, AssetManager, Engine, Handle, Query, Res, SystemError, Time,
    World,
};
use pill_rendering::{CameraComponent, Material, Mesh, MeshRendererComponent, TransformComponent};

/// The path to the cube's model, relative to `res`.
const CUBE_MESH_PATH: &str = "models/cube.obj";

/// The path to the cube's material, relative to `res`.
const CUBE_MATERIAL_PATH: &str = "materials/cube.material";

/// Spin speed around X, Y and Z, in radians per second.
pub const SPIN_RATES_RADIANS_PER_SECOND: [f32; 3] = [0.6, 1.0, 0.3];

/// Where the camera stands: on +Z, looking down -Z at the cube.
const CAMERA_TRANSLATION: [f32; 3] = [0.0, 0.0, 5.0];

/// Registers the renderer's data, loads the cube's files and builds the scene.
///
/// Returns `0` when the scene is ready, or `1` after logging why loading the
/// files or building the scene failed.
#[pill_project]
pub fn init(engine: &mut Engine) -> u32 {
    // Step 1: Register the renderer's data and point the asset store at this
    // project's `res`. The material names the PBR shader by guid, so the chain
    // has to be installed before the material is imported.
    pill_rendering::register(engine);
    AssetLoader::set_root(res_directory());

    // Step 2: Load the mesh and the material.
    let (mesh_handle, material_handle) = match load_assets(engine.world_mut()) {
        Ok(asset_handles) => asset_handles,
        Err(error) => {
            pill_engine::tracing::error!("asset loading failed: {error}");
            return 1;
        }
    };

    // Step 3: Add the camera and the cube to the world.
    if let Err(error) = create_scene(engine.world_mut(), &mesh_handle, &material_handle) {
        pill_engine::tracing::error!("scene creation failed: {error}");
        return 1;
    }
    0
}

/// Returns the path to this project's `res` directory.
///
/// In a normal build, `file!()` gives this source's path relative to the
/// project manifest. When the dev host builds the project from a generated
/// crate elsewhere in the engine workspace, the manifest lives elsewhere and
/// `file!()` gives an absolute path instead. Joining it onto
/// `CARGO_MANIFEST_DIR` gives this file's real location in both cases.
fn res_directory() -> PathBuf {
    let source_file_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file!());
    source_file_path
        .parent()
        .and_then(Path::parent)
        .expect("this file lives in the project's `src`")
        .join("res")
}

/// Imports the model and the material file, in that order.
///
/// The order matters: the material names its shader by guid, and a reference
/// to a shader that is not loaded yet resolves to the renderer's built-in
/// shader instead.
///
/// # Errors
///
/// Returns the engine's message when no asset types are registered, when a
/// file cannot be read, does not decode as its type, or names an asset of
/// another type. It returns its own message when the world has no
/// `AssetManager` resource.
fn load_assets(world: &mut World) -> Result<(Handle<Mesh>, Handle<Material>), String> {
    let mesh_handle = world.load_asset::<Mesh>(CUBE_MESH_PATH)?;
    let asset_manager = world
        .get_resource_mut::<AssetManager>()
        .ok_or("the engine AssetManager resource is missing")?;
    let material_handle = asset_manager
        .import_standalone::<Material>(Path::new(CUBE_MATERIAL_PATH))
        .map_err(|error| error.to_string())?
        .handle;
    Ok((mesh_handle, material_handle))
}

/// Adds the camera and the cube, unless the world already has them.
///
/// There is no light entity: every shipped pass writes its own lights into
/// its shader, so a `DirectionalLightComponent` would be saved with the scene
/// and never read.
///
/// # Errors
///
/// Returns the engine's entity-creation error as text.
fn create_scene(
    world: &mut World,
    mesh_handle: &Handle<Mesh>,
    material_handle: &Handle<Material>,
) -> Result<(), String> {
    // Step 1: Add a camera unless the world already has one.
    if Query::<&CameraComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        world
            .create_entity()
            .with(CameraComponent::default())
            .with(TransformComponent {
                translation: CAMERA_TRANSLATION,
                ..Default::default()
            })
            .build()
            .map_err(|error| error.to_string())?;
    }

    // Step 2: Add the cube unless an entity is already drawn.
    if Query::<&MeshRendererComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        world
            .create_entity()
            .with(TransformComponent::default())
            .with(
                MeshRendererComponent::builder()
                    .mesh(mesh_handle)
                    .material(material_handle)
                    .build(),
            )
            .build()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Spins every drawn entity around X, Y and Z.
///
/// The rotation is scaled by the time the frame took, so the spin speed does
/// not depend on the frame rate.
#[system(name = "spin_cube")]
fn spin_cube_system(
    time_resource: Res<Time>,
    mut drawn_entities: Query<(&mut TransformComponent, &MeshRendererComponent)>,
) -> Result<(), SystemError> {
    let Some(frame_time) = time_resource.get() else {
        return Ok(());
    };
    let delta_seconds = frame_time.delta_seconds() * 1.0;
    let [x, y, z] = SPIN_RATES_RADIANS_PER_SECOND;
    let frame_rotation = glam::Quat::from_euler(
        glam::EulerRot::XYZ,
        x * delta_seconds,
        y * delta_seconds,
        z * delta_seconds,
    );
    for (mut transform, _) in drawn_entities.iter_mut() {
        let current_rotation = glam::Quat::from_array(
            pill_engine::common_components::rotation_or_identity(transform.rotation),
        );
        transform.rotation = (frame_rotation * current_rotation).normalize().to_array();
    }
    Ok(())
}
