//! The test extension's editor part: the editor API's proving case.
//!
//! # Responsibilities
//!
//! - Register one callback per editor event the API has, through the shared
//!   request queue, so a reload rebinds them instead of doubling them.
//! - Log what every callback received, under `[test] hook <event>`, which is
//!   what the editor suite's `editor_api` area reads.
//! - Read the editor state (the gizmo, the active tool, the shared selection
//!   and the viewport) from a field callback, and log what it read.
//! - Prove the before-callback's two verdicts: replace a magic value and refuse
//!   another.
//! - Change the editor from callbacks: a follow-up field write inside the step
//!   being applied, and the shared command builders for a component and a
//!   selection.
//!
//! # Design
//!
//! The crate is an ordinary extension crate with the `_editor` suffix, so only
//! an editor host loads it, after the `pill_test` crate beside it. It holds no
//! state of its own: every value it needs is in the world the callback is
//! handed, and every log line is derived from a payload. That is what lets a
//! reload replace the image without repeating anything.
//!
//! Registration goes through [`EditorHookRequests`], never through the
//! registry: the executable drains the queue, ties each request to this
//! crate's liveness marker and drops both together, so an unloaded image is
//! never called. The callbacks are plain functions because the registry holds
//! a function address, not a closure.
//!
//! The magic values exist so a suite can name a behaviour with one number: a
//! write of [`MAGIC_CLAMP`] applies as [`MAGIC_CLAMPED`], a write of
//! [`MAGIC_REFUSE`] is refused, and a write of [`MAGIC_FOLLOW_UP`] makes the
//! after-callback queue a second write that joins the same undo step.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_editor_core::editor_state::EditorCommand;
use pill_editor_core::hooks::{
    AssetEdit, ComponentEdit, EditorHookRequests, ElementSelectionChanged, EntityEdit, FieldEdit,
    FieldEditHookRequest, HistoryEdit, HookContext, HookRequest, HookVerdict, PlayEdit, SceneEdit,
    SelectionChanged, ToolActivatedEdit,
};
use pill_engine::component::ComponentRegistry;
use pill_engine::{pill_module, Engine, FieldValue};
use pill_test::{ModuleTest, ModuleTestField};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`; the `include!` is what makes every function
// resolvable by qualified path with nothing in this file annotated.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Constants
// =============================================================================

/// The crate registering the callbacks, which the log lines and the liveness
/// marker are keyed by.
pub const SOURCE: &str = env!("CARGO_PKG_NAME");

/// The value the before-callback replaces with [`MAGIC_CLAMPED`].
pub const MAGIC_CLAMP: f32 = 999.0;

/// The value the before-callback writes instead of [`MAGIC_CLAMP`].
pub const MAGIC_CLAMPED: f32 = 1.0;

/// The value the before-callback refuses.
pub const MAGIC_REFUSE: f32 = -1.0;

/// The value whose after-callback queues a second write in the same step.
pub const MAGIC_FOLLOW_UP: f32 = 5.0;

/// The `processed_frame_count` the follow-up writes.
pub const FOLLOW_UP_COUNT: u64 = 42;

// =============================================================================
// Field callbacks
// =============================================================================

/// The registered name of the component these callbacks answer.
fn probe_component_name() -> String {
    ComponentRegistry::registered_name::<ModuleTest>()
}

/// Reads the editor state and logs the field edit, replacing or refusing a
/// magic value.
///
/// # Errors
///
/// Never: the verdict is always built here. The signature is the one the
/// request constructor takes.
pub fn before_every_field_edit(
    context: &mut HookContext<'_>,
    edit: &FieldEdit<'_, ModuleTest>,
) -> HookVerdict {
    let gizmo = context.gizmo().map(|state| state.settings);
    let viewport = context.viewport();
    let gizmo_text = gizmo
        .map(|settings| format!("{:?}/{:?}", settings.mode, settings.space))
        .unwrap_or_default();
    let pointer_text = viewport
        .pointer_in_view
        .map(|(x, y)| format!("{x:.0},{y:.0}"))
        .unwrap_or_default();
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %edit.entity,
        field = edit.field_name,
        value = ?edit.current,
        cause = edit.cause.name(),
        phase = edit.phase.name(),
        primary = edit.is_primary,
        gizmo = %gizmo_text,
        snap = gizmo.map(|settings| settings.snap).unwrap_or(false),
        tool = context.active_tool().unwrap_or_default(),
        selection = context.selection().entities().len(),
        view = viewport.active_view.map_or(-1, |view| view as i64),
        pointer = %pointer_text,
        "[test] hook field-edit-before"
    );
    match edit.current.as_f32() {
        Some(value) if value == MAGIC_CLAMP => HookVerdict::Replace(FieldValue::F32(MAGIC_CLAMPED)),
        Some(value) if value == MAGIC_REFUSE => {
            HookVerdict::Refuse("the editor api refuses -1".to_string())
        }
        _ => HookVerdict::Keep,
    }
}

/// Logs the applied field edit, and queues the follow-up a magic value asks
/// for.
pub fn after_every_field_edit(context: &mut HookContext<'_>, edit: &FieldEdit<'_, ModuleTest>) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %edit.entity,
        field = edit.field_name,
        value = ?edit.current,
        previous = ?edit.previous,
        cause = edit.cause.name(),
        phase = edit.phase.name(),
        "[test] hook field-edit-after"
    );
    // The follow-up: a second write that joins the step the edit is already
    // part of, so one undo reverts both.
    if edit.field == ModuleTestField::AccumulatedTime
        && edit.current.as_f32() == Some(MAGIC_FOLLOW_UP)
    {
        context.queue(EditorCommand::SetField {
            entity: edit.entity,
            component: probe_component_name(),
            field: "processed_frame_count".to_string(),
            value: FieldValue::U64(FOLLOW_UP_COUNT),
        });
        pill_core::info!(
            target: telemetry_target::EDITOR,
            entity = %edit.entity,
            count = FOLLOW_UP_COUNT,
            "[test] hook follow-up"
        );
    }
}

// =============================================================================
// The remaining events
// =============================================================================

/// Logs a component add or remove.
pub fn on_component_edit(context: &mut HookContext<'_>, edit: &ComponentEdit<'_>) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %edit.entity,
        component = edit.component,
        added = edit.added,
        "[test] hook component-edit"
    );
    // The selection builder: a component that arrives while nothing is
    // selected selects its entity, as one recorded step.
    if edit.added && context.selection().primary().is_none() {
        context.queue_selection(vec![edit.entity]);
        pill_core::info!(
            target: telemetry_target::EDITOR,
            entity = %edit.entity,
            "[test] hook command selection"
        );
    }
}

/// Logs an entity create or destroy.
pub fn on_entity_edit(_context: &mut HookContext<'_>, edit: &EntityEdit) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %edit.entity,
        created = edit.created,
        "[test] hook entity-edit"
    );
}

/// Logs an asset-file edit.
pub fn on_asset_edit(_context: &mut HookContext<'_>, edit: &AssetEdit<'_>) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        kind = edit.kind.name(),
        path = edit.path,
        "[test] hook asset-edit"
    );
}

/// Logs a history walk.
pub fn on_history_edit(_context: &mut HookContext<'_>, edit: &HistoryEdit<'_>) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        direction = edit.direction,
        label = edit.label,
        "[test] hook history-edit"
    );
}

/// Logs a selection change and attaches the probe component to a bare entity.
pub fn on_selection_changed(context: &mut HookContext<'_>, edit: &SelectionChanged) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        previous = edit.previous.entities.len(),
        current = edit.current.entities.len(),
        primary = edit.current.entities.last().map(|entity| entity.to_string()).unwrap_or_default(),
        "[test] hook selection-changed"
    );
    // The component builder: an entity selected without the probe component
    // gets one, as one recorded step of its own.
    let Some(entity) = edit.current.entities.last().copied() else {
        return;
    };
    if context
        .world()
        .get_component::<ModuleTest>(entity)
        .is_some()
    {
        return;
    }
    let component = probe_component_name();
    context.queue_add_component(entity, &component);
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = %entity,
        component = %component,
        "[test] hook command add-component"
    );
}

/// Logs a scene open or new.
pub fn on_scene_edit(_context: &mut HookContext<'_>, edit: &SceneEdit<'_>) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        loaded = edit.loaded,
        path = edit.path.unwrap_or_default(),
        "[test] hook scene-edit"
    );
}

/// Logs a play or stop.
pub fn on_play_edit(_context: &mut HookContext<'_>, edit: &PlayEdit) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        playing = edit.playing,
        "[test] hook play-edit"
    );
}

/// Logs an active-tool change.
pub fn on_tool_activated(_context: &mut HookContext<'_>, edit: &ToolActivatedEdit) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        previous = edit.previous.as_deref().unwrap_or_default(),
        current = edit.current.as_deref().unwrap_or_default(),
        "[test] hook tool-activated"
    );
}

/// Logs an element-selection change.
pub fn on_element_selection(_context: &mut HookContext<'_>, edit: &ElementSelectionChanged) {
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entity = edit.entity.map(|entity| entity.to_string()).unwrap_or_default(),
        kind = edit.kind,
        current = edit.current.len(),
        "[test] hook element-selection"
    );
}

// =============================================================================
// Registration
// =============================================================================

/// Queues one request per event the editor API has.
///
/// This is the half a reload repeats: the executable drains the queue and
/// re-binds every callback to the arriving image's liveness marker, so the
/// registry never holds two of any event for one source.
///
/// # Errors
///
/// Returns 1 when the world has no request queue, which only happens when the
/// editor never created its command resources. The host reads any non-zero
/// status as a failed generation.
pub fn register(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();
    EditorHookRequests::ensure(world);
    let requests_to_queue = [
        FieldEditHookRequest::new::<ModuleTest>(SOURCE, before_every_field_edit),
        FieldEditHookRequest::after::<ModuleTest>(SOURCE, after_every_field_edit),
        HookRequest::component_edit(SOURCE, on_component_edit),
        HookRequest::entity_edit(SOURCE, on_entity_edit),
        HookRequest::asset_edit(SOURCE, on_asset_edit),
        HookRequest::history_edit(SOURCE, on_history_edit),
        HookRequest::selection_changed(SOURCE, on_selection_changed),
        HookRequest::scene_edit(SOURCE, on_scene_edit),
        HookRequest::play_edit(SOURCE, on_play_edit),
        HookRequest::tool_activated(SOURCE, on_tool_activated),
        HookRequest::element_selection(SOURCE, on_element_selection),
    ];
    let callback_count = requests_to_queue.len();
    let Some(requests) = world.get_resource_mut::<EditorHookRequests>() else {
        return 1;
    };
    for request in requests_to_queue {
        requests.push(request);
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        source = SOURCE,
        callbacks = callback_count,
        "[test] hook registered"
    );
    0
}

/// The artifact's one-time entry point.
///
/// The crate keeps nothing that has to run once: everything it owns is
/// registration, and [`register`] re-runs all of it on every generation. This
/// function exists because a statically linked build loads the module once and
/// calls this entry point alone, so it defers to [`register`].
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}
