//! The world resource that holds the pipeline the renderer runs.
//!
//! # Responsibilities
//!
//! - Hold one optional pipeline handle, and nothing else. It is the pipeline
//!   the renderer should run: the game's choice, or the default that
//!   [`crate::register`] installs.
//! - Cross the module boundary: a project DLL sets the pipeline, the host's
//!   renderer executes it, and both sides have to agree on one resource.
//!
//! # Design
//!
//! [`crate::register`] inserts this resource when it is missing, so it exists
//! before any project code runs and code that sets a pipeline finds it already
//! there. The resource id the host binary and a project DLL both use is the
//! shared name that `#[pill_mirror_resource]` gives the type; `RenderFrame`
//! shares its id the same way. Without a shared name the renderer would read a
//! resource the project never wrote. C#'s generated `RenderingManager` marker
//! carries the same name, so a managed system declares
//! `ResMut<RenderingManager>` and calls `Clear()`.

// External crates
use pill_engine::{pill_mirror_impl, pill_mirror_method, pill_mirror_resource, Handle};

// Current crate
use crate::RenderingPipeline;

// =============================================================================
// RenderingManager
// =============================================================================

/// The renderer's pipeline selection, carried as one optional handle.
#[derive(Clone, Debug, Default)]
#[pill_mirror_resource(
    "pill_rendering::RenderingManager",
    former = "pill_master_renderer::RenderingManager"
)]
pub struct RenderingManager {
    pipeline: Option<Handle<RenderingPipeline>>,
}

#[pill_mirror_impl]
impl RenderingManager {
    /// Creates a manager with no pipeline, which leaves the renderer on its
    /// built-in chain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands the renderer a pipeline to run.
    ///
    /// This call is what makes the renderer build a new chain. On its next
    /// frame the renderer resolves the passes, checks that every offscreen
    /// target a pass reads was declared by an earlier pass, and creates the GPU
    /// pipelines it does not have yet. Until then it keeps drawing the chain it
    /// already has.
    #[pill_mirror_method]
    pub fn set_pipeline(&mut self, pipeline: Handle<RenderingPipeline>) {
        self.pipeline = Some(pipeline);
    }

    /// Returns the pipeline the renderer should run, or `None` when none is set.
    #[pill_mirror_method]
    pub fn pipeline(&self) -> Option<Handle<RenderingPipeline>> {
        self.pipeline
    }

    /// Drops the pipeline, which puts the renderer back on its built-in chain.
    #[pill_mirror_method]
    pub fn clear(&mut self) {
        self.pipeline = None;
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::Resource;

    #[test]
    fn a_new_manager_has_no_pipeline() {
        assert_eq!(RenderingManager::new().pipeline(), None);
    }

    #[test]
    fn setting_and_clearing_the_pipeline_moves_one_handle() {
        let pipeline = Handle::from_raw(3, 1);
        let mut manager = RenderingManager::new();

        manager.set_pipeline(pipeline);
        assert_eq!(manager.pipeline(), Some(pipeline));

        manager.clear();
        assert_eq!(manager.pipeline(), None);
    }

    #[test]
    fn the_resource_crosses_the_module_boundary_by_name() {
        assert_eq!(
            RenderingManager::shared_name(),
            Some("pill_rendering::RenderingManager")
        );
        assert_eq!(
            RenderingManager::former_shared_names(),
            ["pill_master_renderer::RenderingManager"],
            "a snapshot taken before the rename still finds the manager"
        );
    }

    #[test]
    fn a_play_session_can_copy_the_manager() {
        // Play copies every resource it restores on Stop, so a resource the
        // engine cannot copy refuses the session by name.
        let value = pill_engine::resource::ErasedResource::new(RenderingManager::new());
        assert!(RenderingManager::clone_erased(&value).is_some());
    }

    /// A pipeline the game sets during a session is the session's: Stop puts
    /// the editor's selection back.
    #[test]
    fn a_pipeline_set_during_a_play_session_is_reverted_on_stop() {
        let mut engine = pill_engine::Engine::new();
        engine.set_run_mode(pill_engine::RunMode::Editing);
        engine.world_mut().insert_resource(RenderingManager::new());

        engine.enter_play().expect("the manager is copyable");
        let pipeline = Handle::from_raw(5, 2);
        engine
            .world_mut()
            .get_resource_mut::<RenderingManager>()
            .expect("the manager is live")
            .set_pipeline(pipeline);
        assert_eq!(
            engine
                .world()
                .get_resource::<RenderingManager>()
                .and_then(RenderingManager::pipeline),
            Some(pipeline)
        );

        assert!(engine.exit_play(), "the session ends");

        assert_eq!(
            engine
                .world()
                .get_resource::<RenderingManager>()
                .and_then(RenderingManager::pipeline),
            None,
            "Stop puts the edit world's pipeline selection back"
        );
    }
}
