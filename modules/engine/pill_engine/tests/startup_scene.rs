//! The startup scene loads between component registration and the project's
//! `init`.
//!
//! # Responsibilities
//!
//! - Prove the order: a scene queued before the project initializes is in the
//!   world by the time the project's own `init` body runs. A project that adds
//!   entities until it reaches a target count therefore finds the scene's
//!   entities already there, instead of adding its own over them.
//!
//! # Design
//!
//! The entry point is [`run_project_init`]. The `pill_module_init` wrapper that
//! `#[pill_project]` generates calls it, and the development host reaches it
//! through that wrapper. A shipped Rust project runs the same steps itself,
//! through `run_artifact_entry`.

// Standard library
use std::cell::Cell;
use std::path::Path;

// External crates
use pill_engine::asset_store::mount_directory;
use pill_engine::component_registry::{no_registration, register_all_components, run_project_init};
use pill_engine::{
    component, queue_startup_scene, render_standalone, save_scene, AssetManager, Engine, EntityId,
    ImportRegistry, Scene,
};

// =============================================================================
// Recorded Counts
// =============================================================================

// The entity and beacon counts the project's `init` body saw.
//
// The two tests run in parallel and both pass `run_project_init` the same
// function pointer, the only kind of function that signature accepts. A shared
// static would therefore let either test read the other's counts, so each test
// thread keeps its own copy.
thread_local! {
    static RECORDED_COUNTS_AT_INIT: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

// =============================================================================
// Test Types
// =============================================================================

/// The component the scene carries.
#[component]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Beacon {
    /// The value the test writes and reads back unchanged.
    power: f32,
}

// =============================================================================
// Test Helpers
// =============================================================================

/// The project `init` both tests pass in: it records how many entities and
/// beacons the world holds, then reports success.
fn recording_init(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();
    let entity_count = world.entity_rows().len();
    let beacon_count = pill_engine::Query::<&Beacon>::new(world).iter_mut().count();
    RECORDED_COUNTS_AT_INIT.with(|slot| slot.set(Some((entity_count, beacon_count))));
    0
}

/// Returns the entity and beacon counts the current thread's `init` recorded.
///
/// # Panics
///
/// Panics when `init` has not run on this thread yet.
fn recorded_counts_at_init() -> (usize, usize) {
    RECORDED_COUNTS_AT_INIT
        .with(|slot| slot.get())
        .expect("init ran on this thread")
}

// =============================================================================
// Tests
// =============================================================================

/// Saves a world holding one beacon, then starts a project with that scene
/// queued and checks its `init` already saw the beacon.
#[test]
fn the_startup_scene_is_in_the_world_before_init_runs() {
    let asset_directory = std::env::temp_dir().join(format!("pill-startup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&asset_directory);
    std::fs::create_dir_all(&asset_directory).unwrap();
    mount_directory(&asset_directory);

    // Step 1: build a world with one beacon and save it as a scene file.
    let beacon_id = EntityId::from_name("beacon");
    let scene_bytes = {
        let mut engine = Engine::new();
        register_all_components(engine.world_mut()).expect("registration succeeds");
        engine.world_mut().register_standalone_asset::<Scene>();
        engine
            .world_mut()
            .create_entity()
            .with_id(beacon_id)
            .with(Beacon { power: 3.5 })
            .build()
            .expect("the beacon builds");
        let import_registry = engine
            .world()
            .get_resource::<ImportRegistry>()
            .expect("an engine owns an import registry")
            .clone();
        let world = engine.world();
        let asset_manager = world
            .get_resource::<AssetManager>()
            .expect("an engine owns an asset manager");
        let (scene_document, save_report) =
            save_scene(world, asset_manager, &import_registry, None);
        assert!(
            save_report.skipped_fields.is_empty(),
            "{:?}",
            save_report.skipped_fields
        );
        render_standalone::<Scene>(
            pill_engine::AssetGuid::random().expect("a fresh guid"),
            &scene_document,
        )
        .expect("the scene renders")
    };
    let scene_path = asset_directory.join("scenes").join("start.scene");
    std::fs::create_dir_all(scene_path.parent().unwrap()).unwrap();
    std::fs::write(&scene_path, &scene_bytes).unwrap();

    // Step 2: start a fresh engine of the same project with that scene queued.
    let mut engine = Engine::new();
    engine.world_mut().register_standalone_asset::<Scene>();
    queue_startup_scene(engine.world_mut(), "scenes/start.scene");
    let init_status = run_project_init(&mut engine, no_registration, recording_init);
    assert_eq!(init_status, 0, "the project initializes");

    // Step 3: check that `init` saw the scene's one entity and nothing else.
    let (entity_count, beacon_count) = recorded_counts_at_init();
    assert_eq!(
        beacon_count, 1,
        "the scene's beacon existed before the project's init"
    );
    assert_eq!(
        entity_count, 1,
        "the init body saw exactly the scene's entity"
    );
    let world = engine.world_mut();
    let beacon_entity = world
        .entity_by_id(beacon_id)
        .expect("the beacon kept the id the document gave it");
    assert_eq!(
        world
            .get_component::<Beacon>(beacon_entity)
            .map(|beacon| beacon.power),
        Some(3.5)
    );

    let _ = std::fs::remove_dir_all(Path::new(&asset_directory));
}

/// Starts a project that queues no scene and checks its `init` saw an empty
/// world.
#[test]
fn a_project_without_a_startup_scene_is_untouched() {
    let mut engine = Engine::new();
    let init_status = run_project_init(&mut engine, no_registration, recording_init);
    assert_eq!(init_status, 0);
    let (entity_count, beacon_count) = recorded_counts_at_init();
    assert_eq!(entity_count, 0, "an empty project seeds no entity");
    assert_eq!(beacon_count, 0, "an empty project owns no beacon");
}
