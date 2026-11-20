//! The startup scene: the `.scene` file a project's settings name, loaded as
//! soon as the project's component types exist.
//!
//! # Responsibilities
//!
//! - [`StartupScene`]: the request. The front end queues it from the project's
//!   settings, and the project's entry point takes it out of the world and
//!   loads it.
//! - [`apply_startup_scene`]: the load itself.
//!
//! # Design
//!
//! A project's `init` fills its world up to the counts it expects, so a scene
//! loaded after it would double every entity. A scene loaded before the
//! components are registered would find no component types to attach. The
//! generated project entry point therefore loads this request right after
//! registering the project's components and before its `init`. A shipping
//! build loads it in the same place, inside the project's entry point and
//! after its modules.
//!
//! A managed (C#) project registers its components inside `start`, so it gets
//! the scene once `start` has returned - the earliest point at which its
//! component types exist.
//!
//! A load that fails is logged and not passed back: a missing or unreadable
//! scene must not stop a project from starting with its own scene.

// External crates
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::engine::Engine;
use crate::resource::Resource;
use crate::scene::SceneLoad;
use crate::world::World;

// =============================================================================
// StartupScene
// =============================================================================

/// A scene the front end asked the engine to load before the project starts.
///
/// It is stored as a world resource, and loading it removes it.
pub struct StartupScene {
    /// The scene's path below the project's asset directory, as
    /// `project_settings.yaml` names it.
    pub path: String,
}

impl Resource for StartupScene {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Ask for `path` to be loaded while the project starts, once its component
/// types are registered.
///
/// A second call replaces the first request, because both are stored under the
/// same resource type.
pub fn queue_startup_scene(world: &mut World, path: impl Into<String>) {
    world.insert_resource(StartupScene { path: path.into() });
}

/// Load the queued startup scene, if one was queued.
///
/// `None` when nothing was queued, or when the request could not be taken out
/// of the world (that failure is logged). Otherwise the `SceneLoad` says what
/// the load created and what it had to skip.
///
/// # Errors
///
/// The inner `Err` carries the reason the scene could not be read.
/// [`crate::asset_loading::import_scene_document`] returns it when no asset
/// types are registered, when `path` is not a scene file, or when the file
/// cannot be loaded or kept loaded. A scene that reads but loses parts still
/// returns `Ok`: unknown components and unresolved references are counted in
/// the [`SceneLoad`] instead.
pub fn apply_startup_scene(engine: &mut Engine) -> Option<Result<SceneLoad, String>> {
    // Step 1: Take the queued request. If it cannot be removed, log that and
    // return `None`, as if nothing had been queued.
    let startup_scene = match engine.world_mut().remove_resource::<StartupScene>() {
        Ok(queued_scene) => queued_scene?,
        Err(error) => {
            pill_core::warn!(
                target: telemetry_target::ENGINE,
                error = %error,
                "startup scene request could not be taken; no scene is loaded"
            );
            return None;
        }
    };
    // Step 2: Load it.
    Some(load_startup_scene(engine, &startup_scene.path))
}

/// Load the scene at `path` into `engine`, through the one scene loader every
/// entry point shares.
///
/// When the host built an asset database - a shipped pack's index, or an index
/// of the project's files - the scene's references resolve through it. That is
/// what lets a reference carrying only a guid find its file in a pack build,
/// where no directory is mounted. Without one, an index is built from the
/// mounted asset directory, or left empty when nothing is mounted.
///
/// # Errors
///
/// Returns the reason when no asset types are registered, when `path` is not a
/// scene file, or when the file cannot be loaded or kept loaded.
fn load_startup_scene(engine: &mut Engine, path: &str) -> Result<SceneLoad, String> {
    let scene_load = crate::asset_loading::load_scene_asset(engine, path)?;
    pill_core::info!(
        target: telemetry_target::ENGINE,
        path,
        entities = scene_load.entities,
        components = scene_load.components,
        unknown_components = scene_load.unknown_components.len(),
        unresolved_references = scene_load.unresolved_references,
        "startup scene loaded"
    );
    Ok(scene_load)
}
