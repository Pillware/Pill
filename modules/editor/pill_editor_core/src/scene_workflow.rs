//! The editor's open scene: which `.scene` the world came from, whether it
//! changed since the last save, and the file operations behind New, Open, Save
//! and Save As.
//!
//! # Responsibilities
//!
//! - [`SceneSession`]: the open scene's path and guid, and the unsaved marker
//!   every undoable command sets. A world resource, so reloading the editor
//!   shell keeps the open document.
//! - [`new_scene`], [`open_scene`], [`write_scene`]: the scene commands behind
//!   New, Open and Save. Open and Save go through the engine's
//!   [`import_scene_document`], [`load_scene`] and [`save_scene`].
//!
//! # Design
//!
//! The engine owns serialization; this module owns the document's *identity* -
//! the path it was read from and the guid written into it. Save needs that
//! identity, and the scene title is built from it. Opening or starting a new
//! scene replaces the world, so the command history and the selection go with
//! it. The history's steps and the selection both name entities that the
//! previous document owned.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{
    import_scene_document, load_scene, render_standalone, save_scene, scene_database, AssetGuid,
    AssetManager, Engine, ImportRegistry, Resource, Scene, SceneLoad, World,
};

// Current crate
use crate::editor_state::{EditorCommand, EditorSelection};

// =============================================================================
// Types
// =============================================================================

/// Which `.scene` file the editor's world came from: its path and its guid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenScene {
    /// The scene's path below the asset directory, `/`-separated.
    pub path: String,
    /// The guid written into the file; Save keeps it, so references to the
    /// scene from other data stay valid.
    pub guid: AssetGuid,
}

/// The editor's document state: the scene file that is open and the unsaved
/// marker.
#[derive(Default)]
pub struct SceneSession {
    /// The scene file the world came from, or `None` when no file backs it
    /// (a new scene).
    pub open: Option<OpenScene>,
    /// Whether an undoable command has run since the scene was last saved or
    /// opened.
    pub dirty: bool,
}

impl Resource for SceneSession {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl SceneSession {
    /// Register the resource, and keep the value already in the world, so a
    /// reload does not reset the open scene.
    pub fn ensure(world: &mut World) {
        world.register_resource::<SceneSession>();
        if world.get_resource::<SceneSession>().is_none() {
            world.insert_resource(Self::default());
        }
    }

    /// The open scene's display name: the scene's file name, or `untitled`,
    /// with ` *` appended while the scene is marked dirty.
    #[must_use]
    pub fn title(&self) -> String {
        let display_name = match &self.open {
            Some(open) => open
                .path
                .rsplit('/')
                .next()
                .unwrap_or(open.path.as_str())
                .to_string(),
            None => "untitled".to_string(),
        };
        if self.dirty {
            format!("{display_name} *")
        } else {
            display_name
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Mark the document as changed since it was last saved or opened.
///
/// Called after each undoable command that applied, including selection and
/// asset-file commands. The scene commands clear the marker themselves when
/// they start, open or save a scene.
pub fn mark_dirty(world: &mut World) {
    if let Some(session) = world.get_resource_mut::<SceneSession>() {
        session.dirty = true;
    }
}

/// Start an empty scene: every entity is removed, no file is open, and the
/// selection and history are cleared.
///
/// Returns how many entities were removed.
pub fn new_scene(engine: &mut Engine) -> usize {
    let removed_entity_count = clear_world(engine.world_mut());
    let world = engine.world_mut();
    if let Some(session) = world.get_resource_mut::<SceneSession>() {
        session.open = None;
        session.dirty = false;
    }
    reset_document_state(world);
    removed_entity_count
}

/// Open the scene at `path`, replacing the world's entities.
///
/// The path is below the asset directory; the file's own guid is kept, so
/// saving writes it back unchanged. The world is left as it was when the file
/// cannot be imported or carries no guid.
///
/// # Errors
///
/// Returns an error when the file cannot be imported: it is not a scene file,
/// no asset types are registered, or it fails to load. It also fails when the
/// file carries no guid.
pub fn open_scene(engine: &mut Engine, path: &str) -> Result<SceneLoad, String> {
    // Step 1: Read the document and its guid, before the world is touched.
    let (document, guid) = import_scene_document(engine, path)?;
    // A standalone scene carries its guid in the file's own header; the editor
    // needs it to save the file back under the same identity.
    let guid = guid.ok_or_else(|| format!("the scene file `{path}` carries no guid"))?;
    // Step 2: Replace the world.
    let database = scene_database(engine);
    clear_world(engine.world_mut());
    let scene_load = load_scene(engine, &database, &document);
    // Step 3: Record the scene as the open document.
    let world = engine.world_mut();
    if let Some(session) = world.get_resource_mut::<SceneSession>() {
        session.open = Some(OpenScene {
            path: path.to_string(),
            guid,
        });
        session.dirty = false;
    }
    reset_document_state(world);
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path,
        guid = %guid,
        entities = scene_load.entities,
        components = scene_load.components,
        unknown_components = scene_load.unknown_components.len(),
        unresolved_references = scene_load.unresolved_references,
        "[editor] scene opened"
    );
    Ok(scene_load)
}

/// Write the world to the open scene, or to `path` for a save-as.
///
/// Saving to the open scene keeps its guid; a save-as writes a fresh one
/// unless the target file already exists, in which case its guid is kept so
/// anything referencing it still resolves.
///
/// The file is read back after writing, so the loaded scene asset matches what
/// is on disk, and a file the engine cannot read is reported here instead of at
/// the next open.
///
/// # Errors
///
/// Returns an error when no asset types are registered, when no scene is open
/// and `path` is `None`, when no asset directory is mounted, or when the engine
/// has no [`AssetManager`].
///
/// It also fails when the document cannot be rendered or written, or when the
/// written file does not read back with the guid that was assigned.
pub fn write_scene(engine: &mut Engine, path: Option<&str>) -> Result<OpenScene, String> {
    // Step 1: Find the open scene and the file to write.
    let registry = engine
        .world()
        .get_resource::<ImportRegistry>()
        .cloned()
        .ok_or_else(|| "no asset types are registered".to_string())?;
    let open = engine
        .world()
        .get_resource::<SceneSession>()
        .and_then(|session| session.open.clone());
    let target = path
        .map(str::to_string)
        .or_else(|| open.as_ref().map(|open| open.path.clone()))
        .ok_or_else(|| "no scene is open; use Save Scene As".to_string())?;
    let root = pill_engine::asset_store::mounted_directory()
        .ok_or("no asset directory is mounted".to_owned())?;
    let file = root.join(&target);

    // Step 2: Build the document and report what the file cannot hold.
    let (document, save_report) = {
        let world = engine.world();
        let assets = world
            .get_resource::<AssetManager>()
            .ok_or_else(|| "the engine has no AssetManager".to_string())?;
        save_scene(world, assets, &registry, None)
    };
    // Some fields cannot be written to the file: a text or list field, a float
    // that is NaN or infinite, and a handle to an asset with no guid (game code
    // created it instead of loading a file). Each one is a loss the author has
    // to see, so it is logged. The save still goes ahead: refusing would stop a
    // project that builds its meshes in code from saving a scene.
    if !save_report.skipped_fields.is_empty() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            count = save_report.skipped_fields.len(),
            fields = save_report.skipped_fields.join(", ").as_str(),
            "[editor] scene fields not written"
        );
    }
    // A component the field layer cannot reach is left out of the file, so
    // reopening the scene brings the entity back without it. Here too the save
    // goes ahead, and the author is told which components were dropped.
    if !save_report.skipped_components.is_empty() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            count = save_report.skipped_components.len(),
            components = save_report.skipped_components.join(", ").as_str(),
            "[editor] scene components not written"
        );
    }

    // Step 3: Choose the guid and write the file.
    let guid = match (open.as_ref(), path) {
        // Save to the open scene: keep its guid.
        (Some(open), None) => open.guid,
        // Save As onto an existing scene file: keep the guid that file holds.
        _ if file.is_file() => match existing_guid(engine, &target) {
            Some(guid) => guid,
            None => fresh_guid()?,
        },
        _ => fresh_guid()?,
    };
    let bytes = render_standalone::<Scene>(guid, &document)
        .map_err(|error| format!("the scene cannot be rendered: {error}"))?;
    crate::editor_files::write_bytes(&file, &bytes).map_err(|error| error.message())?;

    // Step 4: Read the file back and record it as the open scene. A file the
    // engine cannot read fails here rather than at the next open.
    let reimported_scene = {
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .ok_or_else(|| "the engine has no AssetManager".to_string())?;
        assets
            .reimport_standalone::<Scene>(Path::new(&target))
            .map_err(|error| format!("`{target}` was written but did not load back: {error}"))?
    };
    if reimported_scene.guid != Some(guid) {
        return Err(format!(
            "`{target}` was written with guid {guid} but reads back as {}",
            reimported_scene
                .guid
                .map_or_else(|| "none".to_owned(), |guid| guid.to_string())
        ));
    }
    let saved_scene = OpenScene { path: target, guid };
    let world = engine.world_mut();
    if let Some(session) = world.get_resource_mut::<SceneSession>() {
        session.open = Some(saved_scene.clone());
        session.dirty = false;
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        path = saved_scene.path.as_str(),
        guid = %saved_scene.guid,
        entities = document.entities.len(),
        components = save_report.components,
        "[editor] scene saved"
    );
    Ok(saved_scene)
}

/// The guid the scene file at `path` already carries, or `None` when it cannot
/// be read.
fn existing_guid(engine: &mut Engine, path: &str) -> Option<AssetGuid> {
    let assets = engine.world_mut().get_resource_mut::<AssetManager>()?;
    assets
        .import_standalone::<Scene>(Path::new(path))
        .ok()
        .and_then(|imported| imported.guid)
}

/// A fresh guid for a scene file that does not exist yet.
///
/// # Errors
///
/// Returns an error when the platform random source cannot produce one.
fn fresh_guid() -> Result<AssetGuid, String> {
    AssetGuid::random().map_err(|error| format!("no random source for a scene guid: {error}"))
}

/// Destroy every entity and return how many there were. A destroy that fails
/// is ignored.
fn clear_world(world: &mut World) -> usize {
    let entities: Vec<_> = world
        .entity_rows()
        .into_iter()
        .map(|row| row.entity)
        .collect();
    for entity in &entities {
        let _ = world.destroy_entity(*entity);
    }
    entities.len()
}

/// Forget the selection and the recorded history. Both name entities that
/// belonged to the document this scene replaced.
fn reset_document_state(world: &mut World) {
    if let Some(selection) = world.get_resource_mut::<EditorSelection>() {
        *selection = EditorSelection::default();
    }
    if let Some(history) = world.get_resource_mut::<crate::history::CommandHistory>() {
        history.clear();
    }
}

/// Whether `command` changes which scene document is open. The scene commands
/// are not undoable edits and never mark the document dirty.
#[must_use]
pub fn is_document_command(command: &EditorCommand) -> bool {
    matches!(
        command,
        EditorCommand::SceneNew
            | EditorCommand::SceneOpen { .. }
            | EditorCommand::SceneSave
            | EditorCommand::SceneSaveAs { .. }
    )
}

/// Add the `.scene` extension to `requested_name` when it has none.
///
/// Surrounding spaces are trimmed, and the extension is matched in any letter
/// case, so `My Scene.SCENE` is left alone.
#[must_use]
pub fn scene_file_name(requested_name: &str) -> String {
    let trimmed = requested_name.trim();
    if trimmed.to_ascii_lowercase().ends_with(".scene") {
        trimmed.to_string()
    } else {
        format!("{trimmed}.scene")
    }
}

/// The path a Save As defaults to: the open scene's path, or
/// `scenes/untitled.scene` when no scene file is open.
#[must_use]
pub fn default_save_path(scene_session: &SceneSession) -> String {
    match &scene_session.open {
        Some(open) => open.path.clone(),
        None => PathBuf::from("scenes")
            .join(scene_file_name("untitled"))
            .to_string_lossy()
            .replace('\\', "/"),
    }
}
