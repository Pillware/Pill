//! The startup build plan: which modules a start builds, announced up front
//! and counted as each build starts.
//!
//! # Responsibilities
//!
//! - Log that hot reloading of a project is starting, with the numbered list
//!   of modules the host is about to build, as one multi-line entry
//!   ([`announce_plan`]).
//! - Log `module build started` with the module and its `step` of `steps`
//!   when one of those builds begins ([`announce_build`]), once per module.
//!
//! # Design
//!
//! The plan lives in one global because the start records it in `setup` and
//! the builds that report against it run elsewhere: the extensions and the
//! project build during `setup`, while the renderer's GPU module builds only
//! once a window opens. A build of a module that is not in the plan, or no
//! longer in it (any reload), is not announced here; its caller logs it.

// Standard library
use std::sync::Mutex;

// External crates
use pill_core::info;
use pill_core::telemetry::{log_block, telemetry_target};

// =============================================================================
// Constants
// =============================================================================

/// The plan for the current start. It is `None` until [`announce_plan`]
/// records one, and `None` again once the last planned build has started.
static STARTUP_BUILD_PLAN: Mutex<Option<BuildPlan>> = Mutex::new(None);

// =============================================================================
// Types
// =============================================================================

/// One module the startup will build, and what kind of module it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedBuild {
    /// The module's name, as its build reports it.
    pub(crate) module: String,
    /// Which kind of module it is: `extension`, `project` or `renderer`.
    pub(crate) kind: &'static str,
}

/// The builds a start announced and that have not started yet, plus the
/// counts [`announce_build`] uses for its message.
struct BuildPlan {
    total_build_count: usize,
    started_build_count: usize,
    pending_builds: Vec<PlannedBuild>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Records the startup build plan and logs it as one multi-line entry.
///
/// `project_name` is the project's display name. Any earlier plan is replaced.
pub(crate) fn announce_plan(project_name: &str, builds: Vec<PlannedBuild>) {
    info!(target: telemetry_target::HOT_RELOAD, "{}", render_plan(project_name, &builds));
    // A panic while the lock was held poisons it. Ignore that: the plan holds
    // only counts and names, and dropping it would cost the rest of the start
    // its progress, so the poisoned value is used as it is.
    let mut plan = STARTUP_BUILD_PLAN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *plan = Some(BuildPlan {
        total_build_count: builds.len(),
        started_build_count: 0,
        pending_builds: builds,
    });
}

/// Logs that the build of `module` is starting, if that build belongs to the
/// startup plan and has not started yet.
///
/// Returns `true` when it logged the build. It returns `false` for any other
/// build (any reload), so the caller logs it in its usual way instead.
pub(crate) fn announce_build(module: &str) -> bool {
    let mut plan = STARTUP_BUILD_PLAN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(current) = plan.as_mut() else {
        return false;
    };
    let Some(pending_build_index) = current
        .pending_builds
        .iter()
        .position(|build| build.module == module)
    else {
        return false;
    };
    current.pending_builds.remove(pending_build_index);
    current.started_build_count += 1;
    info!(
        target: telemetry_target::HOT_RELOAD,
        step = current.started_build_count,
        steps = current.total_build_count,
        module,
        "module build started"
    );
    // The plan is dropped once every planned build has started; from then on,
    // any build is a reload.
    if current.pending_builds.is_empty() {
        *plan = None;
    }
    true
}

/// Builds the message [`announce_plan`] logs: a heading and the numbered
/// modules.
fn render_plan(project_name: &str, planned_builds: &[PlannedBuild]) -> String {
    let module_name_width = planned_builds
        .iter()
        .map(|build| build.module.len())
        .max()
        .unwrap_or(0);
    // The numbers use the same `[n/total]` form as the build steps that
    // follow. `n` is padded to the width of the total so every module name
    // starts in the same column, and the names are padded so the kinds line
    // up.
    let total_build_count = planned_builds.len();
    let build_number_width = total_build_count.to_string().len();
    let lines = planned_builds.iter().enumerate().map(|(index, build)| {
        format!(
            "[{:>build_number_width$}/{total_build_count}] {:<module_name_width$}  {}",
            index + 1,
            build.module,
            build.kind
        )
    });
    log_block(
        &format!("Hot reloading of {project_name} is starting. Modules to build:"),
        lines,
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn planned_build(module_name: &str, module_kind: &'static str) -> PlannedBuild {
        PlannedBuild {
            module: module_name.to_string(),
            kind: module_kind,
        }
    }

    /// The banner names the project and lists every module, numbered in
    /// build order.
    #[test]
    fn the_plan_lists_every_module_in_order() {
        let text = render_plan(
            "Bouncing Balls",
            &[
                planned_build("pill_spline", "extension"),
                planned_build("project", "project"),
            ],
        );

        assert!(
            text.contains("Hot reloading of Bouncing Balls is starting"),
            "{text}"
        );
        let spline_index = text
            .find("[1/2] pill_spline")
            .expect("first module numbered 1");
        let project_index = text
            .find("[2/2] project")
            .expect("second module numbered 2");
        assert!(spline_index < project_index, "{text}");
    }

    /// Each planned build is announced once, in the order it starts. Any
    /// other build, including a second build of the same module, is left to
    /// the caller. The plan ends when its last build starts.
    #[test]
    fn planned_builds_are_announced_once_and_the_plan_then_ends() {
        announce_plan(
            "Test",
            vec![
                planned_build("module_a", "extension"),
                planned_build("module_b", "project"),
            ],
        );

        assert!(!announce_build("not_planned"));
        assert!(announce_build("module_b"));
        assert!(!announce_build("module_b"), "a reload of the same module");
        assert!(announce_build("module_a"));
        assert!(
            !announce_build("module_a"),
            "the plan is over once every build started"
        );
    }
}
