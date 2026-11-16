//! Undo and redo history for editor edits.
//!
//! # Responsibilities
//!
//! - Own the undo entries, the cursor that says which of them are applied,
//!   and the redo tail (the entries past the cursor).
//! - Merge continuous edits of one control, such as a slider drag, into a
//!   single step while they keep arriving close together.
//! - Keep the history within the step cap and the memory budget the
//!   preferences set, dropping the oldest entries first and reporting which
//!   limit caused the drop.
//! - Drop entries that name a component type or field a reload removed.
//!
//! # Design
//!
//! The history is a world resource, so it survives reloads of the editor
//! modules that write into it. An engine entry keeps two batches: the
//! commands that made the edit and the commands that reverse it. Undo pops an
//! entry and queues its inverse batch through the same frame-boundary
//! application every edit uses, so an undo reaches the world the same way the
//! edit did. A restore of a destroyed entity assigns a fresh handle; after it
//! runs, the remaining entries are moved to that handle with
//! [`CommandHistory::remap_entity`].

// Standard library
use std::time::Duration;

// External crates
use pill_engine::{Entity, FieldValue};

// Current crate
use crate::editor_state::EditorCommand;

// =============================================================================
// Types
// =============================================================================

/// One undoable step.
#[derive(Clone, Debug)]
pub struct HistoryEntry {
    /// What the step is called in the history's log lines
    /// ("set PhysicsState.position_x").
    pub label: String,
    /// What undoing and redoing this step does.
    pub entry: HistoryEntryKind,
    /// Component and field identifiers the entry addresses.
    pub touched: Vec<String>,
    /// Approximate size the entry keeps alive, including the bytes of a file
    /// a restore has to write back.
    pub memory_bytes: usize,
    /// Continuous edits sharing a key merge while they arrive back to back.
    pub coalesce_key: Option<String>,
    /// Arrival time of the newest coalesced edit, in milliseconds.
    pub coalesced_at_millis: u64,
}

/// The two kinds of undoable step.
#[derive(Clone, Debug)]
pub enum HistoryEntryKind {
    /// An engine batch and the batch that reverses it.
    Engine {
        /// The commands the edit applied.
        forward: Vec<EditorCommand>,
        /// The commands that reverse it, in undo order.
        inverse: Vec<EditorCommand>,
    },
    /// A selection change, carrying the selection on each side of it.
    ///
    /// A history walk queues no commands for this kind, so a step of this
    /// kind only moves the step counts.
    Selection {
        /// The selection before the change.
        before: Option<Entity>,
        /// The selection after the change.
        after: Option<Entity>,
    },
}

impl HistoryEntry {
    /// Builds an entry from the commands an edit applied and the commands that
    /// reverse it.
    pub fn engine(
        label: impl Into<String>,
        forward: Vec<EditorCommand>,
        inverse: Vec<EditorCommand>,
    ) -> Self {
        let touched = forward
            .iter()
            .flat_map(EditorCommand::touched_identifiers)
            .collect();
        Self {
            label: label.into(),
            memory_bytes: estimate_bytes(&forward) + estimate_bytes(&inverse),
            entry: HistoryEntryKind::Engine { forward, inverse },
            touched,
            coalesce_key: None,
            coalesced_at_millis: 0,
        }
    }

    /// Builds an entry for a selection change from the selection before and
    /// after it.
    pub fn selection(before: Option<Entity>, after: Option<Entity>) -> Self {
        Self {
            label: "select".to_string(),
            entry: HistoryEntryKind::Selection { before, after },
            touched: Vec::new(),
            memory_bytes: 64,
            coalesce_key: None,
            coalesced_at_millis: 0,
        }
    }

    /// Marks the entry as one of a continuous edit sequence.
    ///
    /// `coalesce_key` names the control being edited and `coalesced_at_millis`
    /// is when the edit arrived. A later push with the same key merges into
    /// this entry while it arrives within the coalesce window.
    pub fn coalescing(mut self, coalesce_key: impl Into<String>, coalesced_at_millis: u64) -> Self {
        self.coalesce_key = Some(coalesce_key.into());
        self.coalesced_at_millis = coalesced_at_millis;
        self
    }

    /// Replaces `previous` with `assigned` wherever this entry names an
    /// entity.
    pub fn remap_entity(&mut self, previous: Entity, assigned: Entity) {
        match &mut self.entry {
            HistoryEntryKind::Engine { forward, inverse } => {
                for command in forward.iter_mut().chain(inverse.iter_mut()) {
                    command.remap_entity(previous, assigned);
                }
            }
            HistoryEntryKind::Selection { before, after } => {
                if *before == Some(previous) {
                    *before = Some(assigned);
                }
                if *after == Some(previous) {
                    *after = Some(assigned);
                }
            }
        }
    }
}

/// Why the history dropped its oldest entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrimReason {
    /// The step cap was reached.
    Steps,
    /// The memory budget was reached.
    Memory,
}

/// What a push or a limit change did to the history.
#[derive(Clone, Copy, Debug, Default)]
pub struct PushOutcome {
    /// Entries still in the history afterwards.
    pub steps: usize,
    /// How many of the oldest entries the change dropped.
    pub trimmed: usize,
    /// The limit that caused the drop, when one did.
    pub trim_reason: Option<TrimReason>,
    /// True when a push merged into the entry before it.
    pub coalesced: bool,
}

/// The editor's undo stack.
#[derive(Debug)]
pub struct CommandHistory {
    /// Oldest first; `entries[cursor]` is the next redo.
    entries: Vec<HistoryEntry>,
    /// Number of entries currently applied.
    cursor: usize,
    /// Maximum number of entries.
    step_cap: usize,
    /// Maximum retained bytes.
    memory_budget_bytes: usize,
    /// Sum of the entries' `memory_bytes`.
    memory_used_bytes: usize,
    /// The registry stamp (a hash of the registered components and their
    /// fields) that the last prune ran against. It starts at zero, so the first
    /// prune that gets a real stamp does the walk.
    registry_stamp: u64,
}

impl CommandHistory {
    /// Steps kept when the preferences do not say otherwise.
    pub const DEFAULT_STEP_CAP: usize = 500;
    /// Bytes kept when the preferences do not say otherwise.
    pub const DEFAULT_MEMORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;
    /// Continuous edits within this window merge into one step.
    pub const DEFAULT_COALESCE_WINDOW: Duration = Duration::from_millis(400);

    /// Creates a history that enforces `step_cap` entries and
    /// `memory_budget_bytes` bytes.
    pub fn new(step_cap: usize, memory_budget_bytes: usize) -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            step_cap,
            memory_budget_bytes,
            memory_used_bytes: 0,
            registry_stamp: 0,
        }
    }

    /// Registers the history resource, inserting a default only when the world
    /// has none, so a reload keeps the live history.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<CommandHistory>();
        if world.get_resource::<CommandHistory>().is_none() {
            world.insert_resource(Self::default());
        }
    }

    /// Drops every recorded step but keeps the configured limits.
    ///
    /// Starting a new scene or opening a file clears the history this way. The
    /// entries name entities the previous document owned, and undoing one
    /// would edit a world those entities never belonged to.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.memory_used_bytes = 0;
    }

    /// Applies the limits the preferences set and trims while the history is
    /// over either of them.
    ///
    /// The returned outcome says how many entries that dropped and which limit
    /// caused it, so the caller can log it.
    pub fn set_limits(&mut self, step_cap: usize, memory_budget_bytes: usize) -> PushOutcome {
        self.step_cap = step_cap.max(1);
        self.memory_budget_bytes = memory_budget_bytes;
        let (dropped_step_count, trim_reason) = self.trim();
        PushOutcome {
            steps: self.entries.len(),
            trimmed: dropped_step_count,
            trim_reason,
            coalesced: false,
        }
    }

    /// Pushes one step, merging it into the entry before it when the two
    /// coalesce.
    ///
    /// `now_millis` is when the step arrived, and `coalesce_window` is how
    /// close two coalescing edits must be to merge. A push after an undo drops
    /// the redo tail, because the user has started a new branch.
    pub fn push(
        &mut self,
        mut incoming_entry: HistoryEntry,
        now_millis: u64,
        coalesce_window: Duration,
    ) -> PushOutcome {
        let mut outcome = PushOutcome::default();
        // Step 1: Decide whether this step merges into the newest applied
        // entry. Only a push at the end of the applied list merges; anything
        // after an undo starts a new branch.
        let merge_index = if self.cursor > 0 && self.cursor == self.entries.len() {
            Some(self.cursor - 1)
        } else {
            None
        };
        if let (Some(index), Some(key)) = (merge_index, incoming_entry.coalesce_key.as_deref()) {
            let newest_entry = &mut self.entries[index];
            let is_within_window = now_millis.saturating_sub(newest_entry.coalesced_at_millis)
                <= coalesce_window.as_millis() as u64;
            if newest_entry.coalesce_key.as_deref() == Some(key) && is_within_window {
                if let (
                    HistoryEntryKind::Engine { forward, .. },
                    HistoryEntryKind::Engine {
                        forward: new_forward,
                        inverse: new_inverse,
                    },
                ) = (&mut newest_entry.entry, &mut incoming_entry.entry)
                {
                    // Keep the original inverse, which restores the value the
                    // drag started from, and take the newest forward commands.
                    let merged_bytes = estimate_bytes(new_forward) + estimate_bytes(new_inverse);
                    self.memory_used_bytes = self
                        .memory_used_bytes
                        .saturating_sub(newest_entry.memory_bytes)
                        .saturating_add(merged_bytes);
                    newest_entry.memory_bytes = merged_bytes;
                    *forward = std::mem::take(new_forward);
                    for identifier in &incoming_entry.touched {
                        if !newest_entry.touched.contains(identifier) {
                            newest_entry.touched.push(identifier.clone());
                        }
                    }
                    newest_entry.coalesced_at_millis = now_millis;
                    outcome.coalesced = true;
                    outcome.steps = self.entries.len();
                    return outcome;
                }
            }
        }
        // Step 2: A new edit discards the redo tail the last undo left behind.
        if self.cursor < self.entries.len() {
            let dropped_entries: Vec<HistoryEntry> = self.entries.split_off(self.cursor);
            for entry in &dropped_entries {
                self.memory_used_bytes = self.memory_used_bytes.saturating_sub(entry.memory_bytes);
            }
        }
        // Step 3: Record the step, then enforce the limits, dropping the oldest
        // entries first.
        self.memory_used_bytes = self
            .memory_used_bytes
            .saturating_add(incoming_entry.memory_bytes);
        self.entries.push(incoming_entry);
        self.cursor = self.entries.len();
        let (dropped_step_count, trim_reason) = self.trim();
        outcome.trimmed = dropped_step_count;
        outcome.trim_reason = trim_reason;
        outcome.steps = self.entries.len();
        outcome
    }

    /// Drops the oldest entries until both limits hold.
    fn trim(&mut self) -> (usize, Option<TrimReason>) {
        let mut dropped_step_count = 0;
        let mut trim_reason = None;
        while self.entries.len() > self.step_cap
            || self.memory_used_bytes > self.memory_budget_bytes
        {
            let entry = self.entries.remove(0);
            self.memory_used_bytes = self.memory_used_bytes.saturating_sub(entry.memory_bytes);
            self.cursor = self.cursor.saturating_sub(1);
            dropped_step_count += 1;
            // Name the step cap first while both limits are still broken:
            // dropping for the count is the common case, so it must not be
            // reported as a memory trim.
            if self.entries.len() >= self.step_cap {
                trim_reason = Some(TrimReason::Steps);
            } else if trim_reason.is_none() {
                trim_reason = Some(TrimReason::Memory);
            }
        }
        (dropped_step_count, trim_reason)
    }

    /// Pops the newest applied step for undoing.
    pub fn pop_undo(&mut self) -> Option<HistoryEntry> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(self.entries[self.cursor].clone())
    }

    /// Pops the next unapplied step for redoing.
    pub fn pop_redo(&mut self) -> Option<HistoryEntry> {
        if self.cursor >= self.entries.len() {
            return None;
        }
        let entry = self.entries[self.cursor].clone();
        self.cursor += 1;
        Some(entry)
    }

    /// Puts the cursor back after an undo whose commands failed.
    ///
    /// The step is popped before its commands run. When one of them fails, the
    /// world still shows the edit, so the step must be undoable again rather
    /// than offered as redoable.
    pub fn undo_rollback(&mut self) {
        if self.cursor < self.entries.len() {
            self.cursor += 1;
        }
    }

    /// Puts the cursor back after a redo whose commands failed.
    pub fn redo_rollback(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Entries currently held (applied plus redoable).
    pub fn steps(&self) -> usize {
        self.entries.len()
    }

    /// Steps available to undo.
    pub fn undo_depth(&self) -> usize {
        self.cursor
    }

    /// Steps available to redo.
    pub fn redo_depth(&self) -> usize {
        self.entries.len() - self.cursor
    }

    /// Retained size in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.memory_used_bytes
    }

    /// Follows an entity to the fresh handle a restore assigned it.
    pub fn remap_entity(&mut self, previous: Entity, assigned: Entity) {
        for entry in &mut self.entries {
            entry.remap_entity(previous, assigned);
        }
    }

    /// Drops every entry that names an identifier `is_valid` rejects.
    ///
    /// `is_valid` is asked about `"component"` and `"component.field"`
    /// strings. One rejected identifier is enough to drop the whole entry,
    /// because undoing it would target storage that no longer exists.
    ///
    /// `registry_stamp` identifies the registry the check runs against: the
    /// caller hashes the registered component names and their fields. An
    /// unchanged stamp returns without walking the entries, so a caller can
    /// ask again on every frame.
    ///
    /// Returns how many entries were dropped.
    pub fn drop_stale(&mut self, is_valid: impl Fn(&str) -> bool, registry_stamp: u64) -> usize {
        if self.registry_stamp == registry_stamp {
            return 0;
        }
        self.registry_stamp = registry_stamp;
        let applied_count_before = self.cursor;
        let mut dropped_entry_count = 0;
        let mut kept_entries: Vec<HistoryEntry> = Vec::with_capacity(self.entries.len());
        let mut applied_count_kept = 0;
        for (index, entry) in std::mem::take(&mut self.entries).into_iter().enumerate() {
            if entry.touched.iter().any(|identifier| !is_valid(identifier)) {
                dropped_entry_count += 1;
                self.memory_used_bytes = self.memory_used_bytes.saturating_sub(entry.memory_bytes);
                continue;
            }
            if index < applied_count_before {
                applied_count_kept += 1;
            }
            kept_entries.push(entry);
        }
        self.entries = kept_entries;
        self.cursor = applied_count_kept;
        dropped_entry_count
    }
}

impl Default for CommandHistory {
    fn default() -> Self {
        Self::new(Self::DEFAULT_STEP_CAP, Self::DEFAULT_MEMORY_BUDGET_BYTES)
    }
}

impl pill_engine::Resource for CommandHistory {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Estimates how many bytes a batch keeps alive.
///
/// The number only decides when the memory budget drops the oldest entries,
/// so it does not have to be exact: a fixed cost per command plus the bytes
/// its strings and arrays hold is close enough for that decision.
pub fn estimate_bytes(commands: &[EditorCommand]) -> usize {
    const PER_COMMAND_BYTES: usize = 96;
    let mut bytes = commands.len() * PER_COMMAND_BYTES;
    for command in commands {
        let payload_bytes = match command {
            EditorCommand::SetField {
                component,
                field,
                value,
                ..
            }
            | EditorCommand::SetArrayElement {
                component,
                field,
                value,
                ..
            } => component.len() + field.len() + field_value_bytes(value),
            EditorCommand::SetFieldAsset {
                component,
                field,
                reference,
                ..
            } => component.len() + field.len() + reference.as_ref().map_or(0, String::len),
            EditorCommand::SetFieldEntity {
                component, field, ..
            } => component.len() + field.len(),
            EditorCommand::ArmEntityPick {
                component, field, ..
            } => component.len() + field.len(),
            EditorCommand::AddComponent { component, .. }
            | EditorCommand::RemoveComponent { component, .. }
            | EditorCommand::RestoreComponent { component, .. } => component.len(),
            EditorCommand::CreateEntity { components }
            | EditorCommand::RestoreEntity { components, .. } => components
                .iter()
                .map(|seed| {
                    seed.type_name.len()
                        + seed
                            .fields
                            .iter()
                            .map(|(name, value)| name.len() + field_value_bytes(value))
                            .sum::<usize>()
                })
                .sum(),
            EditorCommand::DestroyEntity { .. }
            | EditorCommand::SetSystemEnabled { .. }
            | EditorCommand::SetPaused { .. }
            | EditorCommand::EnterPlay
            | EditorCommand::ExitPlay
            | EditorCommand::SetGizmoTool { .. }
            | EditorCommand::SetGrid { .. }
            | EditorCommand::CycleGizmoSpace
            | EditorCommand::StepOnce => 0,
            // An element move carries one transform per element: sixteen
            // floats each, plus the kind's name.
            EditorCommand::SetElementTransforms { kind, moves, .. } => {
                kind.len() + moves.len() * std::mem::size_of::<crate::tools::ElementMove>()
            }
            EditorCommand::SetPreferences { values } => values
                .iter()
                .map(|(key, text)| key.len() + text.len())
                .sum(),
            EditorCommand::ImportFiles {
                sources,
                destination,
            } => sources.iter().map(String::len).sum::<usize>() + destination.len(),
            EditorCommand::RemoveFiles {
                paths,
                sources,
                destination,
            } => {
                paths.iter().map(String::len).sum::<usize>()
                    + sources.iter().map(String::len).sum::<usize>()
                    + destination.len()
            }
            // Asset commands cost what their paths and names hold, except
            // the restore, which carries the deleted file's old content: that
            // copy is the bulk of what the memory budget counts.
            EditorCommand::CreateStandaloneAsset {
                type_name,
                folder,
                name,
                ..
            } => type_name.len() + folder.len() + name.len(),
            EditorCommand::DeleteAssetFile { path } | EditorCommand::WriteAssetMeta { path } => {
                path.len()
            }
            EditorCommand::RestoreAssetFile { path, bytes, meta } => {
                path.len() + bytes.len() + meta.as_ref().map(Vec::len).unwrap_or(0)
            }
            EditorCommand::MoveAssetFile { from, to } => from.len() + to.len(),
            EditorCommand::SetAssetSettings { path, settings } => {
                path.len() + settings.to_string().len()
            }
            EditorCommand::SetResourceField {
                resource,
                field,
                value,
            } => resource.len() + field.len() + field_value_bytes(value),
            EditorCommand::SetSelection { entities, asset } => {
                entities.len() * 16 + asset.as_ref().map(String::len).unwrap_or(0)
            }
            EditorCommand::SetElementSelection { kind, elements } => {
                kind.len() + elements.len() * 24
            }
            EditorCommand::SelectResource { resource } => {
                resource.as_ref().map(String::len).unwrap_or(0)
            }
            EditorCommand::RestoreSelection {
                entities,
                asset,
                resource,
                elements,
            } => {
                entities.len() * 16
                    + asset.as_ref().map(String::len).unwrap_or(0)
                    + resource.as_ref().map(String::len).unwrap_or(0)
                    + elements.kind().len()
                    + elements.elements().len() * 24
            }
            // Opening and saving a scene is never recorded, so it costs
            // nothing; this arm exists only to cover the commands.
            EditorCommand::SceneNew | EditorCommand::SceneSave => 0,
            EditorCommand::SceneOpen { path } | EditorCommand::SceneSaveAs { path } => path.len(),
        };
        bytes += payload_bytes;
    }
    bytes
}

/// Returns the bytes one field value holds: a string's length, an array's
/// elements added up, or a fixed eight for a plain number.
fn field_value_bytes(field_value: &FieldValue) -> usize {
    match field_value {
        FieldValue::Text(text) => text.len(),
        FieldValue::Array(elements) => elements.iter().map(field_value_bytes).sum(),
        _ => 8,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::EditorCommand;

    /// A field edit entry over a named component, for history-only tests.
    fn field_entry(label: &str, field_value: f32) -> HistoryEntry {
        let command = EditorCommand::SetField {
            entity: Entity::default(),
            component: "Demo".to_string(),
            field: "strength".to_string(),
            value: FieldValue::F32(field_value),
        };
        HistoryEntry::engine(label, vec![command.clone()], vec![command])
    }

    /// The coalesce window these tests use, in milliseconds.
    const COALESCE_WINDOW: Duration = Duration::from_millis(400);

    #[test]
    fn the_memory_budget_counts_asset_pre_images() {
        // Undoing an asset delete needs the whole file back, so that copy
        // is the memory the budget must count.
        let deleted_file_bytes = vec![0u8; 64 * 1024];
        let forward = EditorCommand::DeleteAssetFile {
            path: "textures/helmet.png".to_string(),
        };
        let inverse = EditorCommand::RestoreAssetFile {
            path: "textures/helmet.png".to_string(),
            bytes: deleted_file_bytes.clone(),
            meta: Some(b"{\"guid\":\"abc\"}".to_vec()),
        };
        let entry = HistoryEntry::engine("delete asset", vec![forward], vec![inverse]);
        assert!(
            entry.memory_bytes >= deleted_file_bytes.len(),
            "the pre-image must land in memory_bytes ({} < {})",
            entry.memory_bytes,
            deleted_file_bytes.len()
        );

        let mut history = CommandHistory::new(10, 32 * 1024);
        history.push(entry.clone(), 0, COALESCE_WINDOW);
        assert_eq!(
            history.steps(),
            0,
            "an entry whose pre-image exceeds the budget is dropped"
        );
        // The same entry fits a larger budget: the drop above was the byte
        // count, not the recording.
        let mut roomy_history = CommandHistory::new(10, usize::MAX);
        roomy_history.push(entry, 0, COALESCE_WINDOW);
        assert_eq!(roomy_history.steps(), 1);
        assert!(roomy_history.memory_bytes() >= deleted_file_bytes.len());
    }

    #[test]
    fn push_undo_redo_round_trip() {
        let mut history = CommandHistory::new(10, usize::MAX);
        history.push(field_entry("one", 1.0), 0, COALESCE_WINDOW);
        history.push(field_entry("two", 2.0), 1000, COALESCE_WINDOW);
        assert_eq!(history.steps(), 2);
        assert_eq!(history.undo_depth(), 2);

        let undone = history.pop_undo().expect("second step");
        assert_eq!(undone.label, "two");
        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.redo_depth(), 1);

        let redone = history.pop_redo().expect("redo");
        assert_eq!(redone.label, "two");
        assert_eq!(history.redo_depth(), 0);
        assert!(history.pop_redo().is_none());
    }

    #[test]
    fn push_after_undo_drops_the_redo_tail() {
        let mut history = CommandHistory::new(10, usize::MAX);
        history.push(field_entry("one", 1.0), 0, COALESCE_WINDOW);
        history.push(field_entry("two", 2.0), 1000, COALESCE_WINDOW);
        history.pop_undo();
        history.push(field_entry("three", 3.0), 2000, COALESCE_WINDOW);
        assert_eq!(history.steps(), 2);
        assert_eq!(history.redo_depth(), 0);
        assert_eq!(history.undo_depth(), 2);
    }

    #[test]
    fn continuous_edits_coalesce_into_one_step() {
        let mut history = CommandHistory::new(10, usize::MAX);
        // A slider drag: same key, edits close together in time.
        for step_index in 0..5 {
            let entry =
                field_entry("drag", step_index as f32).coalescing("slider", step_index * 50);
            history.push(entry, step_index * 50, COALESCE_WINDOW);
        }
        // One step; the inverse still restores the drag's starting value.
        assert_eq!(history.steps(), 1);
        let entry = history.pop_undo().expect("drag step");
        let HistoryEntryKind::Engine { forward, inverse } = entry.entry else {
            panic!("engine entry expected");
        };
        assert_eq!(
            forward,
            vec![EditorCommand::SetField {
                entity: Entity::default(),
                component: "Demo".to_string(),
                field: "strength".to_string(),
                value: FieldValue::F32(4.0),
            }]
        );
        assert_eq!(
            inverse,
            vec![EditorCommand::SetField {
                entity: Entity::default(),
                component: "Demo".to_string(),
                field: "strength".to_string(),
                value: FieldValue::F32(0.0),
            }]
        );

        // A gap longer than the window splits the drag into a new step.
        history.push(
            field_entry("drag", 9.0).coalescing("slider", 5000),
            5000,
            COALESCE_WINDOW,
        );
        assert_eq!(history.steps(), 1);
    }

    #[test]
    fn step_cap_trims_oldest_first() {
        let mut history = CommandHistory::new(3, usize::MAX);
        for step_index in 0..5 {
            history.push(
                field_entry("step", step_index as f32),
                step_index as u64 * 1000,
                COALESCE_WINDOW,
            );
        }
        assert_eq!(history.steps(), 3);
        let outcome = history.push(field_entry("more", 9.0), 6000, COALESCE_WINDOW);
        assert_eq!(outcome.trimmed, 1);
        assert_eq!(outcome.trim_reason, Some(TrimReason::Steps));
    }

    #[test]
    fn memory_budget_trims_with_its_own_reason() {
        let mut history = CommandHistory::new(100, 2048);
        for step_index in 0..8 {
            let command = EditorCommand::SetField {
                entity: Entity::default(),
                component: "Demo".to_string(),
                field: "name".to_string(),
                value: FieldValue::Text("x".repeat(400)),
            };
            let entry = HistoryEntry::engine("big", vec![command.clone()], vec![command]);
            history.push(entry, step_index as u64 * 1000, COALESCE_WINDOW);
        }
        assert!(history.memory_bytes() <= 2048);
        assert!(history.steps() < 8);
    }

    #[test]
    fn drop_stale_removes_invalid_entries_and_keeps_the_cursor() {
        let mut history = CommandHistory::new(10, usize::MAX);
        history.push(field_entry("keep one", 1.0), 0, COALESCE_WINDOW);
        history.push(
            HistoryEntry::engine(
                "stale",
                vec![EditorCommand::SetField {
                    entity: Entity::default(),
                    component: "Gone".to_string(),
                    field: "value".to_string(),
                    value: FieldValue::F32(2.0),
                }],
                Vec::new(),
            ),
            1000,
            COALESCE_WINDOW,
        );
        history.push(field_entry("keep two", 3.0), 2000, COALESCE_WINDOW);
        history.pop_undo(); // "keep two" is now unapplied.
        let dropped = history.drop_stale(|identifier| !identifier.starts_with("Gone"), 1);
        assert_eq!(dropped, 1);
        assert_eq!(history.steps(), 2);
        assert_eq!(history.undo_depth(), 1);
        assert_eq!(history.redo_depth(), 1);
        // The same registry stamp reports nothing the second time, so the
        // prune runs once per registry change rather than once per call.
        assert_eq!(history.drop_stale(|_| false, 1), 0);
        assert_eq!(history.drop_stale(|_| false, 2), 2);
    }

    #[test]
    fn remap_follows_a_restored_entity() {
        let previous_entity = Entity::default();
        let assigned_entity = Entity::default();
        let mut history = CommandHistory::new(10, usize::MAX);
        history.push(
            HistoryEntry::engine(
                "on the restored entity",
                vec![EditorCommand::SetField {
                    entity: previous_entity,
                    component: "Demo".to_string(),
                    field: "strength".to_string(),
                    value: FieldValue::F32(1.0),
                }],
                vec![EditorCommand::SetField {
                    entity: previous_entity,
                    component: "Demo".to_string(),
                    field: "strength".to_string(),
                    value: FieldValue::F32(0.0),
                }],
            ),
            0,
            COALESCE_WINDOW,
        );
        history.remap_entity(previous_entity, assigned_entity);
        let entry = history.pop_undo().expect("entry");
        let HistoryEntryKind::Engine { forward, inverse } = entry.entry else {
            panic!("engine entry expected");
        };
        for command in forward.iter().chain(inverse.iter()) {
            let EditorCommand::SetField { entity, .. } = command else {
                panic!("set field expected");
            };
            assert_eq!(*entity, assigned_entity);
        }
    }

    #[test]
    fn selection_entries_round_trip() {
        let mut history = CommandHistory::new(10, usize::MAX);
        let before = Entity::default();
        let after = Entity::default();
        history.push(
            HistoryEntry::selection(Some(before), Some(after)),
            0,
            COALESCE_WINDOW,
        );
        let entry = history.pop_undo().expect("selection step");
        match entry.entry {
            HistoryEntryKind::Selection {
                before: carried_before,
                after: carried_after,
            } => {
                assert_eq!(carried_before, Some(before));
                assert_eq!(carried_after, Some(after));
            }
            HistoryEntryKind::Engine { .. } => panic!("selection entry expected"),
        }
    }

    #[test]
    fn limits_shrink_trims_existing_entries() {
        let mut history = CommandHistory::new(10, usize::MAX);
        for step_index in 0..6 {
            history.push(
                field_entry("step", step_index as f32),
                step_index as u64 * 1000,
                COALESCE_WINDOW,
            );
        }
        let outcome = history.set_limits(2, usize::MAX);
        assert_eq!(history.steps(), 2);
        assert_eq!(outcome.trimmed, 4);
    }
}
