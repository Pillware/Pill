//! Proves the editor API's field-edit callbacks end to end, on a real engine.
//!
//! # Responsibilities
//!
//! - Prove a dead liveness marker stops a callback from being called.
//! - Prove a before-callback's replacement is written and undone, and its
//!   refusal drops the whole batch, posts a notice and records nothing.
//! - Prove a follow-up joins the same history step and never re-enters the
//!   callback, and that a replay drops what a callback queues.
//! - Prove the phase and the primary flag reach the callback.
//!
//! # Design
//!
//! Each test builds a small engine with one probe component, binds its
//! callbacks into the registry with a fresh marker, and drives the shared
//! drain - the same path the executable takes. The callbacks are plain `fn`s
//! that record through their own static, so a test's state cannot leak into
//! another test running beside it.

// Standard library
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// External crates
use pill_engine::{Engine, Entity, FieldValue, PillComponent};

// Current crate
use crate::commands::{ensure_command_resources, PendingEditorCommands};
use crate::drain::{self, HistoryDirection};
use crate::editor_state::{EditorCommand, EditorSelection};
use crate::history::CommandHistory;
use crate::hooks::{
    EditorHookRegistry, FieldEdit, FieldEditHookRequest, HookContext, HookNotices, HookRequest,
    HookVerdict,
};
use crate::panels::ModuleLiveness;

// =============================================================================
// Fixtures
// =============================================================================

/// The component every test edits.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, PillComponent)]
struct HookProbe {
    /// The field most tests edit.
    value: f32,
    /// The field a follow-up writes.
    other: f32,
}

/// The name the world registered the probe component under.
fn probe_component() -> String {
    pill_engine::component::ComponentRegistry::registered_name::<HookProbe>()
}

/// An engine with the probe component registered and the command resources
/// present.
fn probe_engine() -> Engine {
    let mut engine = Engine::new();
    engine
        .world_mut()
        .register_component_with_layout::<HookProbe>(HookProbe::FIELD_LAYOUT);
    ensure_command_resources(engine.world_mut());
    engine
}

/// Spawns one probe entity and returns its handle.
fn spawn_probe(engine: &mut Engine) -> Entity {
    engine
        .world_mut()
        .create_entity()
        .with(HookProbe::default())
        .build()
        .expect("the probe entity builds")
}

/// Binds one request to a fresh marker the caller keeps alive.
fn bind(engine: &mut Engine, marker: &Arc<ModuleLiveness>, request: HookRequest) {
    let registry = engine
        .world_mut()
        .get_resource_mut::<EditorHookRegistry>()
        .expect("the registry exists");
    registry.register(
        request.source(),
        request.event(),
        request.callback(),
        marker,
    );
}

/// Queues one recorded field write and applies it through the drain.
fn write_field(engine: &mut Engine, entity: Entity, field: &str, value: f32) {
    write_fields(engine, &[(entity, field)], value);
}

/// Queues one recorded write of `value` to `field` on every entity, as one
/// batch, and applies it through the drain.
fn write_fields(engine: &mut Engine, targets: &[(Entity, &str)], value: f32) {
    let commands = targets
        .iter()
        .map(|(entity, field)| EditorCommand::SetField {
            entity: *entity,
            component: probe_component(),
            field: (*field).to_string(),
            value: FieldValue::F32(value),
        })
        .collect();
    let pending = engine
        .world_mut()
        .get_resource_mut::<PendingEditorCommands>()
        .expect("the command queue exists");
    pending.push_recorded(commands, "test write", None);
    drain::apply_pending(engine, 0);
}

/// Reads one probe field from the world.
fn read_field(engine: &Engine, entity: Entity, field: &str) -> f32 {
    match engine
        .world()
        .read_component_field(entity, &probe_component(), field)
        .expect("the field is readable")
    {
        FieldValue::F32(value) => value,
        other => panic!("the probe field is an f32, not {other:?}"),
    }
}

/// The history's undo depth.
fn undo_depth(engine: &Engine) -> usize {
    engine
        .world()
        .get_resource::<CommandHistory>()
        .map(CommandHistory::undo_depth)
        .unwrap_or(0)
}

/// Steps the history one line back and applies what it queued.
fn undo(engine: &mut Engine) {
    drain::step_history(engine, HistoryDirection::Undo);
    drain::apply_pending(engine, 0);
}

// =============================================================================
// A dead marker stops the callback
// =============================================================================

/// Calls counted by the dropped-marker test.
static DROPPED_MARKER_CALLS: AtomicUsize = AtomicUsize::new(0);

fn count_field_edits(
    _context: &mut HookContext<'_>,
    _edit: &FieldEdit<'_, HookProbe>,
) -> HookVerdict {
    DROPPED_MARKER_CALLS.fetch_add(1, Ordering::SeqCst);
    HookVerdict::Keep
}

#[test]
fn a_dropped_marker_stops_the_callback() {
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::new::<HookProbe>("test.hooks", count_field_edits),
    );

    write_field(&mut engine, entity, "value", 2.0);
    assert_eq!(DROPPED_MARKER_CALLS.load(Ordering::SeqCst), 1);

    drop(marker);
    write_field(&mut engine, entity, "value", 3.0);
    assert_eq!(
        DROPPED_MARKER_CALLS.load(Ordering::SeqCst),
        1,
        "the callback is not called once its marker is gone"
    );
    assert_eq!(read_field(&engine, entity, "value"), 3.0);
}

// =============================================================================
// Replacement and refusal
// =============================================================================

fn clamp_to_nine(_context: &mut HookContext<'_>, _edit: &FieldEdit<'_, HookProbe>) -> HookVerdict {
    HookVerdict::Replace(FieldValue::F32(9.0))
}

#[test]
fn a_replacement_is_written_and_undone() {
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::new::<HookProbe>("test.hooks", clamp_to_nine),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert_eq!(read_field(&engine, entity, "value"), 9.0);
    assert_eq!(undo_depth(&engine), 1);

    undo(&mut engine);
    assert_eq!(
        read_field(&engine, entity, "value"),
        0.0,
        "the step's inverse restores the value before the edit"
    );
}

/// Counts the refusals a test posted.
static REFUSAL_CALLS: AtomicUsize = AtomicUsize::new(0);

/// The entity whose command the refusal test refuses.
static REFUSED_ENTITY: Mutex<Option<Entity>> = Mutex::new(None);

fn refuse_the_second_entity(
    _context: &mut HookContext<'_>,
    edit: &FieldEdit<'_, HookProbe>,
) -> HookVerdict {
    REFUSAL_CALLS.fetch_add(1, Ordering::SeqCst);
    let refused = *REFUSED_ENTITY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if refused == Some(edit.entity) {
        HookVerdict::Refuse("the probe is frozen".to_string())
    } else {
        HookVerdict::Keep
    }
}

#[test]
fn a_refusal_drops_the_whole_batch() {
    let mut engine = probe_engine();
    let first = spawn_probe(&mut engine);
    let second = spawn_probe(&mut engine);
    *REFUSED_ENTITY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(second);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::new::<HookProbe>("test.hooks", refuse_the_second_entity),
    );

    // The first command passes the before pass; the second entity's command is
    // refused. Neither may apply, because the before pass runs over the whole
    // batch first.
    write_fields(&mut engine, &[(first, "value"), (second, "value")], 4.0);
    assert_eq!(read_field(&engine, first, "value"), 0.0);
    assert_eq!(read_field(&engine, second, "value"), 0.0);
    assert_eq!(undo_depth(&engine), 0, "a refused batch records nothing");

    let notices = engine
        .world_mut()
        .get_resource_mut::<HookNotices>()
        .expect("the notice queue exists")
        .drain();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].text.contains("the probe is frozen"));
}

// =============================================================================
// Follow-ups
// =============================================================================

/// Calls counted by the follow-up test.
static FOLLOW_UP_CALLS: AtomicUsize = AtomicUsize::new(0);

fn queue_other_after(context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    FOLLOW_UP_CALLS.fetch_add(1, Ordering::SeqCst);
    if edit.field == HookProbeField::Value {
        context.queue(EditorCommand::SetField {
            entity: edit.entity,
            component: probe_component(),
            field: "other".to_string(),
            value: FieldValue::F32(7.0),
        });
    }
}

#[test]
fn a_follow_up_joins_the_same_history_step() {
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", queue_other_after),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert_eq!(read_field(&engine, entity, "value"), 5.0);
    assert_eq!(read_field(&engine, entity, "other"), 7.0);
    assert_eq!(
        undo_depth(&engine),
        1,
        "one command and its follow-up are one step"
    );

    undo(&mut engine);
    assert_eq!(read_field(&engine, entity, "value"), 0.0);
    assert_eq!(
        read_field(&engine, entity, "other"),
        0.0,
        "one undo reverts the follow-up with the command that caused it"
    );
}

/// Calls counted by the re-entry test.
static REENTRY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Queues the same follow-up as [`queue_other_after`], counted on its own
/// static so the two tests can run beside each other.
fn queue_other_after_counted(context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    REENTRY_CALLS.fetch_add(1, Ordering::SeqCst);
    if edit.field == HookProbeField::Value {
        context.queue(EditorCommand::SetField {
            entity: edit.entity,
            component: probe_component(),
            field: "other".to_string(),
            value: FieldValue::F32(7.0),
        });
    }
}

#[test]
fn a_follow_up_does_not_reenter_the_callback() {
    REENTRY_CALLS.store(0, Ordering::SeqCst);
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", queue_other_after_counted),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert_eq!(
        REENTRY_CALLS.load(Ordering::SeqCst),
        1,
        "the hook sees the edit, not its own follow-up"
    );
}

// =============================================================================
// Replays
// =============================================================================

/// The causes the replay test observed.
static REPLAY_CAUSES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

fn record_cause_and_queue(context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    REPLAY_CAUSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(edit.cause.name());
    context.queue(EditorCommand::SetField {
        entity: edit.entity,
        component: probe_component(),
        field: "other".to_string(),
        value: FieldValue::F32(7.0),
    });
}

#[test]
fn a_replay_calls_the_hook_and_drops_what_it_queues() {
    REPLAY_CAUSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", record_cause_and_queue),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert_eq!(read_field(&engine, entity, "other"), 7.0);

    undo(&mut engine);
    assert_eq!(read_field(&engine, entity, "value"), 0.0);
    assert_eq!(
        read_field(&engine, entity, "other"),
        0.0,
        "a follow-up queued during a replay is dropped"
    );
    let causes = REPLAY_CAUSES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert!(
        causes.contains(&"history_restored"),
        "the hook still hears the replay: {causes:?}"
    );
}

// =============================================================================
// Phase and primary
// =============================================================================

/// The payload data the phase test recorded.
static PHASE_RECORDS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

fn record_phase(_context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    let mut records = PHASE_RECORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    records.push((edit.phase.name().to_string(), edit.is_primary));
}

#[test]
fn the_phase_and_primary_flag_reach_the_callback() {
    PHASE_RECORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    let first = spawn_probe(&mut engine);
    let second = spawn_probe(&mut engine);
    let mut selection = EditorSelection::default();
    selection.set_entities(vec![first, second]);
    engine.world_mut().insert_resource(selection);

    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", record_phase),
    );

    let commands = [first, second]
        .iter()
        .map(|entity| EditorCommand::SetField {
            entity: *entity,
            component: probe_component(),
            field: "value".to_string(),
            value: FieldValue::F32(1.0),
        })
        .collect();
    let pending = engine
        .world_mut()
        .get_resource_mut::<PendingEditorCommands>()
        .expect("the command queue exists");
    pending.push_recorded_with_phase(
        commands,
        "test drag",
        None,
        crate::hooks::FieldEditPhase::Interactive,
    );
    drain::apply_pending(&mut engine, 0);

    let records = PHASE_RECORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|(phase, _)| phase == "interactive"));
    let primary_records: Vec<bool> = records.iter().map(|(_, primary)| *primary).collect();
    assert_eq!(
        primary_records,
        vec![false, true],
        "the last selected entity is the primary"
    );
    drop(records);

    // A one-shot edit reports one committed event instead.
    write_field(&mut engine, first, "value", 2.0);
    let records = PHASE_RECORDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(records.len(), 3);
    assert_eq!(records[2].0, "committed");
}

// =============================================================================
// The request queue
// =============================================================================

/// The static the request-queue test binds by hand.
static REQUESTS_QUEUE_CALLS: AtomicUsize = AtomicUsize::new(0);

fn count_queued(_context: &mut HookContext<'_>, _edit: &FieldEdit<'_, HookProbe>) -> HookVerdict {
    REQUESTS_QUEUE_CALLS.fetch_add(1, Ordering::SeqCst);
    HookVerdict::Keep
}

#[test]
fn a_request_queued_by_a_module_binds_and_fires() {
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let requests = engine
        .world_mut()
        .get_resource_mut::<crate::hooks::EditorHookRequests>()
        .expect("the request queue exists");
    requests.push(FieldEditHookRequest::new::<HookProbe>(
        "test.hooks",
        count_queued,
    ));
    let marker = ModuleLiveness::new();
    let drained = engine
        .world_mut()
        .get_resource_mut::<crate::hooks::EditorHookRequests>()
        .expect("the request queue exists")
        .drain();
    for request in drained {
        bind(&mut engine, &marker, request);
    }

    write_field(&mut engine, entity, "value", 6.0);
    assert_eq!(REQUESTS_QUEUE_CALLS.load(Ordering::SeqCst), 1);
}
