//! A game's world saves to a `.scene` file and loads back into an empty world.
//!
//! # Responsibilities
//!
//! - Prove the whole scene path through the API a project uses: the derive's
//!   field layouts, [`save_scene`], the `.scene` file and [`load_scene`]. An
//!   entity reference and a `Handle<Scene>` reference both survive the trip.
//! - Check that the import registry maps the `.scene` extension, so a scan of
//!   a project finds scene files without being given a list of types.
//!
//! # Design
//!
//! The component comes from `#[component]` and is registered by
//! [`register_all_components`], the same loop a module's registration uses, so
//! the scene reads and writes the generated field layout rather than a
//! hand-written copy. The asset the rider references is itself a scene file,
//! so the file proves that one scene can name another.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_engine::asset_store::mount_directory;
use pill_engine::component_registry::register_all_components;
use pill_engine::{
    component, load_scene, render_standalone, save_scene, AssetDatabase, AssetGuid, AssetManager,
    Engine, Entity, EntityId, Handle, ImportRegistry, Scene, SceneDocument, StandaloneAsset,
};

// =============================================================================
// Test Types
// =============================================================================

/// A rider, the component the test saves: it names the entity it rides, the
/// prop scene that goes with it, and its weight.
///
/// The three fields are one of each kind a scene has to save: an entity
/// reference, an asset reference and a plain number.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Rider {
    /// The entity this rider rides.
    mount: Entity,
    /// The prop scene that travels with the rider.
    prop: Handle<Scene>,
    /// The rider's weight, saved as a plain number.
    weight: f32,
}

/// A temporary directory that stands in for the project's asset folder.
///
/// Creating it mounts it as the asset directory; dropping it deletes it, so a
/// run leaves no files behind.
struct ScratchDirectory {
    root_path: PathBuf,
}

impl ScratchDirectory {
    /// Creates an empty directory named after `test_name` and mounts it as the
    /// project's asset directory.
    fn new(test_name: &str) -> Self {
        let root_path =
            std::env::temp_dir().join(format!("pill-scene-{test_name}-{}", std::process::id()));
        // Start empty, in case a run with the same process id left files here.
        let _ = std::fs::remove_dir_all(&root_path);
        std::fs::create_dir_all(&root_path).expect("the scratch directory is created");
        mount_directory(&root_path);
        Self { root_path }
    }

    /// Writes `bytes` to `relative_path` below the directory, creating the
    /// folders on the way.
    fn write(&self, relative_path: &str, bytes: &[u8]) {
        let path = self.root_path.join(relative_path);
        std::fs::create_dir_all(path.parent().expect("a file below a directory"))
            .expect("the directory is created");
        std::fs::write(path, bytes).expect("the file is written");
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root_path);
    }
}

// =============================================================================
// Test Helpers
// =============================================================================

/// Builds an engine that registers the components and asset types of this
/// project, so every engine the test creates has the same world shape.
fn engine_with_project() -> Engine {
    let mut engine = Engine::new();
    register_all_components(engine.world_mut()).expect("registration succeeds");
    engine.world_mut().register_standalone_asset::<Scene>();
    engine
}

// =============================================================================
// Tests
// =============================================================================

/// Saves a world that holds a rider, then loads the file into a fresh engine
/// and checks that every value came back, both references included.
#[test]
fn a_project_scene_saves_and_loads_back_with_its_references() {
    let scratch_directory = ScratchDirectory::new("round-trip");
    let mount_id = EntityId::from_name("mount");
    let hero_id = EntityId::from_name("hero");

    // The prop scene the rider references. An empty document is a valid
    // scene, so the file only needs a fresh guid.
    let prop_guid = AssetGuid::random().expect("a fresh guid");
    let prop = render_standalone::<Scene>(prop_guid, &SceneDocument::default())
        .expect("an empty scene renders");
    scratch_directory.write("props/scenery.scene", &prop);

    // Step 1: build the world that holds the rider, and save it to a file.
    let scene_guid = {
        let mut engine = engine_with_project();
        let prop = {
            let assets = engine
                .world_mut()
                .get_resource_mut::<AssetManager>()
                .expect("an engine owns an asset manager");
            assets
                .import_standalone::<Scene>(Path::new("props/scenery.scene"))
                .expect("the prop scene loads")
        };
        let mount = engine
            .world_mut()
            .create_entity()
            .with_id(mount_id)
            .build()
            .expect("the mount builds");
        engine
            .world_mut()
            .create_entity()
            .with_id(hero_id)
            .with(Rider {
                mount,
                prop: prop.handle,
                weight: 0.75,
            })
            .build()
            .expect("the hero builds");

        let registry = engine
            .world()
            .get_resource::<ImportRegistry>()
            .expect("an engine owns an import registry")
            .clone();
        let world = engine.world();
        let assets = world
            .get_resource::<AssetManager>()
            .expect("an engine owns an asset manager");
        let (document, report) = save_scene(world, assets, &registry, None);
        assert!(
            report.skipped_fields.is_empty(),
            "every field of the rider is representable: {:?}",
            report.skipped_fields
        );
        assert_eq!(report.components, 1, "only the rider's component");
        assert_eq!(document.entities.len(), 2);

        let scene_guid = AssetGuid::random().expect("a fresh guid");
        let bytes = render_standalone::<Scene>(scene_guid, &document).expect("the scene renders");
        scratch_directory.write("scenes/main.scene", &bytes);
        scene_guid
    };

    // Step 2: a fresh engine of the same project reads the file back.
    let mut engine = engine_with_project();
    let database = {
        let registry = engine
            .world()
            .get_resource::<ImportRegistry>()
            .expect("an engine owns an import registry");
        AssetDatabase::from_directory(&scratch_directory.root_path, registry)
    };
    let scene = {
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .expect("an engine owns an asset manager");
        let imported_scene = assets
            .import_standalone::<Scene>(Path::new("scenes/main.scene"))
            .expect("the scene file loads");
        assert_eq!(
            imported_scene.guid,
            Some(scene_guid),
            "the file keeps the guid it was saved with"
        );
        assets
            .get(imported_scene.handle)
            .expect("the scene asset is live")
            .clone()
    };
    let report = load_scene(&mut engine, &database, scene.document());
    assert_eq!(report.entities, 2);
    assert_eq!(report.components, 1);
    assert!(
        report.unknown_components.is_empty(),
        "{:?}",
        report.unknown_components
    );
    assert!(
        report.skipped_fields.is_empty(),
        "{:?}",
        report.skipped_fields
    );
    assert_eq!(report.unresolved_references, 0);

    // Step 3: read the saved values back through the components.
    let world = engine.world_mut();
    let hero = world.entity_by_id(hero_id).expect("the hero came back");
    let rider = *world
        .get_component::<Rider>(hero)
        .expect("the rider component came back");
    assert_eq!(rider.weight, 0.75, "the scalar came back");
    assert_eq!(
        world.entity_id(rider.mount),
        Some(mount_id),
        "the entity reference found the entity the document created"
    );
    assert_ne!(rider.mount, Entity::NULL);
    let assets = world
        .get_resource::<AssetManager>()
        .expect("an engine owns an asset manager");
    assert_eq!(
        assets.guid_of(rider.prop),
        Some(prop_guid),
        "the scene reference resolved to the prop scene file"
    );
}

/// The import registry maps the `.scene` extension to the scene type, so a
/// scan of a project's asset directory finds scene files without being given a
/// list of types to look for.
#[test]
fn the_scene_extension_is_registered() {
    let engine = engine_with_project();
    let world = engine.world();
    let registry = world
        .get_resource::<ImportRegistry>()
        .expect("an engine owns an import registry");
    assert_eq!(
        registry.type_for_extension("scene"),
        Some(Scene::metadata_type_name()),
        "a .scene file maps to the scene type"
    );
    assert!(registry.is_standalone_extension("scene"));
}
