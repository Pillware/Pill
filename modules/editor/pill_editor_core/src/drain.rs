//! The editor's frame boundary: applies the queued commands, walks the undo
//! history, and prunes that history after a reload.
//!
//! # Responsibilities
//!
//! - Apply the batches the shell and the scripted channel queued
//!   ([`apply_pending`]). A move or delete carries the shared selection with
//!   it, and a recorded batch becomes one undo step.
//! - Walk the history for `editor.undo` and `editor.redo`
//!   ([`step_history`]). A step is only popped when a command queue exists.
//! - Drop the history entries the current component registry can no longer
//!   resolve ([`prune_history`]), at most once per registry change.
//!
//! # Design
//!
//! The drain touches only the engine's world resources, so no runner owns it.
//! The native editor and the browser editor call the same functions at their
//! own frame boundary, so a change to the command contract is made once.
//!
//! Everything a caller can observe is a log line. The suites read `engine
//! commands applied`, `command failed`, `history push` and `history undo`;
//! the module also logs `history remap` and `history pruned`. Both editors
//! therefore report an edit the same way.
//!
//! Nothing here names egui, so the module also works in the browser build,
//! where there is no window at all.

// Standard library
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{Engine, World};

// Current crate
use crate::commands::PendingEditorCommands;
use crate::editor_state::{EditorCommand, EditorSelection};
use crate::history::{CommandHistory, HistoryEntry, HistoryEntryKind};
use crate::hooks::{FieldEditCause, HookSession};

// =============================================================================
// The drain
// =============================================================================

/// What one drain changed outside the world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DrainOutcome {
    /// How many batches ran.
    pub batches: usize,
    /// Whether a batch changed files on disk, so the asset listing must be
    /// refreshed right away.
    pub files_changed: bool,
    /// Whether a batch wrote an asset's settings document, so the Inspector
    /// must read it again.
    pub settings_changed: bool,
}

/// How many rounds of callback-queued work one drain applies.
///
/// A callback that answers a difference it caused itself would otherwise keep
/// producing work for the next round; the bound ends that instead of looping
/// for the frame. Four rounds is far more than an honest chain needs.
const HOOK_ROUNDS_MAX: usize = 4;

/// Applies every queued batch, then prunes a history a reload invalidated.
///
/// A failed command does not stop the rest of its batch. A recorded batch
/// whose commands all succeeded and all have an inverse becomes one history
/// step. The shared selection follows a move and clears with a delete. A
/// batch that touched files reports `files_changed`, and the caller refreshes
/// the asset listing at once.
pub fn apply_pending(engine: &mut Engine, now_millis: u64) -> DrainOutcome {
    // Step 1: Take every queued batch out of the world.
    let mut batches = {
        let world = engine.world_mut();
        world
            .get_resource_mut::<PendingEditorCommands>()
            .map(|pending| std::mem::take(&mut pending.batches))
            .unwrap_or_default()
    };
    let mut drain_outcome = DrainOutcome::default();
    // The editor API's callbacks: one snapshot of the live registrations,
    // taken before any batch applies, and the notices this drain collected.
    let mut hook_session = HookSession::new(engine.world());
    let mut hook_rounds = 0usize;
    // Step 2: Apply each batch in order, then act on what it changed. A
    // callback that answers a difference has no batch to join, so what it
    // queued becomes one more batch of this drain and the differences are read
    // again over what that batch changed.
    loop {
        for mut batch in batches.drain(..) {
            drain_outcome.batches += 1;
            // Step 2a: a tool's menu row is only a request. The tool decides what
            // it means, so it is asked here and its commands run in the row's
            // place, which makes one menu click one undo step.
            expand_element_menu(engine, &mut batch);
            // Step 2b: the editor API's before-callbacks see the whole batch
            // first, so a refusal drops every command of it instead of leaving
            // part of a multi-selection edit applied and unrecorded.
            let cause = if batch.replay.is_some() {
                FieldEditCause::HistoryRestored
            } else {
                FieldEditCause::Authored
            };
            if !hook_session.prepare_field_batch(engine, &mut batch.commands, cause, batch.phase) {
                // The refusal is already a notice and a log line. A replay's step
                // was popped before its commands ran, so it goes back where it
                // came from exactly as a failed replay does.
                if let Some(direction) = batch.replay {
                    let world = engine.world_mut();
                    if let Some(history) = world.get_resource_mut::<CommandHistory>() {
                        match direction {
                            HistoryDirection::Undo => history.undo_rollback(),
                            HistoryDirection::Redo => history.redo_rollback(),
                        }
                    }
                }
                continue;
            }
            let recorded_apply =
                EditorCommand::apply_recorded_observed(engine, &batch.commands, &mut hook_session);
            let failed_command_count = recorded_apply.failures.len();
            let replay_label = batch.replay_label.clone().unwrap_or_default();
            pill_core::info!(
                target: telemetry_target::EDITOR,
                count = batch.commands.len(),
                failed = failed_command_count,
                "[editor] engine commands applied"
            );
            // The selection follows what happened on disk, not what was asked
            // for. A refused move or delete must leave the selection on the file
            // that is still there, or the Inspector shows an asset that does not
            // match the disk.
            let failed_command_indices: std::collections::HashSet<usize> = recorded_apply
                .failures
                .iter()
                .map(|(index, _)| *index)
                .collect();
            follow_selection(engine, &batch.commands, &failed_command_indices);
            // A batch that moved files refreshes the asset listing at once,
            // because the Assets toolbar acts on what the listing says.
            if batch.commands.iter().any(command_touches_files) {
                drain_outcome.files_changed = true;
            }
            // A settings write does not change the listing's shape, but the
            // document the Inspector rows draw is now out of date.
            if batch
                .commands
                .iter()
                .any(|command| matches!(command, EditorCommand::SetAssetSettings { .. }))
            {
                drain_outcome.settings_changed = true;
            }
            for (index, message) in recorded_apply.failures {
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    index,
                    error = %message,
                    "[editor] command failed"
                );
            }
            // A history step is popped before its commands run, so a failed
            // replay must put the step back on the side it came from. Without
            // that, the step would be offered in the wrong direction while the
            // world still shows the edit. The `history undo` line was logged when
            // the step was queued; this block reports the failure.
            if let Some(direction) = batch.replay {
                let world = engine.world_mut();
                if let Some(history) = world.get_resource_mut::<CommandHistory>() {
                    if failed_command_count > 0 {
                        match direction {
                            HistoryDirection::Undo => history.undo_rollback(),
                            HistoryDirection::Redo => history.redo_rollback(),
                        }
                        pill_core::warn!(
                            target: telemetry_target::EDITOR,
                            direction = direction.name(),
                            undo = history.undo_depth(),
                            redo = history.redo_depth(),
                            "[editor] history {} failed",
                            direction.name()
                        );
                    }
                }
                // The editor API hears about the walk only when it happened: a
                // step whose commands failed was put back, so nothing changed.
                if failed_command_count == 0 {
                    hook_session.fire_history(engine, direction.name(), replay_label.as_str());
                }
            }
            // A restore gives its entity a fresh handle. The history follows the
            // new handle before anything else is recorded, so a later undo cannot
            // target the old one.
            for (previous, assigned) in &recorded_apply.remaps {
                let world = engine.world_mut();
                let Some(history) = world.get_resource_mut::<CommandHistory>() else {
                    break;
                };
                history.remap_entity(*previous, *assigned);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    previous = %previous,
                    assigned = %assigned,
                    "[editor] history remap"
                );
            }
            // An edit batch becomes one undo step, unless it failed or could not
            // be fully inverted. A step that cannot be undone would be worse than
            // no step.
            if batch.record && recorded_apply.fully_invertible && !batch.commands.is_empty() {
                let entry = HistoryEntry::engine(
                    batch.label.clone(),
                    batch.commands.clone(),
                    recorded_apply.inverses.clone(),
                );
                let entry = match &batch.coalesce_key {
                    Some(key) => entry.coalescing(key.clone(), now_millis),
                    None => entry,
                };
                let world = engine.world_mut();
                if let Some(history) = world.get_resource_mut::<CommandHistory>() {
                    let pushed =
                        history.push(entry, now_millis, CommandHistory::DEFAULT_COALESCE_WINDOW);
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        label = batch.label.as_str(),
                        steps = pushed.steps,
                        coalesced = pushed.coalesced,
                        "[editor] history push"
                    );
                }
            }
        }
        // Step 3: the selection, the tool and the element slot are differences
        // against the last drain, so they are read once every batch of this round
        // applied.
        hook_session.fire_diffs(engine);
        // Step 3a: what a callback answering a difference queued has no batch of
        // its own, so it comes back as one: an unrecorded batch for its controls,
        // a recorded step for its edits. The round then reads the differences over
        // what it changed.
        let queued = hook_session.take_queued_batches();
        if queued.is_empty() {
            break;
        }
        if hook_rounds >= HOOK_ROUNDS_MAX {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                rounds = hook_rounds,
                batches = queued.len(),
                "[editor] hook commands dropped"
            );
            break;
        }
        hook_rounds += 1;
        batches.extend(queued);
    }
    // Step 4: hand the notices the callbacks posted to the executable's toast
    // path, then drop the history entries the rebuilt registry cannot resolve.
    hook_session.publish_notices(engine.world_mut());
    prune_history(engine);
    // Step 5: Drop the replayed extension states no extension took in time,
    // so an extension that unloaded cannot fill the inbox.
    if let Some(inbox) = engine
        .world_mut()
        .get_resource_mut::<crate::extension_edits::ExtensionEditInbox>()
    {
        inbox.age();
    }
    drain_outcome
}

/// Replaces every [`EditorCommand::RunElementMenu`] in `batch` with the
/// commands the active tool's menu row asks for.
///
/// A menu row is the tool's to define, so the drain asks it at the frame
/// boundary and runs its answer in the row's place: the batch the shell queued
/// then holds exactly the edit the row means, and the history records it as
/// one step. A row the tool refuses keeps its command, whose apply reports the
/// refusal like any other failed command.
fn expand_element_menu(engine: &mut Engine, batch: &mut crate::commands::PendingCommandBatch) {
    if !batch
        .commands
        .iter()
        .any(|command| matches!(command, EditorCommand::RunElementMenu { .. }))
    {
        return;
    }
    let subject = crate::tools::element_selection_subject(engine.world());
    let mut expanded: Vec<EditorCommand> = Vec::with_capacity(batch.commands.len());
    for command in std::mem::take(&mut batch.commands) {
        let EditorCommand::RunElementMenu { item } = &command else {
            expanded.push(command);
            continue;
        };
        let Some((entity, kind, selected)) = subject.clone() else {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                item = item.as_str(),
                error = "no element selection names a subject",
                "[editor] element menu refused"
            );
            expanded.push(command);
            continue;
        };
        let world = engine.world_mut();
        let registry = crate::tools::tool_registry_mut(world).clone();
        match registry.active_menu_run(entity, &kind, &selected, item, world) {
            Ok(commands) => {
                batch.label = format!("tool {item}");
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    item = item.as_str(),
                    commands = commands.len(),
                    "[editor] element menu ran"
                );
                expanded.extend(commands);
            }
            Err(reason) => {
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    item = item.as_str(),
                    error = reason.as_str(),
                    "[editor] element menu refused"
                );
                expanded.push(command);
            }
        }
    }
    batch.commands = expanded;
}

/// Moves the shared selection with the file a batch moved or deleted.
/// The selection follows the disk instead of being recorded as a step, so an
/// undo never has to replay a selection change to keep the two in agreement.
/// A command that failed is skipped: a move refused because its destination
/// exists must not leave the selection on a file that is not there.
fn follow_selection(
    engine: &mut Engine,
    commands: &[EditorCommand],
    failed_command_indices: &std::collections::HashSet<usize>,
) {
    for (index, command) in commands.iter().enumerate() {
        if failed_command_indices.contains(&index) {
            continue;
        }
        let world = engine.world_mut();
        let Some(selection) = world.get_resource_mut::<EditorSelection>() else {
            break;
        };
        match command {
            EditorCommand::MoveAssetFile { from, to }
                if selection.asset.as_deref() == Some(from.as_str()) =>
            {
                selection.asset = Some(to.clone());
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    path = to.as_str(),
                    "[editor] selection followed the move"
                );
            }
            EditorCommand::DeleteAssetFile { path }
                if selection.asset.as_deref() == Some(path.as_str()) =>
            {
                selection.asset = None;
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    path = path.as_str(),
                    "[editor] selection cleared with the delete"
                );
            }
            _ => {}
        }
    }
}

/// Whether one command changes files on disk.
fn command_touches_files(command: &EditorCommand) -> bool {
    matches!(
        command,
        EditorCommand::CreateStandaloneAsset { .. }
            | EditorCommand::RestoreAssetFile { .. }
            | EditorCommand::DeleteAssetFile { .. }
            | EditorCommand::MoveAssetFile { .. }
            | EditorCommand::WriteAssetMeta { .. }
            | EditorCommand::ImportFiles { .. }
            | EditorCommand::RemoveFiles { .. }
    )
}

// =============================================================================
// The history walk
// =============================================================================

/// Which way a history walk goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryDirection {
    /// Applies the newest step's inverse.
    Undo,
    /// Reapplies the next step's forward batch.
    Redo,
}

impl HistoryDirection {
    /// The word this direction is logged with.
    fn name(self) -> &'static str {
        match self {
            HistoryDirection::Undo => "undo",
            HistoryDirection::Redo => "redo",
        }
    }
}

/// What one history walk did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryOutcome {
    /// The step's batch is queued for the next drain.
    Queued {
        /// The step's label, as the history recorded it.
        label: String,
        /// Steps left in that direction after the pop.
        remaining: usize,
    },
    /// Nothing was left in that direction.
    Empty,
    /// The command queue or the history is absent, so nothing was popped.
    NoQueue,
}

/// Pops one history entry and queues its batch, which is applied but never
/// recorded.
///
/// The command queue is checked before the pop. A step popped without a queue
/// would be lost even though the log said the undo ran.
pub fn step_history(engine: &mut Engine, direction: HistoryDirection) -> HistoryOutcome {
    // Step 1: Refuse the walk when no command queue exists.
    let direction_name = direction.name();
    let queue_present = engine
        .world()
        .get_resource::<PendingEditorCommands>()
        .is_some();
    if !queue_present {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            direction = direction_name,
            "[editor] history step without a command queue"
        );
        return HistoryOutcome::NoQueue;
    }
    // Step 2: Pop the step in the walk's direction and read what it carries.
    let world = engine.world_mut();
    let Some(history) = world.get_resource_mut::<CommandHistory>() else {
        return HistoryOutcome::NoQueue;
    };
    let history_entry = match direction {
        HistoryDirection::Undo => history.pop_undo(),
        HistoryDirection::Redo => history.pop_redo(),
    };
    let Some(history_entry) = history_entry else {
        pill_core::info!(
            target: telemetry_target::EDITOR,
            "[editor] history {} empty",
            direction_name
        );
        return HistoryOutcome::Empty;
    };
    let label = history_entry.label.clone();
    let remaining_step_count = match direction {
        HistoryDirection::Undo => history.undo_depth(),
        HistoryDirection::Redo => history.redo_depth(),
    };
    let commands = match history_entry.entry {
        HistoryEntryKind::Engine { forward, inverse } => Some(match direction {
            HistoryDirection::Undo => inverse,
            HistoryDirection::Redo => forward,
        }),
        // A selection-only step carries no commands, so nothing is queued.
        // The log line below still reports its depth.
        HistoryEntryKind::Selection { .. } => None,
    };
    // Step 3: Queue the step's batch for the next drain, then log the walk.
    if let Some(commands) = commands {
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_replay(commands, direction, Some(label.as_str()));
        }
    }
    pill_core::info!(
        target: telemetry_target::EDITOR,
        label = label.as_str(),
        remaining = remaining_step_count,
        "[editor] history {}",
        direction_name
    );
    HistoryOutcome::Queued {
        label,
        remaining: remaining_step_count,
    }
}

// =============================================================================
// The reload prune
// =============================================================================

/// Drops the history entries the current component registry cannot resolve.
///
/// Every entry remembers the component and field identifiers its commands
/// touched. A project or module reload rebuilds the registry, and the
/// reloaded module may no longer register a component or field that an old
/// step names. Running that step would then target storage that no longer
/// exists. Each call hashes the registry, but the entries are only walked
/// when that hash changed.
///
/// Returns how many entries were dropped.
pub fn prune_history(engine: &mut Engine) -> usize {
    let current_registry_stamp = registry_stamp(engine.world());
    let resolvable_identifiers = resolvable_identifiers(engine.world());
    let world = engine.world_mut();
    let Some(history) = world.get_resource_mut::<CommandHistory>() else {
        return 0;
    };
    let step_count = history.steps();
    let dropped_entry_count = history.drop_stale(
        |identifier| resolvable_identifiers.contains(identifier),
        current_registry_stamp,
    );
    if dropped_entry_count > 0 {
        pill_core::info!(
            target: telemetry_target::EDITOR,
            dropped = dropped_entry_count,
            steps = step_count,
            "[editor] history pruned"
        );
    }
    dropped_entry_count
}

/// A stamp of the world's component registry: the component names plus every
/// field name their layouts declare. Removing a field changes it too.
pub fn registry_stamp(world: &World) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for (name, id) in world.registered_components() {
        name.hash(&mut hasher);
        if let Some(layout) = world.component_field_layout(id) {
            for descriptor in layout {
                descriptor.name.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

/// Returns every identifier a history entry may name, as `component` and
/// `component.field`.
fn resolvable_identifiers(world: &World) -> HashSet<String> {
    let mut resolvable_identifiers = HashSet::new();
    for (name, id) in world.registered_components() {
        if let Some(layout) = world.component_field_layout(id) {
            for descriptor in layout {
                resolvable_identifiers.insert(format!("{name}.{}", descriptor.name));
            }
        }
        resolvable_identifiers.insert(name);
    }
    resolvable_identifiers
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// An engine with the command resources an editor's world carries.
    fn engine() -> Engine {
        let mut engine = Engine::default();
        let world = engine.world_mut();
        world.register_resource::<PendingEditorCommands>();
        world.insert_resource(PendingEditorCommands::default());
        world.register_resource::<CommandHistory>();
        world.insert_resource(CommandHistory::default());
        world.register_resource::<EditorSelection>();
        world.insert_resource(EditorSelection::default());
        engine
    }

    #[test]
    fn an_empty_drain_reports_nothing() {
        let mut engine = engine();
        let drain_outcome = apply_pending(&mut engine, 0);
        assert_eq!(drain_outcome, DrainOutcome::default());
    }

    #[test]
    fn a_step_without_a_queue_is_not_popped() {
        // Without a queue the step would be lost while the log claimed the
        // undo ran, so nothing is popped.
        let mut engine = Engine::default();
        let world = engine.world_mut();
        world.register_resource::<CommandHistory>();
        world.insert_resource(CommandHistory::default());
        assert_eq!(
            step_history(&mut engine, HistoryDirection::Undo),
            HistoryOutcome::NoQueue
        );
    }

    #[test]
    fn an_empty_history_reports_empty() {
        let mut engine = engine();
        assert_eq!(
            step_history(&mut engine, HistoryDirection::Undo),
            HistoryOutcome::Empty
        );
        assert_eq!(
            step_history(&mut engine, HistoryDirection::Redo),
            HistoryOutcome::Empty
        );
    }

    /// A world the engine cannot copy refuses Play: the person gets a toast
    /// naming the type, and the editor stays in edit mode.
    #[test]
    fn a_refused_play_shows_a_notice_and_stays_in_edit_mode() {
        /// A component that owns heap data and is not `Clone`.
        #[derive(Debug)]
        struct UncopyableProbe {
            #[allow(dead_code)]
            buffer: Vec<u32>,
        }
        impl pill_engine::Component for UncopyableProbe {}

        let mut engine = engine();
        engine.set_run_mode(pill_engine::RunMode::Editing);
        let world = engine.world_mut();
        world.register_component::<UncopyableProbe>();
        world
            .create_entity()
            .with(UncopyableProbe {
                buffer: vec![1, 2, 3],
            })
            .build()
            .expect("the entity builds");

        // Queue the command the toolbar's Play button queues.
        let pending = engine
            .world_mut()
            .get_resource_mut::<PendingEditorCommands>()
            .expect("the test engine carries a queue");
        pending.push(vec![EditorCommand::EnterPlay]);
        apply_pending(&mut engine, 0);
        assert_eq!(
            engine.run_mode(),
            pill_engine::RunMode::Editing,
            "a refused Play leaves the editor editing"
        );
        let world = engine.world();
        let notices = world
            .get_resource::<crate::notices::EditorNotices>()
            .expect("the refusal pushes a notice");
        let refusal = notices
            .events
            .iter()
            .find(|notice| notice.text.starts_with("Play refused"))
            .unwrap_or_else(|| {
                panic!(
                    "a refused Play must show a toast, but the notices are {:?}",
                    notices.events
                )
            });
        assert_eq!(refusal.level, crate::notices::NoticeLevel::Error);
        assert!(
            refusal.text.contains("UncopyableProbe"),
            "the toast names the type: {}",
            refusal.text
        );
    }

    #[test]
    fn the_registry_stamp_changes_only_when_the_registry_does() {
        let engine = engine();
        let first_registry_stamp = registry_stamp(engine.world());
        assert_eq!(first_registry_stamp, registry_stamp(engine.world()));
    }

    /// The selection follows a move that applied, and stays where it was when
    /// the move was refused. A move whose destination already exists must not
    /// point the selection at that other file.
    #[test]
    fn the_selection_follows_the_moves_that_applied() {
        let _mount = crate::test_support::asset_mount_lock();
        let asset_directory = std::env::temp_dir().join(format!(
            "pill-editor-drain-selection-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&asset_directory);
        std::fs::create_dir_all(&asset_directory).unwrap();
        std::fs::write(asset_directory.join("from.material"), b"source").unwrap();
        std::fs::write(asset_directory.join("taken.material"), b"taken").unwrap();
        pill_engine::asset_store::mount_directory(&asset_directory);

        let mut engine = engine();
        {
            let world = engine.world_mut();
            let mut selection = EditorSelection::default();
            selection.asset = Some("from.material".to_string());
            world.insert_resource(selection);
            world.insert_resource(PendingEditorCommands::default());
        }
        let queue_batch = |engine: &mut Engine, commands: Vec<EditorCommand>| {
            let world = engine.world_mut();
            let pending = world
                .get_resource_mut::<PendingEditorCommands>()
                .expect("the test engine carries the queue");
            pending.push(commands);
        };
        let selected_asset = |engine: &Engine| {
            engine
                .world()
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.asset.clone())
        };

        // Refused: `taken.material` exists, so nothing moved and the selection
        // stays on the source.
        queue_batch(
            &mut engine,
            vec![EditorCommand::MoveAssetFile {
                from: "from.material".to_string(),
                to: "taken.material".to_string(),
            }],
        );
        apply_pending(&mut engine, 0);
        assert_eq!(
            selected_asset(&engine).as_deref(),
            Some("from.material"),
            "a refused move must leave the selection on the file that is still there"
        );

        // Applied: the file moved, so the selection follows it.
        queue_batch(
            &mut engine,
            vec![EditorCommand::MoveAssetFile {
                from: "from.material".to_string(),
                to: "moved.material".to_string(),
            }],
        );
        apply_pending(&mut engine, 0);
        assert_eq!(
            selected_asset(&engine).as_deref(),
            Some("moved.material"),
            "an applied move still carries the selection with it"
        );

        let _ = std::fs::remove_dir_all(&asset_directory);
    }

    #[test]
    fn a_failed_undo_puts_the_step_back() {
        // The step is popped before its commands run. A failed inverse would
        // then leave the log saying the undo ran, the world still showing the
        // edit, and the step waiting to be redone. A step that did not run
        // must stay undoable, so the drain puts it back.
        let mut engine = engine();
        let failing_entry = HistoryEntry::engine(
            "destroy an entity that is gone",
            vec![EditorCommand::DestroyEntity {
                entity: pill_engine::Entity::default(),
            }],
            vec![EditorCommand::DestroyEntity {
                entity: pill_engine::Entity::default(),
            }],
        );
        let world = engine.world_mut();
        let Some(history) = world.get_resource_mut::<CommandHistory>() else {
            panic!("the test engine carries a history");
        };
        history.push(failing_entry, 0, CommandHistory::DEFAULT_COALESCE_WINDOW);

        assert!(matches!(
            step_history(&mut engine, HistoryDirection::Undo),
            HistoryOutcome::Queued { .. }
        ));
        apply_pending(&mut engine, 0);

        let world = engine.world();
        let history = world
            .get_resource::<CommandHistory>()
            .expect("the test engine carries a history");
        assert_eq!(history.undo_depth(), 1, "the failed step is undoable again");
        assert_eq!(history.redo_depth(), 0, "and is not offered as redoable");
    }

    #[test]
    fn a_prune_drops_entries_the_registry_cannot_resolve() {
        // The state a project or module reload leaves behind: a step naming a
        // component the arriving registry no longer has. Undoing it would
        // target storage that is gone, so the first prune after the change
        // drops it - and only the first.
        let mut engine = engine();
        let stale_entry = HistoryEntry::engine(
            "stale",
            vec![EditorCommand::SetField {
                entity: pill_engine::Entity::default(),
                component: "GoneComponent".to_string(),
                field: "value".to_string(),
                value: pill_engine::FieldValue::F32(1.0),
            }],
            Vec::new(),
        );
        let world = engine.world_mut();
        let Some(history) = world.get_resource_mut::<CommandHistory>() else {
            panic!("the test engine carries a history");
        };
        history.push(stale_entry, 0, CommandHistory::DEFAULT_COALESCE_WINDOW);
        assert_eq!(history.steps(), 1);

        assert_eq!(prune_history(&mut engine), 1);
        assert_eq!(prune_history(&mut engine), 0);
    }
}
