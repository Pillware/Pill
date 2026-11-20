//! What a callback reads and queues while it runs.
//!
//! # Responsibilities
//!
//! - Give a callback the world, the shared selection and the cause of the edit
//!   it answers ([`HookContext`]).
//! - Hold the commands a callback queues, so they join the batch that is being
//!   applied ([`HookQueue`]).
//! - Wrap the editor's own recorded actions as builders, so a callback changes
//!   the editor the way the UI does instead of assembling commands.
//! - Collect the notices callbacks post ([`HookNotices`]) for the executable to
//!   fold into the editor's toast list.
//!
//! # Design
//!
//! The context is borrowed for one call and names no type from the registering
//! module, so nothing it holds can dangle when that module unloads. A command
//! queued under [`FieldEditCause::HistoryRestored`] is dropped: the replayed
//! step already holds its result, and applying it again would double the edit.
//! Notices are still collected under a replay, because they are feedback about
//! what the callback saw.
//!
//! The builders queue the same commands the editor's own rows queue, so a
//! callback cannot invent an edit the editor has no history for. An edit joins
//! the step being applied; a control (a tool, a space, snapping) never does,
//! because a control is not something undo should walk back over.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{Entity, Resource, World};

// Current crate
use crate::editor_state::{EditorCommand, EditorSelection};
use crate::gizmo::{GizmoMode, GizmoSpace};
use crate::hooks::events::FieldEditCause;
use crate::notices::NoticeLevel;

// =============================================================================
// Notices
// =============================================================================

/// One notice a callback posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookNotice {
    /// How serious the notice is.
    pub level: NoticeLevel,
    /// The line the editor shows.
    pub text: String,
}

/// The notices this frame's callbacks posted, waiting for the executable.
///
/// The executable republishes [`EditorNotices`](crate::notices::EditorNotices)
/// from its own copy, so a notice written there directly would be erased; it
/// drains this queue and pushes each entry through the same toast path its own
/// notices take. The browser writes the editor's resource directly.
#[derive(Debug, Default)]
pub struct HookNotices {
    /// The notices, oldest first.
    notices: Vec<HookNotice>,
}

impl HookNotices {
    /// Queues one notice.
    pub fn push(&mut self, notice: HookNotice) {
        self.notices.push(notice);
    }

    /// Takes every queued notice and leaves the queue empty.
    pub fn drain(&mut self) -> Vec<HookNotice> {
        std::mem::take(&mut self.notices)
    }

    /// Registers the resource, inserting a default only when the world has
    /// none.
    pub fn ensure(world: &mut World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl Resource for HookNotices {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_editor_core::HookNotices")
    }
}

// =============================================================================
// The queue
// =============================================================================

/// The commands the callbacks of one drain queued.
///
/// The edits go into the batch that is being applied, so one undo step holds
/// them with the command that caused them. The controls (a tool, a space,
/// snapping) are not edits: they run as an unrecorded batch of their own, which
/// is what the toolbox's rows queue.
#[derive(Debug, Default)]
pub(crate) struct HookQueue {
    /// The edits, in the order their callbacks queued them.
    edits: Vec<EditorCommand>,
    /// The controls, in the order their callbacks queued them.
    controls: Vec<EditorCommand>,
    /// The source of the first callback that queued an edit, which names the
    /// batch a callback answering a diff needed.
    source: Option<String>,
    /// Whether a replay's dropped command has already been reported.
    reported_drop: bool,
}

impl HookQueue {
    /// Queues one edit for the batch that is applying.
    fn push_edit(&mut self, command: EditorCommand, source: &str) {
        self.source.get_or_insert_with(|| source.to_string());
        self.edits.push(command);
    }

    /// Queues one control, which never joins an undo step.
    fn push_control(&mut self, command: EditorCommand) {
        self.controls.push(command);
    }

    /// Reports the first command a replay dropped.
    ///
    /// A replay fires every callback of the edit, so a line per dropped command
    /// would bury the log; one line per drain says what happened.
    fn report_drop(&mut self, source: &str) {
        if self.reported_drop {
            return;
        }
        self.reported_drop = true;
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            source,
            "[editor] hook command dropped"
        );
    }

    /// Takes the queued edits and the callback that queued the first one.
    pub(crate) fn take_edits(&mut self) -> (Vec<EditorCommand>, Option<String>) {
        let commands = std::mem::take(&mut self.edits);
        let source = self.source.take();
        (commands, source)
    }

    /// Takes the queued controls.
    pub(crate) fn take_controls(&mut self) -> Vec<EditorCommand> {
        std::mem::take(&mut self.controls)
    }

    /// Whether a replay's dropped command has been reported for this queue.
    #[cfg(test)]
    pub(crate) fn drop_reported(&self) -> bool {
        self.reported_drop
    }
}

// =============================================================================
// The context
// =============================================================================

/// What a callback reads and queues while it runs.
///
/// The world is the live one, so a callback reads any component and resource
/// the editor holds. The selection is a copy taken before the call: a callback
/// cannot change it through this view, because a selection change is a recorded
/// command.
pub struct HookContext<'a> {
    /// The live world.
    world: &'a mut World,
    /// The shared selection as it was when the call began.
    selection: &'a EditorSelection,
    /// The commands the callback queued.
    queue: &'a mut HookQueue,
    /// The notices the callback posted.
    notices: &'a mut Vec<HookNotice>,
    /// What made the edit happen.
    cause: FieldEditCause,
    /// The callback this context belongs to, as it registered.
    source: &'a str,
}

impl<'a> HookContext<'a> {
    /// Builds the context one call sees.
    pub(crate) fn new(
        world: &'a mut World,
        selection: &'a EditorSelection,
        queue: &'a mut HookQueue,
        notices: &'a mut Vec<HookNotice>,
        cause: FieldEditCause,
        source: &'a str,
    ) -> Self {
        Self {
            world,
            selection,
            queue,
            notices,
            cause,
            source,
        }
    }

    /// The live world.
    pub fn world(&self) -> &World {
        self.world
    }

    /// The live world, for a callback that edits it directly.
    pub fn world_mut(&mut self) -> &mut World {
        self.world
    }

    /// The shared selection as it was when the call began.
    pub fn selection(&self) -> &EditorSelection {
        self.selection
    }

    /// The gizmo's settings, when the world holds a gizmo.
    pub fn gizmo(&self) -> Option<&crate::gizmo::GizmoState> {
        self.world.get_resource::<crate::gizmo::GizmoState>()
    }

    /// The active tool's name, when a tool is active.
    pub fn active_tool(&self) -> Option<String> {
        self.world
            .get_resource::<crate::tools::ToolRegistry>()
            .and_then(|registry| registry.active().map(str::to_string))
    }

    /// The editor's viewports as the executable last published them.
    ///
    /// The empty posture when nothing published one, so a callback can read
    /// this unconditionally.
    pub fn viewport(&self) -> crate::viewport::EditorViewportState {
        self.world
            .get_resource::<crate::viewport::EditorViewportState>()
            .copied()
            .unwrap_or_default()
    }

    /// What made the edit happen.
    pub fn cause(&self) -> FieldEditCause {
        self.cause
    }

    /// Queues a command to run right after the edit being reported, inside the
    /// same undo step.
    ///
    /// Dropped when the edit is a history replay: the step already holds the
    /// result. A command queued this way never fires the callbacks again.
    pub fn queue(&mut self, command: EditorCommand) {
        if self.cause == FieldEditCause::HistoryRestored {
            self.queue.report_drop(self.source);
            return;
        }
        self.queue.push_edit(command, self.source);
    }

    /// Queues a gizmo change: the tool, its space or its snapping.
    ///
    /// Not recorded: a tool choice is a control, not an edit, so it adds no
    /// history step and undo never walks back over it. A field left as `None`
    /// keeps its current value. Dropped under a replay, like every other
    /// queued command.
    pub fn queue_gizmo(
        &mut self,
        mode: Option<GizmoMode>,
        space: Option<GizmoSpace>,
        snap: Option<bool>,
    ) {
        if self.cause == FieldEditCause::HistoryRestored {
            self.queue.report_drop(self.source);
            return;
        }
        self.queue
            .push_control(EditorCommand::SetGizmoTool { mode, space, snap });
    }

    /// Queues the recorded attach of a zero-initialised component, under the
    /// Inspector's `add {short}` label.
    ///
    /// The command joins the batch that is applying, so one undo removes the
    /// component together with the edit that added it.
    pub fn queue_add_component(&mut self, entity: Entity, component: &str) {
        let (command, _label) = crate::inspector::component_add(entity, component);
        self.queue(command);
    }

    /// Queues the recorded detach of a component, under the Inspector's
    /// `remove {short}` label.
    ///
    /// Removing an entity's last component destroys the entity, exactly as the
    /// Inspector's own remove does.
    pub fn queue_remove_component(&mut self, entity: Entity, component: &str) {
        let (command, _label) = crate::inspector::component_remove(entity, component);
        self.queue(command);
    }

    /// Queues the recorded creation of one empty entity, under the Entities
    /// tab's `new entity` label.
    pub fn queue_create_entity(&mut self) {
        let (command, _label) = crate::entities::entity_create();
        self.queue(command);
    }

    /// Queues the recorded destroy of one entity, under the Entities tab's
    /// `delete entity` label.
    ///
    /// The selection clears first, as the tab's own delete does, so one undo
    /// restores the entity and the selection that pointed at it. A handle is
    /// valid for this call only: undoing the destroy assigns a fresh one.
    pub fn queue_destroy_entity(&mut self, entity: Entity) {
        let (commands, _label) = crate::entities::entity_delete(entity, self.selected_asset());
        for command in commands {
            self.queue(command);
        }
    }

    /// Queues a recorded selection change.
    ///
    /// The selected asset is kept, so selecting entities does not drop the file
    /// the Assets panel shows. One undo puts the previous selection back.
    pub fn queue_selection(&mut self, entities: Vec<Entity>) {
        let asset = self.selected_asset();
        self.queue(EditorCommand::SetSelection { entities, asset });
    }

    /// The selected asset's path, which the selection commands keep.
    fn selected_asset(&self) -> Option<String> {
        self.selection.asset.clone()
    }

    /// Posts a notice the editor shows as a toast.
    pub fn post_notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.notices.push(HookNotice {
            level,
            text: text.into(),
        });
    }
}
