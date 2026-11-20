//! The editor's half of a play session: the state a game must not touch.
//!
//! # Responsibilities
//!
//! - Define [`EditorPlaySession`]: the resource that keeps the undo history, the
//!   selection and the scene's unsaved marker aside while a game runs, and puts
//!   them back when it stops.
//! - Define [`HeldEditorState`]: the three values a session keeps.
//!
//! # Design
//!
//! The engine copies the world's entities and its game-state resources at Play,
//! and the editor's own state is `PlayState::Keep`, which means the engine leaves
//! it alone. That is the right choice for the shell and the dock - a session must
//! not blank the panels - and the wrong one for three values that describe the
//! *document*: a step the game recorded, a selection it changed and the unsaved
//! marker it set all describe a world that Stop throws away.
//!
//! So the editor keeps them itself. The history is *taken*, not copied: a
//! `CommandHistory` is the undo stack with its limits, and an empty stack in its
//! place means a play-time edit records somewhere harmless. The selection and the
//! marker are small plain values, so they are copied.
//!
//! The resource is `Keep` too, and that is deliberate: it belongs to the editor,
//! and the values it holds outlive the session by definition.

// External crates
use pill_engine::resource::PlayState;
use pill_engine::{Resource, World};

// Current crate
use crate::editor_state::EditorSelection;
use crate::history::CommandHistory;
use crate::scene_workflow::SceneSession;

// =============================================================================
// HeldEditorState
// =============================================================================

/// What a play session keeps aside and puts back at Stop.
#[derive(Default)]
pub struct HeldEditorState {
    /// The undo stack as it was before Play. The live stack is empty while the
    /// session runs, so a play-time edit cannot reach the document's history.
    pub history: CommandHistory,
    /// The selection as it was before Play. A game that selects something must
    /// not leave the Inspector pointed at an entity that Stop destroys.
    pub selection: EditorSelection,
    /// Whether the open scene was unsaved before Play.
    pub scene_dirty: bool,
}

// =============================================================================
// EditorPlaySession
// =============================================================================

/// The editor state a play session holds, if one is running.
#[derive(Default)]
pub struct EditorPlaySession {
    /// The held state; `None` between sessions.
    held: Option<HeldEditorState>,
}

impl Resource for EditorPlaySession {
    /// Editor state, not game state: the session holds it across Play and Stop
    /// itself, and the engine must not copy or restore it.
    fn play_state() -> PlayState {
        PlayState::Keep
    }
}

impl EditorPlaySession {
    /// Registers the resource, and keeps the value already in the world, so a
    /// reload does not forget a running session's held state.
    pub fn ensure(world: &mut World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }

    /// Whether a session is holding editor state.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.held.is_some()
    }

    /// Takes the editor state out of the world and keeps it.
    ///
    /// Call it before the engine copies the world, so the copy a game runs
    /// against cannot hold the document's history.
    pub fn save(world: &mut World) {
        Self::ensure(world);
        let held_history = match world.get_resource_mut::<CommandHistory>() {
            Some(history) => std::mem::take(&mut *history),
            None => CommandHistory::default(),
        };
        let held_selection = world
            .get_resource::<EditorSelection>()
            .cloned()
            .unwrap_or_default();
        let scene_dirty = world
            .get_resource::<SceneSession>()
            .is_some_and(|scene| scene.dirty);
        let Some(session) = world.get_resource_mut::<EditorPlaySession>() else {
            return;
        };
        session.held = Some(HeldEditorState {
            history: held_history,
            selection: held_selection,
            scene_dirty,
        });
    }

    /// Puts the held editor state back, and drops it.
    ///
    /// Returns whether a session was holding anything. The history that took
    /// the session's place is dropped, so a play-time step is never in the
    /// document's stack.
    pub fn restore(world: &mut World) -> bool {
        // The taken state is read out in its own scope, so the borrow of the
        // session ends before the world is borrowed again.
        let Some(held) = world
            .get_resource_mut::<EditorPlaySession>()
            .and_then(|session| session.held.take())
        else {
            return false;
        };
        if let Some(history) = world.get_resource_mut::<CommandHistory>() {
            *history = held.history;
        } else {
            world.insert_resource(held.history);
        }
        if let Some(selection) = world.get_resource_mut::<EditorSelection>() {
            *selection = held.selection;
        } else {
            world.insert_resource(held.selection);
        }
        if let Some(scene) = world.get_resource_mut::<SceneSession>() {
            scene.dirty = held.scene_dirty;
        }
        true
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::EditorSelection;
    use crate::history::HistoryEntry;

    /// A session takes the history and the selection, keeps a play-time step out
    /// of the document's stack, and puts both back at Stop.
    #[test]
    fn a_session_puts_the_editor_state_back() {
        let mut world = World::new();
        EditorPlaySession::ensure(&mut world);
        CommandHistory::ensure(&mut world);
        SceneSession::ensure(&mut world);
        world.insert_resource(EditorSelection::default());
        // One document step, and an open scene with unsaved edits.
        if let Some(history) = world.get_resource_mut::<CommandHistory>() {
            let entry = HistoryEntry::engine("new entity", Vec::new(), Vec::new());
            history.push(entry, 0, std::time::Duration::ZERO);
        }
        if let Some(scene) = world.get_resource_mut::<SceneSession>() {
            scene.dirty = true;
        }

        EditorPlaySession::save(&mut world);
        assert!(world
            .get_resource::<EditorPlaySession>()
            .is_some_and(EditorPlaySession::is_holding));
        // The live stack starts empty while the session runs.
        assert_eq!(
            world
                .get_resource::<CommandHistory>()
                .map(CommandHistory::steps),
            Some(0)
        );

        // A play-time edit records into the empty stack, clears the marker and
        // moves the selection.
        if let Some(history) = world.get_resource_mut::<CommandHistory>() {
            let entry = HistoryEntry::engine("play step", Vec::new(), Vec::new());
            history.push(entry, 0, std::time::Duration::ZERO);
        }
        if let Some(scene) = world.get_resource_mut::<SceneSession>() {
            scene.dirty = false;
        }
        if let Some(selection) = world.get_resource_mut::<EditorSelection>() {
            selection.asset = Some("models/play_time.obj".to_string());
        }

        assert!(EditorPlaySession::restore(&mut world));
        assert_eq!(
            world
                .get_resource::<CommandHistory>()
                .map(CommandHistory::steps),
            Some(1),
            "the document's own stack comes back, without the play-time step"
        );
        assert!(!world
            .get_resource::<EditorPlaySession>()
            .is_some_and(EditorPlaySession::is_holding));
        assert_eq!(
            world
                .get_resource::<SceneSession>()
                .map(|scene| scene.dirty),
            Some(true),
            "the unsaved marker comes back as it was"
        );
        assert_eq!(
            world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.asset.clone()),
            None,
            "the selection comes back as it was, not where the session left it"
        );
    }
}
