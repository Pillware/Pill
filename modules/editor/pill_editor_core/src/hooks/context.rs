//! What a callback reads and queues while it runs.
//!
//! # Responsibilities
//!
//! - Give a callback the world, the shared selection and the cause of the edit
//!   it answers ([`HookContext`]).
//! - Hold the commands a callback queues, so they join the batch that is being
//!   applied.
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

// External crates
use pill_engine::{Resource, World};

// Current crate
use crate::editor_state::{EditorCommand, EditorSelection};
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
        Some("pill_editor_core::hooks::HookNotices")
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
    /// The commands the callback queued for the same undo step.
    followups: &'a mut Vec<EditorCommand>,
    /// The notices the callback posted.
    notices: &'a mut Vec<HookNotice>,
    /// What made the edit happen.
    cause: FieldEditCause,
}

impl<'a> HookContext<'a> {
    /// Builds the context one call sees.
    pub(crate) fn new(
        world: &'a mut World,
        selection: &'a EditorSelection,
        followups: &'a mut Vec<EditorCommand>,
        notices: &'a mut Vec<HookNotice>,
        cause: FieldEditCause,
    ) -> Self {
        Self {
            world,
            selection,
            followups,
            notices,
            cause,
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
            return;
        }
        self.followups.push(command);
    }

    /// Posts a notice the editor shows as a toast.
    pub fn post_notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.notices.push(HookNotice {
            level,
            text: text.into(),
        });
    }
}
