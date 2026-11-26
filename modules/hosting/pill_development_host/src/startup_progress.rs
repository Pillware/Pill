//! The start-up and quit progress a frontend can poll: one entry per module,
//! and which of them is being built, loaded or released.
//!
//! # Responsibilities
//!
//! - [`publish_plan`]: record the modules a start will work through, which is
//!   the bar's section count.
//! - [`mark_building`], [`mark_loading`], [`mark_loaded`] and
//!   [`mark_unloading`]: move one module through those states.
//! - [`snapshot`]: hand a frontend a plain copy it can draw from.
//!
//! # Design
//!
//! The host already knows this and logs it; this module is the same knowledge
//! in a form a window can poll. It is a process-global behind a `Mutex` for the
//! same reason [`crate::build_progress`] is: the plan is recorded during
//! `setup` while the builds and loads that report against it run in the build
//! runner and the extension slots.
//!
//! The snapshot holds owned `String`s and plain enums only - no module type, no
//! pointer into an image - so a frontend can read it across a DLL boundary and
//! keep it while an image is released. A state a start has not reported yet is
//! simply absent: the reader draws what it finds.

// Standard library
use std::cell::RefCell;
use std::sync::Mutex;

// =============================================================================
// Constants
// =============================================================================

/// How many entries the last start planned, and how far each has got.
static PROGRESS: Mutex<Progress> = Mutex::new(Progress::new());

// =============================================================================
// Types
// =============================================================================

/// How far one module has got in the start (or the quit) it belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupModuleState {
    /// Planned, and nothing has happened to it yet.
    Pending,
    /// Its sources are being compiled.
    Building,
    /// Its artifact is being loaded into the process.
    Loading,
    /// Loaded: its section is filled.
    Loaded,
    /// Being released on quit.
    Unloading,
    /// Released on quit.
    Released,
}

/// What is being done to the module the progress is working on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupStep {
    /// The module is being built.
    Building,
    /// The artifact is being loaded.
    Loading,
}

/// One module of the start, and how far it has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupModule {
    /// The module's name, as its build and load report it.
    pub name: String,
    /// What kind of module it is: `extension`, `project`, `renderer` or
    /// `project editor`.
    pub kind: String,
    /// How far it has got.
    pub state: StartupModuleState,
}

/// A copy of the progress, for a frontend to draw.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupProgress {
    /// One entry per planned module, in the order the start works through
    /// them.
    pub modules: Vec<StartupModule>,
    /// The index of the module being worked on, if any.
    pub active: Option<usize>,
    /// What is being done to it.
    pub step: Option<StartupStep>,
    /// Whether the host has started releasing modules.
    pub closing: bool,
}

impl StartupProgress {
    /// The module being worked on, if any.
    #[must_use]
    pub fn active_module(&self) -> Option<&StartupModule> {
        self.active.and_then(|index| self.modules.get(index))
    }
}

/// The live state behind [`snapshot`].
struct Progress {
    modules: Vec<StartupModule>,
    active: Option<usize>,
    step: Option<StartupStep>,
    closing: bool,
}

impl Progress {
    const fn new() -> Self {
        Self {
            modules: Vec::new(),
            active: None,
            step: None,
            closing: false,
        }
    }

    /// The index of `module`, if this start planned it.
    fn index_of(&self, module: &str) -> Option<usize> {
        self.modules.iter().position(|entry| entry.name == module)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Records the modules a start will work through, and drops any earlier plan.
pub(crate) fn publish_plan(plan: &[(String, &'static str)]) {
    let mut progress = lock();
    progress.modules = plan
        .iter()
        .map(|(name, kind)| StartupModule {
            name: name.clone(),
            kind: (*kind).to_owned(),
            state: StartupModuleState::Pending,
        })
        .collect();
    progress.active = None;
    progress.step = None;
    progress.closing = false;
}

/// Reports that `module`'s build has started.
pub(crate) fn mark_building(module: &str) {
    mark(
        module,
        StartupModuleState::Building,
        Some(StartupStep::Building),
    );
}

/// Reports that `module`'s artifact is being loaded.
#[allow(dead_code)]
pub(crate) fn mark_loading(module: &str) {
    mark(
        module,
        StartupModuleState::Loading,
        Some(StartupStep::Loading),
    );
}

/// Reports that `module` has loaded: its section is now filled.
pub(crate) fn mark_loaded(module: &str) {
    mark(module, StartupModuleState::Loaded, None);
    tick();
}

/// Reports that `module` is being released: the quit's own progress.
///
/// The module that was being released when this one starts is marked
/// [`StartupModuleState::Released`], so the bar fills one section per module as
/// the unload walks the plan. No module waits for its own release: a slot
/// reports this from its destructor, before its images go.
pub(crate) fn mark_unloading(module: &str) {
    {
        let mut progress = lock();
        progress.closing = true;
        progress.step = None;
        for entry in &mut progress.modules {
            if entry.state == StartupModuleState::Unloading {
                entry.state = StartupModuleState::Released;
            }
        }
        if let Some(index) = progress.index_of(module) {
            progress.modules[index].state = StartupModuleState::Unloading;
            progress.active = Some(index);
        }
    }
    tick();
}

/// A copy of the progress, for a frontend to draw.
#[must_use]
pub fn snapshot() -> StartupProgress {
    let progress = lock();
    StartupProgress {
        modules: progress.modules.clone(),
        active: progress.active,
        step: progress.step,
        closing: progress.closing,
    }
}

// Standard library

thread_local! {
    /// The frontend's tick, installed around a start: called from the build
    /// loop while it waits for the compiler, so a window can draw.
    static TICK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
}

/// Runs `start` with `tick` installed, and takes the tick away afterwards.
///
/// The tick is called from the build loop's poll iteration, after each module
/// loads and as each module starts being released, on the thread running
/// `start`, so a frontend's closure may own the window and the GPU device it
/// draws with - no `Send` is required of it, or of the host it is drawing
/// over.
pub fn with_tick<R>(tick: Box<dyn FnMut()>, start: impl FnOnce() -> R) -> R {
    TICK.with(|slot| *slot.borrow_mut() = Some(tick));
    let outcome = start();
    TICK.with(|slot| *slot.borrow_mut() = None);
    outcome
}

/// Calls the installed tick, if a start installed one.
pub(crate) fn tick() {
    TICK.with(|slot| {
        // The tick is not re-entered: it draws and returns.
        if let Some(tick) = slot.borrow_mut().as_mut() {
            tick();
        }
    });
}

/// Writes one module's state and, when it is being worked on, makes it active.
fn mark(module: &str, state: StartupModuleState, step: Option<StartupStep>) {
    let mut progress = lock();
    progress.closing = false;
    progress.step = step;
    let Some(index) = progress.index_of(module) else {
        // A build the start did not plan (a reload) is not the splash's.
        return;
    };
    progress.modules[index].state = state;
    progress.active = step.map(|_| index);
}

/// The progress, forgiving a panic that poisoned the lock: it holds names and
/// plain enums, and dropping the rest of the start's progress would cost more
/// than the poisoned value does.
fn lock() -> std::sync::MutexGuard<'static, Progress> {
    PROGRESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// The progress is one process-global, so the tests take turns.
    fn exclusive() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One start's plan, as `setup` records it.
    fn plan() -> Vec<(String, &'static str)> {
        vec![
            ("pill_rendering".to_owned(), "extension"),
            ("pill_renderer".to_owned(), "renderer"),
            ("scene".to_owned(), "project"),
        ]
    }

    fn states() -> Vec<(String, StartupModuleState)> {
        snapshot()
            .modules
            .into_iter()
            .map(|module| (module.name, module.state))
            .collect()
    }

    #[test]
    fn a_plan_is_one_pending_section_per_module() {
        let _turn = exclusive();
        publish_plan(&plan());
        let progress = snapshot();
        assert_eq!(progress.modules.len(), 3);
        assert!(progress
            .modules
            .iter()
            .all(|module| module.state == StartupModuleState::Pending));
        assert_eq!(progress.active, None);
        assert!(!progress.closing);
        assert_eq!(progress.modules[1].kind, "renderer");
    }

    #[test]
    fn building_then_loading_then_loaded_moves_one_module_forward() {
        let _turn = exclusive();
        publish_plan(&plan());
        mark_building("scene");
        let progress = snapshot();
        assert_eq!(progress.modules[2].state, StartupModuleState::Building);
        assert_eq!(progress.step, Some(StartupStep::Building));
        assert_eq!(
            progress.active_module().map(|module| module.name.as_str()),
            Some("scene")
        );

        mark_loading("scene");
        assert_eq!(snapshot().modules[2].state, StartupModuleState::Loading);
        assert_eq!(snapshot().step, Some(StartupStep::Loading));

        mark_loaded("scene");
        let progress = snapshot();
        assert_eq!(progress.modules[2].state, StartupModuleState::Loaded);
        assert_eq!(
            progress.active, None,
            "a loaded module is not being worked on"
        );
    }

    #[test]
    fn an_unplanned_build_is_left_out() {
        let _turn = exclusive();
        publish_plan(&plan());
        mark_building("some_other_module");
        assert!(snapshot()
            .modules
            .iter()
            .all(|module| module.state == StartupModuleState::Pending));
        assert_eq!(snapshot().active, None);
    }

    #[test]
    fn the_quit_runs_the_bar_down_in_reverse() {
        let _turn = exclusive();
        publish_plan(&plan());
        for name in ["pill_rendering", "pill_renderer", "scene"] {
            mark_loaded(name);
        }
        mark_unloading("pill_renderer");
        let progress = snapshot();
        assert!(progress.closing);
        assert_eq!(progress.modules[1].state, StartupModuleState::Unloading);
        assert_eq!(
            progress.active_module().map(|module| module.name.as_str()),
            Some("pill_renderer")
        );
        // The next module's release is what fills the previous one's section.
        mark_unloading("scene");
        assert_eq!(snapshot().modules[1].state, StartupModuleState::Released);
        assert_eq!(snapshot().modules[2].state, StartupModuleState::Unloading);
    }

    #[test]
    fn a_second_plan_replaces_the_first() {
        let _turn = exclusive();
        publish_plan(&plan());
        mark_loaded("scene");
        publish_plan(&[("other".to_owned(), "extension")]);
        assert_eq!(
            states(),
            vec![("other".to_owned(), StartupModuleState::Pending)]
        );
    }
}
