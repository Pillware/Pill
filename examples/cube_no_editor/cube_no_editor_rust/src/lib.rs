//! One spinning cube. Its model is an OBJ file, and its material is built in
//! code: the project ships no `.meta` file, no scene file and no editor theme.
//!
//! # Responsibilities
//!
//! - Register the renderer's data and install the simple lit pipeline: one
//!   geometry pass drawn onto the surface, with none of the PBR chain's
//!   post-processing.
//! - Import `res/models/cube.obj` without writing metadata beside it, then
//!   build the tinted material in memory.
//! - Spin the cube around all three axes.
//!
//! # Design
//!
//! The counterpart of the `cube` example. That project ships the same model
//! with a committed `.meta`, which gives the asset a stable guid for references
//! and lets the editor edit it. This project imports the model with
//! [`MetadataPolicy::ReadIfPresent`]. The model decodes with the importer's
//! default settings, its guid exists only in memory, and no run adds a file to
//! the `res` tree.
//!
//! Setup is idempotent, so a hot reload that runs `init` again finds the live
//! scene. The asset store returns the handle of a path that is already loaded,
//! and the entities are looked up before they are created, so nothing is stored
//! twice or fails on a name that is already taken.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::{
    pill_project, system, AssetImport, AssetLoader, AssetManager, Engine, Handle, MetadataPolicy,
    Query, Res, SystemError, Time, World,
};
use pill_rendering::{
    config::simple_pipeline, CameraComponent, Material, Mesh, MeshRendererComponent,
    RenderingManager, Shader, TransformComponent,
};

/// The cube's model, relative to `res`. There is no `cube.obj.meta` beside
/// it, and the import must not create one.
const CUBE_MESH: &str = "models/cube.obj";

/// The material's name in the asset store. Looking it up by name is what makes
/// a reload reuse the live material instead of adding a second one.
const CUBE_MATERIAL: &str = "cube.material";

/// The cube's surface tint, the same colour as the `cube` example's material.
const CUBE_TINT: [f32; 3] = [0.80, 0.80, 0.82];

/// The camera's position: on the +Z axis, looking down -Z at the cube.
const CAMERA_TRANSLATION: [f32; 3] = [0.0, 0.0, 5.0];

/// Spin speed around X, Y and Z, in radians per second.
pub const SPIN_RADIANS_PER_SECOND: [f32; 3] = [2.0, 3.5, 1.0];

/// Registers the renderer's data and creates the camera and the cube.
#[pill_project]
pub fn init(engine: &mut Engine) -> u32 {
    // Step 1: Register the renderer's data and point the asset loader at the
    // project's `res`. The model is read from there and nothing is written
    // back.
    pill_rendering::register(engine);
    AssetLoader::set_root(res_directory());

    // Step 2: Build the mesh and the material.
    let (mesh, material) = match create_assets(engine.world_mut()) {
        Ok(mesh_and_material) => mesh_and_material,
        Err(error) => {
            pill_engine::tracing::error!("asset setup failed: {error}");
            return 1;
        }
    };

    // Step 3: Create the camera and the cube.
    if let Err(error) = create_scene(engine.world_mut(), &mesh, &material) {
        pill_engine::tracing::error!("scene creation failed: {error}");
        return 1;
    }
    0
}

/// Returns this project's `res` directory.
///
/// `file!()` gives this source's path relative to the project manifest in an
/// ordinary build, and an absolute path when the dev host builds the project
/// through its generated workspace member, whose manifest lives elsewhere.
/// Joining it onto `CARGO_MANIFEST_DIR` names the real file either way; the
/// two parent folders are the project root, and `res` lives there.
fn res_directory() -> PathBuf {
    let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file!());
    source_path
        .parent()
        .and_then(Path::parent)
        .expect("this file lives in the project's `src`")
        .join("res")
}

/// Installs the simple pipeline, imports the model, finds or adds the
/// material, and sets the pipeline the renderer runs.
///
/// # Errors
///
/// Returns an error when:
///
/// - the `AssetManager` or `RenderingManager` resource is missing;
/// - the pipeline cannot be installed, or its shader is missing after the
///   install;
/// - the model file does not import;
/// - the material cannot be stored under its name.
fn create_assets(
    world: &mut World,
) -> Result<(Handle<Mesh>, Handle<Material>), Box<dyn std::error::Error>> {
    // Step 1: Install the simple pipeline and find the shader it installed.
    let asset_manager = world
        .get_resource_mut::<AssetManager>()
        .ok_or("the engine AssetManager resource is missing")?;

    let pipeline = simple_pipeline::install(asset_manager)?;
    let shader = asset_manager
        .handle_by_name::<Shader>(simple_pipeline::SHADER_PATH)
        .ok_or("the simple pipeline installed without its shader")?;

    // Step 2: Import the mesh, then find the material or add it. `ReadIfPresent`
    // is the point of this project: the model decodes with the importer's
    // default settings, its guid stays in memory, and no `.meta` file appears
    // beside it. Importing a path that is already loaded returns the existing
    // handle, so a reload imports nothing twice.
    let mesh = asset_manager
        .import(AssetImport::<Mesh>::new(
            CUBE_MESH,
            MetadataPolicy::ReadIfPresent,
        ))?
        .handle;
    let material = match asset_manager.handle_by_name::<Material>(CUBE_MATERIAL) {
        Some(material) => material,
        None => asset_manager.add_named(
            CUBE_MATERIAL,
            Material::builder(CUBE_MATERIAL)
                .shader(&shader)
                .color_parameter("tint", CUBE_TINT)
                .build(),
        )?,
    };

    // Step 3: Make the simple pipeline the one the renderer runs.
    world
        .get_resource_mut::<RenderingManager>()
        .ok_or("the RenderingManager resource is missing")?
        .set_pipeline(pipeline);
    Ok((mesh, material))
}

/// Adds the camera and the cube, unless the world already has them.
///
/// # Errors
///
/// Returns the engine's entity-creation error as text.
fn create_scene(
    world: &mut World,
    mesh: &Handle<Mesh>,
    material: &Handle<Material>,
) -> Result<(), String> {
    // Step 1: Add the camera unless the world already has one.
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

    // Step 2: Add the cube unless the world already draws something.
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
                    .mesh(mesh)
                    .material(material)
                    .build(),
            )
            .build()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Spins every drawn entity around the X, Y and Z axes. Each step is scaled by
/// the frame's delta time, so the spin speed does not depend on the frame rate.
#[system(name = "rotate_cubes")]
fn rotate_cubes_system(
    time: Res<Time>,
    mut cubes: Query<(&mut TransformComponent, &MeshRendererComponent)>,
) -> Result<(), SystemError> {
    let Some(time) = time.get() else {
        return Ok(());
    };
    let delta_seconds = time.delta_seconds();
    let [x, y, z] = SPIN_RADIANS_PER_SECOND;
    let rotation_step = glam::Quat::from_euler(
        glam::EulerRot::XYZ,
        x * delta_seconds,
        y * delta_seconds,
        z * delta_seconds,
    );
    for (mut transform, _) in cubes.iter_mut() {
        let current_rotation = glam::Quat::from_array(
            pill_engine::common_components::rotation_or_identity(transform.rotation),
        );
        transform.rotation = (rotation_step * current_rotation).normalize().to_array();
    }
    Ok(())
}
