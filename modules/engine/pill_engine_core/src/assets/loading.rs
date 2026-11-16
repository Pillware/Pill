//! Loading project assets and scenes from game code, by path.
//!
//! # Responsibilities
//!
//! - [`World::load_asset`]: turns a path below `res` into a typed handle, with
//!   the guid recorded so references can save.
//! - [`Engine::load_scene_file`]: a `.scene` file instantiated into the world
//!   from code, through the same [`load_scene`] pass the editor's Open and the
//!   startup scene run.
//!
//! # Design
//!
//! Assets are named by path: a project writes `"models/helmet.obj"` and gets a
//! `Handle<Mesh>`, instead of decoding the bytes itself and inventing a name for
//! the result. The path is found through the mounted asset directory and the
//! type comes from the file's extension. A file with no `.meta` gets one with a
//! new guid, when its directory can be written, so a scene reference saves the
//! same identity the game holds.

// Standard library
use std::path::Path;

// Current crate
use crate::asset::{AssetGuid, Handle};
use crate::asset_database::AssetDatabase;
use crate::asset_import_registry::ImportRegistry;
use crate::asset_metadata::{ImportedAsset, MetadataPolicy};
use crate::asset_store;
use crate::engine::Engine;
use crate::scene::{load_scene, Scene, SceneDocument, SceneLoad};
use crate::world::World;

// =============================================================================
// World
// =============================================================================

impl World {
    /// Loads the asset at `path` as `T` and returns its handle.
    ///
    /// `path` is below the project's asset directory: `res` in a checkout, the
    /// embedded pack in a shipping build. A file with no `.meta` gets one
    /// holding a new guid when its directory can be written, so the handle can
    /// be saved as a reference later. A path that is already loaded returns its
    /// existing handle and decodes nothing.
    ///
    /// # Errors
    ///
    /// Returns the reason when no asset types are registered, when the engine
    /// has no asset manager, when the file cannot be read or decoded as `T`,
    /// and when the path names an asset of another type.
    pub fn load_asset<T: ImportedAsset>(&mut self, path: &str) -> Result<Handle<T>, String> {
        let registry = self
            .get_resource::<ImportRegistry>()
            .cloned()
            .ok_or_else(|| format!("no asset types are registered (loading `{path}`)"))?;
        let assets = self
            .get_resource_mut::<crate::asset::AssetManager>()
            .ok_or_else(|| "the engine has no AssetManager".to_string())?;
        registry
            .load_handle_by_path::<T>(assets, Path::new(path), MetadataPolicy::CreateIfMissing)
            .map_err(|error| format!("`{path}` did not load: {error}"))
    }
}

// =============================================================================
// Engine
// =============================================================================

impl Engine {
    /// Loads an asset through the engine, for code that owns the engine rather
    /// than the world.
    ///
    /// Behaves as [`World::load_asset`].
    ///
    /// # Errors
    ///
    /// As [`World::load_asset`].
    pub fn load_asset<T: ImportedAsset>(&mut self, path: &str) -> Result<Handle<T>, String> {
        self.world_mut().load_asset::<T>(path)
    }

    /// Loads the scene at `path` into this engine's world.
    ///
    /// Loads through [`load_scene_asset`], which the startup scene also uses.
    /// Entities are created under the stable ids the file carries, so
    /// references between them resolve.
    ///
    /// # Errors
    ///
    /// As [`load_scene_asset`].
    pub fn load_scene_file(&mut self, path: &str) -> Result<SceneLoad, String> {
        load_scene_asset(self, path)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Reads the `.scene` at `path` and returns its document and the guid the file
/// carries.
///
/// Every scene entry point starts here: [`Engine::load_scene_file`], the
/// startup scene and the editor's Open. Sharing one import step means they
/// agree on what a scene document is and which asset types it needs.
///
/// # Errors
///
/// Returns the reason when no asset types are registered, when the engine has
/// no asset manager, when `path` is not a scene file, or when the file cannot
/// be imported or does not stay loaded.
pub fn import_scene_document(
    engine: &mut Engine,
    path: &str,
) -> Result<(SceneDocument, Option<AssetGuid>), String> {
    // Step 1: Find the registry that knows which types import which files.
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .cloned()
        .ok_or_else(|| format!("no asset types are registered (loading `{path}`)"))?;
    // Step 2: Refuse a path that is not a scene file.
    if !registry.is_standalone_extension(extension_of(path)) {
        return Err(format!("`{path}` is not a scene file"));
    }
    let assets = engine
        .world_mut()
        .get_resource_mut::<crate::asset::AssetManager>()
        .ok_or_else(|| "the engine has no AssetManager".to_string())?;
    // Step 3: Import the scene and read its document back out. The guid comes
    // from the file's own header, which a standalone scene always carries.
    let imported = assets
        .import_standalone::<Scene>(Path::new(path))
        .map_err(|error| format!("`{path}` did not load: {error}"))?;
    let guid = imported.guid;
    let document = assets
        .get(imported.handle)
        .map(|scene| scene.document().clone())
        .ok_or_else(|| format!("`{path}` did not stay loaded"))?;
    Ok((document, guid))
}

/// Returns the asset index a scene load resolves its references through.
///
/// The world's [`AssetDatabase`] comes first, when the host built one: a
/// shipped pack's index, or an index of the project's files. Without it, the
/// index is built from the mounted asset directory, and it is empty when
/// nothing is mounted. That preference is what lets a guid-only reference
/// resolve in a pack build, where no directory is mounted.
pub fn scene_database(engine: &Engine) -> AssetDatabase {
    let world = engine.world();
    if let Some(database) = world.get_resource::<AssetDatabase>() {
        return database.clone();
    }
    let Some(root) = asset_store::mounted_directory() else {
        return AssetDatabase::new();
    };
    match world.get_resource::<ImportRegistry>() {
        Some(registry) => AssetDatabase::from_directory(&root, registry),
        None => AssetDatabase::new(),
    }
}

/// Loads the `.scene` at `path` into the engine's world.
///
/// The document comes from [`import_scene_document`] and the references
/// resolve through [`scene_database`]. [`Engine::load_scene_file`] and the
/// startup scene load through here, so both resolve a reference by guid under
/// the same rules. The editor's Open calls those two helpers as well, but
/// replaces the world itself because it keeps the scene's guid.
///
/// # Errors
///
/// As [`import_scene_document`].
pub fn load_scene_asset(engine: &mut Engine, path: &str) -> Result<SceneLoad, String> {
    let (document, _guid) = import_scene_document(engine, path)?;
    let database = scene_database(engine);
    Ok(load_scene(engine, &database, &document))
}

/// Returns the extension of `path` without the dot, or `""` when it has none.
fn extension_of(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset::{Asset, AssetGuid, AssetLoadResult};
    use crate::asset_metadata::ImportedAsset;
    use crate::asset_standalone::render_standalone;
    use crate::asset_store::{mount_directory, mounted_directory_test_lock};
    use crate::entity::EntityId;
    use crate::scene::save_scene;
    use trait_type_map::impl_trait_accessible;

    /// The test asset: a prop's note.
    #[derive(Debug)]
    struct Prop(String);
    impl Asset for Prop {}
    impl_trait_accessible!(dyn Asset; Prop);

    impl ImportedAsset for Prop {
        type ImportSettings = ();
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["prop"];

        fn metadata_type_name() -> &'static str {
            "pill_engine_core::asset_loading::tests::Prop"
        }

        fn import(_name: &str, bytes: &[u8], _settings: &()) -> AssetLoadResult<Self> {
            Ok(Self(String::from_utf8_lossy(bytes).into_owned()))
        }
    }

    /// A scratch asset directory, mounted while the test holds the lock.
    struct ScratchAssetDirectory {
        root_directory: std::path::PathBuf,
        _mounted: std::sync::MutexGuard<'static, ()>,
    }

    impl ScratchAssetDirectory {
        fn new(label: &str) -> Self {
            let mounted = mounted_directory_test_lock();
            let root_directory = std::env::temp_dir()
                .join(format!("pill-load-asset-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root_directory);
            std::fs::create_dir_all(&root_directory).unwrap();
            mount_directory(&root_directory);
            Self {
                root_directory,
                _mounted: mounted,
            }
        }

        fn write(&self, relative_path: &str, text: &str) {
            let path = self.root_directory.join(relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn write_meta(&self, relative_path: &str, guid: AssetGuid) {
            self.write(
                &format!("{relative_path}.meta"),
                &format!(
                    r#"{{"format_version": 1, "asset_type": "pill_engine_core::asset_loading::tests::Prop", "guid": "{guid}", "settings": null}}"#
                ),
            );
        }
    }

    impl Drop for ScratchAssetDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root_directory);
        }
    }

    /// A project loads an asset by path. The guid it was given is the one the
    /// file's sidecar carries, and a second load of the same path returns the
    /// same handle.
    #[test]
    fn a_project_loads_an_asset_by_path_and_keeps_its_guid() {
        let scratch = ScratchAssetDirectory::new("by-path");
        let guid = AssetGuid::random().expect("a fresh guid");
        scratch.write("props/a.prop", "a note");
        scratch.write_meta("props/a.prop", guid);

        let mut world = World::new();
        world.register_imported_asset::<Prop>();
        let first = world
            .load_asset::<Prop>("props/a.prop")
            .expect("the prop loads");
        let assets = world.get_resource::<crate::asset::AssetManager>().unwrap();
        assert_eq!(assets.guid_of(first), Some(guid), "the sidecar's guid");
        assert_eq!(
            assets.get(first).map(|prop| prop.0.as_str()),
            Some("a note")
        );

        let again = world
            .load_asset::<Prop>("props/a.prop")
            .expect("the second load reuses the asset");
        assert_eq!(again, first, "one path is one handle");
    }

    /// A scene load resolves its references through the world's asset database
    /// when the host built one, even with a directory mounted.
    ///
    /// A pack build mounts nothing, so its index exists only as the world
    /// resource. If the lookup preferred the mounted directory, every guid in a
    /// shipped startup scene would go unresolved.
    #[test]
    fn the_scene_database_prefers_the_worlds_index() {
        // The mounted directory maps the guid to one path...
        let scratch = ScratchAssetDirectory::new("database-preference");
        let guid = AssetGuid::random().expect("a fresh guid");
        scratch.write("props/a.prop", "a note");
        scratch.write_meta("props/a.prop", guid);

        let mut engine = Engine::new();
        engine.world_mut().register_imported_asset::<Prop>();
        let registry = engine
            .world()
            .get_resource::<ImportRegistry>()
            .expect("an engine owns an import registry")
            .clone();

        // ...and the index the host would insert maps it to another, so which
        // one a load used is visible in the record it finds.
        let other_root_directory =
            std::env::temp_dir().join(format!("pill-load-asset-other-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&other_root_directory);
        std::fs::create_dir_all(other_root_directory.join("props")).unwrap();
        std::fs::write(other_root_directory.join("props/other.prop"), "a note").unwrap();
        std::fs::write(
            other_root_directory.join("props/other.prop.meta"),
            format!(
                r#"{{"format_version": 1, "asset_type": "pill_engine_core::asset_loading::tests::Prop", "guid": "{guid}", "settings": null}}"#
            ),
        )
        .unwrap();
        let asset_index = AssetDatabase::from_directory(&other_root_directory, &registry);

        engine.world_mut().insert_resource(asset_index);
        let database = scene_database(&engine);
        assert_eq!(
            database.by_guid(guid).map(|record| record.path.as_str()),
            Some("props/other.prop"),
            "the world's index is the one a scene load uses"
        );

        let _ = std::fs::remove_dir_all(&other_root_directory);
    }

    /// A file with no sidecar gets one, so the handle it returns can be saved
    /// as a reference later.
    #[test]
    fn a_source_without_a_sidecar_is_given_a_guid() {
        let scratch = ScratchAssetDirectory::new("no-meta");
        scratch.write("props/b.prop", "another note");

        let mut world = World::new();
        world.register_imported_asset::<Prop>();
        let handle = world
            .load_asset::<Prop>("props/b.prop")
            .expect("the prop loads");
        assert!(
            world
                .get_resource::<crate::asset::AssetManager>()
                .unwrap()
                .guid_of(handle)
                .is_some(),
            "the load recorded a guid"
        );
        assert!(
            scratch.root_directory.join("props/b.prop.meta").is_file(),
            "the sidecar was written beside the source"
        );
    }

    /// A scene file loads from code, entities and all, under the ids it
    /// carries.
    #[test]
    fn a_scene_loads_from_code() {
        let scratch = ScratchAssetDirectory::new("scene");
        let mut writer = Engine::new();
        let id = EntityId::from_name("crate");
        writer
            .world_mut()
            .create_entity()
            .with_id(id)
            .build()
            .unwrap();
        let registry = writer
            .world()
            .get_resource::<ImportRegistry>()
            .cloned()
            .unwrap();
        let (document, report) = {
            let world = writer.world();
            let assets = world.get_resource::<crate::asset::AssetManager>().unwrap();
            save_scene(world, assets, &registry, None)
        };
        assert!(
            report.skipped_fields.is_empty(),
            "{:?}",
            report.skipped_fields
        );
        let bytes =
            render_standalone::<Scene>(AssetGuid::random().expect("a fresh guid"), &document)
                .expect("the scene renders");
        std::fs::create_dir_all(scratch.root_directory.join("scenes")).unwrap();
        std::fs::write(scratch.root_directory.join("scenes/a.scene"), bytes).unwrap();

        let mut engine = Engine::new();
        let loaded = engine
            .load_scene_file("scenes/a.scene")
            .expect("the scene loads");
        assert_eq!(loaded.entities, 1);
        assert!(
            engine.world().entity_by_id(id).is_some(),
            "the entity kept the id the document gave it"
        );
    }

    /// The engine registers its own scene type. A fresh engine lists it as a
    /// standalone type and loads a `.scene` file with nothing else registered.
    #[test]
    fn the_engine_registers_its_own_scene_type() {
        let scratch = ScratchAssetDirectory::new("engine-scene");
        let bytes = render_standalone::<Scene>(
            AssetGuid::random().expect("a fresh guid"),
            &crate::scene::SceneDocument::default(),
        )
        .expect("the scene renders");
        std::fs::create_dir_all(scratch.root_directory.join("scenes")).unwrap();
        std::fs::write(scratch.root_directory.join("scenes/empty.scene"), bytes).unwrap();

        let mut engine = Engine::new();
        let registry = engine
            .world()
            .get_resource::<ImportRegistry>()
            .expect("the engine registers its import registry")
            .clone();
        assert!(
            registry
                .standalone_types()
                .iter()
                .any(|(type_name, extension)| {
                    type_name
                        == <Scene as crate::asset_standalone::StandaloneAsset>::metadata_type_name()
                        && extension == "scene"
                }),
            "a fresh engine lists the scene type: {:?}",
            registry.standalone_types()
        );
        let loaded = engine
            .load_scene_file("scenes/empty.scene")
            .expect("the scene loads without a caller registering it");
        assert_eq!(loaded.entities, 0);
    }
}
