//! Who is changing the world, for the lifecycle methods that report it.
//!
//! # Responsibilities
//!
//! - Defines [`ChangeOrigin`], the label a structural change carries while it
//!   is made.
//! - Provides `World::with_change_origin` and `Engine::with_change_origin`, the
//!   scopes that set it and put the previous one back.
//! - Translates an origin into the cause a lifecycle method reports, so the
//!   components never learn the editor's vocabulary.
//!
//! # Design
//!
//! The origin is world state, not a parameter of every structural call: the
//! command executors, the eager API and the scene loader all reach the same
//! field, and a nested scope restores what it found. The default is
//! [`ChangeOrigin::Simulation`], which is what game code gets for free and what
//! a shipping build never changes.
//!
//! A scope only labels what happens inside it. The attachment and removal
//! passes read the origin when they run, which is inside the scope that made
//! the change: the editor wraps the batch application *and* its flush, so a
//! command queued under `Authoring` is executed under `Authoring` too.

use super::*;
use crate::scripting::{CreateCause, DestroyCause};

// =============================================================================
// ChangeOrigin
// =============================================================================

/// Who is changing the world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChangeOrigin {
    /// Game code, through the frame's command queue or the eager world API.
    /// The default, and the only origin a shipping build produces.
    #[default]
    Simulation,
    /// Code outside the simulation: the editor, a tool, a test.
    Authoring,
    /// An undo or a redo replaying a recorded step.
    HistoryRestore,
    /// A paste or a duplicate of an existing entity.
    Duplicate {
        /// The entity the copy was made from, when the caller knows it. A paste
        /// carries only the decoded document, so it names none.
        source: Option<Entity>,
    },
    /// Loading a scene into the world.
    SceneLoad,
    /// Stopping play, which restores the edit world's components.
    PlayStopRestore,
}

impl ChangeOrigin {
    /// The cause a component created under this origin reports.
    pub(crate) fn create_cause(self) -> CreateCause {
        match self {
            Self::Simulation => CreateCause::Spawned,
            Self::Authoring => CreateCause::Authored,
            Self::HistoryRestore => CreateCause::HistoryRestored,
            Self::Duplicate { .. } => CreateCause::Duplicated,
            Self::SceneLoad => CreateCause::SceneLoaded,
            Self::PlayStopRestore => CreateCause::PlayStopRestored,
        }
    }

    /// The entity a duplicated component was copied from, when the origin names
    /// one.
    pub(crate) fn duplicated_from(self) -> Option<Entity> {
        match self {
            Self::Duplicate { source } => source,
            _ => None,
        }
    }

    /// The cause a component destroyed under this origin reports.
    pub(crate) fn destroy_cause(self) -> DestroyCause {
        match self {
            Self::Simulation => DestroyCause::Despawned,
            Self::Authoring => DestroyCause::Authored,
            Self::HistoryRestore => DestroyCause::HistoryRestored,
            Self::Duplicate { .. } => DestroyCause::Despawned,
            Self::SceneLoad => DestroyCause::SceneUnloaded,
            Self::PlayStopRestore => DestroyCause::PlayStopRestored,
        }
    }
}

// =============================================================================
// World - Change Origin
// =============================================================================

impl World {
    /// The origin every structural change is currently labelled with.
    pub fn change_origin(&self) -> ChangeOrigin {
        self.change_origin
    }

    /// Sets the origin structural changes are labelled with.
    ///
    /// [`Self::with_change_origin`] is the form that puts the previous value
    /// back; this one is for a caller that manages the pair itself.
    pub(crate) fn set_change_origin(&mut self, origin: ChangeOrigin) {
        self.change_origin = origin;
    }

    /// Runs `run` with `origin` as the world's change origin.
    ///
    /// The previous origin is put back when `run` returns, so nested scopes
    /// layer. A panic inside `run` leaves the scoped value in place: the
    /// process is unwinding out of a frame, which the engine does not continue
    /// from.
    pub fn with_change_origin<R>(
        &mut self,
        origin: ChangeOrigin,
        run: impl FnOnce(&mut World) -> R,
    ) -> R {
        let previous = self.change_origin;
        self.change_origin = origin;
        let result = run(self);
        self.change_origin = previous;
        result
    }
}
