//! Starts a hosted .NET runtime for a C# project, finds the systems it
//! declares, and registers them with the engine's scheduler.
//!
//! # Responsibilities
//!
//! - Start .NET (CoreCLR through hostfxr, or a NativeAOT library) and resolve
//!   every managed export the host calls.
//! - Register the component manifest the managed side reports, then run its
//!   startup methods so a failure leaves no half-built world.
//! - Reflect each system's accesses and register the system under that exact
//!   read/write list.
//! - Poll the managed loader after a rebuild, decide whether the arriving
//!   component manifest can be applied, and re-register the project's systems
//!   when their signatures changed.
//!
//! # Design
//!
//! Every unmanaged export is resolved once at startup into a plain function
//! pointer held by the [`CSharpRuntime`] host. A registered system never calls
//! the runtime directly: each registration captures the resolved pointers, its
//! reflected access list and a shared [`BindingStore`] handle. [`CSharpRuntime`]
//! owns the runtime and the API table those pointers come from, so keeping the
//! host alive keeps every registered system valid.
//!
//! A reload is a handshake with the managed loader. The loader holds the new
//! assembly back until the host has applied its component manifest, and only
//! then swaps it in. [`ManagedSystemSnapshot`] records what the active assembly
//! declared, so a reload can tell a changed system body (nothing to
//! re-register) from a changed signature (rebuild the project's systems).

// Standard library
use std::path::{Path, PathBuf};
use std::sync::Arc;

// External crates
use pill_core::error::CSharpError;
use pill_core::telemetry::telemetry_target;
#[cfg(feature = "hot_reload")]
use pill_core::{error, info, warn};
use pill_engine::commands::CommandQueue;
#[cfg(feature = "hot_reload")]
use pill_engine::SystemOwner;
use pill_engine::{Engine, RunPhase, SystemAccess, SystemError, SystemOptions, World};

// Current crate
use super::abi::{CsEngineApi, NativeSystemAccess};
use super::aot_runtime::AotRuntimeContext;
#[cfg_attr(not(feature = "hot_reload"), allow(unused_imports))]
use super::components::apply_component_manifest_on_reload;
use super::components::{
    module_native_bindings, register_component_manifest, BindingStore, ComponentBindings,
    ModuleExposedComponent, StableComponentId,
};
use super::context::ActiveScopeGuard;
use super::csharp_runtime::DotnetRuntimeContext;
use super::managed_buffer::{fetch_managed_buffer, manifest_fetch_error};
// Only the reload path tells the failure kinds apart. A shipping build maps
// them all through `manifest_fetch_error` and never names them.
#[cfg(feature = "hot_reload")]
use super::managed_buffer::ManagedBufferError;
use super::ResolvedMirrorMethod;
use crate::CSharpModuleConfig;

// =============================================================================
// Constants
// =============================================================================

/// Poll found no reload to do. Nothing was due, or the file is unchanged.
#[cfg(feature = "hot_reload")]
pub const POLL_NO_CHANGE: u8 = 0;

/// Poll swapped in a behaviour-compatible assembly.
#[cfg(feature = "hot_reload")]
pub const POLL_RELOADED: u8 = 1;

/// Poll rejected the new assembly, and the current one stays loaded.
#[cfg(feature = "hot_reload")]
pub const POLL_REJECTED: u8 = 2;

/// A behaviour-compatible assembly is loaded and waiting for the host to
/// decide whether its component manifest can be applied.
///
/// The managed loader cannot make that decision. Whether a manifest change is
/// safe depends on what each component is bound to natively, and only the host
/// holds those bindings. The host answers during the same poll.
#[cfg(feature = "hot_reload")]
pub const POLL_MANIFEST_PENDING: u8 = 3;

/// Upper bound for the serialized managed component manifest.
///
/// Real manifests are a few hundred bytes. The cap keeps a buggy or hostile
/// managed assembly from driving a multi-gigabyte host allocation.
pub(super) const MAX_COMPONENT_MANIFEST_BYTES: u32 = 16 * 1024 * 1024;

/// Maximum UTF-8 byte length accepted for a managed system name.
const MAX_SYSTEM_NAME_BYTES: u32 = 1024;

/// Maximum UTF-8 byte length accepted for a managed system error message.
const MAX_SYSTEM_ERROR_BYTES: u32 = 4096;

/// Upper bound on the number of systems one managed assembly may report.
///
/// The count is used to reserve the startup snapshot before any system has
/// been read, so it needs the same kind of bound as the manifest lengths above.
/// No real project comes near 4,096 systems.
pub(super) const MAX_SYSTEMS_PER_ASSEMBLY: u32 = 4096;

/// Upper bound on the accesses one managed system may declare.
pub(super) const MAX_ACCESSES_PER_SYSTEM: u32 = 1024;

/// The unmanaged ABI contract version, shared with `csharp_runtime`.
///
/// Increase it when an `UnmanagedCallersOnly` export signature changes, when
/// the field layout of the [`CsEngineApi`] table changes, or when a struct the
/// two sides exchange changes shape. The managed runtime copies that table
/// field by field, so one new slot makes the two sides disagree about every
/// slot after it. The host refuses to start against a runtime built for
/// another version, and the managed side refuses a table whose version and
/// size it does not accept.
///
/// Every byte payload has its own named length/copy export pair instead of one
/// generic pair keyed by a payload-kind number. A wrong name then fails while
/// loading, in the host's `resolve_unmanaged_function_pointer`, and the error
/// names the export it could not find. A numeric kind would instead dispatch
/// wrongly at run time, on a live ABI that AOT projects forward by hand.
pub const INTEROP_CONTRACT_VERSION: u32 = 19;

// =============================================================================
// Types + Impls
// =============================================================================

/// Signature of the managed runtime entry point that receives the API table.
type InitFn = extern "system" fn(*const CsEngineApi) -> u8;
/// Signature returning the unmanaged ABI contract version.
type InteropVersionFn = extern "system" fn() -> u32;
/// Signature returning the number of registered scheduler systems.
type SystemCountFn = extern "system" fn() -> u32;
/// Signature returning the number of managed startup methods.
type StartupCountFn = extern "system" fn() -> u32;
/// Signature reporting whether one system declares a Commands parameter.
type SystemUsesCommandsFn = extern "system" fn(u32) -> u8;
/// Signature returning one system's declared order within its phase.
type SystemOrderFn = extern "system" fn(u32) -> i32;
/// Signature returning one system's declared frame phase (0, 1 or 2).
type SystemRunPhaseFn = extern "system" fn(u32) -> u8;
/// Signature reporting whether one system also runs in edit mode.
type SystemRunInEditModeFn = extern "system" fn(u32) -> u8;
/// Signature executing one managed startup method by index.
type RunStartupFn = extern "system" fn(u32) -> u8;
/// Signature returning the serialized component manifest byte length.
type ComponentManifestLengthFn = extern "system" fn() -> u32;
/// Signature copying the serialized component manifest into a caller buffer.
type CopyComponentManifestFn = extern "system" fn(*mut u8, u32) -> u8;
/// Signature returning the UTF-8 byte length of one system's reflected name.
type SystemNameLengthFn = extern "system" fn(u32) -> u32;
/// Signature copying one system's reflected name into a caller buffer.
type CopySystemNameFn = extern "system" fn(u32, *mut u8, u32) -> u8;
/// Signature returning how many accesses one system declared.
type SystemAccessCountFn = extern "system" fn(u32) -> u32;
/// Signature copying one system's reflected accesses into a caller buffer.
type GetSystemAccessFn = extern "system" fn(u32, u32, *mut NativeSystemAccess) -> u8;
/// Signature running one scheduler system by index.
///
/// Returns one on success and zero after the managed side records the
/// failure for [`SystemErrorMessageLengthFn`] retrieval.
type RunSystemFn = extern "system" fn(u32) -> u8;
/// Signature returning the UTF-8 byte length of one system's last error message.
type SystemErrorMessageLengthFn = extern "system" fn(u32) -> u32;
/// Signature copying one system's last error message into a caller buffer.
type CopySystemErrorMessageFn = extern "system" fn(u32, *mut u8, u32) -> u8;
/// Signature polling the collectible loader for a new project assembly. The
/// return value is one of the `POLL_*` codes above.
type PollReloadFn = extern "system" fn() -> u8;
/// Signature telling the loader a new assembly is already complete on disk.
#[cfg(feature = "hot_reload")]
type NotifyAssemblyReplacedFn = extern "system" fn();
/// Signature returning the byte length of the pending version's manifest.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
type PendingManifestLengthFn = extern "system" fn() -> u32;
/// Signature copying the pending version's manifest into a caller buffer.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
type CopyPendingManifestFn = extern "system" fn(*mut u8, u32) -> u8;
/// Signature installing the pending version once its manifest is in force.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
type CommitReloadFn = extern "system" fn() -> u8;
/// Signature discarding the pending version when its manifest is refused.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
type AbortReloadFn = extern "system" fn() -> u8;

// Every field below is filled from the loaded assembly in both postures, but
// only `verify_systems_unchanged` reads it, and that runs only on the reload
// path. The allowance is limited to the build where the reader is compiled
// out, rather than applied unconditionally, so a field that becomes genuinely
// unused still warns in a reloading build.
/// Reflected metadata of one managed system, captured at startup.
///
/// The snapshot is compared against a re-reflection after every successful
/// reload, so a managed-side bug that silently changes system metadata cannot
/// run stale index bindings unnoticed.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
struct ManagedSystemSnapshot {
    /// Reflected native access list resolved at startup.
    accesses: Box<[NativeSystemAccess]>,
    /// Whether the managed system declared a Commands parameter.
    uses_commands: bool,
    /// The system's declared order within its phase.
    order: i32,
    /// The system's declared frame phase.
    phase: RunPhase,
    /// Whether the system also runs in edit mode.
    run_in_edit_mode: bool,
    /// The name the system was registered under, after the synthetic fallback.
    ///
    /// Recorded because a rename changes nothing else a reload can see: not
    /// the accesses, not the Commands flag. The name is still the identity the
    /// scheduler, the profiler and every log line report.
    name: Box<str>,
}

/// Owns one hosted managed runtime: CoreCLR through hostfxr, or a loaded
/// NativeAOT library. Keeping this value alive keeps every function pointer
/// resolved from that runtime valid.
///
/// A NativeAOT project is the shipping posture, where nothing reloads, so a
/// reloading build never constructs the `Aot` variant and it is allowed to be
/// unused there.
#[cfg_attr(feature = "hot_reload", allow(dead_code))]
pub(crate) enum ManagedRuntimeContext {
    /// CoreCLR booted through hostfxr (framework-dependent posture).
    #[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
    Dotnet(DotnetRuntimeContext),
    /// A NativeAOT library loaded directly (self-contained posture).
    #[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
    Aot(AotRuntimeContext),
}

/// Owns the hosted .NET context, the stable API table and the reload callback.
///
/// Keeping `_runtime` and `_api` alive keeps the managed runtime and every
/// native function pointer in this struct valid, for as long as registered
/// scheduler closures may run.
// The fields below are the assembly's polling and reflection exports. They are
// resolved at startup either way, but only `poll_reload` and
// `verify_systems_unchanged` read them, and both are reload-only, so the
// allowance is limited to builds where those readers are compiled out.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub struct CSharpRuntime {
    /// Unmanaged export polling the collectible loader for a rebuilt assembly.
    poll_reload: PollReloadFn,
    /// The project assembly this runtime hosts, as configured.
    ///
    /// Carried so a refusal on the reload path - "declares no `[System]`
    /// methods" is the one that needs it - names the artifact that failed
    /// rather than a fixed project name.
    subject: String,
    /// Byte length of the manifest the pending version carries.
    #[cfg(feature = "hot_reload")]
    pending_manifest_length: PendingManifestLengthFn,
    /// Copies the pending version's manifest for validation.
    #[cfg(feature = "hot_reload")]
    copy_pending_manifest: CopyPendingManifestFn,
    /// Installs the pending version once its manifest is in force.
    #[cfg(feature = "hot_reload")]
    commit_reload: CommitReloadFn,
    /// Discards the pending version when its manifest cannot be applied.
    #[cfg(feature = "hot_reload")]
    abort_reload: AbortReloadFn,
    /// Every export a re-registration pass needs, including the ones the
    /// reload's metadata comparison reads one system at a time.
    ///
    /// Held so a reload can rebuild the scheduler's managed systems from the
    /// arriving assembly, and so the comparison and the re-registration read
    /// the same descriptors.
    exports: SystemExports,
    /// Loader answer of the most recent reload poll, for one-shot rejection
    /// logging.
    ///
    /// Every path of `poll_reload` records it, so the rejection check never
    /// compares against a stale answer. The stopped guard records
    /// `POLL_REJECTED` without polling, and a manifest-pending answer stays
    /// recorded as the loader's answer even when the host then aborts the
    /// swap.
    #[cfg(feature = "hot_reload")]
    last_poll_status: u8,
    /// Set once the world has taken a manifest whose assembly never loaded.
    ///
    /// This is the one outcome the commit handshake cannot make safe by
    /// ordering. The manifest is applied while the outgoing assembly still
    /// runs, so a refusal normally costs an unload and nothing else. If the
    /// *commit* then fails, the world holds layouts that belong to an assembly
    /// which was never installed, and every later frame would read migrated
    /// rows through the old assembly's expectations. The project's systems are
    /// cleared, and this flag keeps them cleared until the host restarts.
    #[cfg(feature = "hot_reload")]
    manifest_applied_without_assembly: bool,
    /// Metadata snapshot the active assembly is verified against after reload.
    system_snapshot: Vec<ManagedSystemSnapshot>,
    /// The live component-binding table, shared with every registered system.
    ///
    /// It is shared instead of copied per system so a reload can rewrite it in
    /// place. Each run takes a read lock, so a component whose layout was
    /// reshaped is seen by the systems already registered to read it.
    bindings: Arc<BindingStore>,
    /// Clears the loader's poll interval after an in-process compile.
    ///
    /// Only an in-process compile knows for certain that the assembly file is
    /// complete, and only `pill_development_host` compiles in process, so no
    /// shipped build ever calls this. The AOT posture has no export for it at
    /// all: a shipped project may only export from its root assembly, and it
    /// would otherwise have to re-export a function it can never call.
    #[cfg(feature = "hot_reload")]
    notify_assembly_replaced: Option<NotifyAssemblyReplacedFn>,
    /// Keeps the hosted .NET runtime alive for the host's lifetime.
    _runtime: ManagedRuntimeContext,
    /// Keeps the native API table alive so registered closures stay valid.
    _api: Box<CsEngineApi>,
}

/// The managed exports one system-registration pass needs.
///
/// Collected instead of passed one by one: the pass has two callers - the
/// startup and the reload that re-registers a changed system set - and both
/// read the same twelve function pointers.
#[derive(Clone, Copy)]
struct SystemExports {
    /// Reports how many systems the assembly registered.
    system_count: SystemCountFn,
    /// Reports how many accesses one system declared.
    access_count: SystemAccessCountFn,
    /// Copies one system's reflected accesses into a caller buffer.
    get_access: GetSystemAccessFn,
    /// Reports whether one system declares a Commands parameter.
    system_uses_commands: SystemUsesCommandsFn,
    /// Reports one system's declared order within its phase.
    system_order: SystemOrderFn,
    /// Reports one system's declared frame phase (0, 1 or 2).
    system_run_phase: SystemRunPhaseFn,
    /// Reports whether one system also runs in edit mode.
    system_run_in_edit_mode: SystemRunInEditModeFn,
    /// Invokes one system for a frame.
    run_system: RunSystemFn,
    /// Byte length of one system's reflected name.
    system_name_length: SystemNameLengthFn,
    /// Copies that name into a caller buffer.
    copy_system_name: CopySystemNameFn,
    /// Byte length of the message a failed system left.
    system_error_length: SystemErrorMessageLengthFn,
    /// Copies that message into a caller buffer.
    copy_system_error: CopySystemErrorMessageFn,
}

/// One managed system that has been resolved and checked, but not yet given to
/// the engine.
///
/// The type exists to separate the two halves of registration. Resolving a
/// system can fail, for example when an access names a component nobody
/// registered. Handing a resolved system to the scheduler cannot fail. Because
/// every system is resolved before any is registered, a reload can refuse a bad
/// assembly while the previous generation's systems are still installed,
/// instead of clearing them first and finding out that the new set cannot run.
struct PreparedManagedSystem {
    /// Name the system registers under, after the synthetic fallback.
    name: String,
    /// Scheduler access list, already resolved against the binding table.
    access: SystemAccess,
    /// Reflected managed accesses the run closure installs its scope from.
    managed_access: Box<[NativeSystemAccess]>,
    /// Whether the managed system declared a `Commands` parameter.
    uses_commands: bool,
    /// Declared order within the phase.
    order: i32,
    /// Declared frame phase.
    phase: RunPhase,
    /// Whether the system also runs in edit mode.
    run_in_edit_mode: bool,
    /// Managed discovery index this system is dispatched by.
    system_index: u32,
    /// Metadata a later reload compares to tell a behaviour swap from a
    /// signature change.
    snapshot: ManagedSystemSnapshot,
}

/// Resolves every managed system without touching the engine.
///
/// This is the half that can fail. It reads each system's reflected accesses,
/// resolves them against the binding table, and records the metadata the
/// scheduler closure needs, but it registers nothing. A failure here therefore
/// leaves the engine exactly as it was.
///
/// # Errors
///
/// Returns [`CSharpError::NoSystems`] when the assembly declares no systems,
/// and [`CSharpError::SystemCountOutOfRange`] when a reported system or access
/// count is above its limit. [`CSharpError::SystemSnapshotAllocationFailed`]
/// reports a snapshot that could not be reserved, and
/// [`CSharpError::SystemAccessFailed`] an access entry the managed side would
/// not hand over.
///
/// Returns what `derive_system_access` refuses for a bad access:
/// [`CSharpError::UnregisteredComponent`] when the access names a component
/// nobody registered, [`CSharpError::UnregisteredResource`] when it names an
/// unregistered managed resource, and [`CSharpError::UnknownAccessMode`] when
/// its mode is neither read nor write.
fn prepare_managed_systems(
    bindings: &Arc<BindingStore>,
    exports: SystemExports,
    subject: &str,
) -> Result<Vec<PreparedManagedSystem>, CSharpError> {
    let system_count = (exports.system_count)();
    if system_count == 0 {
        return Err(CSharpError::NoSystems {
            subject: subject.to_string(),
        });
    }
    let mut prepared_systems = Vec::new();
    prepared_systems
        .try_reserve_exact(checked_system_count(system_count)?)
        .map_err(|_| CSharpError::SystemSnapshotAllocationFailed)?;
    for system_index in 0..system_count {
        let system_access_count = (exports.access_count)(system_index);
        let mut managed_access = Vec::with_capacity(checked_access_count(system_access_count)?);
        for access_index in 0..system_access_count {
            let mut native_access = NativeSystemAccess {
                component_key: 0,
                component_key_high: 0,
                mode: 0,
                kind: 0,
            };
            if (exports.get_access)(system_index, access_index, &mut native_access) == 0 {
                return Err(CSharpError::SystemAccessFailed {
                    system: system_index,
                    access: access_index,
                });
            }
            managed_access.push(native_access);
        }
        // The stored accesses must match the count the managed side reported.
        // The assert catches a later change to the fill loop that could exit
        // early and leave the snapshot short.
        debug_assert_eq!(managed_access.len(), system_access_count as usize);

        let uses_commands = (exports.system_uses_commands)(system_index) != 0;
        // The schedule values are read beside the access list, so one
        // registration pass describes the whole declaration. An unknown phase
        // is refused by number: running a system in a phase its declaration
        // never named is a schedule nobody asked for.
        let order = (exports.system_order)(system_index);
        let reported_phase = (exports.system_run_phase)(system_index);
        let Some(phase) = run_phase_from_export(reported_phase) else {
            return Err(CSharpError::UnknownRunPhase {
                system: system_index,
                phase: reported_phase,
            });
        };
        let run_in_edit_mode = (exports.system_run_in_edit_mode)(system_index) != 0;
        let mut access = derive_system_access(&managed_access, &bindings.read())?;
        access.set_uses_commands(uses_commands);
        // Prefer the reflected managed name (type and method) so profiling and
        // scheduler debugging show real identities. `resolved_system_name`
        // falls back to an index-based name when the managed side reports no
        // usable one.
        let name = resolved_system_name(
            exports.system_name_length,
            exports.copy_system_name,
            system_index,
        );
        // Snapshot the reflected metadata before moving the access list into
        // the scheduler closure, so a later reload can tell a behaviour-only
        // swap from one that changed a signature.
        let snapshot = ManagedSystemSnapshot {
            accesses: managed_access.clone().into_boxed_slice(),
            uses_commands,
            order,
            phase,
            run_in_edit_mode,
            name: name.as_str().into(),
        };
        prepared_systems.push(PreparedManagedSystem {
            name,
            access,
            managed_access: managed_access.into_boxed_slice(),
            uses_commands,
            order,
            phase,
            run_in_edit_mode,
            system_index,
            snapshot,
        });
    }
    Ok(prepared_systems)
}

/// Hands every prepared system to the scheduler.
///
/// The half that cannot fail. Every access was resolved by
/// [`prepare_managed_systems`], so nothing here can refuse, which is what makes
/// the caller's clear-then-register sequence safe.
fn commit_managed_systems(
    engine: &mut Engine,
    bindings: &Arc<BindingStore>,
    exports: SystemExports,
    prepared_systems: Vec<PreparedManagedSystem>,
) -> Vec<ManagedSystemSnapshot> {
    let mut system_snapshots = Vec::with_capacity(prepared_systems.len());
    for system in prepared_systems {
        let PreparedManagedSystem {
            name,
            access,
            managed_access,
            uses_commands,
            order,
            phase,
            run_in_edit_mode,
            system_index,
            snapshot,
        } = system;
        system_snapshots.push(snapshot);
        let system_bindings = Arc::clone(bindings);
        let run_system = exports.run_system;
        let system_error_length = exports.system_error_length;
        let copy_system_error = exports.copy_system_error;
        // The declaration's schedule reaches the engine through the same
        // options the Rust `#[system]` attribute fills, so a managed system
        // takes its place in the one `(phase, order)` list beside every other
        // artifact's.
        let options = SystemOptions::new(phase, order).with_run_in_edit_mode(run_in_edit_mode);
        // SAFETY: `access` was built by `derive_system_access` from the same
        // `managed_access` list the closure gives `ActiveSystemGuard`, so the
        // declaration covers every component and resource the closure can
        // reach.
        unsafe {
            engine.register_system_with_access_and_options(
                name,
                options,
                access,
                move |world: &mut World, queue: &mut CommandQueue| -> Result<(), SystemError> {
                    // Every managed callback needs an active scope, so a
                    // refused scope is reported as a system error instead of
                    // running the system without one. The live table is read
                    // once per run: a reload can add or reshape a component
                    // between frames, so the scope must describe the storage
                    // this run touches.
                    let system_bindings = system_bindings.read();
                    let Some(_guard) = ActiveScopeGuard::install_scope_with_commands(
                        world,
                        queue,
                        &managed_access,
                        &system_bindings,
                        uses_commands,
                    ) else {
                        return Err(SystemError::Managed {
                            message: "nested managed system invocation".to_string(),
                        });
                    };
                    if run_system(system_index) == 0 {
                        return Err(SystemError::Managed {
                            message: managed_system_error_message(
                                system_error_length,
                                copy_system_error,
                                system_index,
                            ),
                        });
                    }
                    Ok(())
                },
            );
        }
    }
    system_snapshots
}

/// Resolves and registers every managed system in one step.
///
/// The startup path's entry point: there is nothing installed yet, so the
/// prepare/commit split buys nothing and the two halves run back to back. A
/// reload calls them separately - see `CSharpRuntime::reregister_systems`.
///
/// # Errors
///
/// Whatever [`prepare_managed_systems`] refuses.
fn register_managed_systems(
    engine: &mut Engine,
    bindings: &Arc<BindingStore>,
    exports: SystemExports,
    subject: &str,
) -> Result<Vec<ManagedSystemSnapshot>, CSharpError> {
    let prepared_systems = prepare_managed_systems(bindings, exports, subject)?;
    Ok(commit_managed_systems(
        engine,
        bindings,
        exports,
        prepared_systems,
    ))
}

/// Resolves one managed artifact (a runtime assembly, its `runtimeconfig.json`,
/// or the project assembly / AOT library) shipped next to the host executable,
/// falling back to the source-tree location the generated bundle baked in.
///
/// A shipping bundle is self-contained: every managed file the host needs is
/// copied into the same dated output folder as the executable, so a shipped
/// build prefers `current_exe()`'s directory and runs on any machine with no
/// engine source tree present. A developer run straight from `cargo run` has no
/// such files next to its binary, so the `workspace_root`-relative path is the
/// fallback it ends up using.
fn shipped_or_baked(workspace_root: &Path, baked_relative_dir: &str, file_name: &str) -> PathBuf {
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let shipped = exe_dir.join(file_name);
        if shipped.is_file() {
            return shipped;
        }
    }
    workspace_root.join(baked_relative_dir).join(file_name)
}

/// Every managed export the host calls, resolved once per posture.
///
/// The postures differ only in how a function pointer is obtained - a
/// `(assembly, type, method)` triple through hostfxr's delegate, or a
/// `pill_*` symbol through `libloading`. Keeping that difference inside two
/// constructors lets one startup consume either, so the `unsafe`
/// system-registration closure and the transactional startup rollback have
/// one copy.
struct ManagedExports {
    /// The managed runtime entry point that receives the API table.
    init: InitFn,
    /// Number of registered scheduler systems.
    system_count: SystemCountFn,
    /// Number of managed startup methods.
    startup_count: StartupCountFn,
    /// Whether one system declares a Commands parameter.
    system_uses_commands: SystemUsesCommandsFn,
    /// Declared order of one system within its phase.
    system_order: SystemOrderFn,
    /// Declared frame phase of one system (0, 1 or 2).
    system_run_phase: SystemRunPhaseFn,
    /// Whether one system also runs in edit mode.
    system_run_in_edit_mode: SystemRunInEditModeFn,
    /// Executes one managed startup method by index.
    run_startup: RunStartupFn,
    /// Byte length of the serialized component manifest.
    manifest_length: ComponentManifestLengthFn,
    /// Copies the serialized component manifest into a caller buffer.
    copy_manifest: CopyComponentManifestFn,
    /// Byte length of one system's reflected name.
    system_name_length: SystemNameLengthFn,
    /// Copies one system's reflected name into a caller buffer.
    copy_system_name: CopySystemNameFn,
    /// Number of accesses one system declared.
    access_count: SystemAccessCountFn,
    /// Copies one system's reflected accesses into a caller buffer.
    get_access: GetSystemAccessFn,
    /// Runs one scheduler system by index.
    run_system: RunSystemFn,
    /// Byte length of one system's last error message.
    system_error_length: SystemErrorMessageLengthFn,
    /// Copies one system's last error message into a caller buffer.
    copy_system_error: CopySystemErrorMessageFn,
    /// Polls the collectible loader for a rebuilt assembly.
    poll_reload: PollReloadFn,
    /// Byte length of the manifest the pending version carries.
    #[cfg(feature = "hot_reload")]
    pending_manifest_length: PendingManifestLengthFn,
    /// Copies the pending version's manifest for validation.
    #[cfg(feature = "hot_reload")]
    copy_pending_manifest: CopyPendingManifestFn,
    /// Installs the pending version once its manifest is in force.
    #[cfg(feature = "hot_reload")]
    commit_reload: CommitReloadFn,
    /// Discards the pending version when its manifest cannot be applied.
    #[cfg(feature = "hot_reload")]
    abort_reload: AbortReloadFn,
    /// Clears the loader's poll interval after an in-process compile.
    #[cfg(feature = "hot_reload")]
    notify_assembly_replaced: Option<NotifyAssemblyReplacedFn>,
}

impl ManagedExports {
    /// Resolves every export through hostfxr's managed delegate.
    ///
    /// The ABI contract is validated before any export is resolved: a runtime
    /// built against different signatures must be rebuilt before a single one
    /// of its pointers can be trusted.
    fn from_dotnet(
        runtime: &DotnetRuntimeContext,
        assembly: &Path,
        type_name: &str,
    ) -> Result<Self, CSharpError> {
        let interop_version = runtime.resolve_unmanaged_function_pointer::<InteropVersionFn>(
            assembly,
            type_name,
            "InteropVersion",
        )?;
        let reported = interop_version();
        if reported != INTEROP_CONTRACT_VERSION {
            return Err(CSharpError::InteropVersionMismatch {
                expected: INTEROP_CONTRACT_VERSION,
                actual: reported,
            });
        }
        Ok(Self {
            init: runtime
                .resolve_unmanaged_function_pointer::<InitFn>(assembly, type_name, "Init")?,
            system_count: runtime.resolve_unmanaged_function_pointer::<SystemCountFn>(
                assembly,
                type_name,
                "SystemCount",
            )?,
            startup_count: runtime.resolve_unmanaged_function_pointer::<StartupCountFn>(
                assembly,
                type_name,
                "StartupCount",
            )?,
            system_uses_commands: runtime
                .resolve_unmanaged_function_pointer::<SystemUsesCommandsFn>(
                    assembly,
                    type_name,
                    "SystemUsesCommands",
                )?,
            system_order: runtime.resolve_unmanaged_function_pointer::<SystemOrderFn>(
                assembly,
                type_name,
                "SystemOrder",
            )?,
            system_run_phase: runtime.resolve_unmanaged_function_pointer::<SystemRunPhaseFn>(
                assembly,
                type_name,
                "SystemRunPhase",
            )?,
            system_run_in_edit_mode: runtime
                .resolve_unmanaged_function_pointer::<SystemRunInEditModeFn>(
                    assembly,
                    type_name,
                    "SystemRunInEditMode",
                )?,
            run_startup: runtime.resolve_unmanaged_function_pointer::<RunStartupFn>(
                assembly,
                type_name,
                "RunStartup",
            )?,
            manifest_length: runtime
                .resolve_unmanaged_function_pointer::<ComponentManifestLengthFn>(
                    assembly,
                    type_name,
                    "ComponentManifestLength",
                )?,
            copy_manifest: runtime.resolve_unmanaged_function_pointer::<CopyComponentManifestFn>(
                assembly,
                type_name,
                "CopyComponentManifest",
            )?,
            #[cfg(feature = "hot_reload")]
            pending_manifest_length: runtime
                .resolve_unmanaged_function_pointer::<PendingManifestLengthFn>(
                    assembly,
                    type_name,
                    "PendingManifestLength",
                )?,
            #[cfg(feature = "hot_reload")]
            copy_pending_manifest: runtime
                .resolve_unmanaged_function_pointer::<CopyPendingManifestFn>(
                    assembly,
                    type_name,
                    "CopyPendingManifest",
                )?,
            #[cfg(feature = "hot_reload")]
            commit_reload: runtime.resolve_unmanaged_function_pointer::<CommitReloadFn>(
                assembly,
                type_name,
                "CommitReload",
            )?,
            #[cfg(feature = "hot_reload")]
            abort_reload: runtime.resolve_unmanaged_function_pointer::<AbortReloadFn>(
                assembly,
                type_name,
                "AbortReload",
            )?,
            system_name_length: runtime.resolve_unmanaged_function_pointer::<SystemNameLengthFn>(
                assembly,
                type_name,
                "SystemNameLength",
            )?,
            copy_system_name: runtime.resolve_unmanaged_function_pointer::<CopySystemNameFn>(
                assembly,
                type_name,
                "CopySystemName",
            )?,
            access_count: runtime.resolve_unmanaged_function_pointer::<SystemAccessCountFn>(
                assembly,
                type_name,
                "SystemAccessCount",
            )?,
            get_access: runtime.resolve_unmanaged_function_pointer::<GetSystemAccessFn>(
                assembly,
                type_name,
                "GetSystemAccess",
            )?,
            run_system: runtime.resolve_unmanaged_function_pointer::<RunSystemFn>(
                assembly,
                type_name,
                "RunSystem",
            )?,
            system_error_length: runtime
                .resolve_unmanaged_function_pointer::<SystemErrorMessageLengthFn>(
                    assembly,
                    type_name,
                    "SystemErrorMessageLength",
                )?,
            copy_system_error: runtime
                .resolve_unmanaged_function_pointer::<CopySystemErrorMessageFn>(
                    assembly,
                    type_name,
                    "CopySystemErrorMessage",
                )?,
            poll_reload: runtime.resolve_unmanaged_function_pointer::<PollReloadFn>(
                assembly,
                type_name,
                "PollReload",
            )?,
            #[cfg(feature = "hot_reload")]
            notify_assembly_replaced: Some(
                runtime.resolve_unmanaged_function_pointer::<NotifyAssemblyReplacedFn>(
                    assembly,
                    type_name,
                    "NotifyAssemblyReplaced",
                )?,
            ),
        })
    }

    /// Resolves every export by its `pill_*` symbol in the AOT library.
    ///
    /// Same contract gate as [`Self::from_dotnet`], applied before any symbol
    /// is read.
    fn from_aot(runtime: &AotRuntimeContext) -> Result<Self, CSharpError> {
        let interop_version = runtime
            .resolve_unmanaged_function_pointer::<InteropVersionFn>("pill_interop_version")?;
        let reported = interop_version();
        if reported != INTEROP_CONTRACT_VERSION {
            return Err(CSharpError::InteropVersionMismatch {
                expected: INTEROP_CONTRACT_VERSION,
                actual: reported,
            });
        }
        Ok(Self {
            init: runtime.resolve_unmanaged_function_pointer::<InitFn>("pill_init")?,
            system_count: runtime
                .resolve_unmanaged_function_pointer::<SystemCountFn>("pill_system_count")?,
            startup_count: runtime
                .resolve_unmanaged_function_pointer::<StartupCountFn>("pill_startup_count")?,
            system_uses_commands: runtime
                .resolve_unmanaged_function_pointer::<SystemUsesCommandsFn>(
                    "pill_system_uses_commands",
                )?,
            system_order: runtime
                .resolve_unmanaged_function_pointer::<SystemOrderFn>("pill_system_order")?,
            system_run_phase: runtime
                .resolve_unmanaged_function_pointer::<SystemRunPhaseFn>("pill_system_run_phase")?,
            system_run_in_edit_mode: runtime
                .resolve_unmanaged_function_pointer::<SystemRunInEditModeFn>(
                    "pill_system_run_in_edit_mode",
                )?,
            run_startup: runtime
                .resolve_unmanaged_function_pointer::<RunStartupFn>("pill_run_startup")?,
            manifest_length: runtime
                .resolve_unmanaged_function_pointer::<ComponentManifestLengthFn>(
                    "pill_component_manifest_length",
                )?,
            copy_manifest: runtime.resolve_unmanaged_function_pointer::<CopyComponentManifestFn>(
                "pill_copy_component_manifest",
            )?,
            system_name_length: runtime.resolve_unmanaged_function_pointer::<SystemNameLengthFn>(
                "pill_system_name_length",
            )?,
            copy_system_name: runtime
                .resolve_unmanaged_function_pointer::<CopySystemNameFn>("pill_copy_system_name")?,
            access_count: runtime.resolve_unmanaged_function_pointer::<SystemAccessCountFn>(
                "pill_system_access_count",
            )?,
            get_access: runtime.resolve_unmanaged_function_pointer::<GetSystemAccessFn>(
                "pill_get_system_access",
            )?,
            run_system: runtime
                .resolve_unmanaged_function_pointer::<RunSystemFn>("pill_run_system")?,
            system_error_length: runtime
                .resolve_unmanaged_function_pointer::<SystemErrorMessageLengthFn>(
                    "pill_system_error_message_length",
                )?,
            copy_system_error: runtime
                .resolve_unmanaged_function_pointer::<CopySystemErrorMessageFn>(
                    "pill_copy_system_error_message",
                )?,
            poll_reload: runtime
                .resolve_unmanaged_function_pointer::<PollReloadFn>("pill_poll_reload")?,
            #[cfg(feature = "hot_reload")]
            pending_manifest_length: runtime
                .resolve_unmanaged_function_pointer::<PendingManifestLengthFn>(
                    "pill_pending_manifest_length",
                )?,
            #[cfg(feature = "hot_reload")]
            copy_pending_manifest: runtime
                .resolve_unmanaged_function_pointer::<CopyPendingManifestFn>(
                    "pill_copy_pending_manifest",
                )?,
            #[cfg(feature = "hot_reload")]
            commit_reload: runtime
                .resolve_unmanaged_function_pointer::<CommitReloadFn>("pill_commit_reload")?,
            #[cfg(feature = "hot_reload")]
            abort_reload: runtime
                .resolve_unmanaged_function_pointer::<AbortReloadFn>("pill_abort_reload")?,
            #[cfg(feature = "hot_reload")]
            notify_assembly_replaced: None,
        })
    }
}

impl CSharpRuntime {
    /// Starts .NET, loads `csharp_runtime`, discovers the managed systems and
    /// registers each one under its reflected read/write access declaration.
    ///
    /// `on_runtime_booted` receives the .NET runtime once it is up and before
    /// the project starts. Development tooling loads its own assemblies into it
    /// at that point: `pill_development_host` starts its in-process compiler
    /// here, so the compiler's warmup runs during startup rather than during
    /// the developer's first edit. A shipped CoreCLR build passes a no-op.
    ///
    /// # Errors
    ///
    /// Returns a [`CSharpError`] when .NET cannot be started, such as
    /// [`CSharpError::HostfxrNotFound`] or
    /// [`CSharpError::RuntimeInitializationFailed`].
    /// [`CSharpError::ManagedEntryPointFailed`] reports an export that could not
    /// be resolved, [`CSharpError::InteropVersionMismatch`] a runtime built for
    /// another ABI version, and [`CSharpError::RuntimeInitFailed`] a runtime
    /// that refused host initialization.
    ///
    /// It also reports every failure of [`Self::start_with`]: a component
    /// manifest that could not be read or registered, a startup method that
    /// failed, an assembly that declares no systems, and a reflected access
    /// that names a component or resource nobody registered.
    pub fn start(
        engine: &mut Engine,
        workspace_root: &Path,
        config: &CSharpModuleConfig,
        exposed_components: &[ModuleExposedComponent],
        mirror_methods: &[ResolvedMirrorMethod],
        on_runtime_booted: &mut dyn FnMut(&DotnetRuntimeContext),
    ) -> Result<Self, CSharpError> {
        // Step 1: Resolve assembly paths, start .NET, and load managed exports.
        // A shipped bundle keeps the runtime sidecars and the project assembly
        // flat next to the executable, so prefer those copies (portable); the
        // generated bundle's source-tree paths are the developer fallback.
        let runtime_assembly_name = format!("{}.dll", config.runtime_assembly_name);
        let runtime_config_name = format!("{}.runtimeconfig.json", config.runtime_assembly_name);
        let project_assembly_name = format!("{}.dll", config.project_assembly_name);
        let assembly = shipped_or_baked(
            workspace_root,
            &config.runtime_output_subdirectory,
            &runtime_assembly_name,
        );
        let runtime_config = shipped_or_baked(
            workspace_root,
            &config.runtime_output_subdirectory,
            &runtime_config_name,
        );
        let project_assembly = shipped_or_baked(
            workspace_root,
            &config.project_output_subdirectory,
            &project_assembly_name,
        );
        // The managed side resolves the project assembly against this directory
        // plus the assembly file name; keep them in the same folder (shipped:
        // the exe's directory, developer: the baked build output).
        let project_dir = project_assembly
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| workspace_root.join(&config.project_output_subdirectory));
        std::env::set_var("ECS_CSHARP_PROJECT_DIR", &project_dir);
        std::env::set_var("ECS_CSHARP_PROJECT_ASSEMBLY", project_assembly_name);

        let runtime = DotnetRuntimeContext::new(&runtime_config)?;
        on_runtime_booted(&runtime);
        let type_name = format!(
            "TracyLive.Loader.LoaderInterop, {}",
            config.runtime_assembly_name
        );
        let exports = ManagedExports::from_dotnet(&runtime, &assembly, &type_name)?;

        Self::start_with(
            engine,
            ManagedRuntimeContext::Dotnet(runtime),
            exports,
            exposed_components,
            mirror_methods,
            &config.project_assembly_name,
        )
    }

    /// The one startup lifecycle, shared by both postures.
    ///
    /// Consumes an already-resolved [`ManagedExports`] and the runtime it was
    /// resolved from: registers the component manifest, runs the managed
    /// startup methods transactionally, reflects each system's accesses and
    /// registers it with the scheduler. Everything here has one copy, so the
    /// `unsafe` access-registration closure and the startup rollback guarantee
    /// cannot drift between the hostfxr and NativeAOT paths.
    ///
    /// # Errors
    ///
    /// [`CSharpError::RuntimeInitFailed`] when the managed side refuses host
    /// initialization. The manifest copy reports
    /// [`CSharpError::ManifestLengthOutOfRange`],
    /// [`CSharpError::ManifestAllocationFailed`] or
    /// [`CSharpError::ManifestCopyFailed`], and registering it reports a parse
    /// or validation failure of the manifest itself.
    /// [`CSharpError::StartupFailed`] reports a startup method that failed and
    /// [`CSharpError::StartupCommandsFailed`] commands that would not apply.
    /// System registration returns what [`prepare_managed_systems`] refuses.
    fn start_with(
        engine: &mut Engine,
        runtime: ManagedRuntimeContext,
        exports: ManagedExports,
        exposed_components: &[ModuleExposedComponent],
        mirror_methods: &[ResolvedMirrorMethod],
        subject: &str,
    ) -> Result<Self, CSharpError> {
        // Step 1: Byte-level bindings for every native component exposed to
        // managed code - the renderer data's and each extension's - so a
        // generated mirror whose full name matches resolves to the native
        // storage. No component type is bound by hand.
        let bindings = module_native_bindings(engine, exposed_components);

        // Step 2: Initialize the runtime bridge and register the component
        // manifest copied from the managed assembly.
        let api = Box::new(CsEngineApi::new(mirror_methods));
        if (exports.init)(api.as_ref() as *const CsEngineApi) == 0 {
            return Err(CSharpError::RuntimeInitFailed);
        }

        // The length comes from managed code, so `fetch_managed_buffer` checks
        // it against the cap before allocating and reserves the memory in a way
        // that can fail. The exports are `extern "system"` function pointers,
        // which do not implement the `Fn` traits, so each one is wrapped in a
        // closure that does.
        let manifest = fetch_managed_buffer(
            || (exports.manifest_length)(),
            |pointer, length| (exports.copy_manifest)(pointer, length),
            MAX_COMPONENT_MANIFEST_BYTES,
        )
        .map_err(manifest_fetch_error)?;
        let bindings = Arc::new(BindingStore::new(register_component_manifest(
            engine, &manifest, bindings,
        )?));

        // Step 3: Run every reflected managed startup method transactionally.
        // Commands are queued first and applied only when every startup
        // method reports success, so a failing generation leaves no partial
        // world state behind.
        let startup_bindings = Arc::clone(&bindings);
        let mut startup_failed = None;
        engine.queue_deferred_commands(|world, queue| {
            // One read of the live table for the whole startup batch: every
            // method in it resolves components through the entries the manifest
            // has just registered.
            let startup_bindings = startup_bindings.read();
            for startup_index in 0..(exports.startup_count)() {
                // A rejected scope means a managed startup method re-entered
                // the host. Running it without a scope would only produce
                // failing FFI calls, so treat it as this startup's failure.
                // The scope is exclusive: startups run one at a time before the
                // scheduler starts, so a startup's `Res<T>`/`ResMut<T>`
                // parameters need no declared access.
                let Some(_guard) =
                    ActiveScopeGuard::install_exclusive_scope(world, queue, &startup_bindings)
                else {
                    startup_failed = Some(startup_index);
                    break;
                };
                if (exports.run_startup)(startup_index) == 0 {
                    startup_failed = Some(startup_index);
                    break;
                }
            }
        });
        if let Some(startup_index) = startup_failed {
            // Discard the queued commands, so the failure leaves no partial
            // world state behind.
            engine.discard_deferred_commands();
            return Err(CSharpError::StartupFailed {
                index: startup_index,
            });
        }
        engine
            .flush_deferred_commands()
            .map_err(|errors| CSharpError::StartupCommandsFailed {
                details: format!("{errors:?}"),
            })?;

        // Step 4: Reflect each system's accesses and register it with the
        // scheduler under the exact resolved read/write list.
        let system_exports = SystemExports {
            system_count: exports.system_count,
            access_count: exports.access_count,
            get_access: exports.get_access,
            system_uses_commands: exports.system_uses_commands,
            system_order: exports.system_order,
            system_run_phase: exports.system_run_phase,
            system_run_in_edit_mode: exports.system_run_in_edit_mode,
            run_system: exports.run_system,
            system_name_length: exports.system_name_length,
            copy_system_name: exports.copy_system_name,
            system_error_length: exports.system_error_length,
            copy_system_error: exports.copy_system_error,
        };
        let system_snapshot = register_managed_systems(engine, &bindings, system_exports, subject)?;

        Ok(Self {
            poll_reload: exports.poll_reload,
            subject: subject.to_string(),
            exports: system_exports,
            #[cfg(feature = "hot_reload")]
            last_poll_status: POLL_NO_CHANGE,
            system_snapshot,
            #[cfg(feature = "hot_reload")]
            pending_manifest_length: exports.pending_manifest_length,
            #[cfg(feature = "hot_reload")]
            copy_pending_manifest: exports.copy_pending_manifest,
            #[cfg(feature = "hot_reload")]
            commit_reload: exports.commit_reload,
            #[cfg(feature = "hot_reload")]
            abort_reload: exports.abort_reload,
            bindings,
            #[cfg(feature = "hot_reload")]
            manifest_applied_without_assembly: false,
            #[cfg(feature = "hot_reload")]
            notify_assembly_replaced: exports.notify_assembly_replaced,
            _runtime: runtime,
            _api: api,
        })
    }

    /// Tells the loader that a complete project assembly is already on disk.
    ///
    /// The loader samples the assembly's timestamp on an interval because it
    /// cannot otherwise tell a finished build from a half-copied one. An
    /// in-process compile writes the file itself, through an atomic rename,
    /// so its caller uses this to let the next poll skip that wait entirely.
    /// Does nothing in the AOT posture, which never reloads.
    #[cfg(feature = "hot_reload")]
    pub fn notify_assembly_replaced(&self) {
        if let Some(notify) = self.notify_assembly_replaced {
            notify();
        }
    }

    /// Starts a NativeAOT-published library, resolves the loader exports by
    /// symbol, discovers the managed systems and registers each one under its
    /// declared read/write access.
    ///
    /// Mirrors [`Self::start`]: only the export resolution differs - a direct
    /// `libloading` load of the AOT native library (which embeds a trimmed
    /// runtime, so no .NET install and no JIT are involved) instead of the
    /// hostfxr bootstrap. Everything after that is [`Self::start_with`].
    ///
    /// # Errors
    ///
    /// Returns [`CSharpError::AotLibraryNotFound`] or
    /// [`CSharpError::AotLibraryLoadFailed`] when the library cannot be loaded,
    /// [`CSharpError::AotExportMissing`] for a symbol the library does not
    /// export, and [`CSharpError::InteropVersionMismatch`] for a library built
    /// against another ABI version.
    ///
    /// It also reports every failure of [`Self::start_with`].
    pub fn start_aot(
        engine: &mut Engine,
        workspace_root: &Path,
        config: &CSharpModuleConfig,
        exposed_components: &[ModuleExposedComponent],
        mirror_methods: &[ResolvedMirrorMethod],
    ) -> Result<Self, CSharpError> {
        // Step 1: load the AOT native library and resolve every export by its
        // `pill_*` symbol. A shipped bundle carries the library next to the
        // executable (portable); the `dotnet publish` output the generated
        // bundle describes is the developer fallback.
        let library_path = shipped_or_baked(
            workspace_root,
            &config.project_output_subdirectory,
            &format!("{}.dll", config.project_assembly_name),
        );
        let runtime = AotRuntimeContext::new(&library_path)?;
        let exports = ManagedExports::from_aot(&runtime)?;

        Self::start_with(
            engine,
            ManagedRuntimeContext::Aot(runtime),
            exports,
            exposed_components,
            mirror_methods,
            &config.project_assembly_name,
        )
    }

    /// Polls the collectible loader and reports the outcome of any swap attempt.
    ///
    /// The loader swaps a behaviour-compatible assembly by itself. A version
    /// whose component manifest changed is held back
    /// ([`POLL_MANIFEST_PENDING`]) until the host applies the manifest and
    /// commits or aborts, and a changed system set is re-registered from the
    /// arriving assembly rather than refused. Only a changed startup method
    /// set or an assembly that cannot be loaded is refused. A rejection is
    /// logged once per attempt, so the poll that runs every frame cannot
    /// flood the terminal.
    ///
    /// # Errors
    ///
    /// Returns [`CSharpError::UnknownPollStatus`] for a code outside
    /// [`POLL_NO_CHANGE`], [`POLL_RELOADED`], [`POLL_REJECTED`] and
    /// [`POLL_MANIFEST_PENDING`]: a status the host cannot interpret is a
    /// failure to report, not a change that never happened. The currently
    /// loaded assembly is kept either way.
    #[cfg(feature = "hot_reload")]
    pub fn poll_reload(&mut self, engine: &mut Engine) -> Result<u8, CSharpError> {
        // Step 1: Stay stopped once the world and the assembly disagree. A
        // world that took a manifest whose assembly never loaded cannot be
        // repaired by another poll: the mismatch is already in the world's
        // layouts.
        if self.manifest_applied_without_assembly {
            // The guard answers without polling, so it records the rejection
            // it reports like every other path.
            record_poll_answer(&mut self.last_poll_status, POLL_REJECTED);
            return Ok(POLL_REJECTED);
        }
        // Step 2: Poll the loader, and refuse a status this host cannot read.
        let status = (self.poll_reload)();
        if !poll_status_is_known(status) {
            // Logged once per distinct code: the poll runs every frame and a
            // persistent unknown status must not flood the terminal.
            if self.last_poll_status != status {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    status,
                    "the managed loader reported an unknown reload status; keeping the currently loaded assembly"
                );
            }
            self.last_poll_status = status;
            return Err(CSharpError::UnknownPollStatus { status });
        }
        // Step 3: Act on the outcome. A pending manifest is applied and then
        // committed or aborted here; a swap has to have its system metadata
        // checked. The answer is recorded before it is acted on: the rejection
        // check reads the previous answer, and the manifest handshake below
        // keeps the loader's answer even when the host then aborts the swap.
        if record_poll_answer(&mut self.last_poll_status, status) {
            error!(
                target: telemetry_target::HOT_RELOAD,
                "C# reload rejected: the managed loader refused the new assembly (its startup method set changed, or it failed to load); the running assembly stays loaded"
            );
        }
        if status == POLL_MANIFEST_PENDING {
            return self.decide_pending_manifest(engine);
        }
        if status == POLL_RELOADED {
            // A changed system set is rebuilt rather than refused. The
            // comparison stays because it keeps the ordinary behaviour-only
            // swap cheap: that swap keeps its scheduler graph and pays one
            // metadata comparison, while only a real signature change pays for
            // a clear and a re-registration.
            if !self.verify_systems_unchanged() {
                self.reregister_systems(engine)?;
            }
            info!(
                target: telemetry_target::HOT_RELOAD,
                "C# hot reload complete"
            );
            // No manifest check runs after the swap here: a version whose
            // manifest differs never reaches this branch. The managed loader
            // holds such a version back instead (`POLL_MANIFEST_PENDING`), and
            // that handshake applies the manifest before the swap commits,
            // while the outgoing assembly is still the running one. If the
            // managed side ever stops holding those versions back, re-read the
            // swapped assembly's manifest here and apply it before the frame's
            // systems run, so a migrated row is in place before the new
            // generation reads it.
        }
        Ok(status)
    }

    /// Decides what happens to the pending version: applies its manifest, then
    /// commits or aborts the swap.
    ///
    /// The ordering is the point of the handshake. The manifest is applied
    /// while the outgoing assembly is still the running one, so a refusal only
    /// costs an unload. Committing afterwards means the world and the arriving
    /// assembly agree from the moment the swap happens. Applying the manifest
    /// after a swap would leave a running assembly against a world that never
    /// took its layout.
    ///
    /// Returns the status the caller should report: `POLL_RELOADED` when the
    /// swap happened, `POLL_REJECTED` when it did not.
    ///
    /// # Errors
    ///
    /// Whatever re-registering the arriving assembly's systems refuses, when
    /// the committed version changed them.
    #[cfg(feature = "hot_reload")]
    fn decide_pending_manifest(&mut self, engine: &mut Engine) -> Result<u8, CSharpError> {
        // Step 1: Read the manifest the pending version carries.
        let Some(manifest) = self.read_pending_manifest() else {
            self.abort_pending_version("its component manifest could not be read");
            return Ok(POLL_REJECTED);
        };

        // Step 2: Apply the manifest, then commit or abort the swap.
        match apply_component_manifest_on_reload(engine, &manifest, &self.bindings) {
            Ok(report) => {
                if (self.commit_reload)() == 0 {
                    // The manifest is in force, but the swap did not happen, so
                    // the running assembly no longer matches the world. This is
                    // the one outcome the handshake cannot make safe by
                    // ordering alone. Scheduling the project's systems against
                    // a world they no longer describe is never right, so they
                    // are cleared and the runtime stays stopped until the host
                    // restarts.
                    let cleared_system_count = engine.clear_systems_owned_by(SystemOwner::PROJECT);
                    self.system_snapshot.clear();
                    self.manifest_applied_without_assembly = true;
                    error!(
                        target: telemetry_target::HOT_RELOAD,
                        cleared_systems = cleared_system_count,
                        "the managed loader failed to install a version whose manifest was already applied; the project's systems have been stopped - restart the host"
                    );
                    return Ok(POLL_REJECTED);
                }
                // Only the commit swaps the managed system table, so this is
                // the first point at which the exports describe the arriving
                // assembly rather than the outgoing one - which is why the
                // comparison cannot sit beside the manifest check above, where
                // it could only ever compare the running set against itself.
                if !self.verify_systems_unchanged() {
                    self.reregister_systems(engine)?;
                }
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    added = report.added.len(),
                    migrated = report.migrated.len(),
                    renamed = report.renamed.len(),
                    retired = report.retired.len(),
                    resources_added = report.resources_added.len(),
                    resources_migrated = report.resources_migrated.len(),
                    resources_renamed = report.resources_renamed.len(),
                    resources_retired = report.resources_retired.len(),
                    "C# hot reload complete"
                );
                Ok(POLL_RELOADED)
            }
            Err(error) => {
                self.abort_pending_version(&error.to_string());
                Ok(POLL_REJECTED)
            }
        }
    }

    /// Copies the pending version's manifest, or `None` when it cannot be read.
    #[cfg(feature = "hot_reload")]
    fn read_pending_manifest(&self) -> Option<Vec<u8>> {
        read_reported_manifest(
            || (self.pending_manifest_length)(),
            |pointer, length| (self.copy_pending_manifest)(pointer, length),
            "parked",
            "",
        )
    }

    /// Tells the managed loader to discard the pending version, and says why.
    #[cfg(feature = "hot_reload")]
    fn abort_pending_version(&mut self, reason: &str) {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            reason,
            "C# reload rejected: the component manifest could not be applied; the running assembly keeps its rows"
        );
        if (self.abort_reload)() == 0 {
            error!(
                target: telemetry_target::HOT_RELOAD,
                "the managed loader failed to discard the parked version; restart the host"
            );
        }
    }

    /// Rebuilds the scheduler's managed systems from the arriving assembly.
    ///
    /// Only the project's systems are cleared, so a module's systems survive
    /// untouched (the same scoping a module reload relies on). This runs
    /// between frames, at the point the swap is applied, because clearing
    /// systems while the scheduler is running them is not safe.
    ///
    /// A failed re-registration leaves the running generation's systems, and
    /// the snapshot that describes them, in place: every system is resolved
    /// before anything is cleared.
    ///
    /// # Errors
    ///
    /// Whatever [`prepare_managed_systems`] refuses - most often a system
    /// declaring a component or resource the arriving manifest never
    /// registered.
    #[cfg(feature = "hot_reload")]
    fn reregister_systems(&mut self, engine: &mut Engine) -> Result<(), CSharpError> {
        // Resolve every system first. A refusal here - an access naming a
        // component the arriving assembly never registered is the usual one -
        // must leave the running generation's systems in place, because there
        // is nothing to fall back to once they are cleared.
        let prepared_systems =
            prepare_managed_systems(&self.bindings, self.exports, &self.subject)?;
        let removed = engine.clear_systems_owned_by(SystemOwner::PROJECT);
        self.system_snapshot.clear();
        self.system_snapshot =
            commit_managed_systems(engine, &self.bindings, self.exports, prepared_systems);
        info!(
            target: telemetry_target::HOT_RELOAD,
            removed,
            registered = self.system_snapshot.len(),
            "re-registered the project's managed systems after a signature change"
        );
        Ok(())
    }

    /// Re-reads the active project assembly and reports whether its system
    /// metadata still matches the snapshot taken at registration.
    ///
    /// The answer decides how much work the reload has to do. On a match only
    /// the behaviour changed, so the scheduler graph stays as it is and the
    /// reload costs one comparison. On a mismatch the reload goes through
    /// [`CSharpRuntime::reregister_systems`] and rebuilds that graph. The
    /// comparison is worth its cost because a match is the common case: most
    /// reloads edit a system body, not its signature.
    #[cfg(feature = "hot_reload")]
    fn verify_systems_unchanged(&self) -> bool {
        let system_count = (self.exports.system_count)();
        if system_count as usize != self.system_snapshot.len() {
            return false;
        }
        for system_index in 0..system_count {
            let snapshot = &self.system_snapshot[system_index as usize];
            let access_count = (self.exports.access_count)(system_index);
            if access_count as usize != snapshot.accesses.len() {
                return false;
            }
            for access_index in 0..access_count {
                let mut native_access = NativeSystemAccess {
                    component_key: 0,
                    component_key_high: 0,
                    mode: 0,
                    kind: 0,
                };
                if (self.exports.get_access)(system_index, access_index, &mut native_access) == 0 {
                    return false;
                }
                if native_access != snapshot.accesses[access_index as usize] {
                    return false;
                }
            }
            if ((self.exports.system_uses_commands)(system_index) != 0) != snapshot.uses_commands {
                return false;
            }
            // The schedule belongs to the signature: a changed order, phase or
            // edit-mode gate changes where the system runs, so the reload has
            // to re-register it instead of keeping a schedule the source no
            // longer describes.
            if (self.exports.system_order)(system_index) != snapshot.order
                || run_phase_from_export((self.exports.system_run_phase)(system_index))
                    != Some(snapshot.phase)
                || ((self.exports.system_run_in_edit_mode)(system_index) != 0)
                    != snapshot.run_in_edit_mode
            {
                return false;
            }
            // Resolved the same way registration resolves it, fallback
            // included, so an assembly that exposes no name export compares
            // equal instead of re-registering on every single poll.
            if resolved_system_name(
                self.exports.system_name_length,
                self.exports.copy_system_name,
                system_index,
            ) != *snapshot.name
            {
                return false;
            }
        }
        true
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Reports whether a managed-reported poll status is one this host understands.
///
/// The four codes are the whole vocabulary the loader reports. The check is a
/// function of its own so a test can pin that set without a live runtime.
#[cfg(feature = "hot_reload")]
pub(super) fn poll_status_is_known(status: u8) -> bool {
    matches!(
        status,
        POLL_NO_CHANGE | POLL_RELOADED | POLL_REJECTED | POLL_MANIFEST_PENDING
    )
}

/// Maps a managed system's exported phase number to the engine's phase.
///
/// The managed side sends the enum's value: `0` pre-project, `1` project and
/// `2` post-project. `None` is an unknown value, which the caller refuses by
/// number rather than guessing, because running a system in a phase its
/// declaration never named is a schedule nobody asked for.
pub(super) fn run_phase_from_export(phase: u8) -> Option<RunPhase> {
    match phase {
        0 => Some(RunPhase::PreProject),
        1 => Some(RunPhase::Project),
        2 => Some(RunPhase::PostProject),
        _ => None,
    }
}

/// Records a loader poll answer and answers whether it is a new rejection.
///
/// The poll runs every frame, so a loader that keeps refusing the same
/// assembly is named once; any other answer ends that attempt, and the next
/// rejection is named again. Recording the answer on every path keeps the
/// comparison current instead of comparing against a stale one. The helper is
/// a free function so a test can pin the rule without a live .NET runtime.
#[cfg(feature = "hot_reload")]
pub(super) fn record_poll_answer(last_poll_status: &mut u8, status: u8) -> bool {
    let rejection_is_new = status == POLL_REJECTED && *last_poll_status != POLL_REJECTED;
    *last_poll_status = status;
    rejection_is_new
}

/// Reads the pending version's component manifest and logs why it could not be
/// read.
///
/// `assembly` is the word that names the assembly in each message, and
/// `consequence` is appended to say what the host does next. A caller with
/// nothing to add passes an empty string.
#[cfg(feature = "hot_reload")]
fn read_reported_manifest(
    length: impl FnOnce() -> u32,
    copy: impl FnOnce(*mut u8, u32) -> u8,
    assembly: &str,
    consequence: &str,
) -> Option<Vec<u8>> {
    match fetch_managed_buffer(length, copy, MAX_COMPONENT_MANIFEST_BYTES) {
        Ok(manifest) => Some(manifest),
        Err(ManagedBufferError::LengthOutOfRange { length, .. }) => {
            error!(
                target: telemetry_target::HOT_RELOAD,
                length,
                "the {assembly} assembly reported a component manifest length outside the accepted range{consequence}"
            );
            None
        }
        Err(ManagedBufferError::AllocationFailed { length }) => {
            error!(
                target: telemetry_target::HOT_RELOAD,
                length,
                "could not reserve a buffer for the {assembly} component manifest{consequence}"
            );
            None
        }
        Err(ManagedBufferError::CopyFailed) => {
            error!(
                target: telemetry_target::HOT_RELOAD,
                "could not copy the {assembly} component manifest{consequence}"
            );
            None
        }
    }
}

/// Validates a managed-reported system count before it sizes a host allocation.
///
/// `Vec::with_capacity` aborts the process when the allocation fails, so the
/// count is checked against [`MAX_SYSTEMS_PER_ASSEMBLY`] first and refused as
/// a typed error instead.
///
/// # Errors
///
/// Returns [`CSharpError::SystemCountOutOfRange`] when `count` is above
/// [`MAX_SYSTEMS_PER_ASSEMBLY`].
pub(super) fn checked_system_count(count: u32) -> Result<usize, CSharpError> {
    if count > MAX_SYSTEMS_PER_ASSEMBLY {
        return Err(CSharpError::SystemCountOutOfRange {
            count,
            limit: MAX_SYSTEMS_PER_ASSEMBLY,
        });
    }
    Ok(count as usize)
}

/// Validates a managed-reported access count the same way; see
/// [`checked_system_count`].
///
/// # Errors
///
/// Returns [`CSharpError::SystemCountOutOfRange`] when `count` is above
/// [`MAX_ACCESSES_PER_SYSTEM`].
pub(super) fn checked_access_count(count: u32) -> Result<usize, CSharpError> {
    if count > MAX_ACCESSES_PER_SYSTEM {
        return Err(CSharpError::SystemCountOutOfRange {
            count,
            limit: MAX_ACCESSES_PER_SYSTEM,
        });
    }
    Ok(count as usize)
}

/// Returns the name a system is registered under: the reflected one, or a
/// synthetic fallback when the assembly exposes none.
///
/// Separate from [`managed_system_name`] because two callers must agree on the
/// fallback - registration records what this returns, and the reload compares
/// against it. A fallback applied in only one of them would make every poll of
/// a nameless assembly look like a rename.
fn resolved_system_name(
    name_length: SystemNameLengthFn,
    copy_name: CopySystemNameFn,
    system_index: u32,
) -> String {
    managed_system_name(name_length, copy_name, system_index)
        .unwrap_or_else(|| format!("csharp_system_{system_index}"))
}

/// Copies one managed system's declared name.
///
/// A thin wrapper over the shared two-call protocol that closes over the
/// system index. Every failure comes back as `None`: a name the managed side
/// does not report, one that is too long, one that fails to copy, or bytes
/// that are not valid UTF-8. In each case there is no name to use.
fn managed_system_name(
    name_length: SystemNameLengthFn,
    copy_name: CopySystemNameFn,
    system_index: u32,
) -> Option<String> {
    let buffer = fetch_managed_buffer(
        || name_length(system_index),
        |pointer, length| copy_name(system_index, pointer, length),
        MAX_SYSTEM_NAME_BYTES,
    )
    .ok()?;
    String::from_utf8(buffer).ok()
}

/// Copies the failure message one managed system reported after a failed run.
///
/// Falls back to a fixed message when the managed side reports no text, an
/// oversized message, or a failed copy.
fn managed_system_error_message(
    length: SystemErrorMessageLengthFn,
    copy: CopySystemErrorMessageFn,
    system_index: u32,
) -> String {
    const FALLBACK_ERROR_MESSAGE: &str = "managed system reported failure";
    match fetch_managed_buffer(
        || length(system_index),
        |pointer, message_length| copy(system_index, pointer, message_length),
        MAX_SYSTEM_ERROR_BYTES,
    ) {
        Ok(buffer) => String::from_utf8_lossy(&buffer).into_owned(),
        Err(_) => String::from(FALLBACK_ERROR_MESSAGE),
    }
}

/// Turns declared access modes into the scheduler's metadata.
///
/// # Errors
///
/// Returns [`CSharpError::UnregisteredComponent`] when an access names a
/// component nobody registered, [`CSharpError::UnregisteredResource`] when it
/// names an unregistered managed resource, and
/// [`CSharpError::UnknownAccessMode`] when its mode is neither read nor write.
pub(super) fn derive_system_access(
    accesses: &[NativeSystemAccess],
    bindings: &ComponentBindings,
) -> Result<SystemAccess, CSharpError> {
    let mut system_access = SystemAccess::new();
    for access in accesses {
        // A resource access resolves through its own table. Both key spaces are
        // name hashes, so only the declared kind tells them apart, and
        // recording a resource as a component would let two writers of the
        // same resource run in one batch.
        // A Rust-owned resource (`ResMut<AssetManager>`) is reached by its
        // shared name, whose identity is the key itself. It has no managed
        // binding to look up.
        if access.kind == super::context::ACCESS_KIND_NATIVE_RESOURCE {
            let resource = pill_engine::ResourceId::Shared(
                StableComponentId::from_halves(access.component_key, access.component_key_high).0,
            );
            match access.mode {
                0 => system_access.add_resource_read(resource),
                1 => system_access.add_resource_write(resource),
                mode => return Err(CSharpError::UnknownAccessMode { mode }),
            }
            continue;
        }
        if access.kind == super::resources::RESOURCE_ACCESS_KIND {
            let resource = super::resources::resolve_resource_access(
                access.component_key,
                access.component_key_high,
            )?;
            match access.mode {
                0 => system_access.add_resource_read(resource),
                1 => system_access.add_resource_write(resource),
                mode => return Err(CSharpError::UnknownAccessMode { mode }),
            }
            continue;
        }
        let stable_id =
            StableComponentId::from_halves(access.component_key, access.component_key_high);
        let component = bindings
            .get(&stable_id)
            .map(|binding| binding.component_id())
            .ok_or_else(|| CSharpError::UnregisteredComponent {
                key: format!(
                    "{:016X}{:016X}",
                    access.component_key_high, access.component_key
                ),
            })?;
        match access.mode {
            0 => system_access.add_read(component),
            1 => system_access.add_write(component),
            mode => return Err(CSharpError::UnknownAccessMode { mode }),
        }
    }
    Ok(system_access)
}
