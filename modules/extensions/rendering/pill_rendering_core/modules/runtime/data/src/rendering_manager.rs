//! The world resource that holds the pipelines the renderer runs.
//!
//! # Responsibilities
//!
//! - Hold the pipeline the renderer should run: the game's choice, or the
//!   default that [`crate::register`] installs.
//! - Hold the append pipelines owners add, each with the surfaces it runs on and
//!   an order, so any owner can draw after a surface's chain without replacing
//!   the chain.
//! - Cross the module boundary: a project DLL sets the pipelines, the host's
//!   renderer executes them, and both sides have to agree on one resource.
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
//!
//! The appended pipelines make the manager's layout depend on this file - a map
//! of owned entries where a single optional handle used to be - so the type
//! declares that and a reload that changes the layout drops the value with a
//! `resource layout changed` warning instead of reading it through the old type.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::{pill_mirror_impl, pill_mirror_method, pill_mirror_resource, Handle};
use pill_renderer_api::PassSurfaces;

// Current crate
use crate::RenderingPipeline;

// =============================================================================
// AppendPipeline
// =============================================================================

/// One owner's pipeline, appended after a surface's chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppendPipeline {
    /// The package that appended it, which is what replaces its entry.
    pub owner: String,
    /// The pipeline to run.
    pub pipeline: Handle<RenderingPipeline>,
    /// The surfaces its passes run on.
    pub surfaces: PassSurfaces,
    /// Where it runs in the append list: the higher the number, the later.
    pub order: i32,
}

// =============================================================================
// RenderingManager
// =============================================================================

/// The renderer's pipeline selection: the chain a surface runs, and the
/// pipelines owners append after it.
#[derive(Clone, Debug, Default)]
#[pill_mirror_resource(
    "pill_rendering::RenderingManager",
    former = "pill_master_renderer::RenderingManager",
    layout_sources = ["rendering_manager.rs"]
)]
pub struct RenderingManager {
    pipeline: Option<Handle<RenderingPipeline>>,
    append_pipelines: BTreeMap<String, AppendPipeline>,
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

    /// Appends `pipeline` after the chain of the surfaces `surfaces` names.
    ///
    /// The entry belongs to `owner` - the package that asked for it - so a
    /// second call from the same owner replaces the first instead of stacking a
    /// second copy. `order` places it among the other owners' appended
    /// pipelines: the higher the number, the later it runs. It is not mirrored
    /// to C#, because a managed host has no asset handle to pass.
    pub fn set_append_pipeline(
        &mut self,
        owner: &str,
        pipeline: Handle<RenderingPipeline>,
        surfaces: PassSurfaces,
        order: i32,
    ) {
        self.append_pipelines.insert(
            owner.to_owned(),
            AppendPipeline {
                owner: owner.to_owned(),
                pipeline,
                surfaces,
                order,
            },
        );
    }

    /// Drops the pipeline `owner` appended, when it appended one.
    ///
    /// An owner that stops appending calls this on every generation, so a
    /// reload never leaves a pass pointing at an asset the new image no longer
    /// holds.
    pub fn clear_append_pipeline(&mut self, owner: &str) {
        self.append_pipelines.remove(owner);
    }

    /// The appended pipelines, in the order they run: by `order`, then by owner
    /// name, so two owners with the same number always run in the same order.
    pub fn append_pipelines(&self) -> impl Iterator<Item = &AppendPipeline> {
        let mut entries: Vec<&AppendPipeline> = self.append_pipelines.values().collect();
        entries.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.owner.cmp(&right.owner))
        });
        entries.into_iter()
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

    /// Appended pipelines run by order, then by owner name, and the same owner
    /// replaces its own entry.
    #[test]
    fn appended_pipelines_run_by_order_then_owner() {
        let mut manager = RenderingManager::new();
        manager.set_append_pipeline("late", Handle::from_raw(1, 1), PassSurfaces::Every, 900);
        manager.set_append_pipeline("early", Handle::from_raw(2, 1), PassSurfaces::Every, 100);
        manager.set_append_pipeline("early", Handle::from_raw(3, 1), PassSurfaces::Every, 100);

        let owners: Vec<&str> = manager
            .append_pipelines()
            .map(|entry| entry.owner.as_str())
            .collect();
        assert_eq!(
            owners,
            ["early", "late"],
            "order first, one entry per owner"
        );
        assert_eq!(
            manager
                .append_pipelines()
                .next()
                .map(|entry| entry.pipeline),
            Some(Handle::from_raw(3, 1)),
            "the second call from one owner replaced the first"
        );

        manager.set_append_pipeline(
            "also-early",
            Handle::from_raw(4, 1),
            PassSurfaces::Every,
            100,
        );
        let owners: Vec<&str> = manager
            .append_pipelines()
            .map(|entry| entry.owner.as_str())
            .collect();
        assert_eq!(
            owners,
            ["also-early", "early", "late"],
            "equal orders go by owner name"
        );
    }

    /// Clearing an owner's appended pipeline leaves every other owner's alone.
    #[test]
    fn clearing_an_appended_pipeline_keeps_the_others() {
        let mut manager = RenderingManager::new();
        manager.set_append_pipeline(
            "first",
            Handle::from_raw(1, 1),
            PassSurfaces::EditorViews,
            10,
        );
        manager.set_append_pipeline("second", Handle::from_raw(2, 1), PassSurfaces::Every, 20);

        manager.clear_append_pipeline("first");

        let entries: Vec<(&str, PassSurfaces)> = manager
            .append_pipelines()
            .map(|entry| (entry.owner.as_str(), entry.surfaces))
            .collect();
        assert_eq!(entries, [("second", PassSurfaces::Every)]);
    }

    /// The manager declares the file that defines its layout, so a reload that
    /// changes the fields drops the value instead of reading it.
    #[test]
    fn the_manager_declares_its_layout_sources() {
        assert_eq!(
            <RenderingManager as Resource>::layout_sources(),
            Some(&["rendering_manager.rs"][..])
        );
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
