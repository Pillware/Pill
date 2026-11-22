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
//! - Register one command and log every invocation of it, under
//!   `[test] command invoked`, so the suite proves a module's command reaches
//!   it with its target. A second command proves the free-form draw function,
//!   and a third is placed in the context menus and the viewport toolbox.
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
//! a function address, not a closure. The command is queued the same way, so
//! its row disappears when this image does.
//!
//! The magic values exist so a suite can name a behaviour with one number: a
//! write of [`MAGIC_CLAMP`] applies as [`MAGIC_CLAMPED`], a write of
//! [`MAGIC_REFUSE`] is refused, and a write of [`MAGIC_FOLLOW_UP`] makes the
//! after-callback queue a second write that joins the same undo step.

// External crates
use std::collections::HashMap;

use pill_core::telemetry::telemetry_target;
use pill_editor_core::commands::{
    CommandInbox, CommandRegistration, CommandRegistrations, CommandSpec, CommandTarget,
    KeymapPreset,
};
use pill_editor_core::editor_state::EditorCommand;
use pill_editor_core::egui;
use pill_editor_core::hooks::{
    AssetEdit, ComponentEdit, EditorHookRequests, ElementSelectionChanged, EntityEdit, FieldEdit,
    FieldEditHookRequest, HistoryEdit, HookContext, HookRequest, HookVerdict, PlayEdit, SceneEdit,
    SelectionChanged, ToolActivatedEdit,
};
use pill_editor_core::icons::Icon;
use pill_editor_core::placements::{
    CommandDrawContext, CommandDrawRequest, CommandState, CommandStates, UiSlot,
};
use pill_engine::component::ComponentRegistry;
use pill_engine::system;
use pill_engine::{pill_module, Engine, Entity, FieldValue, Query, ResMut};
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

/// The command this crate registers: the one the suite runs to prove a
/// module's command reaches it.
pub const COMMAND_ID: &str = "pill_test.editor_command";

/// The command whose widget this crate draws itself, the free-form exception.
pub const DRAWN_COMMAND_ID: &str = "pill_test.editor_drawn_command";

/// The command placed in the places that need no Inspector header: the
/// component, entity and asset context menus and the viewport toolbox.
pub const CONTEXT_COMMAND_ID: &str = "pill_test.editor_context_command";

/// The component both commands place a button on, named by its last segment so
/// this crate does not depend on the crate that defines it.
pub const PROBE_COMPONENT: &str = "ModuleTest";

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

/// The highest view index this crate publishes a toolbox state for, so a
/// layout with several scene views still shows a labelled row in each.
pub const MAX_TESTED_VIEWS: u8 = 3;

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
// The registered commands
// =============================================================================

/// Takes every invocation of this crate's commands and logs it with its target.
///
/// It also publishes the state the places draw for each entity that carries
/// the probe component (the Inspector header and the component and entity
/// menus) and for the editor's views (the toolbox): a label override, a toggle
/// mark and a status dot. A post-project system that runs in edit mode, like
/// the shell, so a command run from a menu, the palette, a shortcut, a header
/// button, a toolbox row or the scripted channel arrives here on the frame it
/// ran.
#[system(
    name = "test_editor_commands",
    run_phase = "post_project",
    run_in_edit_mode = true
)]
pub fn test_editor_commands(
    mut inbox: ResMut<CommandInbox>,
    mut states: ResMut<CommandStates>,
    mut probe_entities: Query<(Entity, &ModuleTest)>,
) {
    let (Some(mut inbox), Some(mut states)) = (inbox.get_mut(), states.get_mut()) else {
        return;
    };
    let component = probe_component_name();
    let mut header_states = HashMap::new();
    let mut context_states = HashMap::new();
    for (entity, _) in probe_entities.iter_mut() {
        header_states.insert(
            CommandTarget::Entity {
                entity,
                component: Some(component.clone()),
            },
            CommandState::default()
                .with_label("Test Command")
                .with_checked(true)
                .with_indicator([0x4a, 0xd9, 0x91, 0xff])
                .with_tooltip(vec!["The test editor crate's command".to_string()]),
        );
        // An entity's own menu names no component, so the state is published
        // for that target too.
        context_states.insert(
            CommandTarget::Entity {
                entity,
                component: None,
            },
            CommandState::default()
                .with_label("Test Context Command")
                .with_tooltip(vec!["The test editor crate's menu command".to_string()]),
        );
    }
    // The toolbox is drawn once per view, so its rows carry a view target.
    for view in 0..=MAX_TESTED_VIEWS {
        context_states.insert(
            CommandTarget::View(view),
            CommandState::default().with_label("Test Context Command"),
        );
    }
    states.publish(COMMAND_ID, header_states.clone());
    states.publish(DRAWN_COMMAND_ID, header_states);
    states.publish(CONTEXT_COMMAND_ID, context_states);
    for id in [COMMAND_ID, DRAWN_COMMAND_ID, CONTEXT_COMMAND_ID] {
        for invocation in inbox.take(id) {
            let target = match &invocation.target {
                CommandTarget::None => "none".to_string(),
                CommandTarget::Entity { entity, component } => match component {
                    Some(component) => format!("entity {entity} {component}"),
                    None => format!("entity {entity}"),
                },
                CommandTarget::Asset(key) => format!("asset {key}"),
                CommandTarget::View(view) => format!("view {view}"),
            };
            pill_core::info!(
                target: telemetry_target::EDITOR,
                command = invocation.id.as_str(),
                target = target.as_str(),
                "[test] command invoked"
            );
        }
    }
}

/// Draws [`DRAWN_COMMAND_ID`]'s own widget: the free-form exception, drawn by
/// the module rather than from data.
pub fn draw_test_command(ui: &mut egui::Ui, context: &mut CommandDrawContext<'_>) {
    let label = context
        .state
        .and_then(|state| state.label.as_deref())
        .unwrap_or("Test Mark");
    if ui.add(egui::Button::new(label).small()).clicked() {
        context.invoke();
    }
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
    // The commands the suite runs: one with an Edit row and a chord in every
    // preset, one whose widget this crate draws itself. Both place a button on
    // the probe component's Inspector header. The queues are claimed here the
    // same way the hooks' is, so the sets are bound to this image's liveness
    // marker.
    CommandRegistrations::ensure(world);
    let mut command = CommandSpec::new(COMMAND_ID, "Test Editor Command")
        .menu(&["Edit", "Test Editor Command"])
        .icon(Icon::CheckCircle)
        .place(UiSlot::inspector_component_header(PROBE_COMPONENT))
        .place(UiSlot::Toolbar);
    for preset in [
        KeymapPreset::Blender,
        KeymapPreset::Unity,
        KeymapPreset::Godot,
        KeymapPreset::Unreal,
    ] {
        command = command.bind(preset, "ctrl+alt+t");
    }
    let drawn = CommandSpec::new(DRAWN_COMMAND_ID, "Test Drawn Command")
        .place(UiSlot::inspector_component_header(PROBE_COMPONENT));
    // The remaining places: the two context menus around the probe component,
    // the menu of a scene asset and the viewport toolbox. One command, so the
    // target the invocation carries says which place it came from.
    let context = CommandSpec::new(CONTEXT_COMMAND_ID, "Test Context Command")
        .icon(Icon::CheckCircle)
        .place(UiSlot::component_context_menu(PROBE_COMPONENT))
        .place(UiSlot::entity_context_menu())
        .place(UiSlot::asset_context_menu(".scene"))
        .place(UiSlot::ViewportToolbox);
    if let Some(registrations) = world.get_resource_mut::<CommandRegistrations>() {
        registrations.push(CommandRegistration {
            owner: SOURCE.to_string(),
            commands: vec![command, drawn, context],
        });
        registrations.push_draw(CommandDrawRequest {
            id: DRAWN_COMMAND_ID,
            slot: UiSlot::inspector_component_header(PROBE_COMPONENT),
            source: SOURCE,
            draw: draw_test_command,
        });
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
