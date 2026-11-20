//! The error type of every host subsystem, and the root that combines them.
//!
//! # Responsibilities
//!
//! - Declare the error each subsystem returns: configuration, build, library
//!   loading, extension loading, the file watcher, and the C# backend.
//! - Combine them into [`HostError`] with transparent `#[from]` wrappers, so
//!   `?` works across a subsystem boundary and the original error plus its
//!   source chain survive.
//!
//! # Design
//!
//! Every subsystem variant carries one `#[message(...)]` definition. The
//! `#[engine_error]` macro turns that definition into the plain `Display`
//! text, a stable `namespace::variant` code, and the [`EngineMessage`]
//! implementation that renders the same definition with semantic roles. No
//! variant names a colour, a glyph or any other way of showing a message.

use super::*;

// =============================================================================
// Subsystem Errors
// =============================================================================

/// Setup and configuration failures for a project and its modules.
///
/// Every one of them except the asset-pack failure is raised before any build
/// work starts, so a bad setup stops the host at once with a precise message
/// instead of a confusing error much later.
#[engine_error(namespace = host::config)]
pub enum ConfigError {
    /// The configured module name is empty.
    #[message("module name must not be empty")]
    EmptyModuleName,

    /// The configured watch directory is empty.
    #[message("watch directory must not be empty")]
    EmptyWatchDirectory,

    /// The configured build command is empty.
    #[message("build command must not be empty")]
    EmptyBuildCommand,

    /// A required configuration environment variable was not set.
    #[message("required environment variable ", value(variable), " is not set")]
    MissingEnvironmentVariable { variable: &'static str },

    /// A configuration environment variable holds a value the host does not
    /// accept.
    #[message(
        "environment variable ",
        value(variable),
        " is set to ",
        value(value),
        "; expected ",
        value(expected)
    )]
    InvalidEnvironmentVariable {
        variable: &'static str,
        value: String,
        expected: &'static str,
    },

    /// The directory the engine workspace lives in could not be determined.
    ///
    /// The host finds it from the build location of its own crate, which is
    /// fixed at compile time, and resolves every stored relative path against
    /// it. The process's working directory is not used: the two agree only
    /// when the host is started from the workspace root, and a launcher that
    /// sets a working directory of its own would resolve `extensions/` to a
    /// directory that does not exist.
    #[message("engine workspace root could not be determined from the host build")]
    EngineWorkspaceRootUndetermined,

    /// The configured project directory does not exist.
    #[message("project directory does not exist: ", name_style(path))]
    ProjectDirectoryMissing { path: String },

    /// The project directory contains no Cargo.toml or .csproj manifest.
    #[message(
        "no project manifest (Cargo.toml or .csproj) found in ",
        name_style(path)
    )]
    ProjectManifestMissing { path: String },

    /// The project manifest could not be read.
    #[message("failed to read project manifest ", name_style(path))]
    ProjectManifestReadFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The project manifest is not valid TOML, so no workspace member can be
    /// generated from it.
    #[message(
        "failed to parse project manifest ",
        name_style(path),
        ": ",
        value(details)
    )]
    ProjectManifestParseFailed { path: String, details: String },

    /// The project manifest does not declare a package name.
    #[message(
        "project manifest ",
        name_style(path),
        " does not declare a package name"
    )]
    ProjectPackageNameMissing { path: String },

    /// The generated host workspace member for a native project could not be
    /// created or written.
    #[message("failed to create generated host project member at ", name_style(path))]
    HostProjectMemberCreationFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The generated workspace member that wraps an extension as a loadable
    /// artifact could not be created or written.
    #[message("failed to create generated host module wrapper at ", name_style(path))]
    HostModuleMemberCreationFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A `path` dependency in the project manifest does not point at a path
    /// that exists, once the host has rewritten it for the generated workspace
    /// member.
    ///
    /// Reported instead of writing the member: Cargo refuses the whole
    /// workspace when one member cannot load, which stops every build,
    /// including the one that would replace that member.
    #[message(
        "project manifest ",
        name_style(manifest_path),
        " declares dependency ",
        name_style(dependency),
        " at a path that does not exist: ",
        name_style(resolved_path)
    )]
    ProjectDependencyPathMissing {
        manifest_path: String,
        dependency: String,
        resolved_path: String,
    },

    /// The project's settings file does not exist.
    #[message("project settings file not found at ", name_style(path))]
    ProjectSettingsFileMissing { path: String },

    /// The project's settings file could not be read.
    #[message("failed to read project settings file ", name_style(path))]
    ProjectSettingsFileReadFailed {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The project's settings file could not be parsed.
    #[message(
        "failed to parse project settings file ",
        name_style(path),
        ": ",
        value(details)
    )]
    ProjectSettingsFileParseFailed { path: String, details: String },

    /// The project's settings file does not declare a `name`.
    #[message(
        "project settings file ",
        name_style(path),
        " must declare a `name` (used for the window title)"
    )]
    MissingProjectName { path: String },

    /// The project's settings file's `build_binary_name` is missing or not
    /// filename-safe.
    #[message(
        "project settings file ",
        name_style(path),
        " must declare a `build_binary_name` with only letters, digits and underscores (no spaces or special characters)"
    )]
    InvalidBuildBinaryName { path: String },

    /// The settings file's `logging:` section names a level, a timestamp
    /// format or a target the engine does not recognize.
    #[message(
        "project settings file ",
        name_style(path),
        " has an invalid `logging:` entry: ",
        value(details)
    )]
    InvalidLoggingSettings { path: String, details: String },

    /// The settings file's `startup_scene:` is not a relative `.scene` path
    /// below the asset directory.
    #[message(
        "project settings file ",
        name_style(path),
        " has an invalid `startup_scene:`: ",
        value(details),
        " (use a relative path to a .scene file below `res`)"
    )]
    InvalidStartupScene { path: String, details: String },

    /// An extension name in the settings file is not a usable crate
    /// directory name.
    #[message(
        "project settings file lists extension ",
        value(name),
        ", which is not a crate directory name (letters, digits, `_`, `-`; must start with a letter)"
    )]
    InvalidExtensionName { name: String },

    /// The settings file lists one extension twice.
    #[message("project settings file lists extension ", value(name), " twice")]
    DuplicateExtensionName { name: String },

    /// The settings file lists an editor crate, which only loads with its
    /// extension in an editor host.
    #[message(
        "extension ",
        value(name),
        " is an editor crate: it loads with its extension in an editor host, never from `modules:`"
    )]
    EditorCrateListed { name: String },

    /// An editor crate is tagged for the `pill_ui` editor frontend, which no
    /// host implements.
    #[message(
        "editor crate ",
        value(name),
        " is tagged `editor_frontend = \"pill_ui\"`, the retired editor; tag it `egui` or drop the tag"
    )]
    RetiredEditorFrontend { name: String },

    /// An editor crate's package name does not end in `_editor`, so nothing
    /// else could tell it apart from a runtime crate.
    #[message(
        "editor crate ",
        value(name),
        " sits in an extension's `modules/editor` folder but its package name does not end in \"_editor\""
    )]
    EditorCrateNameMismatch { name: String },

    /// A runtime crate's package name ends in `_editor`, which marks a crate an
    /// editor host loads and no game build links.
    #[message(
        "runtime crate ",
        value(name),
        " sits in an extension's runtime folder but its package name ends in \"_editor\""
    )]
    RuntimeCrateNameMismatch { name: String },

    /// No extension folder holds the configured extension crate.
    #[message(
        "extension ",
        value(name),
        " is in no extension folder under extensions/"
    )]
    ExtensionDirectoryMissing { name: String },

    /// The asset pack a shipped build mounts is not a valid pack.
    #[message(
        "the embedded asset pack of ",
        name_style(project),
        " is damaged: ",
        value(detail)
    )]
    AssetPackInvalid { project: String, detail: String },
}

/// Project-module build execution failures.
///
/// One variant for each step of running the external command that builds a
/// project module: starting it, waiting for it, its exit status, its deadline
/// and its cancellation. Two more cover the artifact it should have written:
/// one that never appeared, and one that was staged and then changed on disk.
#[engine_error(namespace = host::build)]
pub enum BuildError {
    /// The build command list is empty.
    #[message("build command must not be empty")]
    EmptyCommand,

    /// The build executable could not be spawned.
    #[message("failed to spawn build command for ", name_style(name))]
    SpawnFailed {
        name: String,
        #[source]
        source: std::io::Error,
    },

    /// The build child could not be waited on.
    #[message("failed to wait for the build command of ", name_style(name))]
    WaitFailed {
        name: String,
        #[source]
        source: std::io::Error,
    },

    /// The build command exited with a non-zero status.
    #[message(
        "build command failed for ",
        name_style(name),
        " with status ",
        value(status)
    )]
    CommandFailed {
        name: String,
        status: std::process::ExitStatus,
    },

    /// The build command exceeded its wall-clock deadline.
    #[message(
        "build command for ",
        name_style(name),
        " timed out after ",
        value(seconds),
        " seconds"
    )]
    TimedOut { name: String, seconds: u64 },

    /// Sources changed again while the build was running.
    #[message("build cancelled: sources changed again during compilation")]
    Cancelled,

    /// The build succeeded but no artifact appeared at the configured path.
    #[message("shared library not found at expected path: ", name_style(path))]
    #[diagnostic(help("check the selected backend output directory in ProjectModuleConfig"))]
    OutputMissing { path: String },

    /// The built library could not be staged into its hot-load directory.
    #[message(
        "failed to copy built library ",
        name_style(source_path),
        " to hot-load path ",
        name_style(target_path)
    )]
    HotArtifactCopyFailed {
        source_path: String,
        target_path: String,
        #[source]
        source: std::io::Error,
    },

    /// A staged artifact changed after the build that produced it finished.
    #[message(
        "staged artifact ",
        name_style(path),
        " no longer matches what was built for ",
        name_style(name),
        "; another build or a cancelled compiler is writing to the staging directory"
    )]
    #[diagnostic(help("retry the build; check for a stray rustc or linker process"))]
    StagedArtifactChanged { name: String, path: String },
}

/// Native project-library loading and initialization failures.
///
/// One variant for each step: preparing the per-process temporary directory,
/// copying the artifact there, mapping it, resolving its imports, and calling
/// its `pill_module_init` entry point.
///
/// The loader's own error arrives as a boxed source. `pill_core` is the shared
/// dylib that every posture links, so it may not name `libloading`. The real
/// cause still reaches the reader through `Error::source()` and the error
/// chain.
#[engine_error(namespace = host::library)]
pub enum LibraryError {
    /// The per-process temporary directory could not be created.
    #[message(
        "failed to prepare temporary library directory ",
        name_style(directory)
    )]
    TemporaryDirectory {
        directory: String,
        #[source]
        source: std::io::Error,
    },

    /// The built library could not be copied to its temporary path.
    #[message(
        "failed to copy built library ",
        name_style(source_path),
        " to ",
        name_style(target_path)
    )]
    CopyFailed {
        source_path: String,
        target_path: String,
        #[source]
        source: std::io::Error,
    },

    /// The temporary copy is not a loadable native library.
    #[message("failed to load native project library from ", name_style(path))]
    LoadFailed {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The artifact was linked against a different engine dylib than the host
    /// has mapped, so the loader cannot resolve its imports.
    ///
    /// Cargo puts the features a dependency resolved into the `-C metadata` of
    /// the crate that uses it, and that hash is part of every symbol name
    /// `pill_core` exports. Two builds that resolved different feature sets
    /// therefore produce two copies of the dylib with different symbols, and an
    /// artifact built against one cannot load against the other. Neither loader
    /// names the cause: Windows reports `LoadLibrary` error 127, "The specified
    /// procedure could not be found", and a Unix loader reports
    /// `undefined symbol: <one mangled name>`. Both name neither the dylib nor
    /// the feature that moved.
    ///
    /// Copying the matching dylib beside the artifact does not help on its own.
    /// The loader uses the copy already mapped in the process, whatever
    /// directory a new one sits in, and the host maps the engine dylibs before
    /// it builds anything.
    #[message(
        "native library for ",
        name_style(subject),
        " was built against a different engine dylib than this host runs, so its ",
        "imports cannot resolve. The host maps ",
        name_style(host_engine),
        " and the artifact links ",
        name_style(module_engine),
        ". The two builds resolved different dependency graphs, which changes ",
        "every exported symbol name. One build contains a crate the other does ",
        "not, and that crate enables a feature on a dependency of the engine: a ",
        "headless host against a project that links the renderer, or a windowed ",
        "host whose module build has no renderer. Either run the host with the ",
        "feature set the project implies (`--features rendering` for the ",
        "renderer), or build the host and the project in one cargo invocation ",
        "so both resolve the same graph"
    )]
    EngineDylibMismatch {
        subject: String,
        host_engine: String,
        module_engine: String,
    },

    /// A named import the module needs is not exported by the copy of the
    /// imported dylib the loader would use.
    ///
    /// A loader names neither the dylib nor the symbol: Windows reports a
    /// missing named import as `LoadLibrary` error 127, "The specified procedure
    /// could not be found", and a Unix loader reports
    /// `undefined symbol: <name>` for one symbol only. Comparing the module's
    /// import table with the export table of each candidate copy says which
    /// symbol is missing and which copy was looked at.
    #[message(
        "native library for ",
        name_style(subject),
        " imports ",
        name_style(symbol),
        " from ",
        name_style(dll),
        ", which does not export it; the copy the loader would use is ",
        name_style(path)
    )]
    MissingImport {
        subject: String,
        dll: String,
        symbol: String,
        path: String,
    },

    /// The library does not export a required entry point.
    ///
    /// The error names the artifact as well as the symbol. A project and an
    /// extension export the same entry points, so the symbol alone would not
    /// say which artifact is missing one.
    #[message(
        "native library for ",
        name_style(subject),
        " is missing required export ",
        name_style(symbol)
    )]
    MissingExport {
        subject: String,
        symbol: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// `pill_module_init` reported a failed generation.
    #[message("project module initialization failed with status ", value(status))]
    InitializationFailed { status: u32 },
}

/// Extension loading and compatibility failures.
///
/// Distinct from [`LibraryError`], which covers the mechanics of mapping a
/// library. These report a broken contract between the host and a module.
#[engine_error(namespace = host::module)]
pub enum ModuleError {
    /// The module was built against a different module ABI revision.
    #[message(
        "extension ",
        name_style(module),
        " reports ABI version ",
        value(module_version),
        " but the host requires ",
        value(host_version)
    )]
    #[diagnostic(help("rebuild the module against the current engine sources"))]
    AbiVersionMismatch {
        module: String,
        module_version: u32,
        host_version: u32,
    },

    /// The module does not export an ABI revision at all.
    #[message(
        "extension ",
        name_style(module),
        " does not export pill_module_abi_version"
    )]
    #[diagnostic(help("an extension must export pill_module_abi_version and pill_module_init"))]
    AbiVersionMissing { module: String },

    /// The module's registration entry point reported a failed generation.
    #[message(
        "extension ",
        name_style(module),
        " initialization failed with status ",
        value(status)
    )]
    InitializationFailed { module: String, status: u32 },

    /// A module registered components or resources, although a failed load of
    /// it clears only its systems.
    ///
    /// When a failed load keeps the world and removes just the module's
    /// systems, that is sound only if the module owns no data: the components
    /// and resources it uses were registered by someone else. A module that
    /// registers its own is refused instead of trusted.
    #[message(
        "extension ",
        name_style(module),
        " registered ",
        value(components),
        " component type(s) and ",
        value(resources),
        " resource type(s), but may register no data"
    )]
    #[diagnostic(help(
        "register the data in pill_renderer_api, which the host registers before any module loads"
    ))]
    RegisteredForbiddenData {
        module: String,
        components: usize,
        resources: usize,
    },
}

/// Source-watching startup failures.
///
/// Distinct from build failures: the watcher has to run before a source change
/// can trigger a build.
///
/// The watcher's own error arrives as a boxed source. `pill_core` is the shared
/// dylib that every posture links, so it may not name `notify`. The real cause
/// still reaches the reader through `Error::source()` and the error chain.
#[engine_error(namespace = host::watcher)]
pub enum WatcherError {
    /// The configured watch directory does not exist.
    #[message("watch directory does not exist: ", name_style(path))]
    WatchDirectoryMissing { path: String },

    /// The platform file watcher could not be created.
    #[message("failed to create file watcher")]
    CreationFailed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The watch path could not be registered.
    #[message("failed to register watch path ", name_style(path))]
    RegistrationFailed {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Managed (C#) backend startup and registration failures.
///
/// One variant for each step: finding and loading hostfxr, resolving the
/// runtime delegates, exchanging the component manifest, generating the host's
/// C# mirror, and starting the managed systems.
///
/// The loader's own error arrives as a boxed source. `pill_core` is the shared
/// dylib that every posture links, so it may not name `libloading`. The real
/// cause still reaches the reader through `Error::source()` and the error
/// chain.
#[engine_error(namespace = host::csharp)]
pub enum CSharpError {
    /// No hostfxr installation was found in any configured dotnet root.
    #[message("no .NET hostfxr installation found; searched the configured dotnet roots")]
    HostfxrNotFound,

    /// The `ECS_DOTNET_HOSTFXR` override does not point at a file.
    #[message("ECS_DOTNET_HOSTFXR does not point at a file: ", name_style(path))]
    HostfxrOverrideNotAFile { path: String },

    /// The selected hostfxr library could not be loaded.
    #[message("failed to load hostfxr from ", name_style(path))]
    HostfxrLoadFailed {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The NativeAOT library could not be found on disk.
    #[message("no NativeAOT library at ", name_style(path))]
    AotLibraryNotFound { path: String },

    /// The NativeAOT library could not be loaded.
    #[message("failed to load NativeAOT library from ", name_style(path))]
    AotLibraryLoadFailed {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A required export is missing from the NativeAOT library.
    #[message("failed to resolve NativeAOT export ", name_style(symbol))]
    AotExportMissing {
        symbol: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A required hostfxr export is missing.
    #[message("failed to resolve hostfxr export ", name_style(symbol))]
    HostfxrExportMissing {
        symbol: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Runtime initialization failed with a negative hostfxr status.
    #[message("hostfxr_initialize_for_runtime_config failed with ", value(status))]
    RuntimeInitializationFailed { status: String },

    /// The runtime delegate request failed with a negative hostfxr status.
    #[message("hostfxr_get_runtime_delegate failed with ", value(status))]
    RuntimeDelegateFailed { status: String },

    /// The managed export type is not a pointer-sized function pointer.
    #[message("managed export type is not pointer-sized")]
    ManagedExportNotPointerSized,

    /// A managed entry point could not be resolved or returned no pointer.
    #[message(
        "load_assembly_and_get_function_pointer failed for ",
        name_style(method),
        " with ",
        value(status)
    )]
    ManagedEntryPointFailed { method: String, status: String },

    /// The managed loader rejected host initialization.
    #[message("csharp_runtime initialization failed")]
    RuntimeInitFailed,

    /// The host-side C# mirror codegen step failed.
    #[message("C# component mirror generation failed: ", value(message))]
    CodegenFailed { message: String },

    /// The managed runtime was built for a different ABI contract version.
    #[message(
        "csharp_runtime interop version mismatch: host expects ",
        value(expected),
        " but the runtime reports ",
        value(actual)
    )]
    #[diagnostic(help("rebuild csharp_runtime and the host together"))]
    InteropVersionMismatch { expected: u32, actual: u32 },

    /// The managed-reported manifest length lies outside the supported range.
    #[message(
        "component manifest length ",
        value(length),
        " is outside the supported range (1..=",
        value(limit),
        ")"
    )]
    ManifestLengthOutOfRange { length: u32, limit: u32 },

    /// A managed-reported system or access count exceeds what the host
    /// reflects.
    ///
    /// The count decides how much memory the host reserves before it reads any
    /// item, so it is checked against a fixed limit first. Without that check a
    /// buggy assembly could ask for an allocation that aborts the process
    /// instead of failing startup.
    #[message(
        "managed-reported count ",
        value(count),
        " exceeds the supported limit of ",
        value(limit)
    )]
    SystemCountOutOfRange { count: u32, limit: u32 },

    /// The reflected system snapshot could not be allocated.
    #[message("out of memory allocating the reflected system snapshot")]
    SystemSnapshotAllocationFailed,

    /// The managed loader reported a reload status the host does not know.
    ///
    /// The poll answers `0` (nothing happened), `1` (swapped), `2` (rejected)
    /// or `3` (a rebuilt assembly is waiting for its manifest to be applied).
    /// Anything else means the ABI drifted or the loader failed. Treating it
    /// as "nothing happened" would keep the previous assembly running without
    /// saying so anywhere.
    #[message("the managed loader reported unknown reload status ", value(status))]
    UnknownPollStatus { status: u8 },

    /// A managed system's exported frame phase is not one the host knows.
    ///
    /// The export names the phase by number (`0` pre-project, `1` project,
    /// `2` post-project), so an unknown value means the ABI drifted or the
    /// managed side has a bug. Guessing `project` would run the system in a
    /// phase its declaration never named.
    #[message(
        "managed system ",
        value(system),
        " declares unknown run phase ",
        value(phase)
    )]
    UnknownRunPhase { system: u32, phase: u8 },

    /// The manifest buffer could not be allocated.
    #[message("out of memory allocating the component manifest buffer")]
    ManifestAllocationFailed,

    /// The manifest copy from the managed runtime failed.
    #[message("failed to copy the component manifest from the managed runtime")]
    ManifestCopyFailed,

    /// The managed component manifest failed host-side JSON parsing.
    #[message("failed to parse the component manifest JSON")]
    ManifestParseFailed {
        #[source]
        source: serde_json::Error,
    },

    /// The managed component manifest failed host-side validation.
    #[message("invalid component manifest: ", value(message))]
    ManifestInvalid { message: String },

    /// A reflected managed startup method failed.
    #[message("startup method ", value(index), " failed")]
    StartupFailed { index: u32 },

    /// Queued startup commands failed to apply.
    #[message("startup commands failed: ", value(details))]
    StartupCommandsFailed { details: String },

    /// The managed assembly contains no schedulable systems.
    #[message("C# assembly ", name_style(subject), " declares no [System] methods")]
    NoSystems {
        /// The assembly the host inspected, so the message names the artifact
        /// that failed instead of a fixed example project.
        subject: String,
    },

    /// The managed runtime refused to copy one system access declaration.
    #[message(
        "failed to get access ",
        value(access),
        " for C# system ",
        value(system)
    )]
    SystemAccessFailed { system: u32, access: u32 },

    /// A reflected access mode is neither read nor write.
    #[message("unknown C# access mode ", value(mode))]
    UnknownAccessMode { mode: u8 },

    /// A reflected access references a component the host never registered.
    #[message("C# system references unregistered component key ", value(key))]
    UnregisteredComponent { key: String },

    /// A reflected access references a resource the host never registered.
    ///
    /// In almost every case the resource struct is missing `[EcsResource]`, so
    /// the manifest never told the host about it, even though a system asks
    /// for it through `Res<T>` or `ResMut<T>`.
    #[message("C# system references unregistered resource key ", value(key))]
    UnregisteredResource { key: String },

    /// A managed string passed to hostfxr contains an interior NUL.
    #[message(".NET host string contains an interior NUL")]
    InteriorNul,
}

impl From<String> for CSharpError {
    /// Turns a free-form manifest validation message into the typed
    /// `ManifestInvalid` variant.
    fn from(message: String) -> Self {
        Self::ManifestInvalid { message }
    }
}

impl From<serde_json::Error> for CSharpError {
    /// Keeps the real JSON parse error, instead of turning it into text.
    fn from(source: serde_json::Error) -> Self {
        Self::ManifestParseFailed { source }
    }
}

// =============================================================================
// Host Error Composition
// =============================================================================

/// Transparent composition of every host subsystem error.
///
/// A subsystem function returns its own narrow error. [`HostError`] exists
/// only at the boundary where `?` moves from one subsystem to another.
#[engine_error(namespace = host)]
pub enum HostError {
    /// Configuration validation failed.
    #[transparent]
    Config(#[from] ConfigError),

    /// The project-module build failed.
    #[transparent]
    Build(#[from] BuildError),

    /// Native library loading or initialization failed.
    #[transparent]
    Library(#[from] LibraryError),

    /// The source watcher could not start.
    #[transparent]
    Watcher(#[from] WatcherError),

    /// An extension failed to load or is incompatible.
    #[transparent]
    Module(#[from] ModuleError),

    /// The managed backend failed to start.
    #[transparent]
    CSharp(#[from] CSharpError),
}
