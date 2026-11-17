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
    AssetEdit, ComponentEdit, EditorHookRegistry, EntityEdit, FieldEdit, FieldEditHookRequest,
    HistoryEdit, HookContext, HookNotices, HookRequest, HookVerdict, PlayEdit, SelectionChanged,
    ToolActivatedEdit,
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

// =============================================================================
// Component, entity and history events
// =============================================================================

/// A second component, so a test can add one the probe entity does not carry.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PillComponent)]
struct HookExtra {
    /// The only field, unused.
    amount: f32,
}

/// The name the world registered [`HookExtra`] under.
fn extra_component() -> String {
    pill_engine::component::ComponentRegistry::registered_name::<HookExtra>()
}

/// The component events a test observed.
static COMPONENT_EVENTS: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

fn record_component_edit(_context: &mut HookContext<'_>, edit: &ComponentEdit<'_>) {
    COMPONENT_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.component.to_string(), edit.added));
}

/// The entity events a test observed.
static ENTITY_EVENTS: Mutex<Vec<(Entity, bool)>> = Mutex::new(Vec::new());

fn record_entity_edit(_context: &mut HookContext<'_>, edit: &EntityEdit) {
    ENTITY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.entity, edit.created));
}

/// Queues one unrecorded batch and applies it through the drain.
fn queue_plain(engine: &mut Engine, commands: Vec<EditorCommand>) {
    let pending = engine
        .world_mut()
        .get_resource_mut::<PendingEditorCommands>()
        .expect("the command queue exists");
    pending.push(commands);
    drain::apply_pending(engine, 0);
}

#[test]
fn the_component_and_entity_events_name_what_changed() {
    COMPONENT_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    ENTITY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    engine
        .world_mut()
        .register_component_with_layout::<HookExtra>(HookExtra::FIELD_LAYOUT);
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::component_edit("test.hooks", record_component_edit),
    );
    bind(
        &mut engine,
        &marker,
        HookRequest::entity_edit("test.hooks", record_entity_edit),
    );

    queue_plain(
        &mut engine,
        vec![EditorCommand::AddComponent {
            entity,
            component: extra_component(),
        }],
    );
    queue_plain(
        &mut engine,
        vec![EditorCommand::RemoveComponent {
            entity,
            component: extra_component(),
        }],
    );
    let component_events = COMPONENT_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        component_events.as_slice(),
        [(extra_component(), true), (extra_component(), false)],
        "one event per add and one per remove, naming the component"
    );
    drop(component_events);

    queue_plain(
        &mut engine,
        vec![EditorCommand::CreateEntity {
            components: Vec::new(),
        }],
    );
    queue_plain(&mut engine, vec![EditorCommand::DestroyEntity { entity }]);
    let entity_events = ENTITY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(entity_events.len(), 2);
    assert!(
        entity_events[0].1,
        "the create reports an entity that now exists"
    );
    assert_eq!(
        entity_events[1],
        (entity, false),
        "the destroy names the entity it removed"
    );
}

/// The history events a test observed.
static HISTORY_EVENTS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn record_history_edit(_context: &mut HookContext<'_>, edit: &HistoryEdit<'_>) {
    HISTORY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.direction.to_string(), edit.label.to_string()));
}

#[test]
fn the_history_event_names_the_direction_and_the_step() {
    HISTORY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::history_edit("test.hooks", record_history_edit),
    );

    write_field(&mut engine, entity, "value", 5.0);
    undo(&mut engine);
    let events = HISTORY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        events.as_slice(),
        [("undo".to_string(), "test write".to_string())],
        "one event for the walk, naming the step it replayed"
    );
}

// =============================================================================
// Selection and tool events
// =============================================================================

/// The selection events a test observed, as `(previous, current)` counts.
static SELECTION_EVENTS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

fn record_selection_changed(_context: &mut HookContext<'_>, edit: &SelectionChanged) {
    SELECTION_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.previous.entities.len(), edit.current.entities.len()));
}

#[test]
fn the_selection_event_fires_once_per_change() {
    SELECTION_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::selection_changed("test.hooks", record_selection_changed),
    );

    // The first drain records the state it found; a session that starts with
    // nothing selected must not report a change nobody made.
    drain::apply_pending(&mut engine, 0);
    assert!(SELECTION_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_empty());

    let mut selection = EditorSelection::default();
    selection.set_entities(vec![entity]);
    engine.world_mut().insert_resource(selection);
    drain::apply_pending(&mut engine, 0);
    drain::apply_pending(&mut engine, 0);

    let events = SELECTION_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        events.as_slice(),
        [(0, 1)],
        "one event for the change and none for the drain after it"
    );
}

/// The tool events a test observed.
static TOOL_EVENTS: Mutex<Vec<(Option<String>, Option<String>)>> = Mutex::new(Vec::new());

fn record_tool_activated(_context: &mut HookContext<'_>, edit: &ToolActivatedEdit) {
    TOOL_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.previous.clone(), edit.current.clone()));
}

#[test]
fn the_tool_event_fires_when_a_command_switches_the_tool() {
    TOOL_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::tool_activated("test.hooks", record_tool_activated),
    );

    drain::apply_pending(&mut engine, 0);
    queue_plain(
        &mut engine,
        vec![EditorCommand::SetGizmoTool {
            mode: Some(crate::gizmo::GizmoMode::Rotate),
            space: None,
            snap: None,
        }],
    );

    let events = TOOL_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        events.as_slice(),
        [(None, Some("rotate".to_string()))],
        "the tool command is what moves the tool, and it fires one event"
    );
}

// =============================================================================
// Play and asset events
// =============================================================================

/// The play events a test observed.
static PLAY_EVENTS: Mutex<Vec<bool>> = Mutex::new(Vec::new());

fn record_play_edit(_context: &mut HookContext<'_>, edit: &PlayEdit) {
    PLAY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(edit.playing);
}

#[test]
fn play_and_stop_fire_their_events() {
    PLAY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    // An engine plays unless the editor puts it into editing, so the test
    // starts where the editor's engine starts.
    engine.set_run_mode(pill_engine::RunMode::Editing);
    spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::play_edit("test.hooks", record_play_edit),
    );

    queue_plain(&mut engine, vec![EditorCommand::EnterPlay]);
    assert_eq!(
        engine.run_mode(),
        pill_engine::RunMode::Playing,
        "the start command puts the engine into play"
    );
    queue_plain(&mut engine, vec![EditorCommand::ExitPlay]);
    assert_eq!(
        engine.run_mode(),
        pill_engine::RunMode::Editing,
        "the stop command puts the engine back into editing"
    );

    let events = PLAY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        events.as_slice(),
        [true, false],
        "the start reports the run mode the session reached, the stop reports editing"
    );
}

/// The play events a refused start observed.
static REFUSED_PLAY_EVENTS: Mutex<Vec<bool>> = Mutex::new(Vec::new());

fn record_refused_play(_context: &mut HookContext<'_>, edit: &PlayEdit) {
    REFUSED_PLAY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(edit.playing);
}

#[test]
fn a_refused_play_fires_no_started_event() {
    /// A component that owns heap data and is not `Clone`, so the world
    /// cannot be copied into a session.
    #[derive(Debug)]
    struct UncopyableProbe {
        /// The heap data that makes the type uncopyable.
        #[allow(dead_code)]
        buffer: Vec<u32>,
    }
    impl pill_engine::Component for UncopyableProbe {}

    REFUSED_PLAY_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    engine.set_run_mode(pill_engine::RunMode::Editing);
    engine.world_mut().register_component::<UncopyableProbe>();
    engine
        .world_mut()
        .create_entity()
        .with(UncopyableProbe { buffer: vec![1] })
        .build()
        .expect("the uncopyable entity builds");
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::play_edit("test.hooks", record_refused_play),
    );

    queue_plain(&mut engine, vec![EditorCommand::EnterPlay]);

    assert_eq!(
        engine.run_mode(),
        pill_engine::RunMode::Editing,
        "a refused play leaves the editor editing"
    );
    assert!(
        REFUSED_PLAY_EVENTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_empty(),
        "a session that never started must not report a start"
    );
}

// =============================================================================
// The editor state a callback reads
// =============================================================================

/// One callback's reading of the editor state.
type StateRead = (
    String,
    String,
    bool,
    Option<String>,
    Option<usize>,
    Option<(f32, f32)>,
    Option<Entity>,
);

/// The state reads a test observed.
static STATE_READS: Mutex<Vec<StateRead>> = Mutex::new(Vec::new());

fn record_editor_state(context: &mut HookContext<'_>, _edit: &ComponentEdit<'_>) {
    let gizmo = context
        .gizmo()
        .map(|state| state.settings)
        .unwrap_or_default();
    let viewport = context.viewport();
    STATE_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((
            format!("{:?}", gizmo.mode),
            format!("{:?}", gizmo.space),
            gizmo.snap,
            context.active_tool(),
            viewport.active_view,
            viewport.pointer_in_view,
            context.selection().primary(),
        ));
}

#[test]
fn a_hook_reads_the_editor_state() {
    STATE_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let mut engine = probe_engine();
    engine
        .world_mut()
        .register_component_with_layout::<HookExtra>(HookExtra::FIELD_LAYOUT);
    let entity = spawn_probe(&mut engine);
    {
        // The tool and the gizmo as the toolbox leaves them after two clicks.
        let world = engine.world_mut();
        crate::tools::activate_tool(world, Some("rotate")).expect("rotate is a built-in");
        if let Some(state) = world.get_resource_mut::<crate::gizmo::GizmoState>() {
            state.settings.mode = crate::gizmo::GizmoMode::Rotate;
            state.settings.space = crate::gizmo::GizmoSpace::Local;
            state.settings.snap = true;
        }
        let mut selection = EditorSelection::default();
        selection.set_entities(vec![entity]);
        world.insert_resource(selection);
    }
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::component_edit("test.hooks", record_editor_state),
    );

    // The first call runs before any executable published a viewport.
    queue_plain(
        &mut engine,
        vec![EditorCommand::AddComponent {
            entity,
            component: extra_component(),
        }],
    );
    engine
        .world_mut()
        .insert_resource(crate::viewport::EditorViewportState {
            active_view: Some(1),
            pointer_in_view: Some((10.0, 20.0)),
            view_rect: None,
            view_count: 2,
        });
    queue_plain(
        &mut engine,
        vec![EditorCommand::RemoveComponent {
            entity,
            component: extra_component(),
        }],
    );

    let reads = STATE_READS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(reads.len(), 2);
    assert_eq!(
        reads[0],
        (
            "Rotate".to_string(),
            "Local".to_string(),
            true,
            Some("rotate".to_string()),
            None,
            None,
            Some(entity),
        ),
        "the gizmo, the tool, the selection and the empty viewport posture reach the callback"
    );
    assert_eq!(
        reads[1].4,
        Some(1),
        "the published viewport names the active view"
    );
    assert_eq!(reads[1].5, Some((10.0, 20.0)));
}

/// The asset events a test observed.
static ASSET_EVENTS: Mutex<Vec<(crate::hooks::AssetEditKind, String)>> = Mutex::new(Vec::new());

fn record_asset_edit(_context: &mut HookContext<'_>, edit: &AssetEdit<'_>) {
    ASSET_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push((edit.kind, edit.path.to_string()));
}

#[test]
fn an_asset_edit_fires_with_its_kind_and_path() {
    ASSET_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    let _mount = crate::test_support::asset_mount_lock();
    let asset_directory =
        std::env::temp_dir().join(format!("pill-editor-hooks-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&asset_directory);
    std::fs::create_dir_all(&asset_directory).unwrap();
    std::fs::write(asset_directory.join("plain.material"), b"body").unwrap();
    pill_engine::asset_store::mount_directory(&asset_directory);

    let mut engine = probe_engine();
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::asset_edit("test.hooks", record_asset_edit),
    );

    queue_plain(
        &mut engine,
        vec![EditorCommand::DeleteAssetFile {
            path: "plain.material".to_string(),
        }],
    );

    let events = ASSET_EVENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(
        events.as_slice(),
        [(
            crate::hooks::AssetEditKind::Deleted,
            "plain.material".to_string()
        )],
        "the delete reports its kind and the file it removed"
    );
}

// =============================================================================
// Commands from callbacks
// =============================================================================

/// Adds [`HookExtra`] whenever the probe's `value` is written.
fn add_extra_after(context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    if edit.field == HookProbeField::Value {
        context.queue_add_component(edit.entity, &extra_component());
    }
}

#[test]
fn an_after_hook_that_adds_a_component_joins_the_step() {
    let mut engine = probe_engine();
    engine
        .world_mut()
        .register_component_with_layout::<HookExtra>(HookExtra::FIELD_LAYOUT);
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", add_extra_after),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert!(
        engine.world().get_component::<HookExtra>(entity).is_some(),
        "the builder's component is on the entity"
    );
    assert_eq!(
        undo_depth(&engine),
        1,
        "the component joins the step of the edit that caused it"
    );

    undo(&mut engine);
    assert_eq!(read_field(&engine, entity, "value"), 0.0);
    assert!(
        engine.world().get_component::<HookExtra>(entity).is_none(),
        "one undo removes the component with the edit that added it"
    );
}

/// Rotates the gizmo whenever the probe's `value` is written.
fn rotate_after(context: &mut HookContext<'_>, edit: &FieldEdit<'_, HookProbe>) {
    if edit.field == HookProbeField::Value {
        context.queue_gizmo(Some(crate::gizmo::GizmoMode::Rotate), None, Some(true));
    }
}

/// The gizmo settings the world holds.
fn gizmo_settings(engine: &Engine) -> crate::gizmo::GizmoSettings {
    engine
        .world()
        .get_resource::<crate::gizmo::GizmoState>()
        .copied()
        .expect("the tool change registered the gizmo state")
        .settings
}

#[test]
fn a_queued_gizmo_change_records_no_step() {
    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", rotate_after),
    );

    write_field(&mut engine, entity, "value", 5.0);
    let settings = gizmo_settings(&engine);
    assert_eq!(settings.mode, crate::gizmo::GizmoMode::Rotate);
    assert!(settings.snap, "both settings the builder named applied");
    assert_eq!(
        undo_depth(&engine),
        1,
        "the tool change adds no step of its own"
    );

    undo(&mut engine);
    assert_eq!(read_field(&engine, entity, "value"), 0.0);
    assert_eq!(
        gizmo_settings(&engine).mode,
        crate::gizmo::GizmoMode::Rotate,
        "undo does not walk a tool choice back"
    );
}

/// The entity a selection hook selects.
static SELECTION_TARGET: Mutex<Option<Entity>> = Mutex::new(None);

/// Selects [`SELECTION_TARGET`] whenever the selection moves.
fn select_the_target(context: &mut HookContext<'_>, _edit: &SelectionChanged) {
    let Some(target) = *SELECTION_TARGET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
    else {
        return;
    };
    if context.selection().entities().len() == 1 && context.selection().primary() == Some(target) {
        return;
    }
    context.queue_selection(vec![target]);
}

/// Replaces the shared selection's entities, keeping everything else.
fn set_selection(engine: &mut Engine, entities: Vec<Entity>) {
    if engine.world().get_resource::<EditorSelection>().is_none() {
        engine
            .world_mut()
            .insert_resource(EditorSelection::default());
    }
    if let Some(selection) = engine.world_mut().get_resource_mut::<EditorSelection>() {
        selection.set_entities(entities);
    }
}

/// The shared selection's primary entity.
fn selected_entity(engine: &Engine) -> Option<Entity> {
    engine
        .world()
        .get_resource::<EditorSelection>()
        .and_then(EditorSelection::primary)
}

#[test]
fn a_selection_hook_that_selects_another_entity_is_one_step() {
    *SELECTION_TARGET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let mut engine = probe_engine();
    let first = spawn_probe(&mut engine);
    let second = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        HookRequest::selection_changed("test.hooks", select_the_target),
    );

    // The first drain records the empty selection silently, so the click below
    // is the change the hook answers.
    drain::apply_pending(&mut engine, 0);
    *SELECTION_TARGET
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(second);

    set_selection(&mut engine, vec![first]);
    drain::apply_pending(&mut engine, 0);
    assert_eq!(
        selected_entity(&engine),
        Some(second),
        "the hook's selection change applied"
    );
    assert_eq!(undo_depth(&engine), 1, "the hook's selection is one step");

    undo(&mut engine);
    assert_eq!(
        selected_entity(&engine),
        Some(first),
        "undo puts the previous selection back, and the replay drops the hook's own"
    );
}

#[test]
fn every_builder_queues_the_editors_own_command() {
    use crate::hooks::context::HookQueue;
    use crate::hooks::FieldEditCause;

    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let selection = EditorSelection::default();
    let mut queue = HookQueue::default();
    let mut notices = Vec::new();
    {
        let mut context = HookContext::new(
            engine.world_mut(),
            &selection,
            &mut queue,
            &mut notices,
            FieldEditCause::Authored,
            "test.hooks",
        );
        context.queue_add_component(entity, &extra_component());
        context.queue_remove_component(entity, &probe_component());
        context.queue_create_entity();
        context.queue_destroy_entity(entity);
        context.queue_selection(vec![entity]);
        context.queue_gizmo(Some(crate::gizmo::GizmoMode::Rotate), None, Some(true));
    }

    let (edits, source) = queue.take_edits();
    assert_eq!(
        source.as_deref(),
        Some("test.hooks"),
        "the queue names the callback that queued the edits"
    );
    let (add, _label) = crate::inspector::component_add(entity, &extra_component());
    let (remove, _label) = crate::inspector::component_remove(entity, &probe_component());
    let (create, _label) = crate::entities::entity_create();
    let (destroy, _label) = crate::entities::entity_delete(entity, None);
    let mut expected = vec![add, remove, create];
    expected.extend(destroy);
    expected.push(crate::editor_state::EditorCommand::SetSelection {
        entities: vec![entity],
        asset: None,
    });
    assert_eq!(
        edits, expected,
        "each builder queues the command the editor's own row queues"
    );
    assert_eq!(
        queue.take_controls(),
        vec![crate::editor_state::EditorCommand::SetGizmoTool {
            mode: Some(crate::gizmo::GizmoMode::Rotate),
            space: None,
            snap: Some(true),
        }],
        "a gizmo change is a control, not an edit"
    );
}

#[test]
fn a_replay_drops_what_the_builders_queue() {
    let mut engine = probe_engine();
    engine
        .world_mut()
        .register_component_with_layout::<HookExtra>(HookExtra::FIELD_LAYOUT);
    let entity = spawn_probe(&mut engine);
    let marker = ModuleLiveness::new();
    bind(
        &mut engine,
        &marker,
        FieldEditHookRequest::after::<HookProbe>("test.hooks", add_extra_after),
    );

    write_field(&mut engine, entity, "value", 5.0);
    assert!(engine.world().get_component::<HookExtra>(entity).is_some());

    // The undo replays the step, so the callback runs again and queues the
    // same component; nothing it queues may apply on top of the step.
    undo(&mut engine);
    assert_eq!(read_field(&engine, entity, "value"), 0.0);
    assert!(
        engine.world().get_component::<HookExtra>(entity).is_none(),
        "a builder's command queued during a replay is dropped"
    );
}

#[test]
fn every_builder_is_a_no_op_under_a_replay_and_reports_once() {
    use crate::hooks::context::HookQueue;
    use crate::hooks::FieldEditCause;

    let mut engine = probe_engine();
    let entity = spawn_probe(&mut engine);
    let selection = EditorSelection::default();
    let mut queue = HookQueue::default();
    let mut notices = Vec::new();
    {
        let mut context = HookContext::new(
            engine.world_mut(),
            &selection,
            &mut queue,
            &mut notices,
            FieldEditCause::HistoryRestored,
            "test.hooks",
        );

        context.queue_add_component(entity, &extra_component());
        context.queue_remove_component(entity, &probe_component());
        context.queue_create_entity();
        context.queue_destroy_entity(entity);
        context.queue_selection(vec![entity]);
        context.queue_gizmo(Some(crate::gizmo::GizmoMode::Rotate), None, Some(true));
        context.queue(EditorCommand::SetSelection {
            entities: Vec::new(),
            asset: None,
        });
    }

    let (edits, source) = queue.take_edits();
    assert!(edits.is_empty(), "no edit survives a replay: {edits:?}");
    assert!(source.is_none(), "nothing was attributed to a callback");
    assert!(
        queue.take_controls().is_empty(),
        "no control survives a replay either"
    );
    assert!(
        queue.drop_reported(),
        "the drop is reported once for the session"
    );

    // The same session keeps dropping without reporting again, and nothing
    // accumulates for a later round.
    {
        let mut context = HookContext::new(
            engine.world_mut(),
            &selection,
            &mut queue,
            &mut notices,
            FieldEditCause::HistoryRestored,
            "test.hooks",
        );
        context.queue_gizmo(None, Some(crate::gizmo::GizmoSpace::Local), None);
    }
    assert!(
        queue.take_controls().is_empty(),
        "a control queued after the first drop is still dropped"
    );
    assert!(queue.drop_reported());
}
