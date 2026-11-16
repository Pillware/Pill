//! System registration, frame processing and parallel dispatch.
//!
//! # Responsibilities
//!
//! - Registers systems by name and enables or disables them.
//! - Runs the frame: the update systems in parallel batches, then the command
//!   flush, then the post-update systems.
//! - Limits the frame rate through `set_fps_limit`.
//! - Measures how long each system takes, for profiling and utilization
//!   metrics.
//! - Provides `world()` and `world_mut()` for queries outside the frame.
//!
//! # Design
//!
//! The engine owns the [`World`], the [`SystemScheduler`] and the
//! [`CommandQueue`]. A frame bumps the world tick, runs the update systems
//! from the scheduler's batches (in parallel on Rayon threads), then applies
//! the commands the systems queued and runs the post-update systems on the
//! result. A system that uses [`Commands`] always runs alone, because it
//! needs exclusive access to the world.
//!
//! The engine is the only owner of the registered systems, and each record
//! carries the access pattern the scheduler builds its conflict data from.
//! Any structural change - a registration, a removal, a clear - marks the
//! scheduler's graph as out of date; the next frame rebuilds the batches
//! once, before it dispatches.

// Standard library
use std::sync::Mutex;
use std::time::Duration;

// External crates
use pill_core::platform::Instant;
use pill_core::rayon;
use pill_core::rayon::prelude::*;
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

// Current crate
use crate::commands::{CommandError, CommandQueue};
use crate::component::Tick;
use crate::config::ParallelProcessingConfig;
use crate::error::{SystemError, SystemFailure};
use crate::scheduler::{RunPhase, SystemAccess, SystemScheduler};
use crate::system::{IntoSystem, ReadOnlySystemParam, System, SystemParam};
use crate::world::{
    set_per_thread_last_run_tick, set_per_thread_this_run_tick, ChangeOrigin, World,
};

// =============================================================================
// SystemOwner
// =============================================================================

/// Says who registered a system: the project, an extension, the renderer or
/// the engine.
///
/// A hot reload replaces one module's code while the rest of the process keeps
/// running. The systems of the module being swapped must be removed, because
/// their function pointers still point into the old library, but the systems
/// of every other module and of the project must keep running. Recording the
/// owner on each registration is what makes that separation possible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SystemOwner(pub u64);

impl SystemOwner {
    /// Owner of systems the project registers, and of any system registered
    /// outside a module scope.
    pub const PROJECT: Self = Self(0);

    /// Owner of the systems the engine registers for itself, such as the
    /// periodic ECS report.
    ///
    /// A reload never clears these systems: the host retires systems by
    /// naming the project or one module in [`Engine::clear_systems_owned_by`],
    /// and an engine-owned system is neither. That suits them, because their
    /// code lives in `pill_engine_core.dll`, which is never unloaded, so
    /// there is nothing to retire.
    ///
    /// The value is `u64::MAX`, which no [`Self::extension`] index reaches.
    pub const ENGINE: Self = Self(u64::MAX);

    /// Owner of the extension loaded at `index`.
    ///
    /// The value is one above `index`, so no extension owner can equal
    /// [`Self::PROJECT`].
    pub const fn extension(index: usize) -> Self {
        Self(index as u64 + 1)
    }

    /// Owner of the systems a renderer registers.
    ///
    /// This value is reserved instead of taken from the extension range. A
    /// host can skip an extension and keep that module's owner reserved until
    /// the module loads (the editor's safe mode retries a failed module), so
    /// counting the extensions that did load does not give the next free
    /// index. An owner derived from that count could therefore equal the
    /// skipped module's owner. Two different subjects under one owner would
    /// let a reload of either clear the other's systems, and would let a
    /// suspension meant for the module resume a renderer that is paused
    /// against a replaced component layout.
    ///
    /// The value is `u64::MAX - 1`, which no [`Self::extension`] index
    /// reaches, and which differs from [`Self::PROJECT`] and [`Self::ENGINE`].
    pub const RENDERER: Self = Self(u64::MAX - 1);

    /// The extension index this owner encodes, or `None` for the project, the
    /// renderer and the engine.
    pub fn extension_index(self) -> Option<usize> {
        if self == Self::PROJECT || self == Self::ENGINE || self == Self::RENDERER {
            None
        } else {
            Some((self.0 - 1) as usize)
        }
    }
}

impl std::fmt::Display for SystemOwner {
    /// Writes `project`, `renderer`, `engine` or `extension <n>`; this is the
    /// one place an owner becomes text.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.extension_index() {
            None if *self == Self::PROJECT => formatter.write_str("project"),
            None if *self == Self::RENDERER => formatter.write_str("renderer"),
            None => formatter.write_str("engine"),
            Some(index) => write!(formatter, "extension {index}"),
        }
    }
}

// =============================================================================
// RegisteredSystem
// =============================================================================

/// One registered system with its runtime bookkeeping.
///
/// Holds the system's name, its boxed instance, its access pattern, whether
/// it is enabled, and the per-frame timing data that the Tracy span and the
/// utilization metrics read.
struct RegisteredSystem {
    /// The phase of the frame this system runs in.
    ///
    /// A system in [`RunPhase::Project`] runs after the first command flush,
    /// and one in [`RunPhase::PostProject`] after the second, even while the
    /// engine is paused because it opted in to edit mode.
    phase: RunPhase,
    /// Sort key within the phase.
    ///
    /// Systems are kept sorted by `(phase, order)` across every registration,
    /// and equal pairs keep the order their artifacts registered them in, so a
    /// number an extension or a project picks orders its systems against every
    /// other artifact's. See [`SystemOptions::order`].
    order: i32,
    /// Whether the system also runs in edit mode, and while paused.
    ///
    /// A game system leaves this `false`: it runs only while the game is
    /// playing, so the editor's Play and Stop mean something. A system that
    /// keeps the editor itself alive opts in - the renderer, the egui shell,
    /// the debug outlines, the audio loader - because a frozen editor with no
    /// picture would be unusable.
    edit_mode: bool,
    /// Name used for registration, debugging, and profiling.
    name: String,
    /// Who registered this system; a hot reload removes systems by owner.
    system_owner: SystemOwner,
    /// Boxed system instance, invoked with the world and command queue.
    system: Box<dyn System>,
    /// The components and resources this system touches.
    ///
    /// The engine keeps this list here, not in the scheduler, so the two
    /// cannot drift apart: there is only one list, and the scheduler builds
    /// its conflict data from it when it needs it.
    system_access: SystemAccess,
    /// World tick at which this system was last executed.
    ///
    /// Set as `World::system_last_run` before the system runs, so
    /// change-detection filters (`Changed<T>`, `Added<T>`) inside it see only
    /// the changes made since its previous run.
    last_run_tick: u32,
    /// Whether this system is enabled, and so runs when the frame dispatches
    /// systems.
    enabled: bool,
    /// Running average of this system's execution time in nanoseconds,
    /// smoothed over about the last 32 frames. It is shown in the system's
    /// Tracy span as the splitting hint; the parallel iterators keep their own
    /// per-label timing.
    average_duration: u64,
    /// Execution time in nanoseconds from the most recent frame, used by the
    /// parallel-utilization metrics.
    last_duration: u64,
    /// The `run_if` conditions, in declaration order.
    ///
    /// Empty for a plain system. A skipped system keeps its place in the
    /// execution graph: the conditions decide whether it runs, never where.
    conditions: Vec<SystemCondition>,
}

// =============================================================================
// BatchMember
// =============================================================================

/// A batch member's data, as the parallel worker closure sees it.
///
/// One record per member keeps the index, the two ticks, the name and the run
/// flag together, so the worker reads them from one place instead of
/// matching separate lists by position.
struct BatchMember<'a> {
    /// Position in the engine's registered-system vector.
    system_index: usize,
    /// Tick at which this system last ran, from before the batch started.
    last_run_tick: u32,
    /// Tick reserved for this member on the dispatching thread.
    this_run_tick: u32,
    /// Registration name, for the Tracy span and the failure record.
    name: &'a str,
    /// The Tracy zone color for this system, from its owner.
    zone_color: u32,
    /// Whether this member runs in the update phase of this batch.
    runs: bool,
}

// =============================================================================
// SystemSnapshot
// =============================================================================

/// A read-only snapshot of one registered system.
///
/// `index` is the position in the registration vector and is the stable key
/// for toggling, because system **names are not unique** —
/// [`Engine::set_system_enabled`] matches the first system with a given name.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemSnapshot {
    /// Position in the registration vector, which is the key that names
    /// exactly one system.
    pub index: usize,
    /// Registration name, shown in the UI and used by the name-keyed calls,
    /// which match the first system that has it.
    pub name: String,
    /// What registered this system: the project, the renderer, the engine, or
    /// one extension.
    pub owner: SystemOwner,
    /// Whether the system currently participates in frame execution.
    pub enabled: bool,
    /// Whether the system has a hot-patch dispatch slot in the registry.
    pub hot_patchable: bool,
    /// True when another registered system shares this name, so the UI can
    /// warn instead of toggling an ambiguous target.
    pub name_is_ambiguous: bool,
}

// =============================================================================
// Engine
// =============================================================================

/// Whether the frame is being edited or played.
///
/// Every posture except the editor plays, which is what the default says: the
/// standalone, shipping and web builds know nothing about editing, so their
/// behaviour does not depend on this type at all.
///
/// The editor starts in [`RunMode::Editing`] and switches to `Playing` when the
/// person presses Play, at which point the game systems and the scripts run
/// again. A system that must keep running while editing opts in at
/// registration, through [`SystemOptions::run_in_edit_mode`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunMode {
    /// The editor is being used. Only the systems that opted in to edit mode
    /// run, and no script runs.
    Editing,
    /// The game is running: every enabled system and every script runs. The
    /// default.
    #[default]
    Playing,
}

/// Why a play session could not start.
///
/// Every type that made the copy impossible is named, components first, so one
/// message tells a person the whole fix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayRefusal {
    /// Component types that are neither `Clone` nor plain data.
    pub component_type_names: Vec<String>,
    /// `Restore` resource types whose values could not be copied.
    pub resource_type_names: Vec<String>,
}

impl PlayRefusal {
    /// Whether the refusal names nothing.
    pub fn is_empty(&self) -> bool {
        self.component_type_names.is_empty() && self.resource_type_names.is_empty()
    }
}

impl std::fmt::Display for PlayRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("cannot play: ")?;
        let mut wrote_a_reason = false;
        if !self.component_type_names.is_empty() {
            write!(
                formatter,
                "these components are neither `Clone` nor plain data: {}",
                self.component_type_names.join(", ")
            )?;
            wrote_a_reason = true;
        }
        if !self.resource_type_names.is_empty() {
            if wrote_a_reason {
                formatter.write_str("; ")?;
            }
            write!(
                formatter,
                "these resources cannot be copied: {}",
                self.resource_type_names.join(", ")
            )?;
        }
        Ok(())
    }
}

/// What a registration asks of the schedule, beside the system itself.
///
/// A `#[system]` declaration becomes one of these: the attribute parses it and
/// the emitted registration passes it to
/// [`Engine::register_system_with_options`]. A host that registers a system by
/// call builds one the same way, so both paths order, phase, gate and condition
/// their systems identically.
///
/// The default is the plain registration: order `0`, the
/// [`Project`](RunPhase::Project) phase, a system that runs only while the game
/// does, and no conditions.
pub struct SystemOptions {
    /// Sort key within the phase; a lower value runs first.
    ///
    /// The engine keeps its systems sorted by `(phase, order)` across every
    /// artifact, so this number orders a system against every other one in its
    /// phase, not only against its own artifact's. Equal values keep
    /// registration order. `AGENTS.md` documents the number range each area
    /// uses.
    pub order: i32,
    /// The phase of the frame the system runs in. Defaults to
    /// [`RunPhase::Project`].
    pub phase: RunPhase,
    /// Whether the system also runs while the editor is editing, and while the
    /// engine is paused.
    ///
    /// A game system leaves this `false`. A system the editor itself needs -
    /// the renderer's frame build, the editor shell's pass, a debug overlay -
    /// sets it, because a frozen editor with no picture would be unusable.
    pub run_in_edit_mode: bool,
    /// The `run_if` conditions; see [`Self::run_if`].
    conditions: Vec<SystemCondition>,
}

/// A `run_if` condition once boxed: it evaluates against the world and the
/// command queue and answers whether the gated system may run.
type BoxedSystemCondition = Box<dyn FnMut(&mut World, &mut CommandQueue) -> bool + Send>;

/// One `run_if` condition of a registered system.
struct SystemCondition {
    /// What the condition reads, merged into the system's own access when the
    /// two are registered together.
    access: SystemAccess,
    /// Evaluates the condition against the world; `true` lets the system run.
    run: BoxedSystemCondition,
}

impl Default for SystemOptions {
    fn default() -> Self {
        Self {
            order: 0,
            phase: RunPhase::Project,
            run_in_edit_mode: false,
            conditions: Vec::new(),
        }
    }
}

impl SystemOptions {
    /// Options for a system in one phase at one order, running only while the
    /// game runs.
    pub fn new(phase: RunPhase, order: i32) -> Self {
        Self {
            phase,
            order,
            ..Self::default()
        }
    }

    /// Marks the system as also running while the editor is editing and while
    /// the engine is paused.
    pub fn with_run_in_edit_mode(mut self, run_in_edit_mode: bool) -> Self {
        self.run_in_edit_mode = run_in_edit_mode;
        self
    }

    /// Adds a `run_if` condition: the system runs only while this returns
    /// `true`.
    ///
    /// A condition takes the same parameters a system does, but only the
    /// read-only ones: [`ReadOnlySystemParam`] is what enforces that, so a
    /// condition taking `ResMut<T>`, `Commands` or a mutable query does not
    /// compile. Its reads join the system's access, so the scheduler never
    /// batches the system beside a writer of what its condition reads.
    ///
    /// Every condition of a system must hold. They are evaluated in the order
    /// they were added, and the first false answer skips the system - which
    /// stays in the execution graph, so adding a condition never reshuffles
    /// the batches.
    ///
    /// A condition that could write is refused at compile time:
    ///
    /// ```compile_fail
    /// use pill_engine_core::*;
    ///
    /// #[derive(Debug, Default)]
    /// struct Gate {
    ///     open: bool,
    /// }
    /// impl Resource for Gate {}
    ///
    /// fn gate_opens(mut gate: ResMut<Gate>) -> bool {
    ///     gate.open = true;
    ///     true
    /// }
    ///
    /// // `ResMut<Gate>` is not a `ReadOnlySystemParam`, so this does not
    /// // compile: a condition may decide whether the system runs, never
    /// // change the world it gates.
    /// let options = SystemOptions::default().run_if(gate_opens);
    /// ```
    pub fn run_if<C, Input>(mut self, mut condition: C) -> Self
    where
        C: FnMut(Input) -> bool + Send + 'static,
        Input: ReadOnlySystemParam + 'static,
    {
        let mut access = SystemAccess::new();
        Input::report_access(&mut access);
        self.conditions.push(SystemCondition {
            access,
            run: Box::new(move |world: &mut World, command_queue: &mut CommandQueue| {
                // SAFETY: the parameter tuple is read-only by the
                // `ReadOnlySystemParam` bound, and it is fetched and dropped
                // inside this call, so it cannot escape or alias another
                // borrow. The caller passes the same exclusive world access a
                // system gets.
                let input = unsafe { Input::fetch(world, command_queue) };
                condition(input)
            }),
        });
        self
    }
}

/// The main engine that drives the ECS.
///
/// It owns the [`World`], the [`SystemScheduler`] and the [`CommandQueue`],
/// and runs every frame: the systems of the pre-project phase run in parallel
/// batches, the queued commands are applied, the project phase runs, the
/// commands that phase queued are applied, and the post-project systems run.
/// Systems are registered by name and can be enabled or disabled while the
/// game runs.
///
/// A project or module never calls [`Self::register_system`]. It declares its
/// systems with `#[system]`, and the artifact entry point registers every
/// declared one before the artifact's own `register` runs; components, resources
/// and asset types arrive the same way, through their derive. The registering
/// methods on this type are what a *host* uses for the systems it owns - the
/// renderer's frame build, an editor's own passes - and what the entry point
/// calls on the artifact's behalf.
///
/// A play session lives here too: [`Engine::enter_play`] copies the world aside
/// and [`Engine::exit_play`] puts it back, so an editor can run a game against
/// the scene it has open and return to it unchanged.
///
/// # Examples
/// ```no_run
/// # use pill_engine::*;
/// let mut engine = Engine::new();
///
/// // Every frame - the declared systems run, then the command flush, then the
/// // post-update systems.
/// engine.process_frame().unwrap();
/// ```
pub struct Engine {
    /// Every registered system, in registration order.
    systems: Vec<RegisteredSystem>,
    /// The commands the systems queued; the frame applies them at its flush.
    command_queue: CommandQueue,
    /// The ECS world
    world: World,
    /// Builds the parallel batches from the systems' access patterns.
    scheduler: SystemScheduler,
    /// Failures the systems reported since the last drain. The reporting
    /// boundary drains them once per frame, and a failing system never aborts
    /// its batch or the frame.
    system_failures: Vec<SystemFailure>,
    /// Whether the frame runs the systems in parallel batches.
    parallel_execution_enabled: bool,
    /// When true, `process_frame` skips the update systems but still bumps the
    /// world tick, updates scripts, flushes deferred commands and runs the
    /// post-update systems.
    ///
    /// This lives on the engine, not in each host: an editor needs the engine
    /// itself to keep ticking and flushing while the update systems are
    /// paused, so it can change a paused world and see the result.
    systems_paused: bool,
    /// Whether the frame is being edited or played; see [`RunMode`].
    ///
    /// Every posture except the editor keeps [`RunMode::Playing`], so its
    /// behaviour is unchanged.
    run_mode: RunMode,
    /// Whether the frame that is running dispatches every system, or only the
    /// ones that opted in to edit mode.
    ///
    /// It is computed once at the start of a frame from the run mode, the pause
    /// state and the step flag, so every phase of that frame agrees.
    runs_every_system_this_frame: bool,
    /// When true, the next `process_frame` runs the system phase once even if
    /// the engine is paused. Set by [`Engine::request_single_step`]; the frame
    /// clears it.
    single_step_pending: bool,
    /// If true, `process_frame` returns every deferred-command failure it
    /// collected. When false (the default), failures are logged and the frame
    /// reports success.
    ///
    /// [`Engine::set_should_exit_on_error`] changes it.
    should_exit_on_error: bool,
    /// Optional frame rate limit. When set, `process_frame` sleeps the
    /// remaining budget after completing the frame's work.
    frame_budget: Option<Duration>,
    /// When true, the sleep that waits for the frame budget is wrapped in a
    /// Tracy zone named `frame wait`; turn it off to keep the timeline tidy.
    ///
    /// [`Engine::set_trace_frame_wait`] changes it.
    should_trace_frame_wait: bool,
    /// The archetype generation seen on the previous frame. Comparing it with
    /// the current one tells the frame when archetypes appeared or went away,
    /// so it can log one memory breakdown per change.
    last_archetype_generation: u64,
    /// The owner given to systems registered from now on.
    ///
    /// The host wraps a module's initialization in
    /// [`Engine::begin_module_registration`] and
    /// [`Engine::end_module_registration`], so module code keeps registering
    /// systems by name and the engine still records the right owner for each.
    active_owner: SystemOwner,
    /// The owners whose systems are currently suspended, set by
    /// [`Engine::set_systems_enabled_for_owner`].
    ///
    /// The owners are kept in their own list, not only as flags on the
    /// systems, so a system registered while its owner is suspended starts
    /// disabled: a new module generation that initializes during a suspension
    /// must not run before it is resumed. A `Vec` is enough because only a
    /// handful of owners ever exist.
    disabled_owners: Vec<SystemOwner>,
    /// Dispatch slots for hot-patchable systems, keyed by registration name
    /// and, when that differs, by the function's qualified path.
    ///
    /// This field must NOT be behind `#[cfg(feature = "development")]`. The
    /// feature would change the struct's layout, and a module built with the
    /// other setting rebuilds `&mut Engine` from a raw pointer through
    /// [`EngineApi`](crate::EngineApi). A layout difference would then corrupt
    /// memory instead of failing to compile. An unused empty map costs a few
    /// bytes; the difference would cost correctness.
    hot_patch_registry: crate::hot_patch::HotPatchRegistry,
}

impl Engine {
    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    /// Creates an engine with no registered systems.
    ///
    /// It adds the resources every build expects (`Time`, `Input`,
    /// `AssetManager`) and registers the `Scene` asset type before any
    /// generation starts, so a reload cannot retire that registration.
    pub fn new() -> Self {
        // Name the main thread before any zone runs on it. Tracy registers a
        // thread the first time it sees a zone on it, and the thread list
        // follows registration order, so naming the thread first keeps "main"
        // at the top.
        crate::profile_thread!("main");

        // Wrap initialization in a non-continuous Tracy frame.
        let _init = crate::profile_non_continuous_frame!("engine init");

        // Configure plot appearance in Tracy UI.
        crate::profile_plot_config!(entity_count, tracy_client::PlotConfiguration::default());
        crate::profile_plot_config!(archetype_count, tracy_client::PlotConfiguration::default());
        crate::profile_plot_config!(frame_time_us, tracy_client::PlotConfiguration::default());
        crate::profile_plot_config!(
            memory_estimate_kb,
            tracy_client::PlotConfiguration::default()
        );
        crate::profile_plot_config!(
            commands_executed,
            tracy_client::PlotConfiguration::default()
        );
        crate::profile_plot_config!(
            parallel_utilization_pct,
            tracy_client::PlotConfiguration::default()
        );
        crate::profile_plot_config!(
            batch_packing_pct,
            tracy_client::PlotConfiguration::default()
        );

        // Warm up the Rayon thread pool so the first parallel batch
        // does not pay thread-spawning costs (can be 1-10ms on some OS).
        rayon::broadcast(|_ctx| {});

        // Print the active parallel-iteration knobs.
        crate::config::print_parallel_config();

        // Resources the engine owns. They exist before any project or module
        // runs, so nothing has to check for them. The engine advances or owns
        // them itself instead of through a registered system, so a hot reload,
        // which removes systems by owner, cannot take them away.
        let mut world = World::new();
        world.insert_resource(crate::time::Time::new());
        world.insert_resource(crate::input::Input::new());
        world.insert_resource(crate::asset::AssetManager::new());
        // The world's restore counter, in the form a system can read. It
        // starts at zero and only a play session's Stop moves it.
        world.insert_resource(crate::resource::RestoreEpoch::default());
        // The scene type belongs to the engine, so every build can list and
        // load a `.scene` file without a module or a project registering it.
        // This runs before any generation starts, so its registration is older
        // than any sequence a reload records, which is what keeps a reload
        // from retiring it.
        world.register_standalone_asset::<crate::scene::Scene>();

        let engine = Self {
            systems: Vec::new(),
            command_queue: CommandQueue::new(),
            world,
            scheduler: SystemScheduler::new(),
            system_failures: Vec::new(),
            parallel_execution_enabled: true,
            systems_paused: false,
            run_mode: RunMode::Playing,
            runs_every_system_this_frame: true,
            single_step_pending: false,
            should_exit_on_error: false,
            frame_budget: None,
            should_trace_frame_wait: true,
            last_archetype_generation: 0,
            active_owner: SystemOwner::PROJECT,
            disabled_owners: Vec::new(),
            hot_patch_registry: crate::hot_patch::HotPatchRegistry::new(),
        };

        // The ECS state report is opt-in: only `set_ecs_diagnostics_interval`
        // registers it. It is not created here because it reads the whole world
        // under exclusive access, which forces a scheduler barrier on every
        // frame, and it prints several lines once per interval. A shipping
        // build must not pay that cost, and no test should have to filter the
        // output. When it is registered it belongs to `SystemOwner::ENGINE`, so
        // no reload retires it.
        engine
    }
}

impl Engine {
    // -------------------------------------------------------------------------
    // Configuration
    // -------------------------------------------------------------------------

    /// Enables or disables parallel system execution.
    ///
    /// When disabled, systems run sequentially in registration order.
    pub fn set_parallel_execution(&mut self, enabled: bool) {
        self.parallel_execution_enabled = enabled;
    }

    /// Sets how often the built-in ECS state report prints.
    ///
    /// The report is opt-in: [`Engine::new`] registers nothing, and this is the
    /// call that registers it. `None` turns it off again, removing the system.
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use pill_engine::diagnostics::DEFAULT_REPORT_INTERVAL;
    /// # let mut engine = pill_engine::Engine::new();
    /// engine.set_ecs_diagnostics_interval(Some(DEFAULT_REPORT_INTERVAL)); // on
    /// engine.set_ecs_diagnostics_interval(Some(Duration::from_secs(10)));  // slower
    /// engine.set_ecs_diagnostics_interval(None);                           // off
    /// ```
    ///
    /// Each call registers the system again, so exactly one report system
    /// exists no matter how often it is called. While registered, it costs one
    /// `Duration` comparison on the frames it does not print, plus the
    /// scheduler barrier its exclusive world access forces on every frame;
    /// `None` removes both.
    pub fn set_ecs_diagnostics_interval(&mut self, interval: Option<Duration>) {
        // Register again instead of changing the existing system: the interval
        // lives inside the closure, so replacing the closure is what keeps
        // exactly one report system registered.
        self.clear_systems_owned_by(SystemOwner::ENGINE);
        self.register_ecs_diagnostics_system(interval);
    }

    /// Registers the periodic ECS state report under
    /// [`SystemOwner::ENGINE`].
    ///
    /// `None`, or a zero interval, registers nothing, which is how the report
    /// is disabled.
    fn register_ecs_diagnostics_system(&mut self, interval: Option<Duration>) {
        let Some(interval) = interval.filter(|interval| !interval.is_zero()) else {
            return;
        };

        let mut system_access = SystemAccess::new();
        // The report reads the whole world: every archetype, the registry and
        // the resources. No per-component declaration can describe that, so
        // the access is declared as exclusive world access. That declaration
        // is what keeps it sound: the scheduler never puts a system that
        // conflicts with everything into a parallel batch, so nothing else
        // touches the world while it runs.
        system_access.set_uses_commands(true);

        // The next report time lives in the closure; the registration takes it
        // as an `FnMut`.
        let mut next_report: Option<Duration> = None;
        let previous_owner = self.active_owner;
        self.active_owner = SystemOwner::ENGINE;
        // SAFETY: `system_access` declares exclusive world access, which covers
        // everything the system touches. The closure only reads the world and
        // keeps no borrow after it returns.
        unsafe {
            self.register_system_with_access(
                crate::diagnostics::SYSTEM_NAME,
                system_access,
                // The closure keeps its own schedule. It starts as `None`, so
                // the first report comes one interval after the first frame
                // instead of immediately at startup, when the world has just
                // been built.
                move |world: &mut World, _queue: &mut CommandQueue| {
                    let elapsed = world
                        .get_resource::<crate::time::Time>()
                        .map_or(Duration::ZERO, crate::time::Time::elapsed);
                    match next_report {
                        Some(due) if elapsed >= due => {}
                        Some(_) => return Ok(()),
                        None => {
                            next_report = Some(elapsed + interval);
                            return Ok(());
                        }
                    }
                    // Count the next interval from now, not from the time the
                    // report was due, so one slow frame cannot leave a backlog
                    // that prints several reports in a row.
                    next_report = Some(elapsed + interval);
                    let snapshot = crate::diagnostics::EcsSnapshot::gather(world).render();
                    pill_core::info!(
                        target: telemetry_target::ECS,
                        "{}",
                        pill_core::telemetry::log_block("ECS snapshot", snapshot.lines())
                    );
                    Ok(())
                },
            );
        }
        self.active_owner = previous_owner;
    }

    /// Pauses or resumes the update systems without stopping the frame.
    ///
    /// A paused engine still bumps the world tick and still flushes deferred
    /// commands, so an editor can change a frozen world and see the result.
    /// While paused, only the systems that opted in to edit mode run, plus the
    /// post-update systems after the flush, so scripts stop with the update
    /// systems rather than running on under them.
    pub fn set_systems_paused(&mut self, paused: bool) {
        self.systems_paused = paused;
        if !paused {
            // Resuming also drops a pending single step: with the systems
            // running freely, there is nothing left to step.
            self.single_step_pending = false;
        }
    }

    /// Returns whether the update systems are currently skipped.
    pub fn systems_paused(&self) -> bool {
        self.systems_paused
    }

    /// Sets whether the engine is editing or playing.
    ///
    /// Editing means the editor is being used: only the systems that opted in
    /// to edit mode run, and no script runs. Playing is the normal posture of
    /// every non-editor build, which is why it is the default.
    ///
    /// The mode is engine state rather than a resource on purpose: project code
    /// gets no play-mode API, so a game cannot behave differently depending on
    /// who runs it.
    pub fn set_run_mode(&mut self, mode: RunMode) {
        self.run_mode = mode;
    }

    /// The current run mode.
    pub fn run_mode(&self) -> RunMode {
        self.run_mode
    }

    /// Whether a frame in the current posture runs every system.
    ///
    /// True while playing and not paused, and for a step, which is one playing
    /// frame by definition.
    fn runs_every_system(&self) -> bool {
        self.single_step_pending || (self.run_mode == RunMode::Playing && !self.systems_paused)
    }

    /// Starts a play session: the edit world is copied aside, the copy becomes
    /// the running world, and the game starts.
    ///
    /// The copy is exact: entity handles, the allocator's state, every
    /// component and every [`PlayState::Restore`](crate::PlayState::Restore)
    /// resource. A `Keep` or `Rebuild` resource stays live and is left alone on
    /// Stop.
    ///
    /// A session that is already running is left as it is, so a second Play
    /// cannot replace the held edit world with the playing one.
    ///
    /// # Errors
    ///
    /// Returns [`PlayRefusal`] naming every component and resource type the
    /// copy could not include, and changes nothing. A component that owns heap
    /// data and does not implement `Clone` is the usual cause.
    pub fn enter_play(&mut self) -> Result<(), PlayRefusal> {
        if self.world.held_entity_storage.is_some() {
            return Ok(());
        }
        let held_storage = match self.world.copy_entity_storage() {
            Ok(held_storage) => held_storage,
            Err(uncopyable) => {
                let refusal = PlayRefusal {
                    component_type_names: uncopyable.type_names,
                    resource_type_names: Vec::new(),
                };
                pill_core::warn!(
                    target: telemetry_target::ECS,
                    reason = %refusal,
                    "[engine] play refused"
                );
                return Err(refusal);
            }
        };
        let resource_copy = match self.world.copy_game_state_resources() {
            Ok(resource_copy) => resource_copy,
            Err(uncopyable) => {
                let refusal = PlayRefusal {
                    component_type_names: Vec::new(),
                    resource_type_names: uncopyable.type_names,
                };
                pill_core::warn!(
                    target: telemetry_target::ECS,
                    reason = %refusal,
                    "[engine] play refused"
                );
                return Err(refusal);
            }
        };

        // Step 3: Take the copy. Nothing above this line changed the world, so
        // a refusal leaves the session exactly as it was.
        let entity_count = self.world.entity_count();
        let component_count = self.world.component_row_count();
        self.world.hold_entity_storage(held_storage);
        self.world.hold_resource_copy(resource_copy);
        self.run_mode = RunMode::Playing;
        self.systems_paused = false;
        // Step 4: Every script the edit world holds starts existing for this
        // session, so the first frames look like a scene load rather than a
        // world that was already running.
        self.world.queue_scripts_for_play_start();
        pill_core::info!(
            target: telemetry_target::ECS,
            entities = entity_count,
            components = component_count,
            "[engine] play entered"
        );
        Ok(())
    }

    /// Runs `run` with `origin` as the world's change origin, and puts the
    /// previous origin back afterwards.
    ///
    /// The editor applies a batch and flushes it inside one scope, so the
    /// commands it queues are executed under the same origin they were queued
    /// under. Game code never sets one, which is why its changes report
    /// [`ChangeOrigin::Simulation`].
    pub fn with_change_origin<R>(
        &mut self,
        origin: ChangeOrigin,
        run: impl FnOnce(&mut Engine) -> R,
    ) -> R {
        let previous = self.world.change_origin();
        self.world.set_change_origin(origin);
        let result = run(self);
        self.world.set_change_origin(previous);
        result
    }

    /// Stops a play session: the edit world comes back and the game's changes
    /// are discarded.
    ///
    /// Everything the game did to entities, to components and to a `Restore`
    /// resource is undone, including a resource the game inserted. A `Keep`
    /// resource keeps whatever the game did to it, and a `Rebuild` resource is
    /// told about the restore through the world's restore epoch.
    ///
    /// Returns whether a session was running.
    pub fn exit_play(&mut self) -> bool {
        if self.world.held_entity_storage.is_none() {
            return false;
        }
        // Step 1: Every running script stops and is told it is being destroyed.
        // The restore discards the play rows, which is the one removal no
        // command describes, so the session reports it here while the rows are
        // still in place.
        self.world
            .run_play_stop_script_passes(&mut self.command_queue);
        let restored = self.world.restore_held_entity_storage();
        debug_assert!(
            restored,
            "a held entity storage was found before the restore"
        );
        // Step 2: Put every copied resource back, and remove the ones the game
        // inserted: a `Restore` resource with no copy was not there at Play.
        self.world.restore_held_resources();
        // Step 3: Commands queued by the last play frame - or by a hook of the
        // stop passes - belong to that world, not to the restored one.
        self.command_queue.clear();
        // Step 4: The session is over: back to editing, unpaused.
        self.run_mode = RunMode::Editing;
        self.systems_paused = false;
        // Step 5: The edit world's scripts start existing again, which is what
        // a restored script hears about. Nothing starts running: no script has
        // begun a session.
        self.world.queue_scripts_for_play_stop_restore();
        pill_core::info!(
            target: telemetry_target::ECS,
            restore_epoch = self.world.restore_epoch(),
            "[engine] play exited"
        );
        true
    }

    /// Runs the update systems once on the next `process_frame`.
    ///
    /// The editor's step control uses it. While the engine is paused, the next
    /// frame runs the update systems once and leaves the engine paused. While
    /// the engine is running, the call changes nothing, because the next frame
    /// runs the systems anyway.
    pub fn request_single_step(&mut self) {
        self.single_step_pending = true;
    }

    /// Limits the frame rate to `fps` frames per second.
    ///
    /// After completing the frame's work, `process_frame` sleeps the
    /// remaining budget. Pass `0.0` or `f64::INFINITY` to disable.
    ///
    /// # Examples
    /// ```no_run
    /// # use pill_engine::*;
    /// # let mut engine = Engine::new();
    /// engine.set_fps_limit(30.0); // Cap at 30 FPS
    /// ```
    pub fn set_fps_limit(&mut self, fps: f64) {
        self.frame_budget = if fps > 0.0 && fps.is_finite() {
            Some(Duration::from_nanos((1_000_000_000.0 / fps) as u64))
        } else {
            None
        };
    }

    /// Sets whether a deferred-command failure fails the frame.
    ///
    /// When `true`, `process_frame` returns every failure it collected and the
    /// caller decides what to do; when `false` (the default), failures are
    /// logged and the frame reports success.
    pub fn set_should_exit_on_error(&mut self, should_exit_on_error: bool) {
        self.should_exit_on_error = should_exit_on_error;
    }

    /// Returns whether a deferred-command failure fails the frame.
    pub fn should_exit_on_error(&self) -> bool {
        self.should_exit_on_error
    }

    /// Sets whether the frame-rate limiter's sleep is traced.
    ///
    /// The zone is named `frame wait`; turn it off to keep the timeline tidy.
    pub fn set_trace_frame_wait(&mut self, should_trace_frame_wait: bool) {
        self.should_trace_frame_wait = should_trace_frame_wait;
    }

    /// Enables a system by name.
    ///
    /// The name-keyed convenience over [`Self::set_system_enabled`]. Names are
    /// not unique, so a caller that has a [`SystemSnapshot`] should pass that
    /// snapshot's index to [`Self::set_system_enabled_at`] instead: this one
    /// hits the first match.
    ///
    /// Returns `true` if the system was found and enabled, `false` otherwise.
    pub fn enable_system(&mut self, name: &str) -> bool {
        self.set_system_enabled(name, true)
    }

    /// Disables a system by name.
    ///
    /// The name-keyed convenience over [`Self::set_system_enabled`]; see
    /// [`Self::enable_system`] for the caveat about duplicate names.
    ///
    /// Returns `true` if the system was found and disabled, `false` otherwise.
    pub fn disable_system(&mut self, name: &str) -> bool {
        self.set_system_enabled(name, false)
    }

    /// Sets the enabled state of a system by name.
    ///
    /// Returns `true` if the system was found, `false` otherwise. The batch
    /// graph does not change: a disabled system keeps its place and is skipped
    /// at dispatch, because batches are decided by conflicts alone.
    pub fn set_system_enabled(&mut self, name: &str, enabled: bool) -> bool {
        for system in &mut self.systems {
            if system.name == name {
                system.enabled = enabled;
                return true;
            }
        }
        false
    }

    // -------------------------------------------------------------------------
    // Inspection
    // -------------------------------------------------------------------------

    /// Checks whether a system is enabled.
    ///
    /// Returns `None` if the system was not found.
    pub fn is_system_enabled(&self, name: &str) -> Option<bool> {
        self.systems
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.enabled)
    }

    /// Checks whether any system carries this name.
    ///
    /// This asks only about existence. [`Self::is_system_enabled`] answers the
    /// same question but reads as one about being enabled. Names are not
    /// unique, so this returns `true` if any registration has the name.
    pub fn is_registered(&self, name: &str) -> bool {
        self.system_index(name).is_some()
    }

    /// The registration index of the first system named `name`.
    ///
    /// That index is what [`Self::set_system_enabled_at`] takes and what
    /// [`SystemSnapshot::index`] carries, so a caller that needs to mutate a
    /// specific registration resolves it here rather than by name.
    pub fn system_index(&self, name: &str) -> Option<usize> {
        self.systems.iter().position(|s| s.name == name)
    }

    /// Takes a snapshot of every registered system, in registration order.
    ///
    /// The editor's Systems tab reads this; it never requires knowing the
    /// concrete system type. `hot_patchable` reports whether the system has a
    /// dispatch slot in the hot-patch registry (only present when the host was
    /// built with the `development` feature).
    pub fn system_snapshots(&self) -> Vec<SystemSnapshot> {
        self.systems
            .iter()
            .enumerate()
            .map(|(index, system)| {
                let name_is_ambiguous = self
                    .systems
                    .iter()
                    .filter(|other| other.name == system.name)
                    .count()
                    > 1;
                SystemSnapshot {
                    index,
                    name: system.name.clone(),
                    owner: system.system_owner,
                    enabled: system.enabled,
                    hot_patchable: self.hot_patch_registry.get(&system.name).is_some(),
                    name_is_ambiguous,
                }
            })
            .collect()
    }

    /// Enables or disables the system at `index`.
    ///
    /// Unlike [`Self::set_system_enabled`], which matches by name and so hits
    /// the first of several same-named systems, this targets exactly one
    /// registration. Returns `false` when the index is out of range.
    pub fn set_system_enabled_at(&mut self, index: usize, enabled: bool) -> bool {
        let Some(system) = self.systems.get_mut(index) else {
            return false;
        };
        system.enabled = enabled;
        true
    }

    /// Prints the execution graph for debugging.
    ///
    /// Rebuilds the batches first when the system set changed since the last
    /// frame, so the report matches what the next frame would dispatch.
    pub fn print_execution_graph(&mut self) {
        self.scheduler.ensure_execution_graphs(
            self.systems.iter().map(|registered_system| {
                (&registered_system.system_access, registered_system.phase)
            }),
        );
        let mut lines = Vec::new();
        for phase in RunPhase::BATCHED {
            lines.push(format!("Phase {}:", phase.label()));
            for (batch_index, batch) in self.scheduler.execution_graph_of(phase).iter().enumerate()
            {
                lines.push(format!(
                    "Batch {batch_index}: {} systems (parallel)",
                    batch.len()
                ));
                for &system_index in batch {
                    let Some(registered_system) = self.systems.get(system_index) else {
                        lines.push(format!("  - <unknown system {system_index}>"));
                        continue;
                    };
                    let system_access = &registered_system.system_access;
                    lines.push(format!(
                        "  - {} (reads: {}, writes: {}, res_reads: {}, res_writes: {}, commands: {})",
                        registered_system.name,
                        system_access.reads_count(),
                        system_access.writes_count(),
                        system_access.resource_reads_count(),
                        system_access.resource_writes_count(),
                        system_access.uses_commands
                    ));
                }
            }
        }
        pill_core::info!(
            target: telemetry_target::ECS,
            "{}",
            pill_core::telemetry::log_block("System execution graph", lines)
        );
    }

    // -------------------------------------------------------------------------
    // World Access
    // -------------------------------------------------------------------------

    /// Returns a shared reference to the world.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// Returns a mutable reference to the world.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    // -------------------------------------------------------------------------
    // Input
    // -------------------------------------------------------------------------

    /// Queues an input event from the frontend. The event takes effect at the
    /// start of the next frame. See [`crate::input`].
    pub fn push_input_event(&mut self, event: crate::input::InputEvent) {
        if let Some(input) = self.world.get_resource_mut::<crate::input::Input>() {
            input.push_event(event);
        }
    }

    /// Takes the rumble requests the systems queued, so the frontend that owns
    /// the gamepads can play them.
    pub fn take_rumble_requests(&mut self) -> Vec<crate::input::RumbleRequest> {
        self.world
            .get_resource_mut::<crate::input::Input>()
            .map(crate::input::Input::take_rumble_requests)
            .unwrap_or_default()
    }

    // -------------------------------------------------------------------------
    // System Registration
    // -------------------------------------------------------------------------

    /// Registers a system with a name.
    ///
    /// The system can be any function whose parameters all implement
    /// [`SystemParam`]. The name is used for debugging and profiling.
    ///
    /// This is the **host-facing** call: a project or module declares its
    /// systems with `#[system]` instead, and its entry point registers them
    /// before the artifact's own code runs. Use this for a system the host
    /// itself owns, such as a renderer's frame build.
    ///
    /// A host that wants the declared systems and the hand-registered ones in
    /// one list registers the declarations first - the artifact entry points do
    /// exactly that - because conflicting systems run in registration order.
    ///
    /// # Examples
    /// ```no_run
    /// # use pill_engine::*;
    /// # #[derive(Debug, Clone)] struct Position { x: f32, y: f32 }
    /// # impl Component for Position {}
    /// # #[derive(Debug, Clone)] struct Velocity { vx: f32, vy: f32 }
    /// # impl Component for Velocity {}
    /// # let mut engine = Engine::new();
    /// engine.register_system("movement", |
    ///     mut commands: Commands,
    ///     query: Query<(Entity, &mut Position, &Velocity)>
    /// | {
    ///     // System implementation
    /// });
    /// ```
    pub fn register_system<F, Input>(&mut self, name: impl Into<String>, system: F)
    where
        F: IntoSystem<Input>,
        Input: SystemParam,
    {
        self.register_system_with_options(name, SystemOptions::default(), system);
    }

    /// Registers a system with the order, phase and edit-mode gate of
    /// `options`.
    ///
    /// This is the entry point every other registering method delegates to.
    /// The system is placed in the frame by `(options.phase, options.order)`,
    /// and the engine keeps that order across every artifact, so a number an
    /// extension picks orders its systems against a project's.
    pub fn register_system_with_options<F, Input>(
        &mut self,
        name: impl Into<String>,
        options: SystemOptions,
        system: F,
    ) where
        F: IntoSystem<Input>,
        Input: SystemParam,
    {
        let (system_name, system_access, boxed_system) = self.prepare_system(name, system);
        self.push_system(system_name, options, system_access, boxed_system);
    }

    /// Derives a registration's access pattern and boxes its system.
    ///
    /// The part both name-registering entry points share: it records the
    /// access the parameter tuple declares, then boxes the system. With hot
    /// patching on, the box goes through a dispatch slot, so a later patch
    /// replaces one pointer instead of registering the system again.
    fn prepare_system<F, Input>(
        &mut self,
        name: impl Into<String>,
        system: F,
    ) -> (String, SystemAccess, Box<dyn System>)
    where
        F: IntoSystem<Input>,
        Input: SystemParam,
    {
        let mut system_access = SystemAccess::new();
        Input::report_access(&mut system_access);
        let system_name: String = name.into();

        // Box the system. With hot patching on, calls to the user's function
        // go through a dispatch slot the engine keeps a handle to, so a patch
        // replaces one pointer instead of registering the system again.
        #[cfg(feature = "development")]
        let boxed_system = {
            let slot = std::sync::Arc::new(crate::hot_patch::HotSlot::new());
            // The qualified path is what a generated patch reports, and what a
            // panic in a patched body is attributed to; the display name is
            // the fallback when the function path cannot be resolved.
            let function_path = crate::hot_patch::function_path::<F>();
            let faults_key = function_path.clone().unwrap_or_else(|| system_name.clone());
            let hot_system = system.into_hot_system(std::sync::Arc::clone(&slot), faults_key);

            // Register the slot under both names. Callers and logs use the
            // display name; `#[pill_hot]` and a generated patch use the
            // function's full path. The two often differ (the `native` test
            // project registers `physics_system` as `"ball_physics"`), and a
            // patch pipeline that knew only one of them would not find the
            // slot.
            self.hot_patch_registry
                .insert(system_name.clone(), std::sync::Arc::clone(&slot));
            if let Some(path) = function_path {
                if path != system_name {
                    self.hot_patch_registry.insert(path, slot);
                }
            }
            hot_system
        };
        #[cfg(not(feature = "development"))]
        let boxed_system = system.into_system();

        (system_name, system_access, boxed_system)
    }

    // -------------------------------------------------------------------------
    // Hot patching
    // -------------------------------------------------------------------------

    /// Replaces one registered system's implementation without registering it
    /// again.
    ///
    /// The system keeps its identity, its place in the scheduler graph, its
    /// access metadata and its change-detection ticks. The [`World`] is
    /// untouched. The replacement is used from the system's next run, so a
    /// caller should apply patches at a frame boundary, when no system is
    /// running.
    ///
    /// `signature_hash` must be the value
    /// [`hot_patch::signature_hash`](crate::hot_patch::signature_hash) returns
    /// for the replacement's parameter tuple and return type. A different hash
    /// is refused, because the running call site was compiled for the old
    /// shape.
    ///
    /// # Errors
    ///
    /// Returns [`HotPatchError`](crate::hot_patch::HotPatchError) when no
    /// hot-patchable system is registered under `name` (`UnknownSystem`), when
    /// the signature changed (`SignatureMismatch`), or when `address` is null
    /// (`NullAddress`). In every case the running implementation stays in
    /// place.
    pub fn hot_patch(
        &mut self,
        name: &str,
        address: usize,
        signature_hash: u64,
    ) -> Result<(), crate::hot_patch::HotPatchError> {
        let slot = self.hot_patch_registry.get(name).ok_or_else(|| {
            crate::hot_patch::HotPatchError::UnknownSystem {
                name: name.to_string(),
            }
        })?;
        slot.install(name, address, signature_hash)
    }

    /// Returns the implementation a system was registered with, and its
    /// signature.
    ///
    /// A host uses this to undo a patch and go back to the code the running
    /// build was compiled with. `None` when no such system is registered, or
    /// when it is not hot-patchable.
    pub fn hot_patch_baseline(&self, name: &str) -> Option<(usize, u64)> {
        let slot = self.hot_patch_registry.get(name)?;
        let original = slot.original();
        if original == 0 {
            return None;
        }
        Some((original, slot.signature_hash()))
    }

    /// Read-only access to the hot-patch registry, for hosts and tooling.
    pub fn hot_patch_registry(&self) -> &crate::hot_patch::HotPatchRegistry {
        &self.hot_patch_registry
    }

    /// Registers a system whose access pattern was derived outside Rust's
    /// [`SystemParam`] machinery (for example from a managed-language system
    /// signature).
    ///
    /// # Safety
    ///
    /// `system_access` must describe every component/resource the system may
    /// touch. An incomplete declaration can make the scheduler run conflicting
    /// systems concurrently and cause undefined behavior.
    pub unsafe fn register_system_with_access<F>(
        &mut self,
        name: impl Into<String>,
        system_access: SystemAccess,
        system: F,
    ) where
        F: FnMut(&mut World, &mut CommandQueue) -> Result<(), SystemError> + Send + 'static,
    {
        // SAFETY: the caller of this unsafe function promised the same
        // completeness this forwards, so the requirement is unchanged.
        unsafe {
            self.register_system_with_access_and_options(
                name,
                SystemOptions::default(),
                system_access,
                system,
            )
        }
    }

    /// Registers a hand-declared-access system with explicit options.
    ///
    /// The form a managed host uses: the order, phase and edit-mode gate come
    /// from `options` and the access pattern was derived outside Rust's
    /// [`SystemParam`] machinery.
    ///
    /// # Safety
    ///
    /// `system_access` must describe every component/resource the system may
    /// touch. An incomplete declaration can make the scheduler run conflicting
    /// systems concurrently and cause undefined behavior.
    pub unsafe fn register_system_with_access_and_options<F>(
        &mut self,
        name: impl Into<String>,
        options: SystemOptions,
        system_access: SystemAccess,
        system: F,
    ) where
        F: FnMut(&mut World, &mut CommandQueue) -> Result<(), SystemError> + Send + 'static,
    {
        self.push_system(name.into(), options, system_access, Box::new(system));
    }

    /// Adds one system to the list and marks the scheduler out of date.
    ///
    /// Every registration entry point ends here, so they all set the owner,
    /// the phase, the order and the enabled state the same way, and they all
    /// invalidate the graph.
    ///
    /// The record is inserted where its `(phase, order)` belongs rather than
    /// appended: the scheduler derives execution order from the list's
    /// positions, and the list spans every artifact, so insertion is what makes
    /// an `order` number global. Equal pairs keep insertion order, so a system
    /// an artifact's `register` half adds after its declarations runs after
    /// them, and a reload that re-registers a module's systems puts them back
    /// where they were.
    ///
    /// The batches are not built here: the next frame rebuilds them once from
    /// the whole list, so a module that registers many systems pays for one
    /// rebuild, not one per system.
    fn push_system(
        &mut self,
        name: String,
        options: SystemOptions,
        mut system_access: SystemAccess,
        system: Box<dyn System>,
    ) {
        // The conditions decide whether the system runs, so what they read has
        // to be part of what the batches know about it: a condition and a
        // writer of what it reads must not share a batch either.
        let SystemOptions {
            order,
            phase,
            run_in_edit_mode,
            conditions,
        } = options;
        for condition in &conditions {
            system_access.merge(&condition.access);
        }
        let registered_system = RegisteredSystem {
            phase,
            order,
            edit_mode: run_in_edit_mode,
            name,
            system_owner: self.active_owner,
            system,
            system_access,
            // Starts disabled when its owner is suspended (see
            // `disabled_owners`).
            enabled: !self.disabled_owners.contains(&self.active_owner),
            last_run_tick: 0,
            average_duration: 0,
            last_duration: 0,
            conditions,
        };
        // Step 1: Find the first system that must run after this one - a later
        // phase, or the same phase at a higher order - and insert before it.
        // Stopping at the first such system leaves every equal pair in front,
        // which is the registration-order tiebreak.
        let insert_at = self
            .systems
            .iter()
            .position(|existing| {
                (existing.phase, existing.order)
                    > (registered_system.phase, registered_system.order)
            })
            .unwrap_or(self.systems.len());
        self.systems.insert(insert_at, registered_system);
        self.scheduler.invalidate();
    }

    /// Attributes every system registered from now on to `system_owner`.
    ///
    /// The host wraps a module's initialization in this call and
    /// [`Self::end_module_registration`], so module code registers systems by
    /// name while the engine records which module each system came from.
    pub fn begin_module_registration(&mut self, system_owner: SystemOwner) {
        self.active_owner = system_owner;
    }

    /// Returns registration attribution to [`SystemOwner::PROJECT`].
    pub fn end_module_registration(&mut self) {
        self.active_owner = SystemOwner::PROJECT;
    }

    /// Suspends or resumes every system registered by `system_owner`.
    ///
    /// Suspending also covers systems `system_owner` registers later, until it
    /// is resumed: a module generation that initializes during a suspension
    /// registers its systems disabled. The suspension survives
    /// [`Self::clear_systems_owned_by`], which removes the systems but not the
    /// owner's state. Resuming turns on every system of `system_owner`,
    /// including any that [`Self::set_system_enabled`] disabled in the
    /// meantime.
    ///
    /// Returns how many systems `system_owner` has now. The batches are not
    /// rebuilt: they list every registered system, and a disabled system is
    /// skipped at dispatch.
    pub fn set_systems_enabled_for_owner(
        &mut self,
        system_owner: SystemOwner,
        enabled: bool,
    ) -> usize {
        // Step 1: Record the owner's state for systems registered later.
        if enabled {
            self.disabled_owners
                .retain(|disabled| *disabled != system_owner);
        } else if !self.disabled_owners.contains(&system_owner) {
            self.disabled_owners.push(system_owner);
        }

        // Step 2: Apply it to the systems registered so far.
        let mut owned_system_count = 0;
        for system in self
            .systems
            .iter_mut()
            .filter(|system| system.system_owner == system_owner)
        {
            owned_system_count += 1;
            system.enabled = enabled;
        }
        owned_system_count
    }

    /// Checks whether the systems of `system_owner` may run, that is, whether
    /// the owner is not suspended by [`Self::set_systems_enabled_for_owner`].
    pub fn is_owner_enabled(&self, system_owner: SystemOwner) -> bool {
        !self.disabled_owners.contains(&system_owner)
    }

    /// Removes only the systems registered by `system_owner`.
    ///
    /// Used when hot reloading one extension: the replacement library's
    /// systems are different function pointers, so the retiring generation's
    /// systems must go, while the project and every other module keep running.
    /// Component registrations, entities, and resources are untouched.
    ///
    /// Returns the number of systems removed.
    pub fn clear_systems_owned_by(&mut self, system_owner: SystemOwner) -> usize {
        // Step 1: Remove the dispatch slots of the systems that are about to
        // go, so a later patch cannot install into a system that no longer
        // runs.
        let mut removed_count = 0;
        for registered_system in &self.systems {
            if registered_system.system_owner == system_owner {
                self.hot_patch_registry.remove(&registered_system.name);
                removed_count += 1;
            }
        }
        if removed_count == 0 {
            return 0;
        }

        // Step 2: Remove the systems. The graph's indices address the system
        // list, whose positions have shifted, so the next frame rebuilds it
        // from the survivors.
        self.systems
            .retain(|registered_system| registered_system.system_owner != system_owner);
        self.scheduler.invalidate();
        removed_count
    }

    /// Removes all registered systems, clearing the scheduler state.
    ///
    /// Their dispatch slots are retired with them, so nothing can be patched
    /// into a system that no longer runs.
    ///
    /// Component registrations, entities, and resources are **not** cleared —
    /// only systems are affected. The world state persists.
    pub fn clear_systems(&mut self) {
        self.systems.clear();
        // With every system gone, no dispatch slot may stay patchable.
        self.hot_patch_registry = crate::hot_patch::HotPatchRegistry::new();
        // The graph and its conflict matrix describe systems that no longer
        // exist; the next frame derives an empty one from the empty list.
        self.scheduler.clear();
    }

    /// Drains the system failures recorded since the previous drain.
    ///
    /// The reporting boundary calls this once per frame and logs each failure
    /// as a message. A failed system never aborts its batch, so the rest still
    /// finish the frame.
    pub fn drain_system_failures(&mut self) -> Vec<SystemFailure> {
        std::mem::take(&mut self.system_failures)
    }

    /// Reads the system failures recorded since the previous drain without
    /// clearing them.
    pub fn system_failures(&self) -> &[SystemFailure] {
        &self.system_failures
    }

    // -------------------------------------------------------------------------
    // Frame Processing
    // -------------------------------------------------------------------------

    /// Runs a one-shot command producer and flushes its deferred mutations.
    ///
    /// This is primarily used by scripting-runtime startup hooks. The callback
    /// receives the same world/queue pair as an ordinary command-producing
    /// system, and all structural changes become visible only after it returns.
    ///
    /// # Errors
    ///
    /// Returns one entry per command that could not be applied.
    pub fn run_deferred_commands<F>(&mut self, produce_commands: F) -> Result<(), Vec<CommandError>>
    where
        F: FnOnce(&mut World, &mut CommandQueue),
    {
        produce_commands(&mut self.world, &mut self.command_queue);
        // One-shot startup has no following frame in which to surface a
        // failure, so always return command errors to its caller.
        self.command_queue
            .execute_queued_commands(&mut self.world, true)
    }

    /// Runs a command-producing closure without applying its commands.
    ///
    /// The transactional counterpart of [`Self::run_deferred_commands`]: the
    /// caller commits with [`Self::flush_deferred_commands`] or rolls back
    /// with [`Self::discard_deferred_commands`], so a failed phase can never
    /// leave partial world state behind.
    pub fn queue_deferred_commands<F>(&mut self, produce_commands: F)
    where
        F: FnOnce(&mut World, &mut CommandQueue),
    {
        produce_commands(&mut self.world, &mut self.command_queue);
    }

    /// Applies every command queued since the last flush.
    ///
    /// # Errors
    ///
    /// Returns one entry per command that could not be applied.
    pub fn flush_deferred_commands(&mut self) -> Result<(), Vec<CommandError>> {
        self.command_queue
            .execute_queued_commands(&mut self.world, true)
    }

    /// Discards every command queued since the last flush without applying it.
    pub fn discard_deferred_commands(&mut self) {
        self.command_queue.clear();
    }

    /// Runs one frame: the pre-project phase, the first command flush, the
    /// project phase, the scripts, the second flush and the post-project phase.
    ///
    /// A frame goes like this:
    /// 1. The pre-project systems run, in parallel batches when that is
    ///    enabled, and the commands they queued are applied.
    /// 2. The project systems run on that result, then the scripts, and the
    ///    commands both queued are applied.
    /// 3. The post-project systems run, on the calling thread, on the frame's
    ///    finished state.
    ///
    /// While the systems run they queue commands (create, destroy, add a
    /// component and so on) instead of changing the world directly. Splitting
    /// the frame this way keeps structural changes from disturbing systems that
    /// are still running.
    ///
    /// # Errors
    ///
    /// Returns `Err(Vec<CommandError>)` when [`should_exit_on_error`] is
    /// `true` and one or more deferred commands fail.  When the flag is
    /// `false` (default), errors are logged and `Ok(())` is returned. A
    /// failure in either flush is collected, so one frame never hides the
    /// other's errors.
    ///
    /// [`should_exit_on_error`]: Engine::should_exit_on_error
    pub fn process_frame(&mut self) -> Result<(), Vec<CommandError>> {
        let frame_start = Instant::now();
        let mut deferred_command_flush: Result<(), Vec<CommandError>> = Ok(());

        {
            // Tracy zone: frame = actual work, not including sleep
            let _tracy_frame = crate::profile_scope!(
                "frame",
                [
                    ("Active entities in world: {}", self.world.entity_count()),
                    (
                        "Archetypes in world: {}",
                        self.world.storage.archetypes.len()
                    )
                ]
            );

            // Step 1: Bump the world change tick and reset the per-frame debug
            // state.
            //
            // Bumping the tick first means every change-detection comparison a
            // mutable query makes during this frame uses a fresh value.
            self.world.increment_change_tick();

            // Advance the clock before anything reads it, so every system in
            // this frame sees the same time and none sees last frame's.
            //
            // This is done here rather than by a registered system because the
            // scheduler batches systems in registration order. A time system
            // would only run before its readers if every project registered it
            // first; a project that registered one late would give its earlier
            // systems stale values, and nothing in the API would show it. The
            // untracked `get_resource_mut` is deliberate: a change tick for a
            // resource that changes on every frame tells a reader nothing.
            if let Some(time) = self.world.get_resource_mut::<crate::time::Time>() {
                time.advance();
            }

            // Apply the input the frontend queued since the last frame, for
            // the same reason and in the same way as the clock: every system
            // in this frame sees the same keys, buttons and motion.
            if let Some(input) = self.world.get_resource_mut::<crate::input::Input>() {
                input.begin_frame();
            }

            // Debug-only: clear the resource write-lock tracker so that the
            // isolation check only guards within a single frame.
            #[cfg(debug_assertions)]
            self.world.debug_clear_resource_locks();

            // Step 2: Decide once whether this frame runs every system. Playing
            // and unpaused runs them all, a step runs them all once, and edit
            // mode - or a pause - runs only the systems that opted in.
            self.runs_every_system_this_frame = self.runs_every_system();

            // Step 2: Rebuild the execution graphs when the system set changed
            // since the last frame, so parallel batches reflect the active
            // set. Each batched phase's matrix and batches are derived here,
            // once per change, from the systems' own access patterns.
            self.scheduler
                .ensure_execution_graphs(self.systems.iter().map(|registered_system| {
                    (&registered_system.system_access, registered_system.phase)
                }));

            // Step 3: Emit per-archetype memory breakdowns when the archetype
            // set changed since the last frame.
            self.emit_archetype_memory_breakdown();

            // Step 4: Run the pre-project phase, in parallel batches or one
            // after another.
            //
            // Every phase runs every frame, and the per-system gate above
            // decides what in it runs: the game's systems run while it plays
            // and is not paused, and a system that opted in to edit mode runs
            // either way. A paused engine therefore keeps its opted-in systems
            // - the physics outlines among them - and stops the rest.
            self.run_phase(RunPhase::PreProject);

            // Step 5: Apply the commands the pre-project phase queued, so the
            // project phase reads the world they produced.
            if let Err(errors) = self.execute_deferred_commands() {
                deferred_command_flush = Err(errors);
            }

            // Step 6: Run the project phase, the game's own frame work.
            //
            // A single step runs the systems once; clearing the pending flag
            // here is what pauses the engine again.
            self.run_phase(RunPhase::Project);
            self.single_step_pending = false;

            // Compute parallel-utilization metrics and emit Tracy plots.
            #[cfg(feature = "profiling")]
            if !self.systems.is_empty() {
                let durations: Vec<u64> = self.systems.iter().map(|s| s.last_duration).collect();
                // Both batched phases run beside each other, so one plot over
                // their combined batches describes the frame's parallelism.
                let mut parallel_batches: Vec<Vec<usize>> = self
                    .scheduler
                    .execution_graph_of(RunPhase::PreProject)
                    .to_vec();
                parallel_batches.extend(
                    self.scheduler
                        .execution_graph_of(RunPhase::Project)
                        .iter()
                        .cloned(),
                );
                crate::profiling::emit_parallel_utilization_plots(
                    &durations,
                    frame_start.elapsed().as_nanos() as u64,
                    rayon::current_num_threads(),
                    &parallel_batches,
                );
            }

            // Step 5: Update the scripts. Each script reads the world, changes
            // only its own component and queues its other changes as commands.
            //
            // A script runs while the game runs, and while the editor edits
            // when its declaration opted in to edit mode. Pause and step keep
            // their meaning: a paused game runs neither.
            if self.runs_every_system_this_frame || self.run_mode == RunMode::Editing {
                let _zone = crate::profile_scope!("update scripts");
                self.world
                    .run_script_lifecycle_passes(&mut self.command_queue, self.run_mode);
                self.world
                    .update_scripts(&mut self.command_queue, self.run_mode);
            }
            crate::profile_secondary_frame_mark!("scripts");

            // Step 6: Execute all deferred commands (including those from scripts).
            if let Err(errors) = self.execute_deferred_commands() {
                match &mut deferred_command_flush {
                    Ok(()) => deferred_command_flush = Err(errors),
                    Err(collected) => collected.extend(errors),
                }
            }
            crate::profile_secondary_frame_mark!("commands");

            // Step 8: Run the post-project phase, on the calling thread after
            // the frame's last flush: it is where the presentation systems
            // draw the frame they have just finished computing.
            self.run_systems_sequential(RunPhase::PostProject);

            // Step 7: Warn about duplicate parallel-iterator labels seen this
            // frame, then emit the per-frame plots and metrics.
            {
                let mut iterator_timings = self
                    .world
                    .iterator_timings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if !iterator_timings
                    .visited_duplicated_iterator_labels
                    .is_empty()
                {
                    let duplicates =
                        std::mem::take(&mut iterator_timings.visited_duplicated_iterator_labels);
                    crate::profile_warn!(
                        "duplicate parallel-iterator labels this frame: {:?} - two iterators sharing a .label() corrupt per-label timing",
                        duplicates
                    );
                    warn!(
                        target: telemetry_target::ECS,
                        labels = %duplicates
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", "),
                        "duplicate parallel-iterator labels this frame; two iterators sharing a .label() corrupt per-label timing"
                    );
                }
                iterator_timings.visited_iterator_labels.clear();
            }

            crate::profile_frame_mark!();

            self.emit_frame_telemetry(frame_start);
        } // <- frame zone ends here

        // Wait out the rest of the frame budget outside the frame zone.
        self.wait_for_frame_budget(frame_start);

        deferred_command_flush
    }

    /// Runs the systems of one phase.
    ///
    /// A batched phase uses the scheduler's parallel batches when parallel
    /// execution is on; [`RunPhase::PostProject`] always runs sequentially, on
    /// the calling thread, because it runs after the frame's last flush.
    fn run_phase(&mut self, phase: RunPhase) {
        let batches_itself = phase.is_batched() && self.parallel_execution_enabled;
        if batches_itself && self.systems.len() > 1 {
            self.run_systems_parallel(phase);
        } else {
            self.run_systems_sequential(phase);
        }
    }

    /// Applies every command the systems queued so far.
    ///
    /// One frame flushes twice - after the pre-project phase, and after the
    /// project phase with the scripts - so the profiling scope and the
    /// error-on-failure rule live here rather than at both call sites.
    fn execute_deferred_commands(&mut self) -> Result<(), Vec<CommandError>> {
        let _zone = crate::profile_scope!(
            "execute commands",
            [(
                "Commands queued this frame: {}",
                !self.command_queue.is_empty()
            )]
        );
        self.command_queue
            .execute_queued_commands(&mut self.world, self.should_exit_on_error)
    }

    /// Emits the per-frame Tracy plots and the metrics-reporter gauges.
    #[cfg_attr(not(feature = "profiling"), allow(unused_variables))]
    fn emit_frame_telemetry(&self, frame_start: Instant) {
        // Emit per-frame time-series plots for Tracy (native plot API).
        crate::profile_plot!(entity_count, self.world.entity_count() as f64);
        crate::profile_plot!(archetype_count, self.world.storage.archetypes.len() as f64);
        crate::profile_plot!(frame_time_us, frame_start.elapsed().as_micros() as f64);
        // Memory estimate: sums archetype columns, entity locations, resources, and overhead.
        crate::profile_plot!(
            memory_estimate_kb,
            self.world.memory_estimate() as f64 / 1024.0
        );
        // ECS internals.
        crate::profile_plot!(
            free_entity_ids,
            self.world.storage.free_entity_ids.len() as f64
        );
        crate::profile_plot!(component_types, self.world.component_registry.len() as f64);
        crate::profile_plot!(resource_count, self.world.resources.len() as f64);
        crate::profile_plot!(registered_systems, self.systems.len() as f64);
        crate::profile_plot!(
            execution_batches,
            (self
                .scheduler
                .execution_graph_of(RunPhase::PreProject)
                .len()
                + self.scheduler.execution_graph_of(RunPhase::Project).len()) as f64
        );
        crate::profile_plot!(
            commands_executed,
            self.world.commands_executed_this_frame as f64
        );

        // The same numbers also go through the shared `metrics` recorder,
        // which passes them to Tracy's plot API while a profiler is attached.
        // The recorder keeps no copy, so a reader that wants these values reads
        // Tracy or the logs.
        //
        // The time recorded here measures this pass only, which is why it is
        // named beside the world numbers rather than as the frame time:
        // `engine.frame_time_ms` is the frontend's frame, which also covers
        // whatever it draws between two of these passes, so the two numbers
        // differ.
        #[cfg(feature = "metrics")]
        {
            metrics::gauge!("ecs.entities").set(self.world.entity_count() as f64);
            metrics::gauge!("ecs.storage.archetypes")
                .set(self.world.storage.archetypes.len() as f64);
            metrics::gauge!("ecs.systems").set(self.systems.len() as f64);
            metrics::gauge!("ecs.components").set(self.world.component_registry.len() as f64);
            metrics::histogram!("ecs.frame_time_ms")
                .record(frame_start.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// Emits one memory-breakdown block when the archetype set has changed.
    #[cfg_attr(not(feature = "profiling"), allow(unused_variables))]
    fn emit_archetype_memory_breakdown(&mut self) {
        let current_generation = self.world.storage.archetype_generation;
        if current_generation == self.last_archetype_generation {
            return;
        }
        self.last_archetype_generation = current_generation;
        for archetype in self.world.storage.archetypes.values() {
            crate::profile_message!(
                "archetype {:?}: {} entities, {} component types, ~{} bytes",
                archetype.id,
                archetype.entity_count(),
                archetype.component_types.len(),
                archetype.memory_estimate(),
            );
        }
        crate::profile_message!(
            "total world memory estimate: ~{} bytes ({} KB)",
            self.world.memory_estimate(),
            self.world.memory_estimate() / 1024,
        );
    }

    /// Waits out whatever remains of the frame budget, outside the frame zone.
    fn wait_for_frame_budget(&self, frame_start: Instant) {
        let Some(budget) = self.frame_budget else {
            return;
        };
        let elapsed = frame_start.elapsed();
        if elapsed >= budget {
            return;
        }
        if self.should_trace_frame_wait {
            let _zone = crate::profile_scope!(
                "frame wait",
                [
                    ("Frame budget (microseconds): {}", budget.as_micros()),
                    ("Work elapsed (microseconds): {}", elapsed.as_micros())
                ]
            );
            std::thread::sleep(budget - elapsed);
        } else {
            std::thread::sleep(budget - elapsed);
        }
    }

    // -------------------------------------------------------------------------
    // Internal: Sequential Execution
    // -------------------------------------------------------------------------

    /// Runs one phase's systems one after another: when parallel execution is
    /// off, when the phase is not batched, and for
    /// [`RunPhase::PostProject`], which always runs on the calling thread after
    /// the frame's last command flush.
    fn run_systems_sequential(&mut self, phase: RunPhase) {
        let _zone = crate::profile_scope!(
            "systems sequential",
            [
                ("Phase: {}", phase.label()),
                (
                    "{} enabled systems ({} total registered)",
                    self.systems.iter().filter(|s| s.enabled).count(),
                    self.systems.len()
                )
            ]
        );
        let runs_every_system = self.runs_every_system_this_frame;
        let world = &mut self.world;
        let command_queue = &mut self.command_queue;
        let failures = &mut self.system_failures;
        for registered_system in &mut self.systems {
            if !registered_system.enabled || registered_system.phase != phase {
                continue;
            }
            // In edit mode, and while paused, only the systems that opted in
            // run; a step runs them all once.
            if !runs_every_system && !registered_system.edit_mode {
                continue;
            }
            run_registered_system(world, command_queue, failures, registered_system);
        }
        // Reset the baseline so ad-hoc queries between frames behave
        // predictably.
        self.world.system_last_run = 0;
    }

    // -------------------------------------------------------------------------
    // Internal: Parallel Execution
    // -------------------------------------------------------------------------

    /// Runs one batched phase's systems in parallel batches, using the
    /// scheduler's conflict analysis.
    ///
    /// The scheduler guarantees that the systems in one batch touch disjoint
    /// components and resources, so they can run at the same time on Rayon
    /// worker threads. A system that uses [`Commands`] always runs alone,
    /// because it needs exclusive access to the world.
    ///
    /// # Safety
    ///
    /// This function contains `unsafe` code that rebuilds `&mut` references
    /// from raw pointers. It is sound because of the scheduler's guarantees:
    /// systems in one batch access disjoint state, command-using systems run
    /// alone, and every batch index stays within the `systems` vector.
    fn run_systems_parallel(&mut self, phase: RunPhase) {
        let _zone = crate::profile_scope!(
            "run systems parallel",
            [
                ("Phase: {}", phase.label()),
                (
                    "{} enabled systems ({} total)",
                    self.systems.iter().filter(|s| s.enabled).count(),
                    self.systems.len()
                ),
                (
                    "{} parallel batches",
                    self.scheduler.execution_graph_of(phase).len()
                )
            ]
        );
        // Execute each batch
        let batch_count = self.scheduler.execution_graph_of(phase).len();
        for (batch_index, systems_batch) in
            self.scheduler.execution_graph_of(phase).iter().enumerate()
        {
            let batch_size = systems_batch.len();
            let _zone: crate::profiling::TracyZone = crate::profile_scope!(
                "run systems batch {}/{} ({} systems)",
                batch_index + 1,
                batch_count,
                batch_size
            );
            if batch_size == 1 {
                // Single system - run directly on main thread, skip rayon
                let system_index = systems_batch[0];
                let runs_every_system = self.runs_every_system_this_frame;
                let runs = {
                    let registered_system = &self.systems[system_index];
                    // The graph holds this phase's systems alone, so the phase
                    // needs no test here.
                    registered_system.enabled && (runs_every_system || registered_system.edit_mode)
                };
                if runs {
                    let registered_system = &mut self.systems[system_index];
                    run_registered_system(
                        &mut self.world,
                        &mut self.command_queue,
                        &mut self.system_failures,
                        registered_system,
                    );
                }
            } else {
                // Several systems, so run them in parallel with Rayon.
                //
                // Change detection is safe here because the scheduler puts only
                // systems with disjoint component access in one batch. Each
                // system's filters look at ticks reserved for that system, so
                // several systems may read `system_last_run` at the same time.
                // Each system's `last_run_tick` is copied into a per-thread
                // value at dispatch, instead of going through the world field.
                let started_at_tick = self.world.change_tick().get();

                // Step 1: Collect one record per batch member, plus the raw
                // pointer the workers need. The record keeps the member's
                // index, its ticks, its name and whether it runs, so the
                // worker never has to line up separate lists.
                let _zone = crate::profile_scope!("setup systems batch ({} systems)", batch_size);
                let systems_ptr = crate::query::ptr::SendPtrMut::new(self.systems.as_mut_ptr());
                let mut members: Vec<BatchMember<'_>> = Vec::with_capacity(batch_size);
                for &system_index in systems_batch {
                    let registered_system = &self.systems[system_index];
                    members.push(BatchMember {
                        system_index,
                        last_run_tick: registered_system.last_run_tick,
                        // Reserve one tick per member here, on this thread,
                        // before any pointer is handed out. This is the last
                        // point where `&mut self.world` is still valid, and
                        // reserving here keeps the workers from updating the
                        // shared counter themselves. Disabled members reserve a
                        // tick too, so every member carries the tick of its own
                        // index.
                        this_run_tick: self.world.increment_change_tick().get(),
                        name: registered_system.name.as_str(),
                        zone_color: system_zone_color(registered_system.system_owner),
                        runs: registered_system.enabled
                            && (self.runs_every_system_this_frame || registered_system.edit_mode),
                    });
                }

                // Prepare the raw pointers the workers share.
                //
                // Rust's borrow checker does not allow passing `&mut self.world`,
                // `&mut self.command_queue` and `&mut self.systems[idx]` into one
                // Rayon closure, even though the scheduler has shown that the
                // accesses are disjoint. Raw pointers carry no borrow obligations,
                // so they get around that check.
                //
                // Each pointer is wrapped in `SendPtrMut` so the closure can be
                // `Send` and `Sync` across Rayon worker threads. The wrapper costs
                // nothing: it holds a single `*mut T` field that LLVM inlines away
                // at any optimisation level.
                //
                // `addr_of_mut!` is used instead of `&mut ... as *mut T` because it
                // never creates a reference first. A `&mut` to memory the workers
                // are about to write through would claim exclusive access that
                // this function cannot uphold. Building the pointer directly also
                // keeps its provenance tied to the allocation, which
                // strict-provenance targets (CHERI, Arm MTE) rely on.
                let world_ptr =
                    crate::query::ptr::SendPtrMut::new(std::ptr::addr_of_mut!(self.world));
                let command_queue_ptr =
                    crate::query::ptr::SendPtrMut::new(std::ptr::addr_of_mut!(self.command_queue));
                drop(_zone);

                // Step 2: Run the batch's systems on Rayon worker threads. One
                // indexed parallel iterator is used instead of chained `zip()`
                // calls: deep iterator nesting makes Rayon spend more time
                // splitting work than running it on small batches.
                let _zone = crate::profile_scope!(
                    "dispatch systems batch ({} systems across rayon)",
                    batch_size
                );

                // Collect failed systems from the worker threads; the
                // scheduler guarantees disjoint system state, but the error
                // buffer itself is shared and must stay synchronized.
                let batch_failures = Mutex::new(Vec::<(String, SystemError)>::new());

                members.into_par_iter().enumerate().for_each(|(_position, member)| {
                    // Name the parallel worker thread for Tracy visibility.
                    crate::profile_thread!("parallel worker");

                    if !member.runs {
                        return;
                    }

                    // A `run_if` condition decides whether the system runs at
                    // all, so it is evaluated before the zone and the ticks; a
                    // skipped member leaves the thread-locals untouched.
                    let runs_conditions = {
                        // SAFETY: the same three pointers the system body uses
                        // below, rebuilt while `&mut self` is held for the whole
                        // batch. The conditions are read-only by
                        // `ReadOnlySystemParam`, and the scheduler guarantees no
                        // other member of this batch touches what they read.
                        unsafe {
                            let world = &mut *world_ptr.as_ptr();
                            let command_queue = &mut *command_queue_ptr.as_ptr();
                            let registered_system =
                                &mut *systems_ptr.as_ptr().add(member.system_index);
                            conditions_hold(registered_system, world, command_queue)
                        }
                    };
                    if !runs_conditions {
                        return;
                    }

                    let _zone = crate::profile_scope!(
                        color = member.zone_color,
                        "system: {}",
                        member.name;
                        [("System position in batch: {}", _position), ("Total systems in this batch: {}", batch_size)]
                    );

                    let previous_last_run_tick =
                        set_per_thread_last_run_tick(Some(Tick::new(member.last_run_tick)));
                    let previous_this_run_tick =
                        set_per_thread_this_run_tick(Some(Tick::new(member.this_run_tick)));

                    // SAFETY: The scheduler put only systems with disjoint
                    // component and resource access in one batch, and a system
                    // that uses commands never shares a batch, so no two worker
                    // threads touch the same world, command queue or system
                    // state. Each pointer comes from `self` while `&mut self`
                    // is held for the whole batch, so the data it points at
                    // outlives every closure call. `member.system_index` comes
                    // from `systems_batch`, which the scheduler built from
                    // `self.systems`, so `add(system_index)` stays inside the
                    // `systems` allocation. The one shared mutable buffer,
                    // `batch_failures`, is behind a `Mutex`, and the last-run
                    // ticks are thread-local. Rebuilding `&mut World`, `&mut
                    // CommandQueue` and `&mut RegisteredSystem` from these
                    // pointers is therefore sound: the accesses are disjoint,
                    // in bounds, alive for as long as they are used, and hold
                    // valid values.
                    unsafe {
                        let world = &mut *world_ptr.as_ptr();
                        let system_start = Instant::now();
                        let command_queue = &mut *command_queue_ptr.as_ptr();
                        let registered_system = &mut *systems_ptr.as_ptr().add(member.system_index);
                        let run_result = registered_system.system.run(world, command_queue);
                        if let Err(error) = run_result {
                            batch_failures
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push((member.name.to_string(), error));
                        }
                        record_system_run(
                            registered_system,
                            started_at_tick,
                            system_start.elapsed().as_nanos() as u64,
                        );
                    }

                    set_per_thread_this_run_tick(previous_this_run_tick);
                    set_per_thread_last_run_tick(previous_last_run_tick);
                });

                crate::profile_message!(
                    "parallel system dispatch: batch {} of {} completed, {} systems spawned as {} rayon tasks across the thread pool",
                    batch_index + 1,
                    batch_count,
                    batch_size,
                    batch_size,
                );
                drop(_zone);

                // Step 3: Fold the batch's failures into the frame record and
                // advance every system's last-run tick. The failure order follows
                // the workers' completion, not the batch index.
                let mut collected_failures = batch_failures
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                self.system_failures.extend(
                    collected_failures
                        .drain(..)
                        .map(|(system, error)| SystemFailure::new(system, error)),
                );

                // After the batch finishes, move every system's
                // `last_run_tick` to the tick recorded when the batch started.
                let _advance_zone =
                    crate::profile_scope!("batch advance ticks ({} systems)", systems_batch.len());
                for &system_index in systems_batch {
                    self.systems[system_index].last_run_tick = started_at_tick;
                }
            }
        }
        self.world.system_last_run = 0;
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The Tracy zone color of a system with this owner.
///
/// A project's systems are drawn in blue so they stand out from the engine's
/// and the extensions' work, which keep the viewer's default color.
fn system_zone_color(system_owner: SystemOwner) -> u32 {
    if system_owner == SystemOwner::PROJECT {
        crate::profiling::PROJECT_SYSTEM_ZONE_COLOR
    } else {
        crate::profiling::DEFAULT_ZONE_COLOR
    }
}

/// Runs one registered system on the calling thread.
///
/// The sequential loop, the single-system batch lane and the post-flush phase
/// all come here, so the change-detection baseline, the timing, the failure
/// record and the bookkeeping exist once. The parallel closure has its own
/// body, because its bookkeeping goes through thread-local ticks and a shared
/// failure buffer.
///
/// It takes the engine's fields instead of `&mut Engine` so the caller can keep
/// the scheduler's execution graph borrowed while it loops.
fn run_registered_system(
    world: &mut World,
    command_queue: &mut CommandQueue,
    failures: &mut Vec<SystemFailure>,
    registered_system: &mut RegisteredSystem,
) {
    // A condition decides whether the system runs at all, so it is evaluated
    // before the zone, the change-detection baseline and the timing: a skipped
    // system leaves every one of them untouched, as if the frame had not
    // reached it.
    if !conditions_hold(registered_system, world, command_queue) {
        return;
    }
    let _tracy_sys = crate::profile_scope!(
        color = system_zone_color(registered_system.system_owner),
        "system: {}",
        registered_system.name;
        [("System last ran at tick: {}", registered_system.last_run_tick), ("System splitting hint execution time (ns): {}", registered_system.average_duration)]
    );
    // Seed the change-detection baseline for this system: filters such as
    // `Changed<T>` compare against `last_run_tick`.
    world.system_last_run = registered_system.last_run_tick;

    // Measure this run's wall-clock duration, and capture the tick it started
    // from, for the bookkeeping below.
    let system_start = Instant::now();
    let started_at_tick = world.change_tick().get();
    if let Err(error) = registered_system.system.run(world, command_queue) {
        failures.push(SystemFailure::new(registered_system.name.clone(), error));
    }
    record_system_run(
        registered_system,
        started_at_tick,
        system_start.elapsed().as_nanos() as u64,
    );
}

/// Whether every `run_if` condition of a system holds.
///
/// A system without conditions always holds. The conditions run in the order
/// the registration added them, and the first false answer stops the check, so
/// a later condition cannot observe a world an earlier one only pretended to
/// accept.
fn conditions_hold(
    registered_system: &mut RegisteredSystem,
    world: &mut World,
    command_queue: &mut CommandQueue,
) -> bool {
    registered_system
        .conditions
        .iter_mut()
        .all(|condition| (condition.run)(world, command_queue))
}

/// Records what every dispatch path owes a system that just ran.
///
/// The sequential path, the single-system batch lane and the parallel batch
/// closure all use this one function. It moves the change-detection baseline
/// to the tick captured when the system started, so the next run sees the
/// changes made during this one. It feeds the observed duration into the
/// smoothed average, and it stores the duration for the utilization metric.
/// The batch-wide `last_run_tick` update after the closure stays where it is,
/// because it also covers disabled members, which never enter a worker task.
fn record_system_run(
    registered_system: &mut RegisteredSystem,
    started_at_tick: u32,
    elapsed_nanos: u64,
) {
    registered_system.last_run_tick = started_at_tick;
    // Update splitting hint of execution time.
    let previous_average_duration = registered_system.average_duration;
    let duration_delta = elapsed_nanos as i64 - previous_average_duration as i64;
    registered_system.average_duration = (previous_average_duration as i64
        + duration_delta / ParallelProcessingConfig::SPLITTING_HINT_WINDOW)
        as u64;
    // Store instantaneous duration for utilization metric.
    registered_system.last_duration = elapsed_nanos;
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {

    /// `Position`'s two `f32` fields, as the derive would declare them.
    ///
    /// A component registered by hand has no field layout, and a play session
    /// refuses to copy a type it cannot prove holds no heap data.
    static POSITION_FIELDS: &[crate::component_registry::ComponentFieldDescriptor] = &[
        crate::component_registry::ComponentFieldDescriptor {
            name: "x",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        crate::component_registry::ComponentFieldDescriptor {
            name: "y",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    // Standard library
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;

    // External crates
    use pill_core::error::EngineMessage as _;

    // Current crate
    use super::*;

    /// A failing system is recorded with its name and drained by the frame.
    #[test]
    fn failing_system_is_recorded_and_drained_by_the_frame() {
        let mut engine = Engine::new();
        engine.register_system("failing_system", || -> Result<(), SystemError> {
            Err(SystemError::Failure {
                message: String::from("simulation exploded"),
            })
        });
        engine.process_frame().unwrap();
        let failures = engine.drain_system_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].system, "failing_system");
        assert_eq!(
            failures[0].to_plain_message(),
            "system failing_system failed: simulation exploded"
        );
        // Draining cleared the record for the boundary...
        assert!(engine.system_failures().is_empty());
        // ...and the same failing system fails again on the next frame.
        engine.process_frame().unwrap();
        assert_eq!(engine.drain_system_failures().len(), 1);
    }

    /// Unit-returning systems still run without producing a failure record.
    #[test]
    fn unit_system_runs_without_recording_a_failure() {
        let mut engine = Engine::new();
        engine.register_system("unit_system", || {});
        engine.process_frame().unwrap();
        assert!(engine.system_failures().is_empty());
    }

    /// A system that panics is contained: the panic is recorded as a failure,
    /// the frame keeps going, and three panics in a row queue one fault report
    /// the host can roll the function back with.
    ///
    /// This test only exists with hot patching on: the containment lives in the
    /// dispatch slot a patched system calls through, and without the feature
    /// there is no patch to roll back.
    #[cfg(feature = "development")]
    #[test]
    fn a_panicking_system_is_contained_and_reports_a_fault() {
        let mut engine = Engine::new();
        // The streak table is process-global and tests share the process, so
        // this function's name is unique to this test.
        let name = "engine_tests::panicking_system";
        engine.register_system(name, || -> () {
            panic!("kaboom in the patched body");
        });
        let _ = crate::hot_patch::take_fault_reports();
        for frame in 1..crate::hot_patch::PANIC_ROLLBACK_FRAMES {
            engine.process_frame().unwrap();
            let failures = engine.drain_system_failures();
            assert_eq!(failures.len(), 1, "frame {frame}");
            assert!(
                failures[0].to_plain_message().contains("panicked: kaboom"),
                "frame {frame}: {}",
                failures[0].to_plain_message()
            );
            assert!(
                crate::hot_patch::take_fault_reports().is_empty(),
                "frame {frame} reported before the threshold"
            );
        }
        engine.process_frame().unwrap();
        let reports = crate::hot_patch::take_fault_reports();
        assert_eq!(reports.len(), 1, "the third panic must report the function");
        assert_eq!(reports[0].function, name);
        assert!(reports[0].message.contains("kaboom"));
    }

    /// A failed system never aborts the systems that follow it.
    #[test]
    fn failing_system_does_not_abort_the_remaining_systems() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_system = Arc::clone(&counter);
        let mut engine = Engine::new();
        engine.register_system("failing_system", || -> Result<(), SystemError> {
            Err(SystemError::Failure {
                message: String::from("boom"),
            })
        });
        engine.register_system("counting_system", move || {
            counter_system.fetch_add(1, AtomicOrdering::SeqCst);
        });
        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(engine.drain_system_failures().len(), 1);
    }

    /// Parallel batches record every failing system in the batch.
    #[test]
    fn parallel_batch_records_every_failing_system() {
        let mut engine = Engine::new();
        engine.register_system("first_failure", || -> Result<(), SystemError> {
            Err(SystemError::Failure {
                message: String::from("first"),
            })
        });
        engine.register_system("second_failure", || -> Result<(), SystemError> {
            Err(SystemError::Failure {
                message: String::from("second"),
            })
        });
        engine.process_frame().unwrap();
        let failures = engine.drain_system_failures();
        assert_eq!(failures.len(), 2);
        assert!(failures
            .iter()
            .any(|failure| failure.system == "first_failure"));
        assert!(failures
            .iter()
            .any(|failure| failure.system == "second_failure"));
    }

    /// A post-project system runs after the project phase and the command
    /// flush, even when it was registered before the system whose work it
    /// follows.
    ///
    /// The phase is a property of the registration, not of its position: a
    /// post-project system registered first would run first if registration
    /// order decided, and the command flush would then happen after it.
    #[test]
    fn post_project_systems_run_after_the_project_phase_whatever_their_order() {
        let order: Arc<std::sync::Mutex<Vec<&'static str>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut engine = Engine::new();
        let post_project_order = Arc::clone(&order);
        engine.register_system_with_options(
            "after_flush",
            SystemOptions::new(RunPhase::PostProject, 0),
            move || {
                post_project_order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push("post_project");
            },
        );
        let project_order = Arc::clone(&order);
        engine.register_system("before_flush", move || {
            project_order
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push("project");
        });

        engine.process_frame().unwrap();

        let order = order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            *order,
            vec!["project", "post_project"],
            "the post-project phase follows the project phase and the command flush"
        );
    }

    /// A frame runs its three phases in frame order, whatever order the systems
    /// were registered in.
    #[test]
    fn a_frame_runs_the_three_phases_in_order() {
        let order: Arc<std::sync::Mutex<Vec<&'static str>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut engine = Engine::new();
        // Out of frame order on purpose: the phase decides, not the
        // registration.
        for (phase, label) in [
            (RunPhase::PostProject, "post_project"),
            (RunPhase::Project, "project"),
            (RunPhase::PreProject, "pre_project"),
        ] {
            let order = Arc::clone(&order);
            engine.register_system_with_options(label, SystemOptions::new(phase, 0), move || {
                order
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(label);
            });
        }

        engine.process_frame().unwrap();

        let order = order
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            *order,
            vec!["pre_project", "project", "post_project"],
            "the three phases run in the order a frame documents"
        );
    }

    /// A command a pre-project system queues is applied before the project
    /// phase runs, so a project system sees the change it made.
    #[test]
    fn a_pre_project_command_is_applied_before_the_project_phase() {
        let observed = Arc::new(AtomicUsize::new(0));

        let mut engine = Engine::new();
        engine
            .world_mut()
            .register_component::<crate::test_fixtures::Position>();

        engine.register_system_with_options(
            "spawn_in_pre_project",
            SystemOptions::new(RunPhase::PreProject, 0),
            |mut commands: crate::Commands| {
                commands
                    .create_entity()
                    .with(crate::test_fixtures::Position { x: 0.0, y: 0.0 })
                    .build();
            },
        );
        let observed_in_project = Arc::clone(&observed);
        engine.register_system_with_options(
            "count_in_project",
            SystemOptions::new(RunPhase::Project, 0),
            move |mut positions: crate::Query<&crate::test_fixtures::Position>| {
                observed_in_project.store(positions.entity_count(), AtomicOrdering::SeqCst);
            },
        );

        engine.process_frame().unwrap();

        assert_eq!(
            observed.load(AtomicOrdering::SeqCst),
            1,
            "the project phase sees the entity the pre-project phase queued"
        );
    }

    /// The system list is ordered by `(phase, order)` across every
    /// registration, and an equal pair keeps the order its registrations came
    /// in.
    #[test]
    fn systems_are_listed_by_phase_then_order_then_registration() {
        let mut engine = Engine::new();
        engine.register_system_with_options(
            "second",
            SystemOptions::new(RunPhase::Project, 20),
            || {},
        );
        engine.register_system_with_options(
            "first",
            SystemOptions::new(RunPhase::Project, 10),
            || {},
        );
        engine.register_system_with_options(
            "tied_after_second",
            SystemOptions::new(RunPhase::Project, 20),
            || {},
        );
        engine.register_system_with_options(
            "post",
            SystemOptions::new(RunPhase::PostProject, 0),
            || {},
        );
        engine.register_system_with_options(
            "pre",
            SystemOptions::new(RunPhase::PreProject, 50),
            || {},
        );

        let names: Vec<String> = engine
            .system_snapshots()
            .into_iter()
            .map(|snapshot| snapshot.name)
            .collect();
        assert_eq!(
            names,
            ["pre", "first", "second", "tied_after_second", "post"],
            "the list is sorted by phase, then order, then registration"
        );
    }

    /// Every member of a parallel batch gets its own reserved tick: a query
    /// inside one system sees the same tick however often it asks, and two
    /// systems in one batch never share a tick, which is what keeps the shared
    /// counter out of the workers' hands.
    #[test]
    fn parallel_batch_systems_receive_one_tick_each() {
        let observed: Arc<std::sync::Mutex<Vec<[u32; 2]>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut engine = Engine::new();
        for name in ["tick_first", "tick_second"] {
            let observed = Arc::clone(&observed);
            // SAFETY: both systems declare no component or resource access at
            // all, so the scheduler batches them together and they touch
            // nothing but the tick counter.
            unsafe {
                engine.register_system_with_access(
                    name,
                    SystemAccess::new(),
                    move |world: &mut World, _queue: &mut CommandQueue| {
                        let first = world.increment_change_tick().get();
                        let second = world.increment_change_tick().get();
                        observed
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push([first, second]);
                        Ok(())
                    },
                );
            }
        }

        engine.process_frame().unwrap();

        let observed = observed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(observed.len(), 2, "both systems ran");
        for ticks in observed.iter() {
            assert_eq!(
                ticks[0], ticks[1],
                "one system's run carries one tick, however many queries ask for it"
            );
        }
        assert_ne!(
            observed[0][0], observed[1][0],
            "each batch member reserves its own tick"
        );
    }

    /// Clearing one module's systems leaves every other owner's systems
    /// registered and still running, which is what makes per-module hot reload
    /// isolated from the project and from other modules.
    #[test]
    fn clearing_one_owner_leaves_other_owners_running() {
        let project_runs = Arc::new(AtomicUsize::new(0));
        let module_runs = Arc::new(AtomicUsize::new(0));

        let mut engine = Engine::new();
        let project_counter = Arc::clone(&project_runs);
        engine.register_system("project_system", move || {
            project_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });

        let module_owner = SystemOwner::extension(0);
        engine.begin_module_registration(module_owner);
        let module_counter = Arc::clone(&module_runs);
        engine.register_system("module_system", move || {
            module_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });
        engine.end_module_registration();

        engine.process_frame().unwrap();
        assert_eq!(project_runs.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(module_runs.load(AtomicOrdering::SeqCst), 1);

        // Reloading the module removes only its own system.
        assert_eq!(engine.clear_systems_owned_by(module_owner), 1);
        engine.process_frame().unwrap();
        assert_eq!(project_runs.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(module_runs.load(AtomicOrdering::SeqCst), 1);

        // The project's own systems are cleared the same way, so they stop
        // running.
        assert_eq!(engine.clear_systems_owned_by(SystemOwner::PROJECT), 1);
        engine.process_frame().unwrap();
        assert_eq!(project_runs.load(AtomicOrdering::SeqCst), 2);
    }

    /// Registers one counting system under `system_owner` and returns its
    /// counter.
    fn register_counting_system(
        engine: &mut Engine,
        system_owner: SystemOwner,
        name: &str,
    ) -> Arc<AtomicUsize> {
        let run_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&run_count);
        engine.begin_module_registration(system_owner);
        engine.register_system(name.to_owned(), move || {
            counter.fetch_add(1, AtomicOrdering::SeqCst);
        });
        engine.end_module_registration();
        run_count
    }

    /// Suspending an owner stops its systems and only its systems, in both the
    /// sequential and the parallel scheduler, and resuming restores them.
    #[test]
    fn suspending_an_owner_stops_only_its_systems() {
        for parallel in [false, true] {
            let mut engine = Engine::new();
            engine.set_parallel_execution(parallel);
            let module_owner = SystemOwner::extension(2);
            let project_runs =
                register_counting_system(&mut engine, SystemOwner::PROJECT, "project");
            let first_runs = register_counting_system(&mut engine, module_owner, "module_first");
            let second_runs = register_counting_system(&mut engine, module_owner, "module_second");

            assert_eq!(engine.set_systems_enabled_for_owner(module_owner, false), 2);
            assert!(!engine.is_owner_enabled(module_owner));
            engine.process_frame().unwrap();
            assert_eq!(
                project_runs.load(AtomicOrdering::SeqCst),
                1,
                "parallel={parallel}"
            );
            assert_eq!(
                first_runs.load(AtomicOrdering::SeqCst),
                0,
                "parallel={parallel}"
            );
            assert_eq!(
                second_runs.load(AtomicOrdering::SeqCst),
                0,
                "parallel={parallel}"
            );

            assert_eq!(engine.set_systems_enabled_for_owner(module_owner, true), 2);
            assert!(engine.is_owner_enabled(module_owner));
            engine.process_frame().unwrap();
            assert_eq!(
                project_runs.load(AtomicOrdering::SeqCst),
                2,
                "parallel={parallel}"
            );
            assert_eq!(
                first_runs.load(AtomicOrdering::SeqCst),
                1,
                "parallel={parallel}"
            );
            assert_eq!(
                second_runs.load(AtomicOrdering::SeqCst),
                1,
                "parallel={parallel}"
            );
        }
    }

    /// A system registered while its owner is suspended starts disabled, so a
    /// module generation that initializes during a suspension does not run
    /// early; resuming the owner starts it.
    #[test]
    fn systems_registered_during_a_suspension_start_disabled() {
        let mut engine = Engine::new();
        let module_owner = SystemOwner::extension(0);
        assert_eq!(engine.set_systems_enabled_for_owner(module_owner, false), 0);

        let run_count = register_counting_system(&mut engine, module_owner, "late");
        assert_eq!(engine.is_system_enabled("late"), Some(false));
        engine.process_frame().unwrap();
        assert_eq!(run_count.load(AtomicOrdering::SeqCst), 0);

        engine.set_systems_enabled_for_owner(module_owner, true);
        engine.process_frame().unwrap();
        assert_eq!(run_count.load(AtomicOrdering::SeqCst), 1);
    }

    /// Clearing a suspended owner's systems still works, and the suspension
    /// survives it: the replacement generation's systems start disabled.
    #[test]
    fn a_suspension_survives_clearing_the_owners_systems() {
        let mut engine = Engine::new();
        let module_owner = SystemOwner::extension(1);
        register_counting_system(&mut engine, module_owner, "old_generation");
        engine.set_systems_enabled_for_owner(module_owner, false);

        assert_eq!(engine.clear_systems_owned_by(module_owner), 1);
        assert!(!engine.is_owner_enabled(module_owner));

        let run_count = register_counting_system(&mut engine, module_owner, "new_generation");
        engine.process_frame().unwrap();
        assert_eq!(run_count.load(AtomicOrdering::SeqCst), 0);

        engine.set_systems_enabled_for_owner(module_owner, true);
        engine.process_frame().unwrap();
        assert_eq!(run_count.load(AtomicOrdering::SeqCst), 1);
    }

    /// Registration outside a module scope is attributed to the project, and
    /// scoping is restored after the module finishes registering.
    #[test]
    fn registration_scope_returns_to_the_project_owner() {
        let mut engine = Engine::new();
        engine.begin_module_registration(SystemOwner::extension(3));
        engine.register_system("owned_by_module", || {});
        engine.end_module_registration();
        engine.register_system("owned_by_project", || {});

        // Only the module's system is removed by clearing its owner.
        assert_eq!(engine.clear_systems_owned_by(SystemOwner::extension(3)), 1);
        assert_eq!(engine.is_system_enabled("owned_by_project"), Some(true));
        assert_eq!(engine.is_system_enabled("owned_by_module"), None);
    }

    /// Clearing an owner with no registered systems changes nothing, so a
    /// module that failed to register cannot disturb the rest of the schedule.
    #[test]
    fn clearing_an_unused_owner_is_a_no_op() {
        let mut engine = Engine::new();
        engine.register_system("project_system", || {});
        assert_eq!(engine.clear_systems_owned_by(SystemOwner::extension(7)), 0);
        assert_eq!(engine.is_system_enabled("project_system"), Some(true));
    }

    /// After removing one system of several, the scheduler's access patterns
    /// stay aligned with the engine's system list, so the surviving systems are
    /// still scheduled and executed.
    #[test]
    fn scheduler_stays_aligned_after_removing_a_middle_system() {
        let first_runs = Arc::new(AtomicUsize::new(0));
        let last_runs = Arc::new(AtomicUsize::new(0));

        let mut engine = Engine::new();
        let first_counter = Arc::clone(&first_runs);
        engine.register_system("first", move || {
            first_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });

        let middle_owner = SystemOwner::extension(1);
        engine.begin_module_registration(middle_owner);
        engine.register_system("middle", || {});
        engine.end_module_registration();

        let last_counter = Arc::clone(&last_runs);
        engine.register_system("last", move || {
            last_counter.fetch_add(1, AtomicOrdering::SeqCst);
        });

        engine.clear_systems_owned_by(middle_owner);
        engine.process_frame().unwrap();

        assert_eq!(first_runs.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(last_runs.load(AtomicOrdering::SeqCst), 1);
        assert_eq!(engine.is_system_enabled("middle"), None);
    }

    /// A paused engine skips the system phase but keeps accepting frames, and
    /// `request_single_step` runs exactly one more phase before pausing again.
    #[test]
    fn paused_engine_skips_systems_but_step_runs_exactly_one_phase() {
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_system = Arc::clone(&counter);
        let mut engine = Engine::new();
        engine.register_system("counter", move || {
            counter_system.fetch_add(1, AtomicOrdering::SeqCst);
        });

        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);

        // Paused: frames still run (the tick advances and commands are
        // flushed) but no system runs.
        engine.set_systems_paused(true);
        engine.process_frame().unwrap();
        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 1);

        // One explicit step advances exactly one phase.
        engine.request_single_step();
        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 2);
        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 2);

        // Resuming runs systems again.
        engine.set_systems_paused(false);
        engine.process_frame().unwrap();
        assert_eq!(counter.load(AtomicOrdering::SeqCst), 3);
        assert!(!engine.systems_paused());
    }

    /// A play session gives the edit world back and keeps what a `Keep`
    /// resource was changed to.
    ///
    /// The game spawns an entity, destroys one, moves a component, changes a
    /// restored resource, inserts a resource of its own and changes a `Keep`
    /// resource. Stop must undo everything but the last of those.
    #[test]
    fn a_play_session_restores_the_world_and_keeps_only_a_kept_resource() {
        #[derive(Debug, Clone, Copy, PartialEq)]
        struct Score(i32);
        impl crate::resource::Resource for Score {
            // A hand-written resource opts in to being copied at Play by
            // answering with a copy of itself; the derive writes this for a
            // type it generates.
            fn clone_erased(
                value: &crate::resource::ErasedResource,
            ) -> Option<crate::resource::ErasedResource> {
                Some(crate::resource::ErasedResource::new(*value.get::<Self>()?))
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq)]
        struct PlayOnly(i32);
        impl crate::resource::Resource for PlayOnly {}

        struct KeptResource(i32);
        impl crate::resource::Resource for KeptResource {
            fn play_state() -> crate::resource::PlayState {
                crate::resource::PlayState::Keep
            }
        }

        let mut engine = Engine::new();
        engine.set_run_mode(crate::engine::RunMode::Editing);
        let world = engine.world_mut();
        world.register_component::<crate::test_fixtures::Position>();
        world.record_component_field_layout(
            crate::component::ComponentId::of::<crate::test_fixtures::Position>(),
            POSITION_FIELDS,
        );
        world.insert_resource(Score(1));
        world.insert_resource(KeptResource(7));
        let kept_entity = world
            .create_entity()
            .with(crate::test_fixtures::Position { x: 1.0, y: 2.0 })
            .build()
            .expect("the edit world holds one entity");
        // A second entity exists so the game has one to destroy.
        let doomed_entity = world
            .create_entity()
            .with(crate::test_fixtures::Position { x: 3.0, y: 4.0 })
            .build()
            .expect("the edit world holds a second entity");

        engine.enter_play().expect("a plain world copies");
        assert_eq!(engine.run_mode(), crate::engine::RunMode::Playing);

        // Step 1: what the game does to the world.
        {
            let world = engine.world_mut();
            world
                .create_entity()
                .with(crate::test_fixtures::Position { x: 9.0, y: 9.0 })
                .build()
                .expect("the game spawns an entity");
            let _ = world.destroy_entity(doomed_entity);
            if let Some(position) =
                world.get_component_mut::<crate::test_fixtures::Position>(kept_entity)
            {
                position.x = 42.0;
            }
            if let Some(score) = world.get_resource_mut::<Score>() {
                score.0 = 100;
            }
            world.insert_resource(PlayOnly(5));
            if let Some(kept) = world.get_resource_mut::<KeptResource>() {
                kept.0 = 11;
            }
        }

        // Step 2: Stop.
        assert!(engine.exit_play());
        assert_eq!(engine.run_mode(), crate::engine::RunMode::Editing);
        assert_eq!(engine.world().restore_epoch(), 1);

        let world = engine.world();
        assert_eq!(world.entity_count(), 2, "the spent entity came back");
        assert_eq!(
            world
                .get_component::<crate::test_fixtures::Position>(kept_entity)
                .map(|position| position.x),
            Some(1.0),
            "the component holds its edit value"
        );
        assert_eq!(
            world.get_resource::<Score>().map(|score| score.0),
            Some(1),
            "a restored resource comes back"
        );
        assert_eq!(
            world
                .get_resource::<PlayOnly>()
                .map(|play_only| play_only.0),
            None,
            "a resource the game inserted is gone"
        );
        assert_eq!(
            world.get_resource::<KeptResource>().map(|kept| kept.0),
            Some(11),
            "a kept resource keeps its play-time value"
        );
    }

    /// Checks that `system_snapshots` reports owners, enabled state and name
    /// ambiguity, and that `set_system_enabled_at` toggles exactly one of two
    /// same-named systems.
    #[test]
    fn system_snapshots_report_owners_and_toggle_by_index() {
        let mut engine = Engine::new();
        // The snapshot indices below assume that no other system is registered.
        // The ECS report is opt-in, and this call makes sure it stays off
        // rather than relying on the default.
        engine.set_ecs_diagnostics_interval(None);
        engine.register_system("duplicate", || {});
        engine.begin_module_registration(SystemOwner::extension(0));
        engine.register_system("duplicate", || {});
        engine.end_module_registration();
        engine.register_system("project_only", || {});

        let snapshots = engine.system_snapshots();
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].index, 0);
        assert_eq!(snapshots[0].owner, SystemOwner::PROJECT);
        assert_eq!(snapshots[1].owner, SystemOwner::extension(0));
        assert!(snapshots.iter().all(|system| system.enabled));
        assert!(snapshots[0].name_is_ambiguous);
        assert!(snapshots[1].name_is_ambiguous);
        assert!(!snapshots[2].name_is_ambiguous);

        // Toggle by index affects exactly the first same-named system.
        assert!(engine.set_system_enabled_at(0, false));
        assert!(!engine.set_system_enabled_at(99, false));
        let after = engine.system_snapshots();
        assert!(!after[0].enabled);
        assert!(after[1].enabled);
        // The name-keyed query sees the first (now disabled) registration.
        assert_eq!(engine.is_system_enabled("duplicate"), Some(false));
    }

    /// The ECS report is opt-in: a fresh engine registers nothing and the
    /// setter is the one call that registers it.
    ///
    /// The docs on `Engine::new`, the setter and the `diagnostics` module all
    /// promise this shape, so the test fails if either side drifts: registering
    /// the report while the engine is built breaks the first assertion, and a
    /// setter that no longer registers breaks the second.
    #[test]
    fn the_ecs_report_is_opt_in() {
        let report_is_registered = |engine: &Engine| {
            engine.system_snapshots().into_iter().any(|system| {
                system.name == crate::diagnostics::SYSTEM_NAME
                    && system.owner == SystemOwner::ENGINE
            })
        };

        let mut engine = Engine::new();
        assert!(
            !report_is_registered(&engine),
            "Engine::new registers no report; a project opts in"
        );

        engine.set_ecs_diagnostics_interval(Some(crate::diagnostics::DEFAULT_REPORT_INTERVAL));
        assert!(
            report_is_registered(&engine),
            "the setter registers the report it documents"
        );

        engine.set_ecs_diagnostics_interval(None);
        assert!(
            !report_is_registered(&engine),
            "None removes the report again"
        );
    }
}
