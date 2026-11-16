//! C# development tooling over the `pill_csharp_bridge` backend.
//!
//! # Responsibilities
//!
//! - Generates the C# mirrors for the components, value types, object
//!   classes, enums, resource markers and free functions an extension exposes
//!   to managed code.
//! - Recompiles the project in-process with Roslyn on a hot reload.
//! - Pairs the bridge's [`CSharpRuntime`] with that compiler as the host's
//!   managed project, [`CSharpProject`].
//! - Re-exports the bridge items the rest of the host names, under one path.
//!
//! # Design
//!
//! The bridge owns everything needed to run C#: hosting .NET, the ABI,
//! components, systems and the reload poll. A C# shipping bundle links the
//! bridge without this crate. This module holds only what exists while
//! developing: the generated mirrors and the compiler. The whole module is
//! `hot_reload` only.

// Standard library
use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

// External crates
use pill_core::error::CSharpError;
use pill_core::platform::Instant;
use pill_engine::Engine;

// Current crate
use crate::watchers::SourceTrigger;

/// Host-side generation of a module's `<module>_Components.g.cs` mirror file.
mod codegen;
/// In-process Roslyn compilation of the C# project, which replaces MSBuild on
/// a reload.
mod fast_compile;
/// The C# members, object classes, enums and resource markers generated from
/// a module's mirror descriptors.
mod mirror_codegen;

// =============================================================================
// Re-exports
// =============================================================================

pub(crate) use pill_csharp_bridge::{
    accessor_operation_name, accessor_rows, exposed_components_from_names, publish_mirror_methods,
    CSharpRuntime, ModuleExposedComponent, ResolvedFieldAccessor, ResolvedMirrorMethod,
    POLL_REJECTED, POLL_RELOADED,
};

/// Generates a module's `<module>_Components.g.cs` mirror file from the surface
/// it exposes to managed code.
pub(crate) use codegen::generate_module_components_csharp;
/// What one in-process compile attempt produced.
pub(crate) use fast_compile::FastCompileOutcome;
/// The in-process Roslyn compiler, shared with the build thread and with the
/// synchronous fallback build.
pub(crate) use fast_compile::FastCompiler;

// =============================================================================
// CSharpProject
// =============================================================================

/// A running C# project in the reloading host, made of the bridge's runtime
/// and the in-process compiler that rebuilds its assembly.
pub(crate) struct CSharpProject {
    /// The managed runtime running the project's systems.
    runtime: CSharpRuntime,
    /// The in-process Roslyn compiler, when one could be loaded.
    ///
    /// `None` leaves every reload on the ordinary `dotnet build` path, the
    /// fallback when the compiler cannot be built or loaded. It is shared
    /// behind an `Arc` because a reload hands a clone to a build thread. Its
    /// own state is a set of export addresses and paths, so sharing it is
    /// safe.
    fast_compiler: Option<Arc<fast_compile::FastCompiler>>,
    /// The assembly build running on its own thread, when one is.
    ///
    /// The build compiles the sources and writes the assembly. No other reload
    /// step may run while it does, because the build compiles the generated
    /// mirror files that a module reload rewrites. The frame itself keeps
    /// rendering and running systems. Collecting the build hands its outcome
    /// to the reload path, which tells the loader and polls for the swap.
    assembly_build: Option<InFlightAssemblyBuild>,
    /// Timing of the managed reload currently in flight, when one is armed.
    ///
    /// It is armed when the frame loop consumes the source signal that starts
    /// a reload, and finished when the loader swaps the new assembly in. The
    /// log can then report the whole save-to-swap span instead of the compile
    /// alone.
    reload_timing: Option<ManagedReloadTiming>,
}

impl CSharpProject {
    /// Starts the project through the bridge and loads the in-process compiler
    /// into the same .NET runtime as soon as it is running.
    ///
    /// The compiler is loaded before the project starts, not after, so its
    /// background warmup runs during startup; see [`CSharpRuntime::start`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`CSharpRuntime::start`]. A compiler that
    /// cannot be built or loaded is not an error: the failure is logged once,
    /// and reloads run a full `dotnet build`.
    pub(crate) fn start(
        engine: &mut Engine,
        workspace_root: &Path,
        module_config: &pill_csharp_bridge::CSharpModuleConfig,
        module_exposed_components: &[ModuleExposedComponent],
        mirror_methods: &[ResolvedMirrorMethod],
    ) -> Result<Self, CSharpError> {
        let mut fast_compiler = None;
        let runtime = CSharpRuntime::start(
            engine,
            workspace_root,
            module_config,
            module_exposed_components,
            mirror_methods,
            &mut |dotnet| {
                fast_compiler =
                    fast_compile::FastCompiler::try_new(dotnet, workspace_root, module_config)
                        .map(Arc::new);
            },
        )?;
        Ok(Self {
            runtime,
            fast_compiler,
            assembly_build: None,
            reload_timing: None,
        })
    }

    /// Asks the managed loader whether it can swap in a rebuilt assembly; see
    /// [`CSharpRuntime::poll_reload`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`CSharpRuntime::poll_reload`].
    pub(crate) fn poll_reload(&mut self, engine: &mut Engine) -> Result<u8, CSharpError> {
        self.runtime.poll_reload(engine)
    }

    /// Arms reload timing for the reload this signal starts.
    ///
    /// `trigger` is the project watcher's record of the save that fired the
    /// signal. `None` means the pipeline queued the reload itself, for example
    /// because a module swap changed the mirror surface. That case times from
    /// now and reports no detection delay, because there is no file save to
    /// measure it from.
    pub(crate) fn arm_reload_timing(&mut self, trigger: Option<SourceTrigger>) {
        self.reload_timing = Some(ManagedReloadTiming {
            detected_at: trigger.map_or_else(Instant::now, |trigger| trigger.fired_at),
            detection_delay_ms: trigger.map_or(f64::NAN, |trigger| trigger.detection_delay_ms),
            triggered_at: Instant::now(),
            rebuilt_assembly: None,
        });
    }

    /// Records that an assembly rebuild finished at `finished_at`.
    ///
    /// The build worker reports the instant it finished, so a build that ran
    /// between two frames still times the swap from its own end instead of the
    /// wait before the frame collected it.
    pub(crate) fn record_assembly_rebuilt_at(&mut self, finished_at: Instant) {
        if let Some(timing) = &mut self.reload_timing {
            timing.rebuilt_assembly = Some(RebuiltAssembly { finished_at });
        }
    }

    /// Returns a handle to the in-process compiler for one build, or `None`
    /// when no compiler is loaded; that build then runs the full `dotnet build`
    /// fallback.
    pub(crate) fn compiler_handle(&self) -> Option<Arc<fast_compile::FastCompiler>> {
        self.fast_compiler.clone()
    }

    /// Stores a build that a worker thread has just started, so the frame loop
    /// can collect its report and complete the reload.
    pub(crate) fn track_build(&mut self, receiver: Receiver<BuildReport>, thread: JoinHandle<()>) {
        self.assembly_build = Some(InFlightAssemblyBuild {
            receiver,
            thread: Some(thread),
        });
    }

    /// Tells whether an assembly build is running on its own thread right now.
    pub(crate) fn build_in_flight(&self) -> bool {
        self.assembly_build.is_some()
    }

    /// Tries to collect a finished build report without waiting.
    ///
    /// Collecting a report waits for the worker thread to end and clears the
    /// in-flight state. A worker that stopped without a report is reported as
    /// [`BuildCollection::Died`], so the caller can fall back to building on
    /// the frame thread.
    pub(crate) fn collect_build(&mut self) -> BuildCollection {
        let Some(in_flight) = self.assembly_build.as_mut() else {
            return BuildCollection::Pending;
        };
        match in_flight.receiver.try_recv() {
            Ok(report) => {
                in_flight.join();
                self.assembly_build = None;
                BuildCollection::Finished(report)
            }
            Err(TryRecvError::Empty) => BuildCollection::Pending,
            Err(TryRecvError::Disconnected) => {
                in_flight.join();
                self.assembly_build = None;
                BuildCollection::Died
            }
        }
    }

    /// Tells the managed loader that the in-process compiler has already
    /// written a complete assembly. The loader's next poll can then skip its
    /// debounce (the wait for the file to stop changing); see
    /// [`CSharpRuntime::notify_assembly_replaced`].
    pub(crate) fn notify_assembly_replaced(&mut self) {
        self.runtime.notify_assembly_replaced();
    }

    /// Drops an armed timing whose reload produced no swap.
    ///
    /// A failed or cancelled build replaces nothing. Keeping the timing would
    /// count a later swap, whichever build produced it, as caused by this
    /// signal.
    pub(crate) fn abandon_reload_timing(&mut self) {
        self.reload_timing = None;
    }

    /// Finishes an armed timing at the swap and returns its breakdown.
    ///
    /// Returns `None` when no timing was armed. A swap with no recorded build,
    /// such as the loader picking up an assembly this host did not time,
    /// reports `NaN` for the build phase rather than a made-up number.
    pub(crate) fn finish_reload_timing(&mut self) -> Option<ManagedReloadSummary> {
        let timing = self.reload_timing.take()?;
        let swapped_at = Instant::now();
        let queue_ms = milliseconds(timing.triggered_at.duration_since(timing.detected_at));
        let (build_ms, swap_ms) = match timing.rebuilt_assembly {
            Some(rebuilt_assembly) => (
                milliseconds(
                    rebuilt_assembly
                        .finished_at
                        .duration_since(timing.triggered_at),
                ),
                milliseconds(swapped_at.duration_since(rebuilt_assembly.finished_at)),
            ),
            None => (
                f64::NAN,
                milliseconds(swapped_at.duration_since(timing.triggered_at)),
            ),
        };
        Some(ManagedReloadSummary {
            detect_ms: timing.detection_delay_ms,
            queue_ms,
            build_ms,
            swap_ms,
        })
    }
}

// =============================================================================
// Managed Reload Timing
// =============================================================================

/// One managed reload's armed timing, filled in as its phases complete.
struct ManagedReloadTiming {
    /// Instant the project watcher fired the signal this reload came from.
    detected_at: Instant,
    /// How long the save waited before the watcher fired the signal, the
    /// debounce wait included; `NaN` when there is no trigger to measure from.
    /// A trigger also carries `NaN` when no file timestamp could be read.
    detection_delay_ms: f64,
    /// Instant the frame loop consumed the signal and armed this timing.
    triggered_at: Instant,
    /// The rebuilt assembly, once a build recorded it.
    rebuilt_assembly: Option<RebuiltAssembly>,
}

/// A finished assembly rebuild: when it was ready.
struct RebuiltAssembly {
    /// Instant the rebuild returned.
    finished_at: Instant,
}

/// A finished managed reload, broken down by phase, in milliseconds.
pub(crate) struct ManagedReloadSummary {
    /// Save to signal, the debounce wait included; `NaN` when the watcher
    /// could not measure it.
    pub(crate) detect_ms: f64,
    /// Signal to the frame loop consuming it.
    pub(crate) queue_ms: f64,
    /// Consume to rebuilt assembly; `NaN` when no build was recorded.
    pub(crate) build_ms: f64,
    /// Rebuilt assembly to the loader's swap; measured from the signal when no
    /// build was recorded.
    pub(crate) swap_ms: f64,
}

impl ManagedReloadSummary {
    /// The report's `stage` phase: the watcher's debounce plus the wait for the
    /// frame loop, which is everything that precedes the build here.
    ///
    /// The native report's `stage` is everything before its cargo build, so
    /// adding the two waits is the honest mapping rather than two phases the
    /// shared grammar has no name for. A `NaN` detect contributes nothing.
    pub(crate) fn stage_ms(&self) -> f64 {
        let detect_ms = if self.detect_ms.is_finite() {
            self.detect_ms
        } else {
            0.0
        };
        detect_ms + self.queue_ms
    }
}

// =============================================================================
// Assembly Build
// =============================================================================

/// One assembly build running on its own worker thread.
///
/// The thread is waited for when its report is collected, when the worker is
/// found dead, and when this value is dropped. A build that outlived its host
/// would keep calling the compiler's function pointers after the managed
/// runtime that owns them was dropped.
struct InFlightAssemblyBuild {
    /// The build's report, once the worker has one.
    receiver: Receiver<BuildReport>,
    /// The worker thread, waited for by [`Self::join`] or on drop.
    thread: Option<JoinHandle<()>>,
}

impl InFlightAssemblyBuild {
    /// Waits for the worker thread to end. Returns at once when it already
    /// finished, which is the case whenever a report was received.
    fn join(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for InFlightAssemblyBuild {
    fn drop(&mut self) {
        self.join();
    }
}

/// What one managed assembly build produced.
pub(crate) enum BuildOutcome {
    /// The in-process compiler rebuilt the assembly.
    Compiled {
        /// How long the compile took, for the reload log.
        milliseconds: f64,
    },
    /// The project does not compile; the diagnostics say why.
    Failed {
        /// Compiler and analyzer errors, already formatted the way csc prints them.
        diagnostics: String,
    },
    /// The in-process compiler could not handle the reload, or was never
    /// loaded, so the full `dotnet build` ran instead; `Err` carries that
    /// build's error text.
    Full(Result<(), String>),
}

/// A finished assembly build: what it produced, and when it finished, so the
/// reload log can cover the build itself instead of the frame that collected
/// it.
pub(crate) struct BuildReport {
    /// What the build produced.
    pub(crate) outcome: BuildOutcome,
    /// When the build finished, on the same monotonic clock as the frame
    /// thread.
    pub(crate) finished_at: Instant,
}

/// What one collection attempt found.
pub(crate) enum BuildCollection {
    /// No report was available: a build is still running, or none was started.
    Pending,
    /// The worker stopped without a report; the caller falls back to building
    /// on the frame thread.
    Died,
    /// The build finished; the report carries what it produced.
    Finished(BuildReport),
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns `duration` in milliseconds.
fn milliseconds(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
