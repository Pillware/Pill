//! Tests for the run mode: which systems and scripts run while the editor
//! edits, while the game plays, while play is paused, and during a step.
//!
//! # Responsibilities
//!
//! - Show that `RunMode::Editing` runs only the systems that opted in, for
//!   both the update and the post-update phase, and that a script does not run.
//! - Show that `RunMode::Playing` runs every system and the scripts.
//! - Show that pausing a session runs the opted-in systems only, and that one
//!   step runs everything exactly once.
//! - Show that a play session refuses a world it cannot copy, naming the type,
//!   and leaves the editor in edit mode.
//!
//! # Design
//!
//! The systems here are closures over `Arc<AtomicUsize>` counters rather than
//! resource readers, so a test says which phase and which mode ran a system
//! without also constructing the ECS state a resource would need.

// Standard library
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// Current crate
use crate::component::Component;
use crate::engine::{Engine, RunMode};
use crate::scheduler::RunPhase;
use crate::scripting::{ScriptComponent, ScriptContext, ScriptDeclaration, ScriptDeclarationData};
use crate::world::World;

// =============================================================================
// Fixtures
// =============================================================================

/// The four systems one test registers, each with its own counter.
struct PhaseCounters {
    /// An update system that runs only while the game plays.
    update_play_only: Arc<AtomicUsize>,
    /// An update system that opted in to edit mode.
    update_edit_mode: Arc<AtomicUsize>,
    /// A post-update system that runs only while the game plays.
    post_update_play_only: Arc<AtomicUsize>,
    /// A post-update system that opted in to edit mode.
    post_update_edit_mode: Arc<AtomicUsize>,
}

/// Registers the four systems, each counting its own dispatches.
fn register_phase_systems(engine: &mut Engine) -> PhaseCounters {
    let counters = PhaseCounters {
        update_play_only: Arc::new(AtomicUsize::new(0)),
        update_edit_mode: Arc::new(AtomicUsize::new(0)),
        post_update_play_only: Arc::new(AtomicUsize::new(0)),
        post_update_edit_mode: Arc::new(AtomicUsize::new(0)),
    };
    let update_play_only = Arc::clone(&counters.update_play_only);
    engine.register_system("update_play_only", move || {
        update_play_only.fetch_add(1, Ordering::SeqCst);
    });
    let update_edit_mode = Arc::clone(&counters.update_edit_mode);
    engine.register_system_with_options(
        "update_edit_mode",
        crate::engine::SystemOptions::default().with_run_in_edit_mode(true),
        move || {
            update_edit_mode.fetch_add(1, Ordering::SeqCst);
        },
    );
    let post_update_play_only = Arc::clone(&counters.post_update_play_only);
    engine.register_system_with_options(
        "post_update_play_only",
        crate::engine::SystemOptions::new(RunPhase::PostProject, 0),
        move || {
            post_update_play_only.fetch_add(1, Ordering::SeqCst);
        },
    );
    let post_update_edit_mode = Arc::clone(&counters.post_update_edit_mode);
    engine.register_system_with_options(
        "post_update_edit_mode",
        crate::engine::SystemOptions::new(RunPhase::PostProject, 0).with_run_in_edit_mode(true),
        move || {
            post_update_edit_mode.fetch_add(1, Ordering::SeqCst);
        },
    );
    counters
}

/// Returns a counter's current value.
fn count(counter: &Arc<AtomicUsize>) -> usize {
    counter.load(Ordering::SeqCst)
}

/// A script component that counts its own dispatches.
#[derive(Debug, Default)]
struct CountingScript {
    /// How many times the engine ran it.
    dispatches: u32,
}

impl Component for CountingScript {}

impl ScriptDeclaration for CountingScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_update(0);
}

impl ScriptComponent for CountingScript {
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        self.dispatches += 1;
    }
}

/// A script component that opted in to edit mode, so it runs while the editor
/// edits as well as while it plays.
#[derive(Debug, Default)]
struct EditModeScript {
    /// How many times the engine ran it.
    dispatches: u32,
}

impl Component for EditModeScript {}

impl ScriptDeclaration for EditModeScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_update(0)
        .with_edit_mode();
}

impl ScriptComponent for EditModeScript {
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        self.dispatches += 1;
    }
}

/// A component that owns heap data and is not `Clone`, so a play session cannot
/// copy it.#[derive(Debug)]
struct UncopyableProbe {
    /// The reason it cannot be copied: its own buffer.
    #[allow(dead_code)]
    buffer: Vec<u32>,
}

impl Component for UncopyableProbe {}

/// Builds an engine holding one entity with [`CountingScript`] on it.
fn engine_with_a_script() -> (Engine, crate::entity::Entity) {
    let mut engine = Engine::new();
    let world: &mut World = engine.world_mut();
    world.register_script_component::<CountingScript>();
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, CountingScript::default())
        .expect("the script attaches");
    (engine, entity)
}

/// Returns the dispatch count of the script on `entity`.
fn script_dispatches(engine: &Engine, entity: crate::entity::Entity) -> u32 {
    engine
        .world()
        .get_component::<CountingScript>(entity)
        .expect("the entity carries the script")
        .dispatches
}

/// Returns the dispatch count of the edit-mode script on `entity`.
fn edit_mode_dispatches(engine: &Engine, entity: crate::entity::Entity) -> u32 {
    engine
        .world()
        .get_component::<EditModeScript>(entity)
        .expect("the entity carries the script")
        .dispatches
}

// =============================================================================
// The run mode
// =============================================================================

/// In edit mode only the systems that opted in run, in both phases.
#[test]
fn editing_runs_only_the_systems_that_opted_in() {
    let mut engine = Engine::new();
    engine.set_run_mode(RunMode::Editing);
    let counters = register_phase_systems(&mut engine);

    engine.process_frame().expect("the frame runs");
    engine.process_frame().expect("the frame runs");

    assert_eq!(count(&counters.update_edit_mode), 2);
    assert_eq!(count(&counters.post_update_edit_mode), 2);
    assert_eq!(count(&counters.update_play_only), 0);
    assert_eq!(count(&counters.post_update_play_only), 0);
}

/// While the game plays every system runs, opted in or not.
#[test]
fn playing_runs_every_system() {
    let mut engine = Engine::new();
    // `Playing` is the default, so nothing has to be set here.
    let counters = register_phase_systems(&mut engine);

    engine.process_frame().expect("the frame runs");

    assert_eq!(count(&counters.update_play_only), 1);
    assert_eq!(count(&counters.update_edit_mode), 1);
    assert_eq!(count(&counters.post_update_play_only), 1);
    assert_eq!(count(&counters.post_update_edit_mode), 1);
}

/// Pausing a session stops the game's systems and keeps the opted-in ones
/// running, so the editor keeps drawing its own frame.
#[test]
fn pausing_a_session_runs_only_the_opted_in_systems() {
    let mut engine = Engine::new();
    let counters = register_phase_systems(&mut engine);
    engine.process_frame().expect("the frame runs");
    assert_eq!(count(&counters.update_play_only), 1);

    engine.set_systems_paused(true);
    engine.process_frame().expect("the frame runs");
    engine.process_frame().expect("the frame runs");

    assert_eq!(count(&counters.update_play_only), 1);
    assert_eq!(count(&counters.post_update_play_only), 1);
    assert_eq!(count(&counters.update_edit_mode), 3);
    assert_eq!(count(&counters.post_update_edit_mode), 3);
}

/// One step runs everything exactly once, then the session is paused again.
#[test]
fn a_step_runs_every_system_exactly_once() {
    let mut engine = Engine::new();
    let counters = register_phase_systems(&mut engine);
    engine.set_systems_paused(true);

    engine.request_single_step();
    engine.process_frame().expect("the frame runs");
    assert_eq!(count(&counters.update_play_only), 1);
    assert_eq!(count(&counters.post_update_play_only), 1);
    assert_eq!(count(&counters.update_edit_mode), 1);
    assert_eq!(count(&counters.post_update_edit_mode), 1);

    // The step is used up: the next frame is a paused one, so only the
    // opted-in systems run again.
    engine.process_frame().expect("the frame runs");
    assert_eq!(count(&counters.update_play_only), 1);
    assert_eq!(count(&counters.update_edit_mode), 2);
}

/// A script runs only while the game does unless its declaration opted in to
/// edit mode: editing and pausing both leave it alone, and a step runs it once.
#[test]
fn a_script_runs_only_while_the_game_runs() {
    let (mut engine, entity) = engine_with_a_script();

    engine.set_run_mode(RunMode::Editing);
    engine.process_frame().expect("the frame runs");
    assert_eq!(script_dispatches(&engine, entity), 0);

    engine.set_run_mode(RunMode::Playing);
    engine.process_frame().expect("the frame runs");
    assert_eq!(script_dispatches(&engine, entity), 1);

    engine.set_systems_paused(true);
    engine.process_frame().expect("the frame runs");
    assert_eq!(script_dispatches(&engine, entity), 1);

    engine.request_single_step();
    engine.process_frame().expect("the frame runs");
    assert_eq!(script_dispatches(&engine, entity), 2);
}

/// A script whose declaration opted in to edit mode runs while the editor
/// edits, beside a plain script that stays out of it.
#[test]
fn an_edit_mode_script_runs_while_the_editor_edits() {
    let mut engine = Engine::new();
    let world: &mut World = engine.world_mut();
    world.register_script_component::<CountingScript>();
    world.register_script_component::<EditModeScript>();
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, CountingScript::default())
        .expect("the plain script attaches");
    world
        .add_component(entity, EditModeScript::default())
        .expect("the edit-mode script attaches");

    engine.set_run_mode(RunMode::Editing);
    engine.process_frame().expect("the frame runs");

    assert_eq!(
        edit_mode_dispatches(&engine, entity),
        1,
        "the opted-in script runs while the editor edits"
    );
    assert_eq!(
        script_dispatches(&engine, entity),
        0,
        "a script that did not opt in stays out of edit mode"
    );
}

// =============================================================================
// A refused session
// =============================================================================

/// A world holding a type the engine cannot copy refuses Play by name, and the
/// editor stays in edit mode.
#[test]
fn a_world_the_engine_cannot_copy_is_refused_by_name() {
    let mut engine = Engine::new();
    engine.set_run_mode(RunMode::Editing);
    let world: &mut World = engine.world_mut();
    world.register_component::<UncopyableProbe>();
    world
        .create_entity()
        .with(UncopyableProbe {
            buffer: vec![1, 2, 3],
        })
        .build()
        .expect("the entity builds");

    let refusal = engine.enter_play().expect_err("the copy is refused");

    assert_eq!(refusal.component_type_names.len(), 1);
    assert!(
        refusal.component_type_names[0].ends_with("UncopyableProbe"),
        "the refusal names the component: {:?}",
        refusal.component_type_names
    );
    assert!(refusal.resource_type_names.is_empty());
    assert_eq!(engine.run_mode(), RunMode::Editing);
    assert_eq!(
        engine.world().restore_epoch(),
        0,
        "a refused session restores nothing"
    );
}
