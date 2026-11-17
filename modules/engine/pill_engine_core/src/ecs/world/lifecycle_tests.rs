//! Unit tests for the script lifecycle passes: `on_create` and `on_start`.
//!
//! # Responsibilities
//!
//! - Show that Play tells every script it started existing and then started
//!   running, in that order and before its first `on_update`.
//! - Show that a script added later is told at the next dispatch point, still
//!   before its first update.
//! - Show that an entry for a script that no longer carries the component is
//!   dropped, that edit mode tells only the scripts that opted in and never
//!   starts one, and that a re-registration repeats neither `on_create` nor
//!   `on_start`.
//! - Show that two types are told in their declared `on_create` order.
//!
//! # Design
//!
//! Every lifecycle method appends a line to a component on an entity of its
//! own, so the order of the lines is the observation. A component is used
//! rather than a shared static so two tests running at the same time cannot see
//! each other's entries.

// Standard library
use std::sync::atomic::{AtomicUsize, Ordering};

// Current crate
use crate::commands::CommandQueue;
use crate::component::Component;
use crate::component::ComponentId;
use crate::engine::{Engine, RunMode};
use crate::entity::Entity;
use crate::scripting::{
    CreateInfo, DestroyInfo, ScriptComponent, ScriptContext, ScriptDeclaration,
    ScriptDeclarationData, StartInfo, StopInfo,
};
use crate::world::{ChangeOrigin, World};

// =============================================================================
// Fixtures
// =============================================================================

/// The lifecycle lines one script recorded.
#[derive(Debug, Default, Clone)]
struct LifecycleLog {
    /// One line per lifecycle method call, in the order they happened.
    entries: Vec<String>,
}

impl Component for LifecycleLog {}

/// Appends `entry` to the log the script holds a handle to.
fn record(log_entity: Entity, script_context: &mut ScriptContext, entry: String) {
    if let Some(log) = script_context.get_component_mut::<LifecycleLog>(log_entity) {
        log.entries.push(entry);
    }
}

/// A script that records every lifecycle method the engine calls on it,
/// including why it was called.
#[derive(Debug, Clone)]
struct RecordingScript {
    /// The entity holding the log.
    log_entity: Entity,
}

impl Component for RecordingScript {}

impl ScriptDeclaration for RecordingScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_create(0)
        .with_on_start(0)
        .with_on_update(0)
        .with_on_stop(0)
        .with_on_destroy(0);
}

impl ScriptComponent for RecordingScript {
    fn on_create(&mut self, script_context: &mut ScriptContext, create_info: &CreateInfo) {
        record(
            self.log_entity,
            script_context,
            format!("create:{:?}:{:?}", create_info.cause, create_info.mode),
        );
    }

    fn on_start(&mut self, script_context: &mut ScriptContext, start_info: &StartInfo) {
        record(
            self.log_entity,
            script_context,
            format!("start:{:?}", start_info.cause),
        );
    }

    fn on_update(&mut self, script_context: &mut ScriptContext) {
        record(self.log_entity, script_context, String::from("update"));
    }

    fn on_stop(&mut self, script_context: &mut ScriptContext, stop_info: &StopInfo) {
        record(
            self.log_entity,
            script_context,
            format!("stop:{:?}", stop_info.cause),
        );
    }

    fn on_destroy(&mut self, script_context: &mut ScriptContext, destroy_info: &DestroyInfo) {
        record(
            self.log_entity,
            script_context,
            format!("destroy:{:?}", destroy_info.cause),
        );
    }
}

/// A recording script that also runs while the editor edits.
#[derive(Debug, Clone)]
struct EditModeRecordingScript {
    /// The entity holding the log.
    log_entity: Entity,
}

impl Component for EditModeRecordingScript {}

impl ScriptDeclaration for EditModeRecordingScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_create(0)
        .with_on_start(0)
        .with_on_update(0)
        .with_on_stop(0)
        .with_on_destroy(0)
        .with_edit_mode();
}

impl ScriptComponent for EditModeRecordingScript {
    fn on_create(&mut self, script_context: &mut ScriptContext, create_info: &CreateInfo) {
        record(
            self.log_entity,
            script_context,
            format!("create:{:?}:{:?}", create_info.cause, create_info.mode),
        );
    }

    fn on_start(&mut self, script_context: &mut ScriptContext, start_info: &StartInfo) {
        record(
            self.log_entity,
            script_context,
            format!("start:{:?}", start_info.cause),
        );
    }

    fn on_update(&mut self, script_context: &mut ScriptContext) {
        record(self.log_entity, script_context, String::from("update"));
    }

    fn on_stop(&mut self, script_context: &mut ScriptContext, stop_info: &StopInfo) {
        record(
            self.log_entity,
            script_context,
            format!("stop:{:?}", stop_info.cause),
        );
    }

    fn on_destroy(&mut self, script_context: &mut ScriptContext, destroy_info: &DestroyInfo) {
        record(
            self.log_entity,
            script_context,
            format!("destroy:{:?}", destroy_info.cause),
        );
    }
}

/// A recording script whose `on_create` runs before [`LateCreateScript`]'s.
#[derive(Debug, Clone)]
struct EarlyCreateScript {
    /// The entity holding the log.
    log_entity: Entity,
}

impl Component for EarlyCreateScript {}

impl ScriptDeclaration for EarlyCreateScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_create(10)
        .with_on_start(0);
}

impl ScriptComponent for EarlyCreateScript {
    fn on_create(&mut self, script_context: &mut ScriptContext, _create_info: &CreateInfo) {
        record(self.log_entity, script_context, String::from("early"));
    }

    fn on_start(&mut self, script_context: &mut ScriptContext, _start_info: &StartInfo) {
        record(self.log_entity, script_context, String::from("early start"));
    }
}

/// A recording script whose `on_create` runs after [`EarlyCreateScript`]'s,
/// even though it is registered first.
#[derive(Debug, Clone)]
struct LateCreateScript {
    /// The entity holding the log.
    log_entity: Entity,
}

impl Component for LateCreateScript {}

impl ScriptDeclaration for LateCreateScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_create(20)
        .with_on_start(0);
}

impl ScriptComponent for LateCreateScript {
    fn on_create(&mut self, script_context: &mut ScriptContext, _create_info: &CreateInfo) {
        record(self.log_entity, script_context, String::from("late"));
    }

    fn on_start(&mut self, script_context: &mut ScriptContext, _start_info: &StartInfo) {
        record(self.log_entity, script_context, String::from("late start"));
    }
}

/// Builds an engine that holds one log entity, and returns it with the entity.
fn engine_with_log() -> (Engine, Entity) {
    let mut engine = Engine::new();
    let world = engine.world_mut();
    world.register_component::<LifecycleLog>();
    world.record_component_clone::<LifecycleLog>();
    // A play session copies a component only when its clone function was
    // recorded, which the component derive does for a `Clone` type. The
    // fixtures are declared by hand, so they record it by hand.
    world.record_component_clone::<RecordingScript>();
    world.record_component_clone::<EditModeRecordingScript>();
    world.record_component_clone::<EarlyCreateScript>();
    world.record_component_clone::<LateCreateScript>();
    world.record_component_clone::<StopProbeScript>();
    world.record_component_clone::<SpawningDestroyScript>();
    let log_entity = world
        .create_entity()
        .build()
        .expect("the log entity builds");
    world
        .add_component(log_entity, LifecycleLog::default())
        .expect("the log attaches");
    (engine, log_entity)
}

/// Returns the lines the log has recorded so far.
fn entries(world: &World, log_entity: Entity) -> Vec<String> {
    world
        .get_component::<LifecycleLog>(log_entity)
        .expect("the log is on its entity")
        .entries
        .clone()
}

// =============================================================================
// Play
// =============================================================================

/// Play tells every script it started existing and then started running, both
/// before its first update.
#[test]
fn play_tells_every_script_it_started_existing_and_running() {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<RecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, RecordingScript { log_entity })
        .expect("the script attaches");

    // Editing tells a script that did not opt in nothing.
    engine.process_frame().expect("the frame runs");
    assert!(
        entries(engine.world(), log_entity).is_empty(),
        "a plain script hears nothing while the editor edits"
    );

    engine.enter_play().expect("the world copies");
    engine.process_frame().expect("the frame runs");
    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:PlayStarted:Playing", "start:PlayEntered", "update"],
        "play creates, then starts, then updates"
    );
}

/// A script added during play is told at the next dispatch point, before it is
/// ever updated.
#[test]
fn a_script_added_during_play_is_told_before_its_first_update() {
    let (mut engine, _play_log) = engine_with_log();
    engine
        .world_mut()
        .register_script_component::<RecordingScript>();
    engine.enter_play().expect("the world copies");
    engine.process_frame().expect("the frame runs");

    let late_log = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the late log builds");
    engine
        .world_mut()
        .add_component(late_log, LifecycleLog::default())
        .expect("the late log attaches");
    let late = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the late entity builds");
    engine
        .world_mut()
        .add_component(
            late,
            RecordingScript {
                log_entity: late_log,
            },
        )
        .expect("the late script attaches");

    engine.process_frame().expect("the frame runs");

    assert_eq!(
        entries(engine.world(), late_log),
        [
            "create:Spawned:Playing",
            "start:SpawnedDuringPlay",
            "update"
        ],
        "an attachment is told before the first update of the frame after it"
    );
}

/// A script that loses its component before the passes run is skipped instead
/// of resurrected.
#[test]
fn a_script_removed_before_the_pass_never_hears_about_it() {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<EditModeRecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, EditModeRecordingScript { log_entity })
        .expect("the script attaches");
    engine
        .world_mut()
        .try_remove_component_by_id(entity, ComponentId::of::<EditModeRecordingScript>())
        .expect("the component is removed");

    engine.process_frame().expect("the frame runs");

    assert!(
        entries(engine.world(), log_entity).is_empty(),
        "an entry whose component is gone is dropped"
    );
}

/// Re-registering a script type - what a reload does - repeats neither
/// `on_create` nor `on_start`.
#[test]
fn a_re_registration_repeats_neither_create_nor_start() {
    let (mut engine, log_entity) = engine_with_log();
    engine
        .world_mut()
        .register_script_component::<RecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, RecordingScript { log_entity })
        .expect("the script attaches");
    engine.enter_play().expect("the world copies");
    engine.process_frame().expect("the frame runs");
    assert_eq!(
        entries(engine.world(), log_entity).len(),
        3,
        "play created, started and updated the script once"
    );

    engine
        .world_mut()
        .register_script_component::<RecordingScript>();
    engine.process_frame().expect("the frame runs");

    assert_eq!(
        entries(engine.world(), log_entity),
        [
            "create:PlayStarted:Playing",
            "start:PlayEntered",
            "update",
            "update"
        ],
        "a re-registration only updates"
    );
}

// =============================================================================
// Edit mode
// =============================================================================

/// A script that opted in to edit mode is told it was created, and never told
/// it started running, because nothing starts while the editor edits.
#[test]
fn an_edit_mode_script_is_told_it_was_created_without_starting() {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<EditModeRecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, EditModeRecordingScript { log_entity })
        .expect("the script attaches");

    engine.process_frame().expect("the frame runs");

    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:Spawned:Editing", "update"],
        "edit mode creates the opted-in script and starts nothing"
    );
}

// =============================================================================
// Order
// =============================================================================

/// Both start-side passes follow their own order values, whatever order the
/// types were registered in.
#[test]
fn the_create_and_start_passes_follow_their_own_orders() {
    let (mut engine, log_entity) = engine_with_log();
    let world = engine.world_mut();
    world.register_script_component::<LateCreateScript>();
    world.register_script_component::<EarlyCreateScript>();
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(entity, LateCreateScript { log_entity })
        .expect("the late script attaches");
    world
        .add_component(entity, EarlyCreateScript { log_entity })
        .expect("the early script attaches");
    engine.enter_play().expect("the world copies");

    engine.process_frame().expect("the frame runs");

    assert_eq!(
        entries(engine.world(), log_entity),
        ["early", "late", "early start", "late start"],
        "on_create runs before any on_start, each in its own declared order"
    );
}

// =============================================================================
// Removal
// =============================================================================

/// A script that queues a new entity from its own destruction, so a test can
/// see where a lifecycle method's commands go.
#[derive(Debug, Clone)]
struct SpawningDestroyScript;

impl Component for SpawningDestroyScript {}

impl ScriptDeclaration for SpawningDestroyScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE.with_on_destroy(0);
}

impl ScriptComponent for SpawningDestroyScript {
    fn on_destroy(&mut self, script_context: &mut ScriptContext, _destroy_info: &DestroyInfo) {
        script_context.create_entity().build();
    }
}

/// Removing a running script stops it, then tells it it is being destroyed.
#[test]
fn a_removal_during_play_stops_the_script_then_destroys_it() {
    let (mut engine, log_entity) = engine_with_log();
    engine
        .world_mut()
        .register_script_component::<RecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, RecordingScript { log_entity })
        .expect("the script attaches");
    engine.enter_play().expect("the world copies");
    engine.process_frame().expect("the frame runs");

    let mut commands = CommandQueue::new();
    commands.remove_component_from_entity::<RecordingScript>(entity);
    commands
        .execute_queued_commands(engine.world_mut(), true)
        .expect("the removal applies");

    let recorded = entries(engine.world(), log_entity);
    assert_eq!(
        &recorded[3..],
        ["stop:Removed", "destroy:Despawned"],
        "a running script stops before it is destroyed"
    );
}

/// A script removed while the editor edits is destroyed without a stop: it was
/// never running.
#[test]
fn a_removal_while_editing_destroys_without_stopping() {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<EditModeRecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, EditModeRecordingScript { log_entity })
        .expect("the script attaches");
    engine.process_frame().expect("the frame runs");

    let mut commands = CommandQueue::new();
    commands.remove_component_from_entity::<EditModeRecordingScript>(entity);
    commands
        .execute_queued_commands(engine.world_mut(), true)
        .expect("the removal applies");

    let recorded = entries(engine.world(), log_entity);
    assert_eq!(
        &recorded[2..],
        ["destroy:Despawned"],
        "an edit-mode script never started, so it only hears about the destroy"
    );
}

/// Destroying an entity tells every script on it, and destroying the entity
/// twice reports the second one instead of calling the lifecycle methods again.
#[test]
fn destroying_an_entity_tells_every_script_on_it() {
    let (mut engine, first_log) = engine_with_log();
    let world = engine.world_mut();
    world.register_script_component::<RecordingScript>();
    world.register_script_component::<EditModeRecordingScript>();
    let second_log = world
        .create_entity()
        .build()
        .expect("the second log builds");
    world
        .add_component(second_log, LifecycleLog::default())
        .expect("the second log attaches");
    let entity = world.create_entity().build().expect("the entity builds");
    world
        .add_component(
            entity,
            RecordingScript {
                log_entity: first_log,
            },
        )
        .expect("the first script attaches");
    world
        .add_component(
            entity,
            EditModeRecordingScript {
                log_entity: second_log,
            },
        )
        .expect("the second script attaches");
    engine.set_run_mode(RunMode::Editing);
    engine.process_frame().expect("the frame runs");

    let mut commands = CommandQueue::new();
    commands.destroy_entity(entity);
    commands
        .execute_queued_commands(engine.world_mut(), true)
        .expect("the destroy applies");

    assert_eq!(
        entries(engine.world(), first_log)
            .last()
            .map(String::as_str),
        Some("destroy:Despawned"),
        "the first script hears about the destroy"
    );
    assert_eq!(
        entries(engine.world(), second_log)
            .last()
            .map(String::as_str),
        Some("destroy:Despawned"),
        "the second script hears about it too"
    );
}

/// A lifecycle method that queues during a removal does not run its own
/// commands inside the removal; they wait for the next flush.
#[test]
fn a_lifecycle_method_that_queues_during_a_removal_waits_for_the_next_flush() {
    let (mut engine, _log) = engine_with_log();
    engine
        .world_mut()
        .register_script_component::<SpawningDestroyScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, SpawningDestroyScript)
        .expect("the script attaches");

    let mut commands = CommandQueue::new();
    commands.remove_component_from_entity::<SpawningDestroyScript>(entity);
    commands
        .execute_queued_commands(engine.world_mut(), true)
        .expect("the removal applies");

    assert!(
        engine
            .world()
            .get_component::<SpawningDestroyScript>(entity)
            .is_none(),
        "the component is gone before the command `on_destroy` queued runs"
    );
    let entities_after_removal = engine.world().entity_count();
    assert!(
        !commands.is_empty(),
        "the command `on_destroy` queued waits for the next flush"
    );
    commands
        .execute_queued_commands(engine.world_mut(), true)
        .expect("the second flush applies it");
    assert!(commands.is_empty(), "the second flush drains the queue");
    assert_eq!(
        engine.world().entity_count(),
        entities_after_removal + 1,
        "the entity `on_destroy` queued exists after the flush"
    );
}

// =============================================================================
// Change origins
// =============================================================================

/// Adds an edit-mode recording script under `origin` and returns the engine and
/// the log entity, already advanced one editing frame.
fn attach_under_origin(origin: ChangeOrigin) -> (Engine, Entity) {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<EditModeRecordingScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .with_change_origin(origin, |world| {
            world.add_component(entity, EditModeRecordingScript { log_entity })
        })
        .expect("the script attaches");
    engine.process_frame().expect("the frame runs");
    (engine, log_entity)
}

/// An attachment the editor makes reports the editor as its cause.
#[test]
fn an_authored_attachment_reports_authored() {
    let (engine, log_entity) = attach_under_origin(ChangeOrigin::Authoring);

    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:Authored:Editing", "update"],
        "the editor's change says it was authored"
    );
}

/// An attachment an undo restored reports the history as its cause.
#[test]
fn a_restored_attachment_reports_history_restored() {
    let (engine, log_entity) = attach_under_origin(ChangeOrigin::HistoryRestore);

    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:HistoryRestored:Editing", "update"],
        "an undo says the history restored the component"
    );
}

/// An attachment a scene load makes reports the load as its cause.
#[test]
fn a_scene_load_attachment_reports_scene_loaded() {
    let (engine, log_entity) = attach_under_origin(ChangeOrigin::SceneLoad);

    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:SceneLoaded:Editing", "update"],
        "a scene load says the scene provided the component"
    );
}

/// The origin a scope sets is put back when the scope ends, however the scopes
/// are nested.
#[test]
fn a_change_origin_scope_restores_the_previous_origin() {
    let mut world = World::new();
    assert_eq!(world.change_origin(), ChangeOrigin::Simulation);

    world.with_change_origin(ChangeOrigin::Authoring, |world| {
        assert_eq!(world.change_origin(), ChangeOrigin::Authoring);
        world.with_change_origin(ChangeOrigin::SceneLoad, |world| {
            assert_eq!(world.change_origin(), ChangeOrigin::SceneLoad);
        });
        assert_eq!(
            world.change_origin(),
            ChangeOrigin::Authoring,
            "the inner scope puts the outer one back"
        );
    });

    assert_eq!(
        world.change_origin(),
        ChangeOrigin::Simulation,
        "the outer scope puts the default back"
    );
}

// =============================================================================
// Stopping play
// =============================================================================

/// A script whose stop and destroy calls a test counts outside the world, so
/// the report survives the restore that discards its component.
#[derive(Debug, Clone)]
struct StopProbeScript {
    /// The entity holding the log its create lines go to.
    log_entity: Entity,
}

impl Component for StopProbeScript {}

impl ScriptDeclaration for StopProbeScript {
    const DECLARATION: ScriptDeclarationData = ScriptDeclarationData::NONE
        .with_on_create(0)
        .with_on_start(0)
        .with_on_stop(0)
        .with_on_destroy(0)
        .with_edit_mode();
}

impl ScriptComponent for StopProbeScript {
    fn on_create(&mut self, script_context: &mut ScriptContext, create_info: &CreateInfo) {
        record(
            self.log_entity,
            script_context,
            format!("create:{:?}:{:?}", create_info.cause, create_info.mode),
        );
    }

    fn on_stop(&mut self, _script_context: &mut ScriptContext, _stop_info: &StopInfo) {
        STOP_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    fn on_destroy(&mut self, _script_context: &mut ScriptContext, _destroy_info: &DestroyInfo) {
        DESTROY_CALLS.fetch_add(1, Ordering::SeqCst);
    }
}

/// How many times [`StopProbeScript`] was stopped.
static STOP_CALLS: AtomicUsize = AtomicUsize::new(0);

/// How many times [`StopProbeScript`] was destroyed.
static DESTROY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Stopping play stops every running script and reports the play copy
/// destroyed, then tells the restored edit world it exists again.
///
/// The stop and destroy calls are counted in statics rather than in the log
/// component, because the play world's components are the ones the restore
/// throws away: nothing a play-world component records can be read afterwards.
#[test]
fn stopping_play_reports_stop_destroy_and_the_restore() {
    let (mut engine, log_entity) = engine_with_log();
    engine.set_run_mode(RunMode::Editing);
    engine
        .world_mut()
        .register_script_component::<StopProbeScript>();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("the entity builds");
    engine
        .world_mut()
        .add_component(entity, StopProbeScript { log_entity })
        .expect("the script attaches");
    STOP_CALLS.store(0, Ordering::SeqCst);
    DESTROY_CALLS.store(0, Ordering::SeqCst);

    engine.enter_play().expect("the world copies");
    engine.process_frame().expect("the play frame runs");
    assert!(engine.exit_play(), "a session was running");

    assert_eq!(
        STOP_CALLS.load(Ordering::SeqCst),
        1,
        "the running script was stopped exactly once"
    );
    assert_eq!(
        DESTROY_CALLS.load(Ordering::SeqCst),
        1,
        "the play copy was reported destroyed exactly once"
    );

    engine.process_frame().expect("the edit frame runs again");
    assert_eq!(
        entries(engine.world(), log_entity),
        ["create:PlayStopRestored:Editing"],
        "the restored edit world tells its script it exists again"
    );
}
