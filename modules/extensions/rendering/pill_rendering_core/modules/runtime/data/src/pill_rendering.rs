//! The rendering data: the assets, components and pipeline choice a game draws
//! with, and no graphics stack behind any of it.
//!
//! # Responsibilities
//!
//! - Define the assets a project draws from ([`Mesh`], [`Texture`], [`Shader`],
//!   [`Material`], [`RenderPass`], [`RenderingPipeline`]) and the [`components`]
//!   that place them in the world.
//! - Define the resource a game selects its pipeline in ([`RenderingManager`]).
//! - Register all of it with an engine through [`register`]. The pipelines and
//!   their shaders are a renderer's own, and so is installing one.
//!
//! # Design
//!
//! This crate never depends on wgpu or winit. It is the data half of a
//! renderer pair: this one holds the data, and a renderer crate
//! (`pill_renderer`) holds the GPU code and is the only renderer crate that
//! links the graphics stack. Each renderer names this crate in its own manifest
//! (`renderer_data`), so two renderers can share it.
//! That split lets a headless build store a game's cameras, meshes and lights
//! without linking a renderer, and lets the GPU module be reloaded without
//! registering any data of its own.
//!
//! This crate builds on `pill_renderer_api`, the contract every renderer
//! shares: the backend trait, the frame, the pass vocabulary and the camera.
//! The types a game needs from it are re-exported here.
//!
//! The renderers' pipelines and their shaders are not here: they belong to the
//! crate that draws them (`pill_renderer`), which ships its own `res/shaders/`
//! and installs the default pipeline. What stays here is what any renderer can
//! read - the assets a game writes, the material's slot vocabulary and the
//! resource a game selects its pipeline in.

/// The drawable assets - meshes, textures, shaders, materials, passes - and the
/// builders that create them.
pub mod assets;
/// The sRGB transfer curve the importer and the renderers share.
pub mod color_space;

/// The world-side draw components and [`register_components`].
pub mod components;

/// The resource a game sets its pipeline in.
mod rendering_manager;

// Standard library
use std::path::Path;
// External crates
pub use pill_engine::AssetLoader;
use pill_engine::{pill_module, Engine};
pub use pill_renderer_api::frame::{
    CullMode, MaterialParameter, PassKind, PassTarget, RenderFrame,
};

// Current crate
pub use assets::{
    load_shipped_texture, neutral_material_parameters, Material, MaterialBuilder, MaterialDocument,
    MaterialTexture, Mesh, MeshImportSettings, MeshVertex, PipelinePass, PipelinePassDocument,
    RenderPass, RenderPassDocument, RenderingPipeline, RenderingPipelineDocument, Shader,
    ShaderBuilder, ShaderImportSettings, ShaderParameterSlot, ShaderParameterType,
    ShaderTextureKind, ShaderTextureSlot, Texture, TextureImportSettings, TextureType,
    DEFAULT_COLOR_TEXTURE, DEFAULT_MATERIAL_PATH, DEFAULT_NORMAL_TEXTURE,
};
pub use components::*;
pub use rendering_manager::RenderingManager;

// =============================================================================
// Constants
// =============================================================================

/// The name the host knows this module by, which is its crate name.
pub const MODULE_NAME: &str = "pill_rendering";

/// Registers the renderer's components, asset types and resources with an
/// engine, then returns zero.
///
/// This is the half a reload repeats: every declaration below points at code
/// in the arriving image, so a generation that skipped it would leave the
/// kept tables calling into an image that is no longer mapped.
///
/// It registers no system and installs no pipeline. Filling the frame, drawing
/// it and shipping a default chain belong to a renderer (`pill_renderer`).
///
/// The components go in through [`register_components`]. The six asset types
/// declare themselves, so the tables that hold their values point at code in
/// the artifact that is still mapped without a call here. The [`RenderFrame`]
/// and [`RenderingManager`] values are only inserted when they are missing.
/// Declaring a type or a resource again changes nothing, so calling this a
/// second time changes nothing either.
///
/// # Panics
///
/// Panics when one of this crate's asset roots exists but a file it names
/// below that root is missing. `declare_crate_roots` makes that check while
/// this function runs.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: Register the components, field layouts included.
    register_components(engine.world_mut());

    // Step 2: Declare the reference fields the asset documents use, so the
    // editor can draw a picker without knowing any renderer type. Each target
    // is the Rust type the field holds, so a renamed type moves here with the
    // code. A `.*` key stands for a family: `textures.*` is every texture slot
    // the material's shader declares, and `passes.*` is every entry of a
    // chain's pass list.
    if let Some(import_registry) = engine
        .world_mut()
        .get_resource_mut::<pill_engine::ImportRegistry>()
    {
        import_registry.declare_references(
            <Material as pill_engine::StandaloneAsset>::metadata_type_name(),
            &[
                (
                    "shader",
                    <Shader as pill_engine::ImportedAsset>::metadata_type_name(),
                ),
                (
                    "textures.*",
                    <Texture as pill_engine::ImportedAsset>::metadata_type_name(),
                ),
            ],
        );
        import_registry.declare_references(
            <RenderingPipeline as pill_engine::StandaloneAsset>::metadata_type_name(),
            &[(
                "passes.*",
                <RenderPass as pill_engine::StandaloneAsset>::metadata_type_name(),
            )],
        );
    }

    // Step 3: Store the two resources this crate's systems read, once. A reload
    // keeps the existing values and never calls `insert_resource` again; the
    // declarations that point their drop functions at code which is still
    // mapped need no call here, because `RenderFrame` and `RenderingManager`
    // both declare themselves and the entry point declares them before this
    // function runs.
    if engine.world().get_resource::<RenderFrame>().is_none() {
        engine.world_mut().insert_resource(RenderFrame::default());
    }
    // The game writes the pipeline here and the renderer reads it back. It is
    // inserted here so a project that never sets one still finds the resource.
    if engine.world().get_resource::<RenderingManager>().is_none() {
        engine.world_mut().insert_resource(RenderingManager::new());
    }

    // Step 4: Declare this crate's own assets - the default material the chain
    // draws with - from the crate's directory. The store then answers
    // `materials/pill.pbr.material` by name and the editor lists the folder
    // read-only. A file that went missing stops the engine here instead of
    // leaving a scene unlit.
    //
    // A shipped build links this crate statically and mounts its pack instead,
    // so it has no module directory to declare.
    declare_crate_roots();
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once: everything it owns is
/// registration, and [`register`] re-runs all of it on every generation. This
/// function exists because a statically linked build loads the module once and
/// calls this entry point alone, so it defers to [`register`].
///
/// The generated `host_module_pill_rendering` wrapper carries it as
/// `pill_module_init` for the host.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Free Functions
// =============================================================================

/// Declares the read-only root this crate ships, its own `res`.
///
/// It holds the default material and the renderer's default textures. The
/// pipelines' shaders are a renderer's own, under its own `res/shaders/`.
///
/// Declaring the same root again is what a reload does, and it changes
/// nothing. It is also what lets a renderer declare this crate's content beside
/// its own, and what lets tests read the same files a running module reads.
///
/// # Panics
///
/// Panics when `res` exists but one of the files this function names below it
/// is missing; the message names the module and the path.
pub fn declare_crate_roots() {
    let resources_directory = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/res"));
    if resources_directory.is_dir() {
        pill_engine::asset_store::declare_module_root_hiding(
            MODULE_NAME,
            resources_directory,
            &[
                assets::DEFAULT_MATERIAL_PATH,
                assets::DEFAULT_COLOR_TEXTURE,
                assets::DEFAULT_NORMAL_TEXTURE,
            ],
            &[],
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{AssetManager, Engine, Handle, ImportRegistry, MetadataPolicy};

    /// A mesh renderer's handle fields are typed asset references: reading one
    /// names the registered asset type, writing another mesh's handle goes
    /// through the field layer into the component the renderer reads, and a
    /// material handle is refused for the mesh field.
    #[test]
    fn the_mesh_renderers_handles_are_writable_asset_references() {
        let mut engine = Engine::new();
        assert_eq!(register(&mut engine), 0);
        let world = engine.world_mut();
        let entity = world
            .create_entity()
            .with(MeshRendererComponent {
                mesh: Handle::from_raw(0, 0),
                material: Handle::from_raw(1, 0),
            })
            .build()
            .unwrap();
        let component_name =
            pill_engine::component::ComponentRegistry::registered_name::<MeshRendererComponent>();
        let mesh_type_name = "pill_master_renderer::Mesh";
        assert_eq!(
            world
                .read_component_field(entity, &component_name, "mesh")
                .unwrap(),
            pill_engine::FieldValue::Asset {
                asset_type: Some(mesh_type_name.to_owned()),
                index: 0,
                generation: 0,
            }
        );

        world
            .write_component_field(
                entity,
                &component_name,
                "mesh",
                pill_engine::FieldValue::Asset {
                    asset_type: Some(mesh_type_name.to_owned()),
                    index: 5,
                    generation: 2,
                },
            )
            .expect("another mesh's handle writes");
        assert_eq!(
            world
                .get_component::<MeshRendererComponent>(entity)
                .unwrap()
                .mesh,
            Handle::from_raw(5, 2),
            "the component the renderer reads holds the new mesh"
        );

        let refused = world.write_component_field(
            entity,
            &component_name,
            "mesh",
            pill_engine::FieldValue::Asset {
                asset_type: Some("pill_master_renderer::Material".to_owned()),
                index: 1,
                generation: 0,
            },
        );
        assert!(
            matches!(
                refused,
                Err(pill_engine::component_field::ComponentFieldError::AssetTypeMismatch { .. })
            ),
            "{refused:?}"
        );
    }

    /// Registering the crate makes texture and mesh files importable by
    /// extension through the registry, without naming their types.
    ///
    /// This is the only test in this crate that mounts a directory, so it needs
    /// no lock against another test replacing the mount.
    #[test]
    fn textures_and_meshes_import_by_extension_through_the_registry() {
        let asset_root_directory =
            std::env::temp_dir().join(format!("pill_data_registry_{}", std::process::id()));
        std::fs::create_dir_all(&asset_root_directory).unwrap();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255]))
            .save(asset_root_directory.join("pixel.png"))
            .unwrap();
        std::fs::write(
            asset_root_directory.join("triangle.obj"),
            "v 0 0 0
v 1 0 0
v 0 1 0
vt 0 0
vt 1 0
vt 0 1
f 1/1 2/2 3/3
",
        )
        .unwrap();
        pill_engine::asset_store::mount_directory(&asset_root_directory);
        let mut engine = Engine::new();
        // The asset types declare themselves, so the import registry learns
        // their extensions from the artifact entry point; `register` keeps the
        // half a reload repeats and declares no type of its own.
        assert_eq!(
            pill_engine::component_registry::run_artifact_registration(&mut engine, register),
            0
        );

        let world = engine.world_mut();
        let import_registry = world
            .remove_resource::<ImportRegistry>()
            .unwrap()
            .expect("register records the imported types");
        let assets = world.get_resource_mut::<AssetManager>().unwrap();
        let texture_import = import_registry
            .import(
                assets,
                std::path::Path::new("pixel.png"),
                MetadataPolicy::ReadIfPresent,
            )
            .unwrap();
        let mesh_import = import_registry
            .import(
                assets,
                std::path::Path::new("triangle.obj"),
                MetadataPolicy::ReadIfPresent,
            )
            .unwrap();

        assert_eq!(texture_import.type_name, "pill_master_renderer::Texture");
        assert_eq!(mesh_import.type_name, "pill_master_renderer::Mesh");
        assert!(assets.get_by_name::<Texture>("pixel.png").is_some());
        assert!(assets.get_by_name::<Mesh>("triangle.obj").is_some());
        std::fs::remove_dir_all(&asset_root_directory).unwrap();
    }

    /// Registering the data registers no system: drawing belongs to the
    /// renderer module, and a headless engine must not run any of it.
    #[test]
    fn registering_the_data_registers_no_rendering_system() {
        let mut engine = Engine::new();

        register(&mut engine);

        assert!(!engine.is_registered("rendering"));
    }
}
