//! Project and extension configuration shared by every host frontend.
//!
//! # Responsibilities
//!
//! - Describes how the project module and each extension are built, watched
//!   and loaded.
//! - Reads the configuration from `PROJECT_PATH` and the project's own
//!   `project_settings.yaml`: backend, extensions, renderer, assets, logging
//!   and startup scene.
//! - Finds the renderer's two crates and an editor host's editor crates, and
//!   writes the generated workspace members that build the project and each
//!   extension crate.
//! - Reads the editor-only `editor_settings.yaml`.
//! - Copies the host's cargo profile, target triple and dioxus-CLI wrapper
//!   onto every build the host starts.
//!
//! # Design
//!
//! [`ProjectModuleConfig`] holds the project module's values, and [`HostConfig`]
//! adds the extension list and the switches that select the editor posture.
//! The backend (native Rust or managed C#) comes from the manifest in the
//! project directory (`Cargo.toml` or `*.csproj`). Every path, command and
//! assembly name is derived from that manifest, so no project identity is
//! compiled into the host binary. A missing or unreadable project fails at
//! once with a [`ConfigError`] that names the offending path.

// Standard library
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// External crates
use pill_core::error::ConfigError;
use pill_csharp_bridge::CSharpModuleConfig;
use toml_edit::{value, DocumentMut, Item, Table, Value};

// =============================================================================
// Constants
// =============================================================================

/// The variable that selects the project to run: the project directory,
/// relative to the workspace root (for example `../examples/local/project_rs`).
const PROJECT_PATH_ENVIRONMENT_VARIABLE: &str = "PROJECT_PATH";

/// Opt-in variable that adds cargo's `--timings` report to every host-driven
/// build.
///
/// Off by default. The report makes cargo write an HTML timing file and adds
/// work to every build, and only automated build measurements read it, so a
/// developer does not usually want it. Set the variable to any value when
/// per-crate build times are needed; the measurements then read the report
/// this process's builds produce.
const CARGO_TIMINGS_ENVIRONMENT_VARIABLE: &str = "PILL_CARGO_TIMINGS";

/// Assembly name of the bundled `csharp_runtime` collectible loader.
const CSHARP_RUNTIME_ASSEMBLY_NAME: &str = "csharp_runtime";

/// Workspace-root-relative directory of the C# runtime project. Its build
/// output appears under `bin/<configuration>/<framework>` inside it.
const CSHARP_RUNTIME_PROJECT_SUBDIRECTORY: &str = "csharp/pill_csharp_runtime";

/// Target framework the managed project and the runtime are built for.
const CSHARP_TARGET_FRAMEWORK: &str = "net8.0";

/// Opt-in variable selecting the MSBuild configuration of the managed project
/// build: `Release` (the default) or `Debug`.
///
/// `Debug` exists for the managed debugger. A `Release` build is optimized by
/// Roslyn, so local variables are gone from the IL and no JIT setting brings
/// them back. The runtime is a `ProjectReference` of every gameplay project,
/// so it is built in the same configuration and its output path follows.
const CSHARP_CONFIGURATION_ENVIRONMENT_VARIABLE: &str = "PILL_CSHARP_CONFIGURATION";

/// Managed build configurations [`CSHARP_CONFIGURATION_ENVIRONMENT_VARIABLE`]
/// accepts, spelled as MSBuild names its output directories.
const CSHARP_CONFIGURATIONS: [&str; 2] = ["Release", "Debug"];

/// Workspace-relative manifest of the in-process C# compiler.
///
/// The compiler is deliberately not referenced by any managed project. Roslyn
/// is large and does not work well with NativeAOT, so the compiler is built
/// and loaded only when the host enables hot reload, and never reaches a
/// shipping bundle.
pub(crate) const CSHARP_COMPILER_MANIFEST: &str =
    "csharp/pill_csharp_compiler/pill_csharp_compiler.csproj";

/// Assembly name of the in-process C# compiler.
pub(crate) const CSHARP_COMPILER_ASSEMBLY_NAME: &str = "pill_csharp_compiler";

/// Workspace-relative output directory of the in-process C# compiler.
pub(crate) const CSHARP_COMPILER_OUTPUT_SUBDIRECTORY: &str =
    "csharp/pill_csharp_compiler/bin/Release/net8.0";

/// Workspace-relative MSBuild file that captures a project's compiler command
/// line, injected into a managed project's build.
pub(crate) const CSHARP_COMPILER_ARGUMENTS_TARGETS: &str =
    "csharp/pill_csharp_compiler/build/PillCaptureCompilerArgs.targets";

/// File the capture target writes inside a managed project's `obj` directory.
///
/// This name is the whole contract between the host and the targets file.
/// MSBuild derives the directory from the project it builds, and the host
/// derives the same path from the project's output directory.
pub(crate) const CSHARP_COMPILER_ARGUMENTS_FILE: &str = "pill_compiler_args.rsp";

/// Workspace-relative directory holding one folder per extension.
///
/// Each folder, `extensions/<extension>/`, holds the extension's crates under
/// its `modules/` folder (`modules/runtime` and `modules/editor`), beside the
/// runnable examples under `examples/` and the host-written wrappers under
/// `generated/`. The workspace manifest globs `extensions/*/modules/*`, so a
/// module is found by existing rather than by being listed anywhere.
const EXTENSION_DIRECTORY: &str = "extensions";

/// Folder, inside an extension's folder or the editor's folder, holding the
/// host-generated wrapper crates of the crates beside it.
const GENERATED_DIRECTORY: &str = "generated";

/// Workspace-relative directory holding the editor's own crates: the editor
/// executables, the shared editor core and the editor modules.
///
/// An editor module is a crate directly under it whose manifest tags the editor
/// frontend it belongs to (`editor_frontend` under `[package.metadata.pill]`);
/// the executables and the core carry no tag. Every editor session of that
/// frontend loads it after the extensions' editor crates, and no game build
/// compiles it. Its generated wrapper is written into this directory's
/// [`GENERATED_DIRECTORY`], which the `editor/generated/*` workspace glob finds.
const EDITOR_DIRECTORY: &str = "editor";

/// Folder, inside an extension's folder, holding its crates: `runtime` holds
/// the extension itself and `editor` its editor part.
const EXTENSION_MODULES_DIRECTORY: &str = "modules";

/// Folder, inside an extension's `modules/` folder, holding the extension
/// crate. The master renderer groups two crates under it instead, one per
/// child directory (`data` and `gpu`).
const RUNTIME_CRATE_DIRECTORY: &str = "runtime";

/// Folder, inside an extension's `modules/` folder, holding its editor crate.
const EDITOR_CRATE_DIRECTORY: &str = "editor";

/// Folder under `extensions/` holding the older renderer crates. No build
/// uses them, so the extension index never reads it.
const OLD_RENDERERS_DIRECTORY: &str = "rendering";

/// Suffix every editor crate's package name carries. The host pairs an
/// extension's editor part by location (`modules/editor`) and refuses a crate
/// there whose name lacks the suffix, so the two markers cannot disagree. The
/// suffix also keeps `modules:` and a shipping bundle from linking an editor
/// crate by accident.
pub(crate) const EDITOR_CRATE_SUFFIX: &str = "_editor";

/// The editor frontend name a configuration uses when its caller names none.
///
/// `pill_editor` names `egui` explicitly through
/// [`HostConfig::from_environment_for_editor_frontend`]. A crate that tags
/// itself for another frontend loads only under that frontend.
pub(crate) const DEFAULT_EDITOR_FRONTEND: &str = "egui";

/// The editor frontend tag no host accepts. A crate that declares it is
/// refused during resolution, because no host would ever load its editor
/// part.
pub(crate) const RETIRED_EDITOR_FRONTEND: &str = "pill_ui";

/// Workspace-relative directory holding the generated project member.
///
/// Projects live outside the workspace, so the member that builds one as part
/// of it is generated here. The `projects_generated/*` workspace glob finds
/// it without any entry in the workspace manifest.
const PROJECT_MEMBER_DIRECTORY: &str = "projects_generated";

/// Name prefix of the generated workspace member that builds a native project.
///
/// The member lives under [`PROJECT_MEMBER_DIRECTORY`], and its package is
/// `<prefix><project package name>`.
pub(crate) const HOST_PROJECT_MEMBER_PREFIX: &str = "host_project_";

/// Name prefix of the generated workspace member that builds one extension's
/// loadable artifact.
///
/// The host writes one wrapper per loaded extension crate, into the
/// [`GENERATED_DIRECTORY`] of the extension folder that holds the crate, so
/// the `extensions/*/generated/*` glob finds it. The wrapper is the `cdylib`;
/// the extension stays a plain library inside it. That is what lets the module
/// build and every project build compile one identical copy of the extension.
pub(crate) const HOST_MODULE_MEMBER_PREFIX: &str = "host_module_";

/// Cargo profile directory this host was compiled into, recorded by `build.rs`.
///
/// `debug` for an ordinary `cargo build`, `release` for `--release`, and the
/// profile's own name for any custom profile (`release-fast`,
/// `release-with-debug`).
const HOST_PROFILE_DIRECTORY: &str = env!("PILL_HOST_PROFILE_DIRECTORY");

/// Target triple the host was compiled for, recorded by `build.rs`.
///
/// Empty when the host was built natively (an ordinary `cargo build` or
/// `cargo run` without `--target`), which is every host except one launched by
/// the dioxus CLI (`dx serve`). dx always passes `--target <host-triple>` to
/// cargo, and cargo folds the target into each crate's metadata hash. A module
/// built natively against a `--target` host would then get different symbol
/// names than the engine dylibs the host loaded export, and every module load
/// would fail with "The specified procedure could not be found" (os error
/// 127). Host-spawned module builds pass the same triple so both sides agree.
const HOST_TARGET_TRIPLE: &str = env!("PILL_HOST_TARGET_TRIPLE");

/// Environment variable the dioxus CLI sets on every app it launches.
///
/// Its presence tells the host it is running as an app built and served by
/// `dx`, which matters because dx builds by wrapping every workspace-member
/// rustc invocation through its own binary (see [`dioxus_executable_path`]).
const DIOXUS_CLI_ENABLED_ENVIRONMENT: &str = "DIOXUS_CLI_ENABLED";

/// Cargo's workspace-wrapper environment variable.
///
/// Cargo wraps only workspace member crates through this executable and folds
/// the executable's path into their `-C metadata` hash, which is how dx's
/// build diverges from a plain cargo build. The name is kept in this one
/// constant so it and the dx-specific `DX_RUSTC` sibling stay together.
const RUSTC_WORKSPACE_WRAPPER_ENVIRONMENT: &str = "RUSTC_WORKSPACE_WRAPPER";

/// Environment variable that makes dx's own binary act as a rustc wrapper.
///
/// Without it, dx treats a `dx <rustc> <args>` invocation as a normal CLI call
/// and fails. With it, dx records the invocation in the directory the variable
/// names and then runs the real `rustc`. A host-spawned build that copies dx's
/// workspace wrapper must set both variables together.
const DX_RUSTC_WRAPPER_ENVIRONMENT: &str = "DX_RUSTC";

/// Private build tree for every cargo build the host spawns.
///
/// The running host binary maps the engine dylibs out of the regular target
/// directory ([`host_target_directory`]). A GUI frontend like the editor adds
/// features to crates those dylibs depend on, so the engine the host runs can
/// need different artifacts than a module build. A module build that rewrote
/// the shared directory would make cargo delete a DLL the host has mapped, and
/// Windows refuses to replace a loaded image. Host-spawned builds write here
/// instead; their engine artifacts are copied into the hot-load directory when
/// the two sets differ.
///
/// With a `--target` host ([`HOST_TARGET_TRIPLE`]) the builds pass the same
/// triple, so the subtree appears under `target/hot/build/<triple>/<profile>`.
pub(crate) const MODULE_BUILD_TARGET_DIRECTORY: &str = "target/hot/build";

/// Project configuration file name, resolved in the project root. A project
/// that ships one declares its own extension list; the project itself is
/// selected by `PROJECT_PATH` alone.
const PROJECT_SETTINGS_FILE: &str = "project_settings.yaml";

/// The editor-only settings file, resolved in the project root beside
/// `project_settings.yaml`. No game build reads it.
pub const EDITOR_SETTINGS_FILE: &str = "editor_settings.yaml";

/// A project's source asset directory, relative to the project root.
const PROJECT_ASSET_DIRECTORY: &str = "res";

/// The editor theme a project's editor layer uses when `editor_settings.yaml`
/// names none and the file is there.
const DEFAULT_EDITOR_THEME: &str = "editor/editor.theme";

/// The renderer a project draws with when its settings name none.
const DEFAULT_RENDERER: &str = "pill_master_renderer";

/// The `renderer:` value that selects no renderer at all: neither its data
/// crate nor its GPU module loads.
const NO_RENDERER: &str = "none";

/// Suffix of a renderer's data crate: renderer `X`'s data is `X_data`.
pub(crate) const RENDERER_DATA_SUFFIX: &str = "_data";

/// Directory at a module crate's root holding the resources it ships, its
/// shader sources among them (under `shaders/`), relative to the crate.
///
/// The renderer data crate's build script cooks this tree and the shader
/// watcher re-cooks the same one, so a stage's path relative to it is the
/// same in both, and it is the stage's asset path too.
#[cfg(feature = "rendering")]
pub(crate) const MODULE_RESOURCES_DIRECTORY: &str = "res";

/// Dependency tables whose entries link code into the build.
///
/// The workspace-wide `[workspace.dependencies]` table is deliberately absent:
/// it is shared infrastructure rather than a dependency of this project.
const LINKING_DEPENDENCY_TABLES: [&str; 3] =
    ["dependencies", "dev-dependencies", "build-dependencies"];

// =============================================================================
// Types
// =============================================================================

/// Backend-specific output information for a hot-reloadable project module.
///
/// The variant determines how the host locates and loads the built output:
/// a native shared library or a managed assembly hosted by `csharp_runtime`.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ProjectModuleBackend {
    /// A native shared library exporting `pill_module_init`.
    NativeLibrary {
        /// Library name without the platform prefix or suffix.
        library_name: String,
        /// Output subdirectory relative to the workspace root.
        output_subdirectory: String,
    },
    /// A managed project assembly loaded through the stable `csharp_runtime` host.
    CSharp(CSharpModuleConfig),
}

impl ProjectModuleBackend {
    /// The backend's kind as the startup lines spell it: `native` or `csharp`.
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::NativeLibrary { .. } => "native",
            Self::CSharp(_) => "csharp",
        }
    }
}

/// Configuration for a hot-reloadable project module.
///
/// Describes how to build the module, where to find its output, and which
/// source directories to watch. [`Self::from_environment`] builds it from the
/// `PROJECT_PATH` variable plus the project's manifest, and it owns every
/// value, so no project identity is compiled into the host binary.
///
/// `#[non_exhaustive]` lets new fields be added without breaking frontends, so
/// prefer [`Self::from_environment`] over struct literals.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProjectModuleConfig {
    /// Human-readable name used in log messages.
    pub name: String,

    /// Workspace-relative path to the project's manifest file.
    ///
    /// The host re-reads it at runtime to answer dependency questions, such as
    /// whether a reloaded extension is linked by this project.
    pub manifest_path: Option<String>,

    /// Directory to watch for source changes, relative to the workspace root.
    pub watch_directory: String,

    /// Build command: the first element is the program and the rest are
    /// arguments.
    pub build_command: Vec<String>,

    /// Environment variables applied to the build process.
    ///
    /// Used to override values the build would otherwise inherit from the
    /// host's environment or from the workspace Cargo configuration.
    pub build_environment: Vec<(String, String)>,

    /// How the built module is loaded and executed.
    pub backend: ProjectModuleBackend,
}

/// Build, watch and load description for one extension.
///
/// Extensions are workspace members of the engine workspace. Each is built
/// through a generated `host_module_<name>` wrapper crate
/// ([`materialize_host_module_wrapper`]) whose `cdylib` carries the
/// loadable-artifact entry points, with the extension itself compiled inside
/// it as a plain library. Wrapper and extension share the workspace's
/// lockfile and Cargo configuration. That shared setup is what lets them link
/// the engine dynamically and share one copy of its statics with the host.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ExtensionConfig {
    /// The crate's package name, also the log field and temporary-copy prefix.
    pub name: String,

    /// The extension folder under `extensions/` that holds the crate, or
    /// `editor` for one of the editor's own modules, which no runtime crate
    /// shares.
    pub folder: String,

    /// The crate's directory, relative to the workspace root, with `/`
    /// separators.
    pub crate_directory: String,

    /// Whether this is an editor crate: a folder's `modules/editor` crate,
    /// which an editor host loads after the folder's runtime crates and reloads
    /// whenever one of them reloads, or a crate only an editor host loads at
    /// all (an editor module, a project's editor crate).
    pub is_editor_crate: bool,

    /// Library name of the artifact the host loads, which is the extension's
    /// own name. The wrapper's DLL is staged under it, and the host names the
    /// module by it in logs, analytics and the reload graveyard.
    pub library_name: String,

    /// Library name of the generated wrapper crate the build command selects;
    /// the artifact it produces is staged under [`Self::library_name`].
    pub wrapper_library_name: String,

    /// Directory of that generated wrapper crate, relative to the workspace
    /// root, with `/` separators: inside the `generated/` folder of the
    /// extension folder or the editor folder that holds the crate, or under
    /// `projects_generated/` for a project's editor crate.
    pub wrapper_directory: String,

    /// Directory to watch for source changes, relative to the workspace root.
    pub watch_directory: String,

    /// Build command: the first element is the program and the rest are
    /// arguments.
    pub build_command: Vec<String>,

    /// Directory of the hot-load artifact, relative to the workspace root.
    ///
    /// Cargo writes the freshly built library into the shared output slot
    /// first. The host copies it into this private directory and loads it from
    /// there, so a project build that overwrites the shared slot cannot
    /// corrupt the loaded module.
    pub output_subdirectory: String,
}

impl ExtensionConfig {
    /// Derives the configuration of an extension crate from where it lives.
    ///
    /// The location determines everything: sources in the crate directory's
    /// `src`, the generated `host_module_<name>` wrapper carrying the loadable
    /// artifact, and the private `target/hot` directory it is copied into.
    /// Both are workspace members found by glob, so a new module needs no
    /// manifest edit.
    ///
    /// Building inside the workspace is required, not merely convenient: it
    /// makes the module resolve the same dependency graph as the host. That
    /// keeps component type identities and the mangled symbol names of the
    /// shared `pill_core` library in agreement.
    pub(crate) fn workspace_member(location: &ExtensionCrateLocation) -> Self {
        let name = location.package_name.as_str();
        let wrapper_library_name = format!("{HOST_MODULE_MEMBER_PREFIX}{name}");
        // The wrapper's manifest enables the extension's own features on its
        // dependency edge (`materialize_host_module_wrapper`). The command
        // line only mirrors the hot-patch feature, which [`host_cargo_command`]
        // applies for every host-spawned build.
        let build_command = host_cargo_command(std::slice::from_ref(&wrapper_library_name));
        let crate_directory = location.workspace_relative_directory();
        let wrapper_directory = format!(
            "{EXTENSION_DIRECTORY}/{}/{GENERATED_DIRECTORY}/{wrapper_library_name}",
            location.folder
        );

        Self {
            name: name.to_string(),
            folder: location.folder.clone(),
            watch_directory: format!("{crate_directory}/src"),
            crate_directory,
            is_editor_crate: location.is_editor_crate,
            library_name: name.to_string(),
            wrapper_library_name,
            wrapper_directory,
            build_command,
            output_subdirectory: "target/hot".to_string(),
        }
    }

    /// The configuration of one of the editor's own modules: the crate in
    /// `editor/<directory_name>` whose manifest declares `package_name`.
    ///
    /// Its generated wrapper is the `host_module_<package>` member in
    /// `editor/generated/`. `folder` is `editor`, which no extension folder's
    /// runtime crate carries, so no runtime crate's reload cascades into the
    /// module.
    pub(crate) fn editor_module(directory_name: &str, package_name: &str) -> Self {
        let wrapper_library_name = format!("{HOST_MODULE_MEMBER_PREFIX}{package_name}");
        let build_command = host_cargo_command(std::slice::from_ref(&wrapper_library_name));
        let crate_directory = format!("{EDITOR_DIRECTORY}/{directory_name}");
        Self {
            name: package_name.to_string(),
            folder: EDITOR_DIRECTORY.to_string(),
            watch_directory: format!("{crate_directory}/src"),
            crate_directory,
            is_editor_crate: true,
            library_name: package_name.to_string(),
            wrapper_directory: format!(
                "{EDITOR_DIRECTORY}/{GENERATED_DIRECTORY}/{wrapper_library_name}"
            ),
            wrapper_library_name,
            build_command,
            output_subdirectory: "target/hot".to_string(),
        }
    }

    /// [`Self::workspace_member`] for the crate `name`, in whichever extension
    /// folder under `workspace_root` holds it (the folder of its own name when
    /// none does yet, so a missing crate fails at its watch path).
    pub fn located_workspace_member(workspace_root: &Path, name: &str) -> Self {
        let location = find_extension_crate(&workspace_root.join(EXTENSION_DIRECTORY), name)
            .unwrap_or_else(|| ExtensionCrateLocation::conventional(name));
        Self::workspace_member(&location)
    }

    /// The configuration of a project's own editor crate.
    ///
    /// The crate lives at `<project_path>/<package>_editor/`, the project
    /// directory's analogue of an extension's `modules/editor` folder. Its
    /// generated wrapper is the `host_module_<package>_editor` member under
    /// `projects_generated/`, so the workspace glob that already finds the
    /// generated project members finds this one too.
    ///
    /// `folder` is the project's package name, which is what makes
    /// [`Self::is_editor_crate`] true for it and keeps the log lines naming
    /// the project rather than a path.
    pub fn project_editor(project_path: &str, package_name: &str) -> Self {
        let name = format!("{package_name}{EDITOR_CRATE_SUFFIX}");
        let wrapper_library_name = format!("{HOST_MODULE_MEMBER_PREFIX}{name}");
        let build_command = host_cargo_command(std::slice::from_ref(&wrapper_library_name));
        let crate_directory = format!("{project_path}/{name}");
        Self {
            is_editor_crate: true,
            library_name: name.clone(),
            watch_directory: format!("{crate_directory}/src"),
            crate_directory,
            build_command,
            output_subdirectory: "target/hot".to_string(),
            wrapper_directory: format!("{PROJECT_MEMBER_DIRECTORY}/{wrapper_library_name}"),
            wrapper_library_name,
            folder: package_name.to_string(),
            name,
        }
    }

    /// Verify that the configuration is internally consistent.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ConfigError`] naming the first invalid field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.name.is_empty() {
            return Err(ConfigError::EmptyModuleName);
        }
        if self.watch_directory.is_empty() {
            return Err(ConfigError::EmptyWatchDirectory);
        }
        if self.build_command.is_empty() {
            return Err(ConfigError::EmptyBuildCommand);
        }
        Ok(())
    }
}

/// Complete host configuration: the project module plus every extension
/// the host should build, watch and load.
///
/// Assembled by [`Self::from_environment`] so executable crates stay free of
/// any project or module identity.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The project's display name from `project_settings.yaml`; used as the
    /// window title.
    pub name: String,

    /// The project directory the configuration was read from, resolved
    /// against the workspace root and canonicalized when it exists; `None`
    /// when the configuration was not read from a project directory.
    pub project_directory: Option<PathBuf>,

    /// The artifact file base from `project_settings.yaml`; letters, digits
    /// and underscores only, so it is safe as a file name.
    pub build_binary_name: String,

    /// The project module selected by `PROJECT_PATH`.
    pub project: ProjectModuleConfig,

    /// Extensions selected by the project's `project_settings.yaml`,
    /// loaded before the project. When a renderer is selected, its data crate
    /// (`<renderer>_data`) comes first, whatever the settings list.
    pub extensions: Vec<ExtensionConfig>,

    /// Whether this host loads extension editor crates: the editor posture.
    ///
    /// Only editor frontends set it, through
    /// [`Self::from_environment_for_editor_frontend`]. Every other build never
    /// builds or loads an editor crate. When set, [`Self::extensions`] ends
    /// with the editor crates of the loaded extensions' folders, then those of
    /// every editor-only folder, then the editor's own modules (the crates
    /// under `editor/` tagged for this frontend, the shell among them).
    pub load_editor_crates: bool,

    /// Editor-only modules (a folder holding only its `modules/editor`
    /// crate, and the editor's own modules under `editor/`) whose first load
    /// may fail without aborting setup; the embedder handles their absence -
    /// the editor's safe mode covers the shell. Filled by
    /// [`Self::from_environment_for_editor_frontend`], empty in every other
    /// posture.
    pub first_load_optional: Vec<String>,

    /// The renderer's GPU module, from the settings' `renderer:` key
    /// (`pill_master_renderer` when absent); `None` for `renderer: none`.
    ///
    /// Only a windowed host loads it. Its data crate is among
    /// [`Self::extensions`] in every build, headless included, so a project's
    /// render components exist whether or not anything draws them.
    pub renderer: Option<String>,

    /// The settings' `assets:` section: what the development host does with
    /// the project's source assets.
    pub assets: ProjectAssetSettings,

    /// The settings' `logging:` section, validated: the log levels the
    /// frontend applies over the engine's defaults once the project is read.
    pub logging: pill_player::LoggingSettings,

    /// The settings' `debug_draw:` key: whether a shipping or web build of
    /// the project starts with debug drawing on (`false` when absent). The
    /// development host always draws.
    pub shipping_debug_draw: bool,

    /// The settings' `startup_scene:` key: the `.scene` the project starts
    /// with, as a path below `res`. Queued into the world before any artifact
    /// loads, so the load happens between component registration and the
    /// project's own `init`.
    pub startup_scene: Option<String>,

    /// The project's `res` directory, which asset paths are relative to;
    /// `None` when the configuration was not read from a project directory.
    pub asset_directory: Option<PathBuf>,

    /// The project's own editor crate, when the project has one and this host
    /// is an editor host: `<PROJECT_PATH>/<package>_editor/`, discovered by
    /// convention like an extension's editor crate.
    ///
    /// Filled by [`Self::from_environment_for_editor_frontend`] and `None` in
    /// every other posture, so a game build neither builds, watches nor loads
    /// it. It loads after the project, because it exists to change the editor
    /// the project runs in.
    pub project_editor: Option<ExtensionConfig>,
}

/// The `assets:` section of `project_settings.yaml`.
///
/// Every key defaults to off, so a project without the section imports nothing
/// at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectAssetSettings {
    /// Import every source under `res` whose extension a registered asset
    /// type imports, right after the project starts, and write the metadata
    /// file of each that has none. Development host only.
    pub scan_on_start: bool,
}

/// The `logging:` section of `project_settings.yaml`, as written.
///
/// ```yaml
/// logging:
///   level: info                 # every target, replacing the engine's defaults
///   targets:                    # per target, applied last
///     engine::rendering: info
/// ```
///
/// Read as text and checked by [`Self::validate`], so a misspelled level is
/// an error naming it rather than a serde message about an enum.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectLoggingSettings {
    /// One level for every target: `off`, `error`, `warn`, `info`, `debug` or
    /// `trace`.
    pub level: Option<String>,
    /// How each line writes its local time: `date_time` or `time`.
    pub timestamp: Option<String>,
    /// Whether each line ends with its source `file:line` (default false).
    pub source_location: Option<bool>,
    /// Per-target levels, by `tracing` target or target prefix.
    pub targets: BTreeMap<String, String>,
}

impl ProjectLoggingSettings {
    /// Check every level and target, and turn them into the runtime's form.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidLoggingSettings`] naming the first level
    /// or target that does not read.
    pub fn validate(&self, path: &Path) -> Result<pill_player::LoggingSettings, ConfigError> {
        let invalid_logging_settings = |details: String| ConfigError::InvalidLoggingSettings {
            path: path.display().to_string(),
            details,
        };
        let target_levels: Vec<(&str, &str)> = self
            .targets
            .iter()
            .map(|(target, level)| (target.as_str(), level.as_str()))
            .collect();
        // One conversion serves the development host and a shipping bundle
        // alike, so the same text rules refuse a bad level or target in both.
        pill_player::LoggingSettings::from_text(
            self.level.as_deref(),
            self.timestamp.as_deref(),
            self.source_location,
            &target_levels,
        )
        .map_err(invalid_logging_settings)
    }
}

impl HostConfig {
    /// Assemble the host configuration from the environment and the project's
    /// own settings file.
    ///
    /// `PROJECT_PATH` selects the project, and the project's own
    /// `project_settings.yaml` supplies the extension list, so which project
    /// runs and which modules load are both visible at the project itself.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] when `PROJECT_PATH` is not set, the project
    /// configuration cannot be derived, the project settings file is missing
    /// or cannot be read/parsed, or a configured module is invalid.
    pub fn from_environment() -> Result<Self, ConfigError> {
        Self::from_environment_in_posture(false, DEFAULT_EDITOR_FRONTEND)
    }

    /// The editor host's configuration: [`Self::from_environment`] plus the
    /// extensions' editor crates, for a named editor frontend.
    ///
    /// Editor crates load, and one whose manifest declares
    /// `[package.metadata.pill] editor_frontend = "<name>"` loads only when
    /// that name is this host's. Untagged editor crates
    /// load under every editor frontend.
    ///
    /// # Errors
    ///
    /// As [`Self::from_environment`].
    pub fn from_environment_for_editor_frontend(
        editor_frontend: &str,
    ) -> Result<Self, ConfigError> {
        Self::from_environment_in_posture(true, editor_frontend)
    }

    /// The configuration of either posture; `load_editor_crates` selects the
    /// editor one and `editor_frontend` names this host's editor frontend for
    /// the editor-crate tag convention (`resolve_editor_crates`).
    ///
    /// # Errors
    ///
    /// Returns the same [`ConfigError`]s as [`Self::from_environment`].
    fn from_environment_in_posture(
        load_editor_crates: bool,
        editor_frontend: &str,
    ) -> Result<Self, ConfigError> {
        // Step 1: Resolve the project from the PROJECT_PATH environment
        // variable; there is no other source for it.
        let project_path = required_environment(PROJECT_PATH_ENVIRONMENT_VARIABLE)?;
        let project = ProjectModuleConfig::from_path(&project_path)?;

        // Step 2: Load the project's own settings file. It is the only source
        // of the extension list and of the required `name` /
        // `build_binary_name`, so a missing file is a configuration error
        // reported as such, not masked as a missing field.
        let workspace_root = engine_workspace_root()?;
        let project_root = workspace_root.join(&project_path);
        let project_directory = resolved_project_directory(&workspace_root, &project_path);
        let settings_path = project_root.join(PROJECT_SETTINGS_FILE);
        let project_settings = read_project_settings_file(&project_root)?.ok_or(
            ConfigError::ProjectSettingsFileMissing {
                path: settings_path.display().to_string(),
            },
        )?;

        // Step 3: Resolve the required project name and binary name. The name
        // is the window title. The build binary name is the artifact file base
        // and must be filename-safe (letters, digits and underscores only). A
        // settings file missing either is a configuration error, not a silent
        // default.
        let project_name = project_settings
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or(ConfigError::MissingProjectName {
                path: settings_path.display().to_string(),
            })?
            .to_string();
        let build_binary_name = project_settings
            .build_binary_name
            .as_deref()
            .map(str::trim)
            .filter(|binary_name| is_valid_build_binary_name(binary_name))
            .ok_or(ConfigError::InvalidBuildBinaryName {
                path: settings_path.display().to_string(),
            })?
            .to_string();

        // Step 4: Resolve the extensions, checking every name before it is
        // used in a watch path or a cargo selector. A traversal or duplicate
        // entry, or a crate that no folder under `extensions/` holds, is a
        // configuration error here. Otherwise the host would watch an
        // arbitrary directory, pass a malformed `--package`, or load a second
        // copy of a module.
        let extensions_root = workspace_root.join(EXTENSION_DIRECTORY);
        validate_extension_crate_names(&extensions_root)?;
        let renderer =
            Self::resolve_renderer(project_settings.renderer.as_deref(), &extensions_root)?;
        let module_names =
            Self::module_names_with_renderer_data(renderer.as_deref(), &project_settings.modules);
        let mut extensions = Self::resolve_extensions(&module_names, &extensions_root)?;
        let mut first_load_optional: Vec<String> = Vec::new();
        // An editor host also loads the editor crates, after every runtime
        // crate, so each finds its extension already registered.
        if load_editor_crates {
            let mut runtime_folders: Vec<String> = extensions
                .iter()
                .map(|extension| extension.folder.clone())
                .collect();
            if let Some(renderer_location) = renderer
                .as_deref()
                .and_then(|renderer_name| find_extension_crate(&extensions_root, renderer_name))
            {
                runtime_folders.push(renderer_location.folder);
            }
            let (editor_crates, editor_only_crate_names) =
                Self::resolve_editor_crates(&runtime_folders, &extensions_root, editor_frontend)?;
            extensions.extend(editor_crates);
            // The editor's own modules (its shell) load last, after every
            // extension's editor crate.
            let editor_modules = Self::resolve_editor_modules(
                &workspace_root.join(EDITOR_DIRECTORY),
                editor_frontend,
            )?;
            // The editor-only modules may fail their first load without
            // taking the editor down; the embedder surfaces the errors and
            // offers a retry.
            first_load_optional = editor_only_crate_names;
            first_load_optional.extend(editor_modules.iter().map(|module| module.name.clone()));
            extensions.extend(editor_modules);
        }

        // Step 4: Resolve the project's own editor crate, when it has one.
        // It is not an extension: it lives beside the project's manifest, loads
        // after the project and reloads with it, so the game postures never
        // see it and a project without one is the ordinary case.
        let project_editor = if load_editor_crates {
            resolve_project_editor_crate(
                &workspace_root,
                &project_path,
                &project.name,
                editor_frontend,
            )?
        } else {
            None
        };
        // Whatever posture this is, a wrapper left by another project would
        // still be a workspace member pointing into a folder that may be gone.
        prune_stale_project_editor_wrappers(
            &workspace_root,
            project_editor
                .as_ref()
                .map(|configuration| workspace_root.join(&configuration.wrapper_directory))
                .as_deref(),
        );

        // Step 5: Write every loaded crate's wrapper into the `generated/`
        // folder beside it (its extension folder's, or the editor folder's)
        // before the first build, and clear wrappers for crates this project
        // no longer loads. The renderer module gets one too, with its renderer
        // ABI entry points added to the generated source.
        let mut kept_wrappers: Vec<PathBuf> = Vec::new();
        for extension in &extensions {
            materialize_host_module_wrapper(&workspace_root, extension, false)?;
            kept_wrappers.push(workspace_root.join(&extension.wrapper_directory));
        }
        if let Some(renderer_name) = renderer.as_deref() {
            let renderer_location = find_extension_crate(&extensions_root, renderer_name)
                .ok_or_else(|| ConfigError::ExtensionDirectoryMissing {
                    name: renderer_name.to_string(),
                })?;
            let renderer_module = ExtensionConfig::workspace_member(&renderer_location);
            materialize_host_module_wrapper(&workspace_root, &renderer_module, true)?;
            kept_wrappers.push(workspace_root.join(&renderer_module.wrapper_directory));
        }
        prune_stale_host_module_wrappers(&workspace_root, &kept_wrappers);

        let logging = project_settings.logging.validate(&settings_path)?;
        let startup_scene = Self::validate_startup_scene(
            project_settings.startup_scene.as_deref(),
            &settings_path,
        )?;
        Ok(Self {
            name: project_name,
            project_directory: Some(project_directory),
            build_binary_name,
            project,
            extensions,
            load_editor_crates,
            first_load_optional,
            renderer,
            assets: project_settings.assets,
            logging,
            shipping_debug_draw: project_settings.debug_draw.unwrap_or(false),
            startup_scene,
            asset_directory: Some(project_root.join(PROJECT_ASSET_DIRECTORY)),
            project_editor,
        })
    }

    /// The settings' `startup_scene:` value, checked to be a relative path to
    /// a `.scene` file below the asset directory.
    ///
    /// A blank value means no startup scene.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidStartupScene`] naming the settings file
    /// and the reason when the value is absolute, climbs out of the asset
    /// directory, or does not name a `.scene` file.
    fn validate_startup_scene(
        setting: Option<&str>,
        settings_path: &Path,
    ) -> Result<Option<String>, ConfigError> {
        let Some(configured_path) = setting else {
            return Ok(None);
        };
        let path = configured_path.trim();
        if path.is_empty() {
            return Ok(None);
        }
        let invalid_startup_scene = |details: &str| ConfigError::InvalidStartupScene {
            path: settings_path.display().to_string(),
            details: details.to_string(),
        };
        if !path.to_ascii_lowercase().ends_with(".scene") {
            return Err(invalid_startup_scene("it does not name a .scene file"));
        }
        if path.starts_with('/') || path.starts_with('\\') || path.contains(':') {
            return Err(invalid_startup_scene("it is not a relative path"));
        }
        if path
            .split(['/', '\\'])
            .any(|component| component == ".." || component == ".")
        {
            return Err(invalid_startup_scene(
                "it climbs out of the asset directory",
            ));
        }
        if pill_engine::asset_metadata::checked_relative(path).is_err() {
            return Err(invalid_startup_scene(
                "it is not a plain path below the asset directory",
            ));
        }
        Ok(Some(path.to_string()))
    }

    /// Resolve the settings' `renderer:` value to a renderer name.
    ///
    /// Absent means [`DEFAULT_RENDERER`]; [`NO_RENDERER`] means none. A named
    /// renderer must have both of its crates in an extension folder under
    /// `extensions/`: the GPU module `<renderer>` and the data crate
    /// `<renderer>_data`. A missing one is a configuration error here rather
    /// than a failed build later.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidExtensionName`] when the name is not a
    /// usable directory name, and [`ConfigError::ExtensionDirectoryMissing`]
    /// when either crate has no folder.
    fn resolve_renderer(
        setting: Option<&str>,
        extensions_root: &Path,
    ) -> Result<Option<String>, ConfigError> {
        let renderer_name = setting.map(str::trim).unwrap_or(DEFAULT_RENDERER);
        if renderer_name == NO_RENDERER {
            return Ok(None);
        }
        if !is_valid_module_directory_name(renderer_name) {
            return Err(ConfigError::InvalidExtensionName {
                name: renderer_name.to_string(),
            });
        }
        let data_crate_name = format!("{renderer_name}{RENDERER_DATA_SUFFIX}");
        for crate_name in [renderer_name, data_crate_name.as_str()] {
            if find_extension_crate(extensions_root, crate_name).is_none() {
                return Err(ConfigError::ExtensionDirectoryMissing {
                    name: crate_name.to_string(),
                });
            }
        }
        Ok(Some(renderer_name.to_string()))
    }

    /// The module load order: the renderer's data crate first, when a renderer
    /// is selected, then the settings' own list without it.
    ///
    /// First because extensions and the project may use its components. A
    /// project that also lists the data crate gets it once, not twice.
    fn module_names_with_renderer_data(
        renderer: Option<&str>,
        listed_module_names: &[String],
    ) -> Vec<String> {
        let Some(renderer) = renderer else {
            return listed_module_names.to_vec();
        };
        let data_crate_name = format!("{renderer}{RENDERER_DATA_SUFFIX}");
        std::iter::once(data_crate_name.clone())
            .chain(
                listed_module_names
                    .iter()
                    .filter(|listed_name| listed_name.trim() != data_crate_name)
                    .cloned(),
            )
            .collect()
    }

    /// Resolves the settings file's module names against the extension folders
    /// under `extensions/`.
    ///
    /// Split from [`Self::from_environment`] so the validation is testable
    /// without changing the process's working directory; no test in this crate
    /// does that, and it would race the parallel test threads. The checks run
    /// in the order their errors matter: a name that is not a crate directory
    /// is refused before anything is derived from it, a repeat is refused
    /// before a second copy of the module could load, and a folder holding the
    /// crate is required before the module is configured at all.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::InvalidExtensionName`],
    /// [`ConfigError::EditorCrateListed`] or
    /// [`ConfigError::DuplicateExtensionName`] for a bad entry, and
    /// [`ConfigError::ExtensionDirectoryMissing`] when no folder holds the
    /// crate.
    fn resolve_extensions(
        module_names: &[String],
        extensions_root: &Path,
    ) -> Result<Vec<ExtensionConfig>, ConfigError> {
        let mut seen_names: HashSet<&str> = HashSet::new();
        let mut extension_configs = Vec::new();
        for name in module_names {
            let name = name.trim();
            if !is_valid_module_directory_name(name) {
                return Err(ConfigError::InvalidExtensionName {
                    name: name.to_string(),
                });
            }
            if name.ends_with(EDITOR_CRATE_SUFFIX) {
                return Err(ConfigError::EditorCrateListed {
                    name: name.to_string(),
                });
            }
            if !seen_names.insert(name) {
                return Err(ConfigError::DuplicateExtensionName {
                    name: name.to_string(),
                });
            }
            let Some(location) = find_extension_crate(extensions_root, name) else {
                return Err(ConfigError::ExtensionDirectoryMissing {
                    name: name.to_string(),
                });
            };
            let extension = ExtensionConfig::workspace_member(&location);
            extension.validate()?;
            extension_configs.push(extension);
        }
        Ok(extension_configs)
    }

    /// Returns the editor crates an editor host loads, in load order, and the
    /// names of the editor-only ones.
    ///
    /// First the editor crate of each folder in `runtime_folders` that has one,
    /// in that order. A folder loads its editor crate once, after its runtime
    /// crates. Then every editor-only folder - a folder whose only crate is its
    /// editor crate - in name order, loaded unconditionally. A folder without an
    /// editor crate contributes nothing, silently. Bookkeeping folders (`_...`)
    /// and the older renderers folder (`rendering`) are never searched. The
    /// second half of the return names the editor-only crates, which an editor
    /// host may load tolerantly (`HostConfig::first_load_optional`).
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::RetiredEditorFrontend`] when an editor crate is
    /// tagged for the retired `pill_ui` frontend.
    fn resolve_editor_crates(
        runtime_folders: &[String],
        extensions_root: &Path,
        editor_frontend: &str,
    ) -> Result<(Vec<ExtensionConfig>, Vec<String>), ConfigError> {
        let crate_locations = extension_crate_locations(extensions_root);
        let editor_crate_in = |folder: &str| {
            crate_locations
                .iter()
                .find(|location| location.folder == folder && location.is_editor_crate)
        };
        // A crate tagged for another editor frontend is left out; an untagged
        // one loads under every editor frontend. The `pill_ui` tag is refused
        // rather than skipped: no host runs that frontend, so skipping the
        // crate would silently drop its editor part.
        let loads_editor_crate =
            |editor_crate: &ExtensionCrateLocation| match declared_editor_frontend(
                &editor_crate.directory_under(extensions_root),
            ) {
                Some(frontend_tag) if frontend_tag == RETIRED_EDITOR_FRONTEND => {
                    Err(ConfigError::RetiredEditorFrontend {
                        name: editor_crate.package_name.clone(),
                    })
                }
                Some(frontend_tag) => Ok(frontend_tag == editor_frontend),
                None => Ok(true),
            };
        let mut editor_crates: Vec<&ExtensionCrateLocation> = Vec::new();
        for folder in runtime_folders {
            let Some(editor_crate) = editor_crate_in(folder) else {
                continue;
            };
            if !editor_crates.contains(&editor_crate) && loads_editor_crate(editor_crate)? {
                editor_crates.push(editor_crate);
            }
        }
        // Editor-only folders: the editor crate is the folder's only crate.
        let mut editor_only_crate_names: Vec<String> = Vec::new();
        for editor_crate in crate_locations
            .iter()
            .filter(|location| location.is_editor_crate)
        {
            if editor_crates.contains(&editor_crate) {
                continue;
            }
            if !loads_editor_crate(editor_crate)? {
                continue;
            }
            let holds_runtime_crate = crate_locations.iter().any(|location| {
                location.folder == editor_crate.folder && !location.is_editor_crate
            });
            if !holds_runtime_crate {
                editor_crates.push(editor_crate);
                editor_only_crate_names.push(editor_crate.package_name.clone());
            }
        }
        let editor_crate_configs = editor_crates
            .into_iter()
            .map(ExtensionConfig::workspace_member)
            .collect();
        Ok((editor_crate_configs, editor_only_crate_names))
    }

    /// Returns the editor's own modules an editor host of `editor_frontend`
    /// loads, in directory name order.
    ///
    /// An editor module is a crate directly under `editor_root` whose manifest
    /// tags an editor frontend; an untagged crate there (an executable, the
    /// shared core) is not a module, and a crate tagged for another frontend
    /// loads only under that one. The `generated/` folder holds the wrappers
    /// and is never searched.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::RetiredEditorFrontend`] when a crate is tagged for
    /// the retired `pill_ui` frontend.
    fn resolve_editor_modules(
        editor_root: &Path,
        editor_frontend: &str,
    ) -> Result<Vec<ExtensionConfig>, ConfigError> {
        let mut directory_names: Vec<String> = std::fs::read_dir(editor_root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                    .filter(|directory_name| directory_name != GENERATED_DIRECTORY)
                    .collect()
            })
            .unwrap_or_default();
        directory_names.sort();
        let mut editor_modules = Vec::new();
        for directory_name in directory_names {
            let directory = editor_root.join(&directory_name);
            let Some(package_name) = declared_package_name(&directory) else {
                continue;
            };
            match declared_editor_frontend(&directory) {
                Some(frontend_tag) if frontend_tag == RETIRED_EDITOR_FRONTEND => {
                    return Err(ConfigError::RetiredEditorFrontend { name: package_name });
                }
                Some(frontend_tag) if frontend_tag == editor_frontend => {
                    editor_modules.push(ExtensionConfig::editor_module(
                        &directory_name,
                        &package_name,
                    ));
                }
                _ => {}
            }
        }
        Ok(editor_modules)
    }
}

impl From<ProjectModuleConfig> for HostConfig {
    /// Runs one project with no extensions.
    ///
    /// Lets an embedder that holds a [`ProjectModuleConfig`] wrap it without
    /// naming every host field.
    fn from(project: ProjectModuleConfig) -> Self {
        Self {
            name: project.name.clone(),
            project_directory: None,
            build_binary_name: project.name.clone(),
            project,
            extensions: Vec::new(),
            load_editor_crates: false,
            first_load_optional: Vec::new(),
            renderer: None,
            assets: ProjectAssetSettings::default(),
            logging: pill_player::LoggingSettings::default(),
            shipping_debug_draw: false,
            startup_scene: None,
            asset_directory: None,
            project_editor: None,
        }
    }
}

impl ProjectModuleConfig {
    /// Checks that the configuration is internally consistent.
    ///
    /// Every field that must be non-empty is checked, and the first violation
    /// returns at once so callers can fix one problem at a time.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ConfigError`] naming the first invalid field.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Step 1: Reject an empty name so log messages never reference a blank module.
        if self.name.is_empty() {
            return Err(ConfigError::EmptyModuleName);
        }
        // Step 2: Reject an empty watch directory so the watcher has a real path to monitor.
        if self.watch_directory.is_empty() {
            return Err(ConfigError::EmptyWatchDirectory);
        }
        // Step 3: Reject an empty build command so the runner never spawns a bare process.
        if self.build_command.is_empty() {
            return Err(ConfigError::EmptyBuildCommand);
        }
        Ok(())
    }

    /// Assemble the module configuration from the single `PROJECT_PATH` variable.
    ///
    /// The backend comes from the directory contents (`Cargo.toml` selects the
    /// native Rust backend, a `*.csproj` the managed C# one), and every path,
    /// command and assembly name is derived from the project manifest.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] when the variable is missing, the directory
    /// cannot be inspected, no supported manifest exists, or a manifest field
    /// cannot be read.
    pub fn from_environment() -> Result<Self, ConfigError> {
        let project_path = required_environment(PROJECT_PATH_ENVIRONMENT_VARIABLE)?;
        Self::from_path(&project_path)
    }

    /// Assembles the module configuration from an explicit workspace-relative
    /// project path.
    ///
    /// Backend detection and path derivation are shared with
    /// [`Self::from_environment`]. [`HostConfig::from_environment`] uses this
    /// entry point to apply the `PROJECT_PATH` value.
    ///
    /// # Errors
    ///
    /// Returns a [`ConfigError`] when the directory cannot be inspected, no
    /// supported manifest exists, or a manifest field cannot be read.
    fn from_path(project_path: &str) -> Result<Self, ConfigError> {
        // Step 1: Resolve the directory against the engine workspace root, not
        // the working directory. A launcher that sets its own working directory
        // (`dx serve` runs the editor from `pill_editor/`) would otherwise
        // resolve the project, and the `extensions/` directory beside it, to
        // directories that do not exist.
        let workspace_root = engine_workspace_root()?;
        let project_root = workspace_root.join(project_path);
        if !project_root.is_dir() {
            return Err(ConfigError::ProjectDirectoryMissing {
                path: project_path.to_string(),
            });
        }

        // Step 2: Detect the backend from the manifest files in the directory.
        let cargo_manifest = project_root.join("Cargo.toml");
        if cargo_manifest.is_file() {
            return Self::native_from_manifest(&workspace_root, project_path, &cargo_manifest);
        }
        let csproj_manifest = find_csproj_manifest(&project_root)?;
        Self::csharp_from_manifest(project_path, &csproj_manifest)
    }

    /// Derive a native shared-library configuration from a `Cargo.toml`.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ProjectManifestReadFailed`] or
    /// [`ConfigError::ProjectPackageNameMissing`], plus the errors
    /// [`materialize_host_project_member`] reports while it writes the
    /// generated member.
    fn native_from_manifest(
        workspace_root: &Path,
        project_path: &str,
        manifest_path: &Path,
    ) -> Result<Self, ConfigError> {
        // Step 1: Read the manifest to discover the package and library names.
        let manifest = std::fs::read_to_string(manifest_path).map_err(|source| {
            ConfigError::ProjectManifestReadFailed {
                path: manifest_path.display().to_string(),
                source,
            }
        })?;
        let package_name =
            manifest_string_value(&manifest, "package", "name").ok_or_else(|| {
                ConfigError::ProjectPackageNameMissing {
                    path: manifest_path.display().to_string(),
                }
            })?;
        let library_name =
            manifest_string_value(&manifest, "lib", "name").unwrap_or_else(|| package_name.clone());

        // Step 2: Write a workspace member for the project so it compiles
        // against the engine workspace (one Cargo.lock, one target directory,
        // one crate-metadata set). This is what keeps `pill_spline::Spline`
        // the same type in the project DLL and in the extension DLLs. The
        // member is generated under `projects_generated/` and found by the
        // `projects_generated/*` glob, so the workspace manifest never names
        // the project.
        materialize_host_project_member(workspace_root, project_path, &package_name)?;

        // Step 3: Derive the source watch directory and the build output
        // location. Sources are watched in place at the real project path,
        // while the built artifact appears in the workspace target directory.
        let watch_directory = format!("{project_path}/src");
        // The artifact goes into the private build tree host-spawned builds
        // write into (see [`module_build_artifact_directory`]), never into the
        // shared `target/<profile>` that holds the engine dylibs the host
        // binary maps.
        let output_subdirectory = module_build_artifact_directory();

        // Step 4: Build the Cargo command. The project is a workspace member,
        // so the build selects it by package name from the workspace root. It
        // inherits the workspace's `-C prefer-dynamic` rustflags, matching the
        // extensions, because a shared crate keeps one metadata identity
        // (and therefore one `TypeId`) only when every side is compiled with
        // the same inputs.
        //
        // The command selects the generated member, not the project package it
        // was copied from. The two are distinct packages so a shipping binary
        // can link the project directly without colliding with this one. The
        // mirrored engine features - the package-qualified hot-patch feature
        // among them - are applied by [`host_cargo_command`].
        let build_command =
            host_cargo_command(&[format!("{HOST_PROJECT_MEMBER_PREFIX}{package_name}")]);

        Ok(Self {
            name: package_name,
            manifest_path: Some(format!("{project_path}/Cargo.toml")),
            watch_directory,
            build_command,
            build_environment: spawned_build_environment(),
            backend: ProjectModuleBackend::NativeLibrary {
                library_name,
                output_subdirectory,
            },
        })
    }

    /// Derive a managed C# configuration from a `.csproj` manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::ProjectManifestMissing`] when the file name has
    /// no stem, [`ConfigError::InvalidEnvironmentVariable`] when the selected
    /// configuration is not one of [`CSHARP_CONFIGURATIONS`], and
    /// [`ConfigError::EngineWorkspaceRootUndetermined`] when the workspace
    /// root cannot be derived.
    fn csharp_from_manifest(project_path: &str, csproj_path: &Path) -> Result<Self, ConfigError> {
        // Step 1: The assembly name follows the project file name.
        let project_assembly_name = csproj_path
            .file_stem()
            .and_then(OsStr::to_str)
            .ok_or_else(|| ConfigError::ProjectManifestMissing {
                path: csproj_path.display().to_string(),
            })?
            .to_string();

        // Step 2: Derive the source watch directory, build command, and output.
        // The runtime and the project share one configuration because the
        // project build is what builds the runtime, as its project reference.
        let build_configuration = csharp_build_configuration()?;
        let watch_directory = format!("{project_path}/src");
        let project_output_subdirectory =
            format!("{project_path}/bin/{build_configuration}/{CSHARP_TARGET_FRAMEWORK}");
        let runtime_output_subdirectory = format!(
            "{CSHARP_RUNTIME_PROJECT_SUBDIRECTORY}/bin/{build_configuration}/{CSHARP_TARGET_FRAMEWORK}"
        );
        // `PillEngineRoot` tells the project where the engine's shared C# build
        // (`csharp/Pill.CSharp.props`) is, so a project's location below the
        // repository is not baked into its `.csproj`.
        let engine_root = engine_workspace_root()?;
        let build_command = vec![
            "dotnet".to_string(),
            "build".to_string(),
            format!("{project_path}/{project_assembly_name}.csproj"),
            "-c".to_string(),
            build_configuration.to_string(),
            format!(
                "-p:PillEngineRoot={}/",
                engine_root.display().to_string().replace('\\', "/")
            ),
            "--nologo".to_string(),
        ];

        Ok(Self {
            name: project_assembly_name.clone(),
            manifest_path: None,
            watch_directory,
            build_command,
            // The managed build never invokes rustc, so it needs no overrides.
            build_environment: Vec::new(),
            backend: ProjectModuleBackend::CSharp(CSharpModuleConfig::new(
                CSHARP_RUNTIME_ASSEMBLY_NAME,
                runtime_output_subdirectory,
                project_assembly_name,
                project_output_subdirectory,
            )),
        })
    }
}

/// What `editor_settings.yaml` selected for the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSettings {
    /// The `editor_theme:` value, as the file spells it: a path below the
    /// project's `res`.
    pub editor_theme: Option<String>,
    /// The `focus_game_on_play:` value: whether Play brings the Game tab
    /// forward. Absent means `true`, which is what a person expects of Play.
    pub focus_game_on_play: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            editor_theme: None,
            focus_game_on_play: true,
        }
    }
}

/// Values declared by the project's own `editor_settings.yaml`.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct EditorSettingsFile {
    /// The editor theme the project's editor layer uses, below `res`.
    editor_theme: Option<String>,
    /// Whether Play brings the Game tab forward.
    focus_game_on_play: Option<bool>,
}

/// Values declared by the project's own `project_settings.yaml`.
///
/// `name` and `build_binary_name` are required (the display/window title and
/// the artifact file base respectively). The rest is optional package
/// metadata.
///
/// Unknown keys are refused rather than dropped, so `module:` instead of
/// `modules:` is an error rather than a silently empty extension list.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ProjectSettingsFile {
    /// Project display name; required, used as the window title.
    name: Option<String>,
    /// Artifact file base; required, letters/digits/underscores only.
    build_binary_name: Option<String>,
    /// Optional package version.
    version: Option<String>,
    /// Optional package author.
    author: Option<String>,
    /// Optional one-line package description.
    description: Option<String>,
    /// Extension crate names, in load order. This key is the only source for
    /// the list: there is no environment-variable override, so the file is
    /// always the complete answer to "which modules load". The one exception
    /// is the selected renderer's data crate, which loads first by convention.
    modules: Vec<String>,
    /// The renderer the project draws with: the name of its GPU module under
    /// `extensions/` (`pill_master_renderer` when absent), or `none`. Its data
    /// crate, `<renderer>_data`, loads in every build.
    renderer: Option<String>,
    /// What the development host does with the project's source assets.
    assets: ProjectAssetSettings,
    /// Log levels over the engine's defaults.
    logging: ProjectLoggingSettings,
    /// Whether a shipping or web build starts with debug drawing on; the
    /// development host always draws.
    debug_draw: Option<bool>,
    /// The `.scene` the project starts with, as a path below `res`; loaded
    /// after every component is registered and before the project's `init`
    /// seeds anything.
    startup_scene: Option<String>,
}

// =============================================================================
// Free functions
// =============================================================================

/// Cargo profile *name* matching [`HOST_PROFILE_DIRECTORY`].
///
/// Cargo names the default profile `dev` but writes it into `target/debug`;
/// every other profile's directory is its own name. This is the only place
/// the two vocabularies are reconciled.
///
/// Module builds must use the host's profile. A host and a module compiled
/// with different profiles resolve different crate-metadata hashes, so the
/// module's exports do not match what the host looks up, and `LoadLibrary`
/// fails with "The specified procedure could not be found" without naming
/// anything useful.
pub(crate) fn host_profile_name() -> &'static str {
    profile_name_for_directory(HOST_PROFILE_DIRECTORY)
}

/// The `--target` triple host-spawned builds must pass, when the host itself
/// was built with one.
pub(crate) fn host_target_triple() -> Option<&'static str> {
    if HOST_TARGET_TRIPLE.is_empty() {
        None
    } else {
        Some(HOST_TARGET_TRIPLE)
    }
}

/// Map a cargo profile *directory* to the profile *name* that produces it.
///
/// Split out from [`host_profile_name`] so the mapping is testable without
/// rebuilding the host under every profile.
fn profile_name_for_directory(directory: &str) -> &str {
    // The one profile whose directory and name differ. `test` and `bench`
    // builds also write into `debug`, but those profiles apply to test and
    // benchmark targets, never to the host executables this name is used for.
    if directory == "debug" {
        "dev"
    } else {
        directory
    }
}

/// Whether a cargo profile enables link-time optimization, the one case where
/// a host-spawned build must drop the workspace's `-C prefer-dynamic` rustflag.
///
/// rustc refuses `prefer-dynamic` combined with `lto = "fat"` outright
/// ("cannot prefer dynamic linking when performing LTO"), so a host running
/// with an LTO profile has to clear the flag for every build it spawns. Every
/// other profile - `dev`, and launcher-injected dev-like profiles such as the
/// dioxus CLI's `desktop-dev` - keeps `prefer-dynamic`. That flag is what
/// makes modules dynamic: they import the one shared `pill_core.dll` instead
/// of embedding a private engine copy.
fn profile_uses_lto(profile: &str) -> bool {
    // The workspace manifest sets `lto = "fat"` on `release` and everything
    // that inherits from it. Custom profiles that inherit `dev` (for example
    // the dioxus CLI's `desktop-dev`) deliberately stay out of this list.
    matches!(profile, "release" | "release-fast" | "release-with-debug")
}

/// Whether this host binary is running as an app launched by the dioxus CLI.
///
/// `dx serve` and `dx run` set `DIOXUS_CLI_ENABLED` (plus `DIOXUS_BUILD_ID`
/// and friends) on the environment of every app they build and launch
/// (dioxus-cli `build/builder.rs`, `child_environment_variables`); a plain
/// `cargo run` never sets it. That is what separates the dx-built editor -
/// whose engine dylibs carry dx's workspace-wrapper hash - from every other
/// host. [`crate::build_runner::apply_cargo_host_overrides`] reads it too,
/// because it keeps the cargo anchor for editor hosts and dx-launched hosts:
/// the editor's renderer graph must be present when project modules are
/// built, and under dx its macro graph adds features to proc-macro units that
/// the engine manifests cannot list.
pub(crate) fn running_under_dioxus_cli() -> bool {
    env::var_os(DIOXUS_CLI_ENABLED_ENVIRONMENT).is_some()
}

/// Canonicalize a path without the Windows `\\?\` verbatim prefix.
///
/// `std::fs::canonicalize` returns `\\?\C:\...` on Windows; dx canonicalizes
/// its own executable with `dunce` (which strips the prefix), and the two
/// strings must be byte-identical or cargo's metadata hash will not agree.
fn canonicalize_plain(path: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(path).ok()?;
    let text = canonical.to_string_lossy();
    let plain_text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    Some(PathBuf::from(plain_text))
}

/// The project directory a start reports: the path joined onto the workspace
/// root, canonicalized when it exists so `..` segments and symlinks do not
/// reach the log.
///
/// A directory that does not resolve keeps the joined path, which is still
/// absolute because the workspace root is.
fn resolved_project_directory(workspace_root: &Path, project_path: &str) -> PathBuf {
    let joined = workspace_root.join(project_path);
    canonicalize_plain(&joined).unwrap_or(joined)
}

/// Canonical path of the `dx` executable that launched this host, if any.
///
/// dx builds by setting `RUSTC_WORKSPACE_WRAPPER` to its own canonical
/// executable path (dioxus-cli `build/request.rs`, `cargo_build_command`),
/// and cargo folds that exact path into the `-C metadata` hash of workspace
/// member crates (cargo `compiler/build_runner/compilation_files.rs`,
/// `compute_metadata`). Host-spawned builds must use the same path string for
/// their member crates to hash identically to the editor's. This function
/// therefore finds dx the same way dx finds itself: it prefers a wrapper value
/// already recorded in the environment, and otherwise looks `dx` up on `PATH`
/// and canonicalizes it. The result is cached because it is read for every
/// spawned build.
fn dioxus_executable_path() -> Option<&'static Path> {
    static DIOXUS_EXECUTABLE_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIOXUS_EXECUTABLE_PATH
        .get_or_init(|| {
            // A launcher may already have set the wrapper (or dx may have left
            // it in the environment); reuse it rather than guessing on PATH.
            if let Some(recorded_wrapper) = env::var_os(RUSTC_WORKSPACE_WRAPPER_ENVIRONMENT) {
                let recorded_path = PathBuf::from(recorded_wrapper);
                return canonicalize_plain(&recorded_path).or(Some(recorded_path));
            }
            let executable_name = if cfg!(windows) { "dx.exe" } else { "dx" };
            for directory in env::split_paths(&env::var_os("PATH")?) {
                let candidate_executable = directory.join(executable_name);
                if candidate_executable.is_file() {
                    return canonicalize_plain(&candidate_executable);
                }
            }
            None
        })
        .as_deref()
}

/// Directory dx's wrapper writes captured member rustc invocations into.
///
/// dx acts as a rustc wrapper only when `DX_RUSTC` names a directory. It
/// records one JSON file per member crate there (this host never reads them
/// back) and then runs the real `rustc`. A private temp directory keeps these
/// host-driven captures out of dx's own `target/dx/.captured-args` scope,
/// which `dx serve` reads for its own thin-rebuild replay.
fn dioxus_wrapper_args_directory() -> PathBuf {
    std::env::temp_dir().join("pill-host-dx-rustc-args")
}

/// Environment every host-spawned cargo build needs, on top of the host's own.
///
/// Both entries keep the spawned build's crate-metadata universe
/// byte-identical to the running host binary. `RUSTFLAGS` is cleared for LTO
/// profiles, where the workspace's `-C prefer-dynamic` cannot survive (rustc
/// rejects the pairing), and as the fail-safe described below. It is the only
/// lever, because cargo MERGES `build.rustflags` arrays, so an explicit empty
/// `RUSTFLAGS` wins. Under the dioxus CLI the build also copies dx's
/// `RUSTC_WORKSPACE_WRAPPER`: dx wraps every member's rustc invocation through
/// its own binary, cargo folds that path into the member crates'
/// `-C metadata`, and registry crates are unaffected. dx intercepts
/// linker-driver invocations only, and host builds link through `link.exe`, so
/// member compiles are passed through to the real `rustc`.
///
/// Fail-safe: under dx with an unresolvable wrapper path, `prefer-dynamic` is
/// dropped too and modules stay static. A static module loads regardless of
/// the wrapper hash, where a dynamic one fails with os error 127.
pub(crate) fn spawned_build_environment() -> Vec<(String, String)> {
    let mut environment = Vec::new();
    let under_dioxus = running_under_dioxus_cli();
    let wrapper_mirrored = under_dioxus && dioxus_executable_path().is_some();
    if profile_uses_lto(host_profile_name()) || (under_dioxus && !wrapper_mirrored) {
        environment.push(("RUSTFLAGS".to_string(), String::new()));
    }
    if wrapper_mirrored {
        if let Some(executable_path) = dioxus_executable_path() {
            environment.push((
                RUSTC_WORKSPACE_WRAPPER_ENVIRONMENT.to_string(),
                executable_path.display().to_string(),
            ));
            environment.push((
                DX_RUSTC_WRAPPER_ENVIRONMENT.to_string(),
                dioxus_wrapper_args_directory().display().to_string(),
            ));
        }
    }
    environment
}

/// Workspace-relative directory cargo wrote this host's profile into.
///
/// Used as the host's own artifact location, so the host looks for the engine
/// dylibs it maps where its own profile put them rather than always in
/// `target/debug`. A host built with `--target` (the dioxus CLI) lives under
/// `target/<triple>/<profile>`; a native one under `target/<profile>`.
pub(crate) fn host_target_directory() -> String {
    match host_target_triple() {
        Some(triple) => format!("target/{triple}/{HOST_PROFILE_DIRECTORY}"),
        None => format!("target/{HOST_PROFILE_DIRECTORY}"),
    }
}

/// Profile subdirectory inside [`MODULE_BUILD_TARGET_DIRECTORY`] where cargo
/// writes a host-spawned build's artifacts (`target/hot/build/debug` for a
/// native dev host, `target/hot/build/x86_64-pc-windows-msvc/desktop-dev`
/// under the dioxus CLI, which builds with an explicit `--target`).
pub(crate) fn module_build_artifact_directory() -> String {
    match host_target_triple() {
        Some(triple) => {
            format!("{MODULE_BUILD_TARGET_DIRECTORY}/{triple}/{HOST_PROFILE_DIRECTORY}")
        }
        None => format!("{MODULE_BUILD_TARGET_DIRECTORY}/{HOST_PROFILE_DIRECTORY}"),
    }
}

/// Read a required configuration variable or report it as missing.
///
/// # Errors
///
/// Returns [`ConfigError::MissingEnvironmentVariable`] when the variable is
/// absent, so callers can fix the launch environment in one step.
fn required_environment(variable: &'static str) -> Result<String, ConfigError> {
    env::var(variable).map_err(|_| ConfigError::MissingEnvironmentVariable { variable })
}

/// Whether host-driven cargo builds should emit a timing report.
///
/// [`host_cargo_command`] reads the variable, and the analytics collector
/// reads the report cargo then writes, so the flag cannot be set on one side
/// and silently unread on the other.
pub(crate) fn cargo_timings_enabled() -> bool {
    std::env::var_os(CARGO_TIMINGS_ENVIRONMENT_VARIABLE).is_some()
}

/// Builds the host's cargo command for one or more selected packages.
///
/// Every host-spawned build - a single extension wrapper, the native project
/// member, or the startup batch - assembles its command here. The order -
/// package selections, `--offline`, the host's profile, the optional timings
/// flag, the package-qualified hot-patch feature - therefore cannot drift
/// between a per-module build and the batch. `--offline` avoids the `~/.cargo`
/// package cache lock rust-analyzer can hold. The profile is explicit because
/// a release host loading a debug module fails inside `LoadLibrary` on
/// mismatched crate-metadata hashes.
///
/// The engine core's symbol names hash its features. A module built without
/// `hot_patch` imports names `pill_engine_core.dll` does not export, and its
/// own `register_system` creates no dispatch slot, so every patch is refused
/// with "no hot-patchable system registered". The feature is package-qualified
/// because one invocation may select several packages. The host's other engine
/// features travel through [`crate::build_runner::apply_cargo_host_overrides`].
pub(crate) fn host_cargo_command(packages: &[String]) -> Vec<String> {
    let mut command = vec!["cargo".to_string(), "build".to_string()];
    for package in packages {
        command.push("--package".to_string());
        command.push(package.clone());
    }
    command.push("--offline".to_string());
    command.push("--profile".to_string());
    command.push(host_profile_name().to_string());
    if cargo_timings_enabled() {
        command.push("--timings".to_string());
    }
    command.push("--features".to_string());
    command.push("pill_engine/development".to_string());
    command
}

/// MSBuild configuration the managed project is built in, from
/// [`CSHARP_CONFIGURATION_ENVIRONMENT_VARIABLE`]; `Release` when it is unset.
///
/// The value is matched without regard to case but returned in MSBuild's own
/// spelling, because the result also names the output directory the host loads
/// from.
///
/// # Errors
///
/// Returns [`ConfigError::InvalidEnvironmentVariable`] for any other value,
/// rather than falling back. A mistyped `Debug` that silently built `Release`
/// would leave the debugger showing optimized-away local variables with no
/// hint why.
fn csharp_build_configuration() -> Result<&'static str, ConfigError> {
    let Ok(requested_configuration) = env::var(CSHARP_CONFIGURATION_ENVIRONMENT_VARIABLE) else {
        return Ok(CSHARP_CONFIGURATIONS[0]);
    };
    CSHARP_CONFIGURATIONS
        .into_iter()
        .find(|configuration| configuration.eq_ignore_ascii_case(requested_configuration.trim()))
        .ok_or(ConfigError::InvalidEnvironmentVariable {
            variable: CSHARP_CONFIGURATION_ENVIRONMENT_VARIABLE,
            value: requested_configuration,
            expected: "Release or Debug",
        })
}

/// Directory holding the engine workspace: the base every stored relative path
/// is resolved against.
///
/// The value is taken from this crate's own build location, a compile-time
/// fact, rather than from the process's working directory. The dioxus CLI runs
/// the editor with `pill_editor/` as its working directory, which would
/// otherwise resolve `extensions/` under the editor crate and fail with
/// [`ConfigError::ExtensionDirectoryMissing`].
///
/// # Errors
///
/// Returns [`ConfigError::EngineWorkspaceRootUndetermined`] when the manifest
/// directory has no parent.
pub(crate) fn engine_workspace_root() -> Result<PathBuf, ConfigError> {
    // Two levels up, because this crate sits one directory below the root it
    // is looking for: `hosting/pill_development_host` -> `hosting` -> the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or(ConfigError::EngineWorkspaceRootUndetermined)
}

/// Finds a `.csproj` manifest inside a project directory.
///
/// Returns the first `.csproj` the directory lists.
///
/// # Errors
///
/// Returns [`ConfigError::ProjectDirectoryMissing`] when the directory cannot
/// be listed and [`ConfigError::ProjectManifestMissing`] when no `.csproj`
/// file is found.
fn find_csproj_manifest(project_root: &Path) -> Result<PathBuf, ConfigError> {
    let entries =
        std::fs::read_dir(project_root).map_err(|_| ConfigError::ProjectDirectoryMissing {
            path: project_root.display().to_string(),
        })?;
    for entry in entries {
        let entry = entry.map_err(|_| ConfigError::ProjectDirectoryMissing {
            path: project_root.display().to_string(),
        })?;
        let path = entry.path();
        if path.extension() == Some(OsStr::new("csproj")) {
            return Ok(path);
        }
    }
    Err(ConfigError::ProjectManifestMissing {
        path: project_root.display().to_string(),
    })
}

/// Reads a string value out of one section of a TOML manifest.
///
/// Used only for the handful of fields the host infers from a `Cargo.toml`
/// (the package name and the library name). A manifest that does not parse
/// therefore answers `None`, exactly like a missing key. The same manifest is
/// parsed again, with the error reported, when the workspace member is
/// written.
fn manifest_string_value(manifest: &str, section: &str, key: &str) -> Option<String> {
    let document = manifest.parse::<DocumentMut>().ok()?;
    document
        .get(section)?
        .as_table()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

/// Whether the project manifest declares a direct dependency on `crate_name`.
///
/// Reads the project's manifest again at runtime because the decision depends
/// on the module that just changed, which is only known during the frame loop.
/// A missing or unreadable manifest counts as "no dependency", so a transient
/// filesystem state can never force the project through an extra reload.
pub(crate) fn project_depends_on_crate(
    workspace_root: &Path,
    project: &ProjectModuleConfig,
    crate_name: &str,
) -> bool {
    // Only a native Rust project can link an extension crate; a managed
    // project cannot reference it, so it never needs this reload.
    if !matches!(project.backend, ProjectModuleBackend::NativeLibrary { .. }) {
        return false;
    }
    let Some(manifest_path) = &project.manifest_path else {
        return false;
    };
    let Ok(manifest) = std::fs::read_to_string(workspace_root.join(manifest_path)) else {
        return false;
    };
    manifest_depends_on_crate(&manifest, crate_name)
}

/// Whether a `Cargo.toml` declares a dependency on `crate_name`.
///
/// Reads the document rather than scanning lines: a dependency may be an
/// inline table, a sub-table, renamed through `package = "..."`, or declared
/// under a target-specific table.
fn manifest_depends_on_crate(manifest: &str, crate_name: &str) -> bool {
    let Ok(document) = manifest.parse::<DocumentMut>() else {
        return false;
    };
    linking_dependency_tables(&document)
        .iter()
        .any(|table| declares_dependency(table, crate_name))
}

/// Every dependency table in `document`: the three linking tables at the top
/// level, and the same three under each `[target.<cfg>]` table.
fn linking_dependency_tables(document: &DocumentMut) -> Vec<&Table> {
    let mut tables = Vec::new();
    for section in LINKING_DEPENDENCY_TABLES {
        if let Some(table) = document.get(section).and_then(Item::as_table) {
            tables.push(table);
        }
    }
    if let Some(targets) = document.get("target").and_then(Item::as_table) {
        for (_, target) in targets.iter() {
            let Some(target_table) = target.as_table() else {
                continue;
            };
            for section in LINKING_DEPENDENCY_TABLES {
                if let Some(table) = target_table.get(section).and_then(Item::as_table) {
                    tables.push(table);
                }
            }
        }
    }
    tables
}

/// Whether one dependency table declares `crate_name`, directly or as the
/// `package` of a renamed entry.
fn declares_dependency(table: &Table, crate_name: &str) -> bool {
    table.iter().any(|(key, item)| {
        if key == crate_name {
            return true;
        }
        match item {
            Item::Value(Value::InlineTable(inline_table)) => {
                inline_table.get("package").and_then(Value::as_str) == Some(crate_name)
            }
            Item::Table(sub_table) => {
                sub_table.get("package").and_then(Item::as_str) == Some(crate_name)
            }
            _ => false,
        }
    })
}

/// Reads the editor's settings file from the project root.
///
/// A project usually has no such file; the settings are then empty. Unknown
/// keys are refused instead of ignored, so a misspelled `editor_themes:` is
/// reported and does not quietly fall back to the default theme.
///
/// # Errors
///
/// Returns a message naming the file when it exists but cannot be read, is
/// not valid YAML, or names an unknown key.
pub fn read_editor_settings(project_directory: &Path) -> Result<EditorSettings, String> {
    let path = project_directory.join(EDITOR_SETTINGS_FILE);
    if !path.is_file() {
        return Ok(EditorSettings::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("`{}` could not be read: {error}", path.display()))?;
    let file: EditorSettingsFile = serde_yaml::from_str(&text)
        .map_err(|error| format!("`{}` is not valid YAML: {error}", path.display()))?;
    Ok(EditorSettings {
        editor_theme: file
            .editor_theme
            .filter(|editor_theme| !editor_theme.trim().is_empty()),
        focus_game_on_play: file.focus_game_on_play.unwrap_or(true),
    })
}

/// Returns the theme path the project layer adds: the `editor_theme:` value
/// when the file names one, `editor/editor.theme` when the project ships that
/// file, and nothing otherwise.
///
/// The path is relative to the project's `res`. It is checked against the
/// editor theme scope when it loads, not here, so a file naming
/// `ui/x.theme` is refused at load rather than resolved.
pub fn resolve_editor_theme(
    project_directory: &Path,
    editor_settings: &EditorSettings,
) -> Option<String> {
    if let Some(path) = editor_settings.editor_theme.clone() {
        return Some(path);
    }
    project_directory
        .join(PROJECT_ASSET_DIRECTORY)
        .join("editor")
        .join("editor.theme")
        .is_file()
        .then(|| DEFAULT_EDITOR_THEME.to_string())
}

/// Whether `build_binary_name` is a safe artifact file base: letters, digits
/// and underscores only.
fn is_valid_build_binary_name(build_binary_name: &str) -> bool {
    !build_binary_name.is_empty()
        && build_binary_name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Whether `module_directory_name` is a usable extension folder name.
///
/// The name becomes a path segment under `extensions/` and a cargo package
/// selector, so it is checked more strictly than `build_binary_name`:
/// a leading digit is refused (crate folders start with a letter), `-` is
/// allowed because real crate folders use it, and every other character -
/// path separators, spaces and dots - is rejected instead of being placed
/// into a watch path.
fn is_valid_module_directory_name(module_directory_name: &str) -> bool {
    let mut characters = module_directory_name.chars();
    let Some(first_character) = characters.next() else {
        return false;
    };
    first_character.is_ascii_alphabetic()
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

/// Where one extension crate lives under `extensions/`, as its manifest
/// declares it.
///
/// Every question the host asks about an extension crate goes through this
/// record: which folder holds it, where its sources are, and whether it is its
/// folder's editor crate. The package name comes from the crate's own
/// `Cargo.toml`, so the directory a crate sits in does not have to repeat it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtensionCrateLocation {
    /// The package name the crate's manifest declares: the name `modules:`
    /// lists, the cargo selector and the library name the host loads.
    pub(crate) package_name: String,

    /// The extension folder under `extensions/` that holds the crate.
    pub(crate) folder: String,

    /// The crate's directory inside its extension folder, with `/`
    /// separators: `modules/runtime`, `modules/editor`, or one level deeper
    /// under the master renderer's `modules/runtime`.
    pub(crate) directory_in_folder: String,

    /// Whether this is the folder's editor crate: the crate in
    /// `modules/editor`.
    pub(crate) is_editor_crate: bool,
}

impl ExtensionCrateLocation {
    /// The location a crate that no folder holds would have:
    /// `extensions/<package>/modules/runtime`. A configuration built from it
    /// fails at its watch path, which names the crate that is missing.
    fn conventional(package_name: &str) -> Self {
        Self {
            package_name: package_name.to_string(),
            folder: package_name.to_string(),
            directory_in_folder: format!("{EXTENSION_MODULES_DIRECTORY}/{RUNTIME_CRATE_DIRECTORY}"),
            is_editor_crate: false,
        }
    }

    /// The crate's directory relative to the workspace root, with `/`
    /// separators.
    pub(crate) fn workspace_relative_directory(&self) -> String {
        format!(
            "{EXTENSION_DIRECTORY}/{}/{}",
            self.folder, self.directory_in_folder
        )
    }

    /// The crate's directory under `extensions_root`.
    fn directory_under(&self, extensions_root: &Path) -> PathBuf {
        extensions_root
            .join(&self.folder)
            .join(&self.directory_in_folder)
    }

    /// Whether the crate sits one level below `modules/runtime`, in a runtime
    /// folder that groups crates instead of holding one (the master renderer's
    /// `data` and `gpu`).
    fn is_nested_runtime_crate(&self) -> bool {
        self.directory_in_folder
            .strip_prefix(EXTENSION_MODULES_DIRECTORY)
            .and_then(|rest| rest.strip_prefix('/'))
            .and_then(|rest| rest.strip_prefix(RUNTIME_CRATE_DIRECTORY))
            .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// Returns every extension crate under `extensions_root`, in folder order.
///
/// A folder holds its extension in `modules/runtime` and its editor part in
/// `modules/editor`; the master renderer's `modules/runtime` groups two
/// crates, one per child directory. A crate is a directory holding a
/// `Cargo.toml` that declares a package name. Folders starting with `_` are
/// workspace bookkeeping, and `rendering` holds the older renderers no build
/// uses; neither is read.
pub(crate) fn extension_crate_locations(extensions_root: &Path) -> Vec<ExtensionCrateLocation> {
    let sorted_directory_names = |directory: &Path| -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    };
    let mut locations = Vec::new();
    for folder in sorted_directory_names(extensions_root) {
        if folder.starts_with('_') || folder == OLD_RENDERERS_DIRECTORY {
            continue;
        }
        let modules_directory = extensions_root
            .join(&folder)
            .join(EXTENSION_MODULES_DIRECTORY);
        let runtime_directory = modules_directory.join(RUNTIME_CRATE_DIRECTORY);
        // The runtime crate is `modules/runtime` itself, unless that folder
        // groups crates instead (the master renderer's `data` and `gpu`).
        let runtime_crates = match declared_package_name(&runtime_directory) {
            Some(package_name) => vec![(
                format!("{EXTENSION_MODULES_DIRECTORY}/{RUNTIME_CRATE_DIRECTORY}"),
                package_name,
            )],
            None => sorted_directory_names(&runtime_directory)
                .into_iter()
                .filter_map(|directory_name| {
                    let directory = runtime_directory.join(&directory_name);
                    let package_name = declared_package_name(&directory)?;
                    let directory_in_folder = format!(
                        "{EXTENSION_MODULES_DIRECTORY}/{RUNTIME_CRATE_DIRECTORY}/{directory_name}"
                    );
                    Some((directory_in_folder, package_name))
                })
                .collect(),
        };
        for (directory_in_folder, package_name) in runtime_crates {
            locations.push(ExtensionCrateLocation {
                package_name,
                folder: folder.clone(),
                directory_in_folder,
                is_editor_crate: false,
            });
        }
        let editor_directory = modules_directory.join(EDITOR_CRATE_DIRECTORY);
        if let Some(package_name) = declared_package_name(&editor_directory) {
            locations.push(ExtensionCrateLocation {
                package_name,
                folder: folder.clone(),
                directory_in_folder: format!(
                    "{EXTENSION_MODULES_DIRECTORY}/{EDITOR_CRATE_DIRECTORY}"
                ),
                is_editor_crate: true,
            });
        }
    }
    locations
}

/// Refuses an extension crate whose package name disagrees with the folder it
/// sits in.
///
/// An editor crate is the crate in `modules/editor`, and its package name must
/// end in [`EDITOR_CRATE_SUFFIX`]; a runtime crate must not carry that suffix.
/// Splitting the two markers would let a crate load as an editor part by
/// location while `modules:` or a shipping bundle selected it as a runtime
/// module.
///
/// The folder also spells the crate it holds: a runtime crate is named after
/// its extension folder (`extensions/pill_audio/modules/runtime` holds
/// `pill_audio`) and an editor crate after that folder plus the suffix
/// (`pill_audio_editor`). A path therefore names the crate it holds, and the
/// conventional path a missing crate falls back to resolves to a real
/// directory. The master renderer's nested runtime crates are exempt, because
/// `extensions/pill_master_renderer/modules/runtime` groups two crates whose
/// names cannot both equal the folder name.
///
/// # Errors
///
/// Returns [`ConfigError::EditorCrateNameMismatch`],
/// [`ConfigError::RuntimeCrateNameMismatch`] or
/// [`ConfigError::ExtensionFolderNameMismatch`], naming the crate.
pub(crate) fn validate_extension_crate_names(extensions_root: &Path) -> Result<(), ConfigError> {
    for location in extension_crate_locations(extensions_root) {
        let has_editor_suffix = location.package_name.ends_with(EDITOR_CRATE_SUFFIX);
        if location.is_editor_crate && !has_editor_suffix {
            return Err(ConfigError::EditorCrateNameMismatch {
                name: location.package_name,
            });
        }
        if !location.is_editor_crate && has_editor_suffix {
            return Err(ConfigError::RuntimeCrateNameMismatch {
                name: location.package_name,
            });
        }
        if location.is_nested_runtime_crate() {
            continue;
        }
        let expected_package_name = if location.is_editor_crate {
            format!("{}{EDITOR_CRATE_SUFFIX}", location.folder)
        } else {
            location.folder.clone()
        };
        if location.package_name != expected_package_name {
            return Err(ConfigError::ExtensionFolderNameMismatch {
                folder: format!("{EXTENSION_DIRECTORY}/{}", location.folder),
                name: location.package_name,
            });
        }
    }
    Ok(())
}

/// Returns the extension crate whose manifest declares `package_name`, or
/// `None` when no folder under `extensions_root` holds it.
///
/// A crate in a folder named after the package wins, which is what a crate
/// seeded outside the layout looks like; otherwise the first match in folder
/// order is returned. Package names are unique across the workspace, so the
/// real tree has exactly one match.
pub(crate) fn find_extension_crate(
    extensions_root: &Path,
    package_name: &str,
) -> Option<ExtensionCrateLocation> {
    let mut matching = extension_crate_locations(extensions_root)
        .into_iter()
        .filter(|location| location.package_name == package_name)
        .peekable();
    let first_match = matching.peek().cloned();
    matching
        .find(|location| location.folder == package_name)
        .or(first_match)
}

/// Returns the absolute directory of the extension crate named
/// `package_name`, or the conventional `extensions/<package>/modules/runtime`
/// when no folder holds it yet.
pub(crate) fn extension_crate_directory(workspace_root: &Path, package_name: &str) -> PathBuf {
    let extensions_root = workspace_root.join(EXTENSION_DIRECTORY);
    find_extension_crate(&extensions_root, package_name)
        .unwrap_or_else(|| ExtensionCrateLocation::conventional(package_name))
        .directory_under(&extensions_root)
}

/// The package name `directory/Cargo.toml` declares, or `None` when the
/// directory holds no manifest or its manifest declares no package.
fn declared_package_name(directory: &Path) -> Option<String> {
    let manifest_text = std::fs::read_to_string(directory.join("Cargo.toml")).ok()?;
    let manifest = manifest_text.parse::<DocumentMut>().ok()?;
    let package_name = manifest.get("package")?.get("name")?.as_str()?;
    Some(package_name.to_string())
}

/// Reads the project's own settings file from the project root.
///
/// Returns `Ok(None)` when no file exists. The caller decides whether that is
/// a missing-file error (as [`HostConfig::from_environment`] treats it,
/// because `name` and `build_binary_name` are required) or simply no
/// settings. A file that exists but cannot be read or parsed is always an
/// error, so a typo cannot silently select a different module set.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the file exists but cannot be read or is
/// not valid YAML.
fn read_project_settings_file(
    project_root: &Path,
) -> Result<Option<ProjectSettingsFile>, ConfigError> {
    let config_path = project_root.join(PROJECT_SETTINGS_FILE);
    if !config_path.is_file() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&config_path).map_err(|source| {
        ConfigError::ProjectSettingsFileReadFailed {
            path: config_path.display().to_string(),
            source,
        }
    })?;
    serde_yaml::from_str(&contents).map(Some).map_err(|source| {
        ConfigError::ProjectSettingsFileParseFailed {
            path: config_path.display().to_string(),
            details: source.to_string(),
        }
    })
}

/// Writes the temporary workspace member that builds the native project.
///
/// A type keeps one identity across DLLs only when the project compiles as a
/// member of the engine workspace: one Cargo.lock, one target directory, one
/// crate-metadata set. The project source lives outside the workspace, so
/// this writes a generated crate under `projects_generated/`, where the
/// workspace glob finds it without an entry in the workspace manifest.
///
/// The generated `Cargo.toml` carries the project's own manifest, with every
/// `path` dependency made absolute and the `[lib]` `path` pointed at the real
/// source, so edits keep triggering rebuilds through the normal watcher. The
/// member is rebuilt from the project manifest on every startup and written
/// only when its content changed, so manifest changes always apply.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the project manifest cannot be read or the
/// generated member directory or manifest cannot be written.
fn materialize_host_project_member(
    workspace_root: &Path,
    project_path: &str,
    package_name: &str,
) -> Result<(), ConfigError> {
    let project_root = workspace_root.join(project_path);
    let manifest_path = project_root.join("Cargo.toml");
    let source_manifest = std::fs::read_to_string(&manifest_path).map_err(|source| {
        ConfigError::ProjectManifestReadFailed {
            path: manifest_path.display().to_string(),
            source,
        }
    })?;
    let mut document: DocumentMut =
        source_manifest
            .parse()
            .map_err(
                |source: toml_edit::TomlError| ConfigError::ProjectManifestParseFailed {
                    path: manifest_path.display().to_string(),
                    details: source.to_string(),
                },
            )?;

    // Step 1: Drop the project's own `[workspace]` tables. A standalone project
    // declares itself a workspace root. Inside the engine workspace that table
    // makes Cargo report "multiple workspace roots found in the same workspace"
    // and fail every rebuild. Removing the top-level table also removes
    // `[workspace.dependencies]` and every other nested workspace table.
    document.remove("workspace");

    // Step 2: Give the generated member its own package name.
    //
    // Keeping the project's name would make two packages in one workspace
    // claim it: this member, and the project crate itself for any build that
    // links the project directly. Cargo rejects that with "package collision
    // in the lockfile", and a statically linked shipping binary has to link
    // the project directly - that is the whole point of it.
    //
    // The `[lib] name` set below keeps the built artifact called
    // `project.dll`, so nothing downstream of the build changes; only the
    // package name cargo selects does. The build command is generated from
    // the same prefix, so the two cannot drift.
    let member_package_name = format!("{HOST_PROJECT_MEMBER_PREFIX}{package_name}");
    if let Some(package) = document.get_mut("package").and_then(Item::as_table_mut) {
        package.insert("name", value(member_package_name.as_str()));
    }

    // Step 3: Resolve every `path` dependency against the real project
    // directory so the generated manifest works from any location. Forward
    // slashes keep the values valid TOML strings on Windows.
    let mut rewritten_paths: Vec<(String, String)> = Vec::new();
    for_each_dependency_table_mut(document.as_table_mut(), &mut |table| {
        for (dependency, item) in table.iter_mut() {
            if let Some(resolved_path) = resolve_dependency_path(project_root.as_path(), item) {
                rewritten_paths.push((dependency.get().to_string(), resolved_path));
            }
        }
    });

    // Step 3a: Refuse to write a member Cargo cannot load. The generated member
    // is picked up by the `projects_generated/*` glob, so one path that does
    // not resolve stops Cargo from loading the workspace at all. That breaks
    // every build, test and lint in the repository, including the build that
    // would replace the member. Report the bad path instead, and remove any
    // member an earlier run left behind, so the workspace stays loadable
    // either way.
    if let Some((dependency, resolved_path)) = rewritten_paths
        .into_iter()
        .find(|(_, resolved_path)| !Path::new(resolved_path).exists())
    {
        let member_directory = workspace_root
            .join(PROJECT_MEMBER_DIRECTORY)
            .join(format!("{HOST_PROJECT_MEMBER_PREFIX}{package_name}"));
        let _ = std::fs::remove_dir_all(&member_directory);
        return Err(ConfigError::ProjectDependencyPathMissing {
            manifest_path: manifest_path.display().to_string(),
            dependency,
            resolved_path,
        });
    }

    // Step 3b: Point the generated member's library at the real source file so
    // the project compiles from its actual location, and pin the library name
    // so the artifact keeps the name the host looks for. A manifest without a
    // `[lib]` section is left alone; a project that builds a loadable library
    // always declares one.
    let lib_source = project_root.join("src").join("lib.rs");
    if let Some(library_table) = document.get_mut("lib").and_then(Item::as_table_mut) {
        if lib_source.is_file() && library_table.get("path").is_none() {
            library_table.insert("path", value(render_manifest_path(&lib_source)));
        }
        if library_table.get("name").is_none() {
            library_table.insert("name", value(package_name));
        }
        // Step 3c: Add an `rlib` artifact when the host can hot patch.
        //
        // A generated patch does `use <project>::*`, so it gets the same types
        // the running world holds, with the same layout and the same `TypeId`.
        // That needs the crate as an rlib. The artifact is added here rather
        // than in the project's own manifest because it slows every project
        // rebuild, and nobody should pay that for a development feature they
        // have not enabled. A `[lib]` without a `crate-type` list builds an
        // rlib by default.
        if let Some(crate_types) = library_table
            .get_mut("crate-type")
            .and_then(Item::as_array_mut)
        {
            if !crate_types
                .iter()
                .any(|entry| entry.as_str() == Some("rlib"))
            {
                crate_types.push("rlib");
            }
        }
    }

    let generated_manifest = document.to_string();

    // Step 4: Write the generated member, replacing any previous generation.
    //
    // When the generated content is unchanged the existing file is left
    // untouched. That avoids a pointless rewrite and keeps the file's
    // modification time stable for anything else watching the tree.
    let member_directory = workspace_root
        .join(PROJECT_MEMBER_DIRECTORY)
        .join(format!("{HOST_PROJECT_MEMBER_PREFIX}{package_name}"));
    std::fs::create_dir_all(&member_directory).map_err(|source| {
        ConfigError::HostProjectMemberCreationFailed {
            path: member_directory.display().to_string(),
            source,
        }
    })?;
    prune_stale_host_project_members(workspace_root, &member_directory);
    let member_manifest = member_directory.join("Cargo.toml");
    let content_changed = std::fs::read_to_string(&member_manifest)
        .map(|existing| existing != generated_manifest)
        .unwrap_or(true);
    if content_changed {
        std::fs::write(&member_manifest, generated_manifest).map_err(|source| {
            ConfigError::HostProjectMemberCreationFailed {
                path: member_manifest.display().to_string(),
                source,
            }
        })?;
    }

    Ok(())
}

/// Rewrites one dependency entry's `path` to an absolute location and returns
/// the resolved path.
///
/// Handles both shapes a dependency takes in a `Cargo.toml`: an inline table
/// and its own sub-table. An entry with no `path` (a registry dependency) is
/// left alone and returns `None`.
fn resolve_dependency_path(project_root: &Path, dependency_entry: &mut Item) -> Option<String> {
    match dependency_entry {
        Item::Value(Value::InlineTable(inline_table)) => {
            let relative_path = inline_table.get("path").and_then(Value::as_str)?;
            let resolved_path = absolute_manifest_path(project_root, relative_path);
            inline_table.insert("path", Value::from(resolved_path.as_str()));
            Some(resolved_path)
        }
        Item::Table(sub_table) => {
            let relative_path = sub_table.get("path").and_then(Item::as_str)?;
            let resolved_path = absolute_manifest_path(project_root, relative_path);
            sub_table.insert("path", value(resolved_path.as_str()));
            Some(resolved_path)
        }
        _ => None,
    }
}

/// Resolves one manifest `path` value against the project root and leaves an
/// already-absolute path as it is.
fn absolute_manifest_path(project_root: &Path, path: &str) -> String {
    if Path::new(path).is_absolute() {
        path.to_string()
    } else {
        render_manifest_path(&project_root.join(path))
    }
}

/// Turns a path into the text stored in a TOML string: lossy, with forward
/// slashes.
fn render_manifest_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Visits every dependency table a generated member must carry: the three
/// linking tables at the top level and the same three under each
/// `[target.<cfg>]` table.
fn for_each_dependency_table_mut(manifest_table: &mut Table, visit: &mut impl FnMut(&mut Table)) {
    for (section, item) in manifest_table.iter_mut() {
        match section.get() {
            "target" => {
                let Some(targets) = item.as_table_mut() else {
                    continue;
                };
                for (_, target) in targets.iter_mut() {
                    let Some(target_table) = target.as_table_mut() else {
                        continue;
                    };
                    for (section, item) in target_table.iter_mut() {
                        if !LINKING_DEPENDENCY_TABLES.contains(&section.get()) {
                            continue;
                        }
                        if let Some(table) = item.as_table_mut() {
                            visit(table);
                        }
                    }
                }
            }
            name if LINKING_DEPENDENCY_TABLES.contains(&name) => {
                if let Some(table) = item.as_table_mut() {
                    visit(table);
                }
            }
            _ => {}
        }
    }
}

/// Removes host-generated workspace members left behind by earlier runs.
///
/// Every directory under `projects_generated/` whose name carries
/// [`HOST_PROJECT_MEMBER_PREFIX`] was written by the host, so any one that is
/// not the member built now belongs to a project the host no longer runs.
/// Leaving it is not harmless: the workspace glob still picks it up as a
/// member, and its dependency paths point into the previous project's folder.
/// Once that folder moves or is deleted, Cargo cannot load the workspace, and
/// every build, test and lint in the repository stops.
///
/// Members written straight into `extensions/` are removed too; nothing globs
/// that location. Failures are ignored rather than returned: a member the
/// host cannot remove is a housekeeping problem, not a reason to refuse to
/// start.
fn prune_stale_host_project_members(workspace_root: &Path, kept_member_directory: &Path) {
    for directory in [PROJECT_MEMBER_DIRECTORY, EXTENSION_DIRECTORY] {
        let Ok(entries) = std::fs::read_dir(workspace_root.join(directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path == kept_member_directory || !path.is_dir() {
                continue;
            }
            let is_generated = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(HOST_PROJECT_MEMBER_PREFIX));
            if is_generated {
                let _ = std::fs::remove_dir_all(&path);
            }
        }
    }
}

/// Writes the wrapper workspace member that builds one extension crate's
/// loadable artifact.
///
/// The wrapper is a generated crate at the configuration's
/// [`ExtensionConfig::wrapper_directory`] - the `generated/` directory of the
/// extension folder or the editor folder holding the crate - found by the
/// workspace glob. Its `cdylib` is the artifact the host loads, staged and
/// addressed under the extension's own name. The extension stays a plain
/// library inside it, so one rlib serves the module build and every project
/// build.
///
/// The generated manifest enables every feature the extension declares except
/// `default`, so its copy matches the copy projects link. The generated
/// `lib.rs` expands the extension's entry-point macro, and for the renderer
/// module also its renderer ABI macro, because a crate linked into several
/// artifacts cannot define the `#[no_mangle]` symbols itself.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the member directory or either generated
/// file cannot be written.
fn materialize_host_module_wrapper(
    workspace_root: &Path,
    extension: &ExtensionConfig,
    is_renderer_module: bool,
) -> Result<(), ConfigError> {
    let name = extension.name.as_str();
    let member_name = extension.wrapper_library_name.as_str();
    let member_directory = workspace_root.join(&extension.wrapper_directory);
    let extension_directory = workspace_root.join(&extension.crate_directory);
    let engine_directory = workspace_root.join("engine").join("pill_engine");

    // Every feature the extension declares, except the `default` key. If the
    // wrapper enabled fewer features than a project's dependency edge, it
    // would compile a second copy of the extension with different features -
    // the collision the wrapper exists to prevent.
    let mut features =
        crate::build_runner::declared_features(workspace_root, name).unwrap_or_default();
    features.retain(|feature| feature != "default");
    features.sort();
    let feature_entry = if features.is_empty() {
        String::new()
    } else {
        let list = features
            .iter()
            .map(|feature| format!("\"{feature}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!(", features = [{list}]")
    };

    let manifest = format!(
        "# Generated by the host for the `{name}` extension; rewritten on every run.\n\
         [package]\n\
         name = \"{member_name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [lints]\n\
         workspace = true\n\
         \n\
         [lib]\n\
         name = \"{member_name}\"\n\
         crate-type = [\"cdylib\", \"rlib\"]\n\
         \n\
         [dependencies]\n\
         pill_engine = {{ path = \"{engine_path}\" }}\n\
         {name} = {{ path = \"{extension_path}\", default-features = false{feature_entry} }}\n",
        engine_path = render_manifest_path(&engine_directory),
        extension_path = render_manifest_path(&extension_directory),
    );

    let crate_name = name.replace('-', "_");
    let renderer_bullet = if is_renderer_module {
        format!(
            "//! - Also carries the renderer ABI (`pill_renderer_attach` and\n\
             //!   `pill_renderer_detach`) by expanding\n\
             //!   `{crate_name}::__pill_renderer_entry_points!()`.\n"
        )
    } else {
        String::new()
    };
    let renderer_entry = if is_renderer_module {
        format!("{crate_name}::__pill_renderer_entry_points!();\n")
    } else {
        String::new()
    };
    let source = format!(
        "//! Loadable-artifact wrapper for the `{name}` extension.\n\
         //!\n\
         //! # Responsibilities\n\
         //!\n\
         //! - Builds the cdylib the host loads as `{name}`, with the extension\n\
         //!   compiled inside it as a plain library.\n\
         //! - Carries the loadable-artifact entry points by expanding\n\
         //!   `{crate_name}::__pill_module_entry_points!()`.\n\
         {renderer_bullet}\
         //!\n\
         //! Generated by the host on startup; rewritten on every run, never\n\
         //! edited by hand.\n\
         \n\
         {crate_name}::__pill_module_entry_points!();\n\
         {renderer_entry}"
    );

    write_module_member(&member_directory, &manifest, &source)
}

/// The editor frontend a crate's manifest tags itself for, if any.
///
/// `editor_frontend = "<name>"` under `[package.metadata.pill]`. Text-scanned
/// rather than parsed, so the host gains no TOML dependency for one string.
fn declared_editor_frontend(directory: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(directory.join("Cargo.toml")).ok()?;
    let mut in_pill_metadata = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_pill_metadata = trimmed == "[package.metadata.pill]";
            continue;
        }
        if !in_pill_metadata {
            continue;
        }
        let Some(after_key) = trimmed.strip_prefix("editor_frontend") else {
            continue;
        };
        let tag_value = after_key.split_once('=')?.1.trim();
        return Some(tag_value.trim_matches('"').to_string());
    }
    None
}

/// Resolves the project's own editor crate, when it has one.
///
/// The crate is discovered by convention: `<project_path>/<package>_editor/`,
/// beside the project's `Cargo.toml`, the project-directory analogue of an
/// extension's `modules/editor` folder. A project without one has no editor
/// part and no error, and a crate tagged for another editor frontend is left
/// out the same way an extension's is.
///
/// The crate's generated wrapper is written here, because nothing else would
/// write it: it lives under `projects_generated/`, which the workspace glob
/// already finds.
///
/// # Errors
///
/// Returns [`ConfigError::RetiredEditorFrontend`] when the crate is tagged for
/// the retired `pill_ui` frontend, and
/// [`ConfigError::HostModuleMemberCreationFailed`] when the wrapper cannot be
/// written.
fn resolve_project_editor_crate(
    workspace_root: &Path,
    project_path: &str,
    package_name: &str,
    editor_frontend: &str,
) -> Result<Option<ExtensionConfig>, ConfigError> {
    let configuration = ExtensionConfig::project_editor(project_path, package_name);
    let directory = workspace_root
        .join(project_path)
        .join(configuration.name.as_str());
    if !directory.join("Cargo.toml").is_file() {
        return Ok(None);
    }
    match declared_editor_frontend(&directory) {
        Some(frontend_tag) if frontend_tag == RETIRED_EDITOR_FRONTEND => {
            return Err(ConfigError::RetiredEditorFrontend {
                name: configuration.name,
            });
        }
        Some(frontend_tag) if frontend_tag != editor_frontend => return Ok(None),
        _ => {}
    }
    materialize_project_editor_wrapper(workspace_root, project_path, package_name)?;
    Ok(Some(configuration))
}

/// Writes the wrapper member that builds a project's editor crate.
///
/// The wrapper is the `host_module_<package>_editor` member under
/// `projects_generated/`, found by the same workspace glob as the generated
/// project members, and its `cdylib` is the artifact the host loads. The
/// editor crate stays a plain library inside it.
///
/// # Errors
///
/// Returns a [`ConfigError`] when the member directory or either generated
/// file cannot be written.
fn materialize_project_editor_wrapper(
    workspace_root: &Path,
    project_path: &str,
    package_name: &str,
) -> Result<(), ConfigError> {
    let crate_name = format!("{package_name}{EDITOR_CRATE_SUFFIX}");
    let member_name = format!("{HOST_MODULE_MEMBER_PREFIX}{crate_name}");
    let member_directory = workspace_root
        .join(PROJECT_MEMBER_DIRECTORY)
        .join(&member_name);
    let crate_directory = workspace_root.join(project_path).join(&crate_name);
    let engine_directory = workspace_root.join("engine").join("pill_engine");
    let manifest = format!(
        "# Generated by the host for the `{crate_name}` project editor crate; \
         rewritten on every run.\n\
         [package]\n\
         name = \"{member_name}\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [lints]\n\
         workspace = true\n\
         \n\
         [lib]\n\
         name = \"{member_name}\"\n\
         crate-type = [\"cdylib\", \"rlib\"]\n\
         \n\
         [dependencies]\n\
         pill_engine = {{ path = \"{engine_path}\" }}\n\
         {crate_name} = {{ path = \"{crate_path}\" }}\n",
        engine_path = render_manifest_path(&engine_directory),
        crate_path = render_manifest_path(&crate_directory),
    );
    let source = format!(
        "//! Loadable-artifact wrapper for the `{crate_name}` project editor crate.\n\
         //!\n\
         //! # Responsibilities\n\
         //!\n\
         //! - Builds the cdylib the editor host loads as `{crate_name}`, with the\n\
         //!   editor crate compiled inside it as a plain library.\n\
         //! - Carries the loadable-artifact entry points by expanding\n\
         //!   `{crate_name}::__pill_module_entry_points!()`.\n\
         //!\n\
         //! Generated by the host on startup; rewritten on every run, never\n\
         //! edited by hand.\n\
         \n\
         {crate_name}::__pill_module_entry_points!();\n"
    );
    write_module_member(&member_directory, &manifest, &source)
}

/// Writes one generated wrapper member's `Cargo.toml` and `src/lib.rs`,
/// leaving a file whose content is unchanged untouched.
///
/// # Errors
///
/// Returns [`ConfigError::HostModuleMemberCreationFailed`] when a directory or
/// file cannot be written.
fn write_module_member(
    member_directory: &Path,
    manifest: &str,
    source: &str,
) -> Result<(), ConfigError> {
    for (relative_path, contents) in [("Cargo.toml", manifest), ("src/lib.rs", source)] {
        let path = member_directory.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                ConfigError::HostModuleMemberCreationFailed {
                    path: parent.display().to_string(),
                    source,
                }
            })?;
        }
        let unchanged = std::fs::read_to_string(&path)
            .map(|existing| existing == contents)
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        std::fs::write(&path, contents).map_err(|source| {
            ConfigError::HostModuleMemberCreationFailed {
                path: path.display().to_string(),
                source,
            }
        })?;
    }
    Ok(())
}

/// Removes project editor wrappers left behind by earlier runs.
///
/// Every `host_module_*` member under `projects_generated/` was written by
/// [`materialize_project_editor_wrapper`] for some project. Any one that is
/// not `kept` belongs to a project the host no longer runs, and the workspace
/// glob would still pick it up as a member whose dependency paths may no
/// longer resolve. With `None` - a game posture, which loads no project editor
/// crate - every one of them is stale.
///
/// Failures are ignored rather than returned: a member the host cannot remove
/// is a housekeeping problem, not a reason to refuse to start.
fn prune_stale_project_editor_wrappers(workspace_root: &Path, kept: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(workspace_root.join(PROJECT_MEMBER_DIRECTORY)) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let wraps_a_crate = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(HOST_MODULE_MEMBER_PREFIX));
        if !wraps_a_crate || !path.is_dir() || kept == Some(path.as_path()) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&path);
    }
}

/// Removes generated module wrappers left behind by earlier runs.
///
/// Every directory in an extension folder's `generated/`, or in the editor
/// folder's, whose name carries [`HOST_MODULE_MEMBER_PREFIX`] was written by
/// [`materialize_host_module_wrapper`]. Any one that is not in
/// `kept_wrappers` belongs to a crate this run does not load (another
/// project's module, the editor's modules under a game host), or to a crate
/// that moved to another folder. The workspace glob would still pick it up as
/// a member whose dependency paths may not resolve, and Cargo then fails to
/// load the workspace at all. A wrapper written straight into `extensions/`
/// (an older layout) is removed too.
///
/// Failures are ignored rather than returned: a stale member the host cannot
/// remove is a housekeeping problem, not a reason to refuse to start.
fn prune_stale_host_module_wrappers(workspace_root: &Path, kept_wrappers: &[PathBuf]) {
    let is_wrapper = |path: &Path| {
        path.is_dir()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(HOST_MODULE_MEMBER_PREFIX))
    };
    // Every `generated/` folder a wrapper can live in: each extension
    // folder's, then the editor folder's.
    let mut generated_directories = Vec::new();
    if let Ok(folders) = std::fs::read_dir(workspace_root.join(EXTENSION_DIRECTORY)) {
        for folder_entry in folders.flatten() {
            let folder_path = folder_entry.path();
            // A wrapper an older host left directly in `extensions/`.
            if is_wrapper(&folder_path) {
                let _ = std::fs::remove_dir_all(&folder_path);
                continue;
            }
            generated_directories.push(folder_path.join(GENERATED_DIRECTORY));
        }
    }
    generated_directories.push(
        workspace_root
            .join(EDITOR_DIRECTORY)
            .join(GENERATED_DIRECTORY),
    );
    for generated_directory in generated_directories {
        let Ok(wrappers) = std::fs::read_dir(&generated_directory) else {
            continue;
        };
        for wrapper_entry in wrappers.flatten() {
            let wrapper_path = wrapper_entry.path();
            if is_wrapper(&wrapper_path) && !kept_wrappers.contains(&wrapper_path) {
                let _ = std::fs::remove_dir_all(&wrapper_path);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A native project manifest that links the extension directly.
    const DEPENDENT_MANIFEST: &str = r#"
[package]
name = "project"

[dependencies]
pill_engine = { path = "../../modules/engine/pill_engine" }
pill_spline = { path = "../../modules/extensions/pill_spline/modules/runtime" }
"#;

    /// A native project manifest with no extension dependency.
    const INDEPENDENT_MANIFEST: &str = r#"
[package]
name = "project"

[dependencies]
pill_engine = { path = "../../modules/engine/pill_engine" }
serde = { version = "1", features = ["derive"] }
"#;

    /// A native `ProjectModuleConfig` for dependency-check tests.
    fn native_config(manifest_path: Option<&str>) -> ProjectModuleConfig {
        ProjectModuleConfig {
            name: "project".to_string(),
            manifest_path: manifest_path.map(str::to_string),
            watch_directory: "project/src".to_string(),
            build_command: vec!["cargo".to_string(), "build".to_string()],
            build_environment: Vec::new(),
            backend: ProjectModuleBackend::NativeLibrary {
                library_name: "project".to_string(),
                output_subdirectory: "target/debug".to_string(),
            },
        }
    }

    /// A managed `ProjectModuleConfig`; it carries no Rust manifest path.
    fn csharp_config() -> ProjectModuleConfig {
        ProjectModuleConfig {
            name: "project_cs".to_string(),
            manifest_path: None,
            watch_directory: "project_cs/src".to_string(),
            build_command: vec!["dotnet".to_string(), "build".to_string()],
            build_environment: Vec::new(),
            backend: ProjectModuleBackend::CSharp(CSharpModuleConfig::new(
                "csharp_runtime",
                "csharp/pill_csharp_runtime/bin/Release/net8.0",
                "project_cs",
                "project_cs/bin/Release/net8.0",
            )),
        }
    }

    /// Root of this test run's temporary files, unique per process so parallel
    /// test binaries never collide.
    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("pill_host_dependency_test_{}", std::process::id()))
    }

    /// Write a manifest into a subdirectory and return its path.
    ///
    /// Each call starts from a clean subdirectory and tests remove only their
    /// own subdirectory when done, so parallel tests never delete a manifest
    /// another test is still reading.
    fn write_manifest(subdirectory: &str, contents: &str) -> PathBuf {
        let directory = temp_root().join(subdirectory);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Cargo.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    // =========================================================================
    // project settings file
    // =========================================================================

    /// A project settings file yields its module list in order.
    #[test]
    fn project_settings_file_lists_modules() {
        let directory = temp_root().join("project_settings_parse");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("project_settings.yaml"),
            "name: \"Bouncing Balls\"\nbuild_binary_name: \"BouncingBalls\"\nmodules:\n  - \"pill_spline\"\n  - \"pill_test\"\n",
        )
        .unwrap();
        let settings = read_project_settings_file(&directory)
            .unwrap()
            .expect("the settings file should parse");
        assert_eq!(settings.name.as_deref(), Some("Bouncing Balls"));
        assert_eq!(settings.build_binary_name.as_deref(), Some("BouncingBalls"));
        assert_eq!(settings.modules, vec!["pill_spline", "pill_test"]);
    }

    /// The `startup_scene:` key reads a relative `.scene` path and refuses
    /// anything that would leave the asset directory.
    #[test]
    fn the_startup_scene_key_reads_a_relative_scene_path() {
        let settings_directory = temp_root().join("project_settings_startup_scene");
        let settings_path = settings_directory.join("project_settings.yaml");

        let accepted =
            HostConfig::validate_startup_scene(Some("scenes/start.scene"), &settings_path)
                .expect("a relative scene path is accepted");
        assert_eq!(accepted.as_deref(), Some("scenes/start.scene"));
        assert_eq!(
            HostConfig::validate_startup_scene(None, &settings_path).unwrap(),
            None
        );
        assert_eq!(
            HostConfig::validate_startup_scene(Some("   "), &settings_path).unwrap(),
            None,
            "a blank value means no startup scene"
        );

        for refused in ["scenes/start.txt", "/scenes/start.scene", "..\\start.scene"] {
            assert!(
                HostConfig::validate_startup_scene(Some(refused), &settings_path).is_err(),
                "`{refused}` must be refused"
            );
        }
    }

    /// The `logging:` section is optional, reads levels by name, and names a
    /// level it cannot read.
    #[test]
    fn the_logging_section_reads_levels_and_refuses_bad_ones() {
        use tracing::level_filters::LevelFilter;

        let directory = temp_root().join("project_settings_logging");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let header = "name: \"Logging\"\nbuild_binary_name: \"Logging\"\n";
        let read = |extra: &str| {
            std::fs::write(
                directory.join("project_settings.yaml"),
                format!("{header}{extra}"),
            )
            .unwrap();
            read_project_settings_file(&directory).unwrap().unwrap()
        };
        let path = directory.join("project_settings.yaml");

        let absent = read("").logging.validate(&path).unwrap();
        assert_eq!(absent, pill_player::LoggingSettings::default());

        let set = read("logging:\n  level: warn\n  targets:\n    engine::rendering: Info\n")
            .logging
            .validate(&path)
            .unwrap();
        assert_eq!(set.level, Some(LevelFilter::WARN));
        assert_eq!(
            set.targets,
            vec![("engine::rendering".to_owned(), LevelFilter::INFO)]
        );

        let error = read("logging:\n  targets:\n    wgpu: loud\n")
            .logging
            .validate(&path)
            .expect_err("an unknown level is refused");
        assert!(error.to_string().contains("loud"), "{error}");
        // The message names the target whose level was rejected. A shipping
        // bundle reports a bad target with the same text, because both builds
        // use one conversion.
        assert!(error.to_string().contains("wgpu: "), "{error}");

        let time_only = read("logging:\n  timestamp: time\n")
            .logging
            .validate(&path)
            .unwrap();
        assert_eq!(
            time_only.timestamp,
            Some(pill_core::telemetry::TimestampFormat::Time)
        );
        let located = read("logging:\n  source_location: true\n")
            .logging
            .validate(&path)
            .unwrap();
        assert_eq!(located.source_location, Some(true));
        let error = read("logging:\n  timestamp: unix\n")
            .logging
            .validate(&path)
            .expect_err("an unknown timestamp format is refused");
        assert!(error.to_string().contains("unix"), "{error}");

        std::fs::write(
            directory.join("project_settings.yaml"),
            format!("{header}logging:\n  levels: info\n"),
        )
        .unwrap();
        assert!(
            read_project_settings_file(&directory).is_err(),
            "a misspelled key is refused"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The `assets:` section is optional, defaults every key to off, and
    /// refuses a key it does not know.
    #[test]
    fn the_assets_section_defaults_off_and_reads_scan_on_start() {
        let directory = temp_root().join("project_settings_assets");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let header = "name: \"Assets\"\nbuild_binary_name: \"Assets\"\n";
        let write = |extra: &str| {
            std::fs::write(
                directory.join("project_settings.yaml"),
                format!("{header}{extra}"),
            )
            .unwrap();
        };

        write("");
        let absent = read_project_settings_file(&directory).unwrap().unwrap();
        assert!(!absent.assets.scan_on_start);

        write("assets:\n  scan_on_start: true\n");
        let enabled = read_project_settings_file(&directory).unwrap().unwrap();
        assert!(enabled.assets.scan_on_start);

        write("assets:\n  scan_on_startup: true\n");
        let error =
            read_project_settings_file(&directory).expect_err("a misspelled key is refused");
        assert!(error.to_string().contains("scan_on_startup"), "{error}");
    }

    /// A missing project settings file reads as absent, not as an error.
    #[test]
    fn missing_project_settings_file_selects_no_modules() {
        let directory = temp_root().join("project_settings_missing");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        assert!(read_project_settings_file(&directory).unwrap().is_none());
    }

    // =========================================================================
    // host configuration identity
    // =========================================================================

    /// The backend kind is the spelling both startup lines print.
    #[test]
    fn the_backend_kind_is_spelled_as_the_startup_lines_print_it() {
        assert_eq!(native_config(None).backend.kind_name(), "native");
        assert_eq!(csharp_config().backend.kind_name(), "csharp");
    }

    /// The project directory a start reports is the joined path, canonical
    /// once it exists so `..` segments never reach the log.
    #[test]
    fn the_project_directory_is_canonicalized_when_it_exists() {
        let workspace = temp_root().join("project_directory");
        let existing = workspace.join("project");
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(&existing).unwrap();

        assert_eq!(
            resolved_project_directory(&workspace, "project"),
            canonicalize_plain(&existing).expect("the created directory canonicalizes")
        );

        let missing = resolved_project_directory(&workspace, "missing_project");
        assert!(missing.is_absolute());
        assert!(missing.ends_with("missing_project"));

        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// A configuration built from a module alone has no project directory to
    /// report.
    #[test]
    fn a_module_only_configuration_has_no_project_directory() {
        let host_config = HostConfig::from(native_config(None));
        assert_eq!(host_config.name, "project");
        assert!(host_config.project_directory.is_none());
    }

    // =========================================================================
    // editor settings file
    // =========================================================================

    /// The editor's settings file is optional and names a theme. A misspelled
    /// key is refused, because the theme key is the only one the editor reads.
    #[test]
    fn the_editor_settings_file_reads_the_theme_key() {
        let directory = temp_root().join("editor_settings");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();

        let absent = read_editor_settings(&directory).expect("no file is not an error");
        assert_eq!(absent, EditorSettings::default());
        assert_eq!(absent.editor_theme, None);

        std::fs::write(
            directory.join("editor_settings.yaml"),
            "editor_theme: \"editor/alt.theme\"\n",
        )
        .unwrap();
        let named_settings = read_editor_settings(&directory).expect("the key parses");
        assert_eq!(
            named_settings.editor_theme.as_deref(),
            Some("editor/alt.theme")
        );

        std::fs::write(directory.join("editor_settings.yaml"), "editor_theme:\n").unwrap();
        assert_eq!(
            read_editor_settings(&directory).unwrap().editor_theme,
            None,
            "an empty value is no selection"
        );

        std::fs::write(
            directory.join("editor_settings.yaml"),
            "editor_themes: \"editor/alt.theme\"\n",
        )
        .unwrap();
        let error = read_editor_settings(&directory).expect_err("a misspelled key is refused");
        assert!(error.contains("editor_themes"), "{error}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The project layer falls back to `editor/editor.theme` only when the
    /// project ships that file, and a named path always wins over the default.
    #[test]
    fn the_editor_theme_resolves_the_default_file() {
        let directory = temp_root().join("editor_settings_default");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(directory.join("res").join("editor")).unwrap();
        let settings = EditorSettings::default();

        assert_eq!(
            resolve_editor_theme(&directory, &settings),
            None,
            "a project without the file has no project layer"
        );

        std::fs::write(
            directory.join("res").join("editor").join("editor.theme"),
            "{}",
        )
        .unwrap();
        assert_eq!(
            resolve_editor_theme(&directory, &settings).as_deref(),
            Some("editor/editor.theme")
        );

        let named_settings = EditorSettings {
            editor_theme: Some("editor/alt.theme".to_string()),
            focus_game_on_play: true,
        };
        assert_eq!(
            resolve_editor_theme(&directory, &named_settings).as_deref(),
            Some("editor/alt.theme"),
            "the key names the layer even when the default file exists"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// An `extensions/` directory holding the named runtime crates, each in
    /// the `modules/runtime` folder of a folder of its own name.
    fn extensions_root_with(name: &str, crates: &[&str]) -> PathBuf {
        let root = temp_root().join(name);
        let _ = std::fs::remove_dir_all(&root);
        for crate_name in crates {
            seed_runtime_crate(&root, crate_name, crate_name);
        }
        root
    }

    /// Creates `<extensions_root>/<folder>/modules/runtime/Cargo.toml`.
    fn seed_runtime_crate(extensions_root: &Path, folder: &str, crate_name: &str) {
        seed_crate_at(extensions_root, folder, "modules/runtime", crate_name);
    }

    /// Creates `<extensions_root>/<folder>/modules/editor/Cargo.toml`.
    fn seed_editor_crate(extensions_root: &Path, folder: &str, crate_name: &str) {
        seed_crate_at(extensions_root, folder, "modules/editor", crate_name);
    }

    /// Creates `<extensions_root>/<folder>/<directory_in_folder>/Cargo.toml`
    /// declaring `crate_name`.
    fn seed_crate_at(
        extensions_root: &Path,
        folder: &str,
        directory_in_folder: &str,
        crate_name: &str,
    ) {
        let directory = extensions_root.join(folder).join(directory_in_folder);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("Cargo.toml"),
            format!("[package]\nname = \"{crate_name}\"\n"),
        )
        .unwrap();
    }

    /// The folder that holds the crate `package_name`, as the index reports it.
    fn folder_holding(extensions_root: &Path, package_name: &str) -> Option<String> {
        find_extension_crate(extensions_root, package_name).map(|location| location.folder)
    }

    /// A crate is found in the folder of its own name first, and otherwise in
    /// whichever folder holds it - the master renderer's folder holds its data
    /// crate too. Bookkeeping folders (`_...`) are never searched.
    #[test]
    fn finds_crates_in_their_extension_folders() {
        let root = temp_root().join("find_extension_crate");
        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(
            &root,
            "pill_master_renderer",
            "modules/runtime/gpu",
            "pill_master_renderer",
        );
        seed_crate_at(
            &root,
            "pill_master_renderer",
            "modules/runtime/data",
            "pill_master_renderer_data",
        );
        seed_crate_at(&root, "_workspace_anchor", "modules/runtime", "pill_hidden");

        assert_eq!(
            folder_holding(&root, "pill_master_renderer").as_deref(),
            Some("pill_master_renderer")
        );
        assert_eq!(
            folder_holding(&root, "pill_master_renderer_data").as_deref(),
            Some("pill_master_renderer"),
            "a second crate in a folder is found by scanning the folders"
        );
        let data_location =
            find_extension_crate(&root, "pill_master_renderer_data").expect("the data crate");
        assert_eq!(data_location.directory_in_folder, "modules/runtime/data");
        assert_eq!(folder_holding(&root, "pill_hidden"), None);
        assert_eq!(folder_holding(&root, "pill_absent"), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The index names a crate by the package its manifest declares, and every
    /// path the host derives follows the directory the crate sits in - here the
    /// renderer's folder, which names only one of its two crates. The generated
    /// wrappers and the examples are not crates of the extension.
    #[test]
    fn a_crate_is_found_by_its_package_name_whatever_its_directory() {
        let root = temp_root().join("package_name_not_directory");
        let _ = std::fs::remove_dir_all(&root);
        let write_crate = |directory_in_folder: &str, package_name: &str| {
            let directory = root.join("pill_master_renderer").join(directory_in_folder);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("Cargo.toml"),
                format!("[package]\nname = \"{package_name}\"\n"),
            )
            .unwrap();
        };
        write_crate("modules/runtime/gpu", "pill_master_renderer");
        write_crate("modules/runtime/data", "pill_master_renderer_data");
        write_crate(
            "generated/host_module_pill_master_renderer",
            "host_module_pill_master_renderer",
        );
        std::fs::create_dir_all(
            root.join("pill_master_renderer")
                .join("examples")
                .join("demo"),
        )
        .unwrap();

        let gpu = find_extension_crate(&root, "pill_master_renderer").expect("the gpu crate");
        assert_eq!(gpu.folder, "pill_master_renderer");
        assert_eq!(gpu.directory_in_folder, "modules/runtime/gpu");
        assert!(!gpu.is_editor_crate);
        let configuration = ExtensionConfig::workspace_member(&gpu);
        assert_eq!(configuration.name, "pill_master_renderer");
        assert_eq!(
            configuration.crate_directory,
            "extensions/pill_master_renderer/modules/runtime/gpu"
        );
        assert_eq!(
            configuration.watch_directory,
            "extensions/pill_master_renderer/modules/runtime/gpu/src"
        );

        let data =
            find_extension_crate(&root, "pill_master_renderer_data").expect("the data crate");
        assert_eq!(data.directory_in_folder, "modules/runtime/data");
        assert!(
            !data.is_editor_crate,
            "a runtime folder's child crate is not an editor crate"
        );
        seed_editor_crate(&root, "pill_widget", "pill_widget_editor");
        let editor = find_extension_crate(&root, "pill_widget_editor").expect("the editor crate");
        assert_eq!(editor.directory_in_folder, "modules/editor");
        assert!(
            editor.is_editor_crate,
            "the crate in `modules/editor` is the folder's editor crate"
        );
        assert_eq!(
            find_extension_crate(&root, "host_module_pill_master_renderer"),
            None,
            "a generated wrapper is not an extension crate"
        );
        assert_eq!(
            extension_crate_locations(&root).len(),
            3,
            "the two runtime crates and the editor crate are indexed"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The editor posture's crates, in load order: each loaded folder's editor
    /// crate after the runtime crates, then every editor-only folder. A folder
    /// without an editor crate contributes nothing, and an unloaded folder's
    /// editor crate stays out.
    #[test]
    fn resolves_editor_crates_by_convention() {
        let root = temp_root().join("editor_crates");
        let _ = std::fs::remove_dir_all(&root);
        seed_runtime_crate(&root, "pill_spline", "pill_spline");
        seed_editor_crate(&root, "pill_spline", "pill_spline_editor");
        seed_runtime_crate(&root, "pill_audio", "pill_audio");
        seed_runtime_crate(&root, "pill_test", "pill_test");
        seed_editor_crate(&root, "pill_test", "pill_test_editor");
        seed_editor_crate(&root, "pill_gadget_shell", "pill_gadget_shell_editor");
        seed_editor_crate(&root, "_workspace_anchor", "_workspace_anchor_editor");

        let (resolved, editor_only) = HostConfig::resolve_editor_crates(
            &["pill_audio".to_string(), "pill_spline".to_string()],
            &root,
            "legacy",
        )
        .expect("a seeded tree resolves");
        let names: Vec<&str> = resolved
            .iter()
            .map(|crate_config| crate_config.name.as_str())
            .collect();
        assert_eq!(names, ["pill_spline_editor", "pill_gadget_shell_editor"]);
        assert_eq!(
            editor_only,
            ["pill_gadget_shell_editor"],
            "only a folder whose editor crate is its only crate is optional at first load"
        );
        assert!(resolved
            .iter()
            .all(|crate_config| crate_config.is_editor_crate));
        assert_eq!(
            resolved[0].watch_directory,
            "extensions/pill_spline/modules/editor/src"
        );

        let (none, none_optional) =
            HostConfig::resolve_editor_crates(&["pill_audio".to_string()], &root, "legacy")
                .expect("a seeded tree resolves");
        assert_eq!(
            none.iter()
                .map(|crate_config| crate_config.name.as_str())
                .collect::<Vec<_>>(),
            ["pill_gadget_shell_editor"],
            "only the editor-only folder loads when no loaded folder has an editor crate"
        );
        assert_eq!(none_optional, ["pill_gadget_shell_editor"]);

        // A crate tagged for another editor frontend stays out of this
        // frontend's editor crates and loads under its own.
        let tagged = root
            .join("pill_gadget_shell")
            .join("modules")
            .join("editor")
            .join("Cargo.toml");
        let mut manifest = std::fs::read_to_string(&tagged).expect("seeded manifest");
        manifest.push_str("\n[package.metadata.pill]\neditor_frontend = \"egui\"\n");
        std::fs::write(&tagged, manifest).expect("tag the manifest");
        let (untagged_editor_crates, untagged_optional_names) =
            HostConfig::resolve_editor_crates(&["pill_audio".to_string()], &root, "legacy")
                .expect("a seeded tree resolves");
        assert!(untagged_editor_crates.is_empty());
        assert!(untagged_optional_names.is_empty());
        let (tagged_editor_crates, tagged_optional_names) =
            HostConfig::resolve_editor_crates(&["pill_audio".to_string()], &root, "egui")
                .expect("a seeded tree resolves");
        assert_eq!(
            tagged_editor_crates
                .iter()
                .map(|crate_config| crate_config.name.as_str())
                .collect::<Vec<_>>(),
            ["pill_gadget_shell_editor"],
            "the tagged crate loads under its own frontend"
        );
        assert_eq!(tagged_optional_names, ["pill_gadget_shell_editor"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A crate still tagged for the `pill_ui` editor is refused. No host runs
    /// that frontend, so skipping the crate would silently drop its editor
    /// part.
    #[test]
    fn refuses_the_retired_editor_frontend_tag() {
        let root = temp_root().join("retired_frontend");
        let _ = std::fs::remove_dir_all(&root);
        seed_runtime_crate(&root, "pill_spline", "pill_spline");
        seed_editor_crate(&root, "pill_spline", "pill_spline_editor");
        let manifest_path = root
            .join("pill_spline")
            .join("modules")
            .join("editor")
            .join("Cargo.toml");
        let mut manifest = std::fs::read_to_string(&manifest_path).expect("seeded manifest");
        manifest.push_str("\n[package.metadata.pill]\neditor_frontend = \"pill_ui\"\n");
        std::fs::write(&manifest_path, manifest).expect("tag the manifest");

        let refused = HostConfig::resolve_editor_crates(
            &["pill_spline".to_string()],
            &root,
            DEFAULT_EDITOR_FRONTEND,
        );
        assert!(
            matches!(&refused, Err(ConfigError::RetiredEditorFrontend { name }) if name == "pill_spline_editor"),
            "{refused:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An editor crate named in `modules:` is refused. It loads with its
    /// extension in an editor host, never on its own.
    #[test]
    fn refuses_editor_crates_in_the_module_list() {
        let root = temp_root().join("editor_crate_listed");
        let _ = std::fs::remove_dir_all(&root);
        seed_editor_crate(&root, "pill_spline", "pill_spline_editor");
        let refused = HostConfig::resolve_extensions(&[String::from("pill_spline_editor")], &root);
        assert!(
            matches!(&refused, Err(ConfigError::EditorCrateListed { name }) if name == "pill_spline_editor"),
            "{refused:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A runtime crate is never mistaken for an editor crate, and neither is
    /// another folder's editor crate.
    #[test]
    fn only_the_folders_editor_crate_is_flagged() {
        let root = temp_root().join("editor_crate_flag");
        let _ = std::fs::remove_dir_all(&root);
        seed_runtime_crate(&root, "pill_spline", "pill_spline");
        seed_editor_crate(&root, "pill_spline", "pill_spline_editor");
        seed_runtime_crate(&root, "pill_other", "pill_other_editor");
        let is_editor_crate = |package_name: &str| {
            let location = find_extension_crate(&root, package_name).expect("a seeded crate");
            ExtensionConfig::workspace_member(&location).is_editor_crate
        };

        assert!(is_editor_crate("pill_spline_editor"));
        assert!(!is_editor_crate("pill_spline"));
        assert!(!is_editor_crate("pill_other_editor"));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// An editor crate must carry the `_editor` suffix and a runtime crate
    /// must not, and each crate must be named after the folder that holds it:
    /// every mismatch is refused by name, so a crate cannot load as an editor
    /// part by location while `modules:` selects it as a module, and a path
    /// cannot stop naming the crate it holds.
    #[test]
    fn refuses_a_crate_whose_name_disagrees_with_its_folder() {
        let root = temp_root().join("crate_name_mismatch");
        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(&root, "pill_widget", "modules/editor", "pill_widget");
        let editor_mismatch = validate_extension_crate_names(&root);
        assert!(
            matches!(
                &editor_mismatch,
                Err(ConfigError::EditorCrateNameMismatch { name }) if name == "pill_widget"
            ),
            "{editor_mismatch:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(
            &root,
            "pill_widget",
            "modules/runtime",
            "pill_widget_editor",
        );
        let runtime_mismatch = validate_extension_crate_names(&root);
        assert!(
            matches!(
                &runtime_mismatch,
                Err(ConfigError::RuntimeCrateNameMismatch { name }) if name == "pill_widget_editor"
            ),
            "{runtime_mismatch:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(&root, "pill_widget", "modules/runtime", "pill_gadget");
        let runtime_folder_mismatch = validate_extension_crate_names(&root);
        assert!(
            matches!(
                &runtime_folder_mismatch,
                Err(ConfigError::ExtensionFolderNameMismatch { folder, name })
                    if name == "pill_gadget" && folder == "extensions/pill_widget"
            ),
            "{runtime_folder_mismatch:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(&root, "pill_widget", "modules/editor", "pill_gadget_editor");
        let editor_folder_mismatch = validate_extension_crate_names(&root);
        assert!(
            matches!(
                &editor_folder_mismatch,
                Err(ConfigError::ExtensionFolderNameMismatch { folder, name })
                    if name == "pill_gadget_editor" && folder == "extensions/pill_widget"
            ),
            "{editor_folder_mismatch:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
        seed_runtime_crate(&root, "pill_widget", "pill_widget");
        seed_editor_crate(&root, "pill_widget", "pill_widget_editor");
        assert!(
            validate_extension_crate_names(&root).is_ok(),
            "a folder that spells its crates is accepted"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The renderer's data crate may share the GPU module's folder: nested
    /// runtime crates are exempt from the folder-names-the-crate rule, because
    /// one folder cannot name both.
    #[test]
    fn a_renderer_with_both_crates_in_one_folder_resolves() {
        let root = temp_root().join("renderer_one_folder");
        let _ = std::fs::remove_dir_all(&root);
        seed_crate_at(
            &root,
            "pill_master_renderer",
            "modules/runtime/gpu",
            "pill_master_renderer",
        );
        seed_crate_at(
            &root,
            "pill_master_renderer",
            "modules/runtime/data",
            "pill_master_renderer_data",
        );
        assert_eq!(
            HostConfig::resolve_renderer(None, &root)
                .unwrap()
                .as_deref(),
            Some("pill_master_renderer")
        );
        assert!(
            validate_extension_crate_names(&root).is_ok(),
            "the renderer's nested crates are exempt from the folder rule"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No `renderer:` key selects the master renderer, whose two crates must
    /// both exist.
    #[test]
    fn an_absent_renderer_key_selects_the_master_renderer() {
        let root = extensions_root_with(
            "renderer_default",
            &["pill_master_renderer", "pill_master_renderer_data"],
        );
        assert_eq!(
            HostConfig::resolve_renderer(None, &root)
                .unwrap()
                .as_deref(),
            Some("pill_master_renderer")
        );
    }

    /// `renderer: none` selects no renderer, and needs no renderer crates.
    #[test]
    fn renderer_none_selects_no_renderer() {
        let root = extensions_root_with("renderer_none", &[]);
        assert_eq!(
            HostConfig::resolve_renderer(Some("none"), &root).unwrap(),
            None
        );
    }

    /// A renderer whose data crate is missing is refused, naming that crate.
    #[test]
    fn a_renderer_without_its_data_crate_is_refused() {
        let root = extensions_root_with("renderer_no_data", &["other_renderer"]);
        let error = HostConfig::resolve_renderer(Some("other_renderer"), &root).unwrap_err();
        assert!(
            matches!(&error, ConfigError::ExtensionDirectoryMissing { name } if name == "other_renderer_data"),
            "{error:?}"
        );
    }

    /// The renderer's data crate loads first, and once even when the settings
    /// list it too; without a renderer the list is unchanged.
    #[test]
    fn the_renderer_data_crate_loads_first_and_once() {
        let listed = vec![
            "pill_spline".to_string(),
            "pill_master_renderer_data".to_string(),
            "pill_audio".to_string(),
        ];
        assert_eq!(
            HostConfig::module_names_with_renderer_data(Some("pill_master_renderer"), &listed),
            ["pill_master_renderer_data", "pill_spline", "pill_audio"]
        );
        assert_eq!(
            HostConfig::module_names_with_renderer_data(None, &listed),
            listed
        );
    }

    /// A misspelled key is an error rather than a silently different module
    /// set.
    #[test]
    fn unknown_settings_key_is_rejected() {
        let directory = temp_root().join("project_settings_unknown_key");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("project_settings.yaml"),
            "name: \"Bouncing Balls\"\nbuild_binary_name: \"BouncingBalls\"\nmodule:\n  - \"pill_spline\"\n",
        )
        .unwrap();

        let error =
            read_project_settings_file(&directory).expect_err("an unknown key must be refused");
        let message = error.to_string();
        assert!(
            message.contains("module"),
            "the refusal names the offending key: {message}"
        );
    }

    /// Extension names are checked before they become a watch path and a
    /// cargo selector.
    ///
    /// `workspace_member` places the name into both without checking it. Left
    /// unchecked, `../pill_spline` would point the watcher outside
    /// `extensions/` and hand cargo a malformed `--package`. The test drives
    /// the same resolution `from_environment` runs, so it needs no
    /// process-wide working directory.
    #[test]
    fn rejects_traversal_and_duplicate_module_names() {
        let root = temp_root().join("extension_names");
        let _ = std::fs::remove_dir_all(&root);
        let extensions_root = root.join("extensions");
        seed_runtime_crate(&extensions_root, "pill_spline", "pill_spline");

        let traversal =
            HostConfig::resolve_extensions(&[String::from("../pill_spline")], &extensions_root);
        assert!(
            matches!(&traversal, Err(ConfigError::InvalidExtensionName { .. })),
            "a traversal entry must be refused: {traversal:?}"
        );

        let duplicate = HostConfig::resolve_extensions(
            &[String::from("pill_spline"), String::from("pill_spline")],
            &extensions_root,
        );
        assert!(
            matches!(&duplicate, Err(ConfigError::DuplicateExtensionName { .. })),
            "a repeated entry must be refused: {duplicate:?}"
        );

        let missing =
            HostConfig::resolve_extensions(&[String::from("pill_absent")], &extensions_root);
        assert!(
            matches!(&missing, Err(ConfigError::ExtensionDirectoryMissing { .. })),
            "a name without a sibling directory must be refused: {missing:?}"
        );

        let resolved =
            HostConfig::resolve_extensions(&[String::from("pill_spline")], &extensions_root)
                .expect("a real sibling directory resolves");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "pill_spline");
        assert_eq!(resolved[0].folder, "pill_spline");
        assert_eq!(
            resolved[0].watch_directory, "extensions/pill_spline/modules/runtime/src",
            "the validated name still derives the documented watch path"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every host-spawned build emits the same common flag sequence, in the
    /// same order, with its package selections up front.
    #[test]
    fn host_cargo_command_emits_the_common_flag_sequence() {
        let command = host_cargo_command(&["first".to_string(), "second".to_string()]);
        let expected: Vec<String> = [
            "cargo",
            "build",
            "--package",
            "first",
            "--package",
            "second",
            "--offline",
            "--profile",
            host_profile_name(),
        ]
        .iter()
        .map(|part| part.to_string())
        .collect();
        assert_eq!(command[..expected.len()], expected[..]);
        assert_eq!(
            command.contains(&"--timings".to_string()),
            cargo_timings_enabled(),
            "the timings flag follows the same gate the analytics collector reads"
        );
        assert_eq!(
            &command[command.len() - 2..],
            ["--features", "pill_engine/development"],
            "the development feature is package-qualified onto the engine and comes last"
        );
    }

    // =========================================================================
    // manifest_depends_on_crate
    // =========================================================================

    /// A direct dependency declared as an inline table is detected.
    #[test]
    fn direct_dependency_is_detected() {
        assert!(manifest_depends_on_crate(DEPENDENT_MANIFEST, "pill_spline"));
    }

    /// A dependency renamed through `package = "..."` is detected.
    #[test]
    fn renamed_dependency_is_detected() {
        let manifest = r#"
[dependencies]
spline_path = { package = "pill_spline", path = "../../modules/extensions/pill_spline/modules/runtime" }
"#;
        assert!(manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A dependency with a quoted key (renamed to a non-identifier) is detected
    /// through its `package` value.
    #[test]
    fn quoted_key_rename_is_detected() {
        let manifest = r#"
[dependencies]
"spline-path" = { package = "pill_spline", path = "../../modules/extensions/pill_spline/modules/runtime" }
"#;
        assert!(manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// Dev and build dependencies also link the crate into the build.
    #[test]
    fn dev_and_build_dependencies_are_detected() {
        let manifest = r#"
[dev-dependencies]
pill_spline = { path = "../../modules/extensions/pill_spline/modules/runtime" }

[build-dependencies]
pill_spline = { path = "../../modules/extensions/pill_spline/modules/runtime" }
"#;
        assert!(manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A target-specific dependency table is detected.
    #[test]
    fn target_specific_dependency_is_detected() {
        let manifest = r#"
[target.'cfg(windows)'.dependencies]
pill_spline = { path = "../../modules/extensions/pill_spline/modules/runtime" }
"#;
        assert!(manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A dependency written as its own sub-table is detected.
    #[test]
    fn sub_table_dependency_is_detected() {
        let manifest = r#"
[dependencies.pill_spline]
path = "../../modules/extensions/pill_spline/modules/runtime"
"#;
        assert!(manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A manifest that never mentions the crate reports no dependency.
    #[test]
    fn missing_dependency_is_not_detected() {
        assert!(!manifest_depends_on_crate(
            INDEPENDENT_MANIFEST,
            "pill_spline"
        ));
    }

    /// A crate listed only under `[workspace.dependencies]` is shared
    /// infrastructure, not something this project links directly.
    #[test]
    fn workspace_shared_dependency_is_not_a_project_dependency() {
        let manifest = r#"
[workspace.dependencies]
pill_spline = { path = "../../modules/extensions/pill_spline/modules/runtime" }

[dependencies]
pill_engine = { path = "../../modules/engine/pill_engine" }
"#;
        assert!(!manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A crate name appearing outside any dependency section is not a link.
    #[test]
    fn same_name_outside_dependency_sections_is_ignored() {
        let manifest = r#"
[package]
name = "pill_spline"

[features]
pill_spline = []

[dependencies]
pill_engine = { path = "../../modules/engine/pill_engine" }
"#;
        assert!(!manifest_depends_on_crate(manifest, "pill_spline"));
    }

    /// A longer crate name sharing the prefix must not match.
    #[test]
    fn shared_prefix_does_not_match() {
        let manifest = r#"
[dependencies]
pill_spline_extra = { path = "../../modules/extensions/spline_extra/modules/runtime" }
"#;
        assert!(!manifest_depends_on_crate(manifest, "pill_spline"));
    }

    // =========================================================================
    // project_depends_on_crate
    // =========================================================================

    /// The project manifest is read from disk and a direct dependency found.
    #[test]
    fn reads_the_project_manifest() {
        let manifest = write_manifest("dependent", DEPENDENT_MANIFEST);
        let project = native_config(Some(manifest.to_str().unwrap()));
        assert!(project_depends_on_crate(
            Path::new("."),
            &project,
            "pill_spline"
        ));
        let _ = std::fs::remove_dir_all(temp_root().join("dependent"));
    }

    /// A project that does not depend on the module is not triggered.
    #[test]
    fn independent_manifest_is_not_triggered() {
        let manifest = write_manifest("independent", INDEPENDENT_MANIFEST);
        let project = native_config(Some(manifest.to_str().unwrap()));
        assert!(!project_depends_on_crate(
            Path::new("."),
            &project,
            "pill_spline"
        ));
        let _ = std::fs::remove_dir_all(temp_root().join("independent"));
    }

    /// A managed project is never triggered, even with a manifest path set,
    /// because it cannot link a Rust extension crate.
    #[test]
    fn managed_project_is_never_triggered() {
        let manifest = write_manifest("managed", "# csproj contents\n");
        let mut project = csharp_config();
        project.manifest_path = Some(manifest.to_string_lossy().into_owned());
        assert!(!project_depends_on_crate(
            Path::new("."),
            &project,
            "pill_spline"
        ));
        let _ = std::fs::remove_dir_all(temp_root().join("managed"));
    }

    /// A missing manifest counts as "no dependency" rather than an error.
    #[test]
    fn missing_manifest_file_is_not_an_error() {
        let project = native_config(Some("does/not/exist/Cargo.toml"));
        assert!(!project_depends_on_crate(
            Path::new("."),
            &project,
            "pill_spline"
        ));
    }

    /// A config without a manifest path counts as "no dependency".
    #[test]
    fn missing_manifest_path_is_not_an_error() {
        let project = native_config(None);
        assert!(!project_depends_on_crate(
            Path::new("."),
            &project,
            "pill_spline"
        ));
    }
}

#[cfg(test)]
mod profile_tests {
    use super::{
        host_profile_name, host_target_directory, profile_name_for_directory, profile_uses_lto,
    };

    /// The one profile whose cargo name and target directory differ.
    #[test]
    fn the_default_profile_is_named_dev_but_builds_into_debug() {
        assert_eq!(profile_name_for_directory("debug"), "dev");
    }

    /// Only the release family enables LTO, which is what forces spawned builds
    /// to drop `-C prefer-dynamic`. Dev-like launcher profiles (for example
    /// the dioxus CLI's `desktop-dev`) must keep it so modules stay dynamic.
    #[test]
    fn only_the_release_family_uses_lto() {
        for lto_profile in ["release", "release-fast", "release-with-debug"] {
            assert!(
                profile_uses_lto(lto_profile),
                "{lto_profile} must be treated as an LTO profile"
            );
        }
        for dynamic_profile in ["dev", "desktop-dev", "shipping", "test", "bench"] {
            assert!(
                !profile_uses_lto(dynamic_profile),
                "{dynamic_profile} must keep `-C prefer-dynamic`"
            );
        }
    }

    /// Every other profile's directory is its own name, including custom ones.
    #[test]
    fn every_other_profile_directory_is_its_own_name() {
        for directory in ["release", "release-fast", "release-with-debug", "shipping"] {
            assert_eq!(
                profile_name_for_directory(directory),
                directory,
                "{directory} should map to itself"
            );
        }
    }

    /// Whatever the build script recorded has to be usable as a cargo
    /// `--profile` value, or every module build this host runs fails.
    #[test]
    fn the_recorded_profile_is_a_usable_cargo_argument() {
        let name = host_profile_name();
        assert!(!name.is_empty(), "the profile name must not be empty");
        assert!(
            !name.starts_with('-'),
            "{name:?} would be parsed as a flag, not a profile"
        );
        assert!(
            !name.contains(char::is_whitespace),
            "{name:?} would split into two arguments"
        );
    }

    /// The output directory must be the workspace-relative `target/<profile>`
    /// cargo actually writes into, not an absolute or parent-relative path.
    #[test]
    fn the_target_directory_is_workspace_relative() {
        let directory = host_target_directory();
        assert!(
            directory.starts_with("target/"),
            "{directory:?} must live under the workspace target directory"
        );
        assert!(
            !directory.contains(".."),
            "{directory:?} must not escape it"
        );
    }

    /// Under `cargo test` the host is a debug build, which is the one case
    /// where the two vocabularies differ - so this pins the pairing end to end.
    #[test]
    fn the_name_and_the_directory_describe_the_same_profile() {
        let expected_name = profile_name_for_directory(
            host_target_directory()
                .strip_prefix("target/")
                .expect("the directory is workspace-relative"),
        )
        .to_string();
        assert_eq!(host_profile_name(), expected_name);
    }
}

#[cfg(test)]
mod host_project_member_validation_tests {
    use super::{
        materialize_host_project_member, EXTENSION_DIRECTORY, HOST_PROJECT_MEMBER_PREFIX,
        PROJECT_MEMBER_DIRECTORY,
    };
    use pill_core::error::ConfigError;

    use crate::scratch::ScratchDirectory;

    /// A project manifest whose one path dependency is filled in per test, so
    /// each case differs only in whether that path resolves.
    fn manifest_with_dependency(dependency_path: &str) -> String {
        format!(
            "[package]\nname = \"project\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\npill_engine = {{ path = \"{dependency_path}\" }}\n"
        )
    }

    /// Lays out a workspace with a project at `project/` and returns its root.
    ///
    /// The project directory is a sibling of `extensions/`, matching the real
    /// layout closely enough that relative paths behave the same way.
    fn workspace(test_name: &str, dependency_path: &str) -> ScratchDirectory {
        let root = ScratchDirectory::new(test_name);
        std::fs::create_dir_all(root.join(EXTENSION_DIRECTORY)).unwrap();
        std::fs::create_dir_all(root.join("project").join("src")).unwrap();
        std::fs::write(
            root.join("project").join("Cargo.toml"),
            manifest_with_dependency(dependency_path),
        )
        .unwrap();
        std::fs::write(root.join("project").join("src").join("lib.rs"), "").unwrap();
        root
    }

    /// Path of the member `materialize_host_project_member` would generate.
    fn member_directory(root: &std::path::Path) -> std::path::PathBuf {
        root.join(PROJECT_MEMBER_DIRECTORY)
            .join(format!("{HOST_PROJECT_MEMBER_PREFIX}project"))
    }

    #[test]
    fn writes_the_member_when_every_path_resolves() {
        let root = workspace("resolves", "../real_dependency");
        std::fs::create_dir_all(root.join("real_dependency")).unwrap();

        materialize_host_project_member(&root, "project", "project").unwrap();

        assert!(member_directory(&root).join("Cargo.toml").is_file());
    }

    #[test]
    fn refuses_to_write_a_member_with_an_unresolvable_path() {
        let root = workspace("unresolvable", "../../modules/engine/pill_engine");

        let error = materialize_host_project_member(&root, "project", "project")
            .expect_err("a dependency path that does not exist must be reported");

        match error {
            ConfigError::ProjectDependencyPathMissing {
                ref dependency,
                ref resolved_path,
                ..
            } => {
                assert_eq!(dependency, "pill_engine");
                assert!(
                    resolved_path.contains("pill_engine"),
                    "the message must name the path that failed, got {resolved_path}"
                );
            }
            other => panic!("expected ProjectDependencyPathMissing, got {other:?}"),
        }
        assert!(
            !member_directory(&root).join("Cargo.toml").exists(),
            "a member Cargo cannot load must never reach the workspace"
        );
    }

    #[test]
    fn clears_a_member_an_earlier_run_left_behind() {
        // The failure this guards against: a member written when the paths did
        // resolve, then invalidated by the project moving. Cargo then refuses
        // to load the workspace, so the build that would rewrite the member
        // cannot run - the host has to clear it on the way out.
        let root = workspace("clears_stale", "../gone");
        let stale = member_directory(&root);
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("Cargo.toml"), "[package]\nname = \"stale\"\n").unwrap();

        let _ = materialize_host_project_member(&root, "project", "project")
            .expect_err("the dependency path does not exist");

        assert!(!stale.exists(), "the unloadable member must be removed");
    }

    #[test]
    fn drops_the_projects_workspace_tables() {
        let root = workspace("workspace_tables", "../real_dependency");
        std::fs::create_dir_all(root.join("real_dependency")).unwrap();
        // A standalone project declares itself a workspace root; the generated
        // member must not carry those tables, or Cargo reports multiple
        // workspace roots.
        let manifest = format!(
            "{}\n[workspace.dependencies]\nshared = {{ version = \"1\" }}\n\n[workspace]\n",
            manifest_with_dependency("../real_dependency")
        );
        std::fs::write(root.join("project").join("Cargo.toml"), manifest).unwrap();

        materialize_host_project_member(&root, "project", "project").unwrap();

        let generated =
            std::fs::read_to_string(member_directory(&root).join("Cargo.toml")).unwrap();
        assert!(
            !generated.contains("[workspace"),
            "workspace tables must be dropped: {generated}"
        );
        assert!(
            generated.contains(&format!("name = \"{HOST_PROJECT_MEMBER_PREFIX}project\"")),
            "the member carries its own package name: {generated}"
        );
    }

    #[test]
    fn reports_a_manifest_that_is_not_valid_toml() {
        let root = workspace("invalid_toml", "../real_dependency");
        std::fs::write(
            root.join("project").join("Cargo.toml"),
            "this is not = = toml",
        )
        .unwrap();

        let error = materialize_host_project_member(&root, "project", "project")
            .expect_err("a manifest that does not parse must be reported");

        assert!(
            matches!(error, ConfigError::ProjectManifestParseFailed { .. }),
            "expected ProjectManifestParseFailed, got {error:?}"
        );
    }
}

#[cfg(test)]
mod host_project_member_pruning_tests {
    use super::{prune_stale_host_project_members, EXTENSION_DIRECTORY, PROJECT_MEMBER_DIRECTORY};

    use crate::scratch::ScratchDirectory;

    /// Creates `<member_directory_name>/<member_name>/Cargo.toml` under `root`
    /// and returns its directory, so a test can assert on the directory rather
    /// than the file.
    fn seed_member_in(
        root: &std::path::Path,
        member_directory_name: &str,
        member_name: &str,
    ) -> std::path::PathBuf {
        let member_directory = root.join(member_directory_name).join(member_name);
        std::fs::create_dir_all(&member_directory).unwrap();
        std::fs::write(
            member_directory.join("Cargo.toml"),
            "[package]
name = \"x\"
",
        )
        .unwrap();
        member_directory
    }

    /// A generated project member under `projects_generated/`.
    fn seed_member(root: &std::path::Path, member_name: &str) -> std::path::PathBuf {
        seed_member_in(root, PROJECT_MEMBER_DIRECTORY, member_name)
    }

    /// Builds a temporary workspace root holding only `projects_generated/`,
    /// removed on drop.
    fn workspace(test_name: &str) -> ScratchDirectory {
        let root = ScratchDirectory::new(test_name);
        std::fs::create_dir_all(root.join(PROJECT_MEMBER_DIRECTORY)).unwrap();
        root
    }

    #[test]
    fn removes_a_member_left_by_a_previous_project() {
        let root = workspace("previous");
        let stale_member = seed_member(&root, "host_project_old");
        let legacy_member = seed_member_in(&root, EXTENSION_DIRECTORY, "host_project_flat");
        let kept_member = seed_member(&root, "host_project_current");

        prune_stale_host_project_members(&root, &kept_member);

        assert!(
            !stale_member.exists(),
            "the previous project's member must be gone"
        );
        assert!(
            !legacy_member.exists(),
            "a member an older host left flat in extensions/ must be gone"
        );
        assert!(
            kept_member.exists(),
            "the member being materialized must survive"
        );
    }

    #[test]
    fn leaves_real_extensions_alone() {
        let root = workspace("real_modules");
        let real_module = seed_member_in(&root, EXTENSION_DIRECTORY, "pill_spline");
        let kept_member = seed_member(&root, "host_project_current");

        prune_stale_host_project_members(&root, &kept_member);

        assert!(
            real_module.exists(),
            "only host-generated members carry the prefix; a real module must not be deleted"
        );
    }

    #[test]
    fn tolerates_a_missing_extensions_directory() {
        let root = ScratchDirectory::new("prune_missing");

        // This must not panic: a workspace with no `extensions/` folder yet is
        // valid, and refusing to start because of that would be worse than the
        // stale member this function exists to clean up.
        prune_stale_host_project_members(&root, &root.join("nothing"));
    }
}

#[cfg(test)]
mod hot_patch_manifest_tests {
    use super::{
        materialize_host_project_member, HOST_PROJECT_MEMBER_PREFIX, PROJECT_MEMBER_DIRECTORY,
    };

    use crate::scratch::ScratchDirectory;

    /// A hot-patching host needs the generated member to build an `rlib` too,
    /// or a patch has nothing to link the project's types from.
    #[test]
    fn generated_member_builds_an_rlib_for_hot_patching() {
        let root = ScratchDirectory::new("member_hot_patch_rlib");
        std::fs::create_dir_all(root.join("project").join("src")).unwrap();
        std::fs::write(
            root.join("project").join("Cargo.toml"),
            "[package]\nname = \"project\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\n",
        )
        .unwrap();
        std::fs::write(root.join("project").join("src").join("lib.rs"), "").unwrap();

        materialize_host_project_member(&root, "project", "project").unwrap();

        let generated = std::fs::read_to_string(
            root.join(PROJECT_MEMBER_DIRECTORY)
                .join(format!("{HOST_PROJECT_MEMBER_PREFIX}project"))
                .join("Cargo.toml"),
        )
        .unwrap();
        assert!(generated.contains("cdylib"), "{generated}");
        assert!(
            generated.contains("rlib"),
            "the member must build an rlib when hot patching: {generated}"
        );
    }
}

#[cfg(test)]
mod host_module_wrapper_pruning_tests {
    use super::{
        prune_stale_host_module_wrappers, EDITOR_DIRECTORY, EXTENSION_DIRECTORY,
        GENERATED_DIRECTORY,
    };

    use crate::scratch::ScratchDirectory;

    /// Creates `<parent>/generated/<wrapper>/Cargo.toml` under `root` and
    /// returns the wrapper's directory.
    fn seed_wrapper(
        root: &std::path::Path,
        parent: &std::path::Path,
        wrapper_name: &str,
    ) -> std::path::PathBuf {
        let directory = root
            .join(parent)
            .join(GENERATED_DIRECTORY)
            .join(wrapper_name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        directory
    }

    /// The extension folder `folder`, relative to the workspace root.
    fn extension_folder(folder: &str) -> std::path::PathBuf {
        std::path::Path::new(EXTENSION_DIRECTORY).join(folder)
    }

    /// A kept wrapper survives, in an extension folder and in the editor
    /// folder alike. A wrapper of a crate no longer loaded, one in a folder the
    /// crate no longer lives in, and one an older host left directly in
    /// `extensions/` are removed. Real crates and the anchor are never touched.
    #[test]
    fn keeps_only_the_wrappers_of_loaded_crates() {
        let root = ScratchDirectory::new("prune_wrappers");
        let kept_wrapper = seed_wrapper(
            &root,
            &extension_folder("pill_spline"),
            "host_module_pill_spline",
        );
        let unloaded_wrapper = seed_wrapper(
            &root,
            &extension_folder("pill_audio"),
            "host_module_pill_audio",
        );
        let moved_wrapper = seed_wrapper(
            &root,
            &extension_folder("pill_old_folder"),
            "host_module_pill_spline",
        );
        let kept_editor_wrapper = seed_wrapper(
            &root,
            std::path::Path::new(EDITOR_DIRECTORY),
            "host_module_pill_editor_shell",
        );
        let unloaded_editor_wrapper = seed_wrapper(
            &root,
            std::path::Path::new(EDITOR_DIRECTORY),
            "host_module_pill_editor_gadget",
        );
        let editor_anchor = root
            .join(EDITOR_DIRECTORY)
            .join(GENERATED_DIRECTORY)
            .join("README.md");
        std::fs::write(&editor_anchor, "anchor").unwrap();
        let flat_wrapper = root.join(EXTENSION_DIRECTORY).join("host_module_pill_test");
        std::fs::create_dir_all(&flat_wrapper).unwrap();
        let real_crate = root
            .join(EXTENSION_DIRECTORY)
            .join("pill_spline")
            .join("pill_spline");
        std::fs::create_dir_all(&real_crate).unwrap();

        prune_stale_host_module_wrappers(
            &root,
            &[kept_wrapper.clone(), kept_editor_wrapper.clone()],
        );

        assert!(
            kept_wrapper.exists(),
            "the loaded crate's wrapper must survive"
        );
        assert!(
            kept_editor_wrapper.exists(),
            "the loaded editor module's wrapper must survive"
        );
        assert!(
            !unloaded_wrapper.exists(),
            "a wrapper of an unloaded crate must be gone"
        );
        assert!(
            !unloaded_editor_wrapper.exists(),
            "a wrapper of an unloaded editor module must be gone"
        );
        assert!(
            !moved_wrapper.exists(),
            "a wrapper in the wrong folder must be gone"
        );
        assert!(
            !flat_wrapper.exists(),
            "a flat wrapper from an older host must be gone"
        );
        assert!(real_crate.exists(), "a real crate must never be removed");
        assert!(
            editor_anchor.exists(),
            "the tracked anchor of the editor's generated folder must stay"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod editor_module_tests {
    use super::{
        materialize_host_module_wrapper, HostConfig, EDITOR_DIRECTORY, GENERATED_DIRECTORY,
    };
    use pill_core::error::ConfigError;

    use crate::scratch::ScratchDirectory;

    /// Creates `editor/<directory_name>/Cargo.toml` under `root`, declaring
    /// `package_name` and, when given, the `editor_frontend` tag.
    fn seed_editor_folder_crate(
        root: &std::path::Path,
        directory_name: &str,
        package_name: &str,
        editor_frontend: Option<&str>,
    ) {
        let directory = root.join(EDITOR_DIRECTORY).join(directory_name);
        std::fs::create_dir_all(directory.join("src")).unwrap();
        let mut manifest = format!("[package]\nname = \"{package_name}\"\nversion = \"0.1.0\"\n");
        if let Some(tag) = editor_frontend {
            manifest.push_str(&format!(
                "\n[package.metadata.pill]\neditor_frontend = \"{tag}\"\n"
            ));
        }
        std::fs::write(directory.join("Cargo.toml"), manifest).unwrap();
    }

    /// Only the crates tagged for this host's frontend are editor modules, in
    /// directory name order: the untagged executables and core, a crate tagged
    /// for another frontend, and the wrappers' `generated/` folder stay out.
    #[test]
    fn resolves_the_tagged_crates_of_the_editor_folder() {
        let root = ScratchDirectory::new("editor_modules");
        seed_editor_folder_crate(&root, "pill_editor", "pill_editor", None);
        seed_editor_folder_crate(&root, "pill_editor_core", "pill_editor_core", None);
        seed_editor_folder_crate(
            &root,
            "pill_editor_shell",
            "pill_editor_shell",
            Some("egui"),
        );
        seed_editor_folder_crate(
            &root,
            "pill_editor_alpha",
            "pill_editor_alpha",
            Some("egui"),
        );
        seed_editor_folder_crate(
            &root,
            "pill_editor_other",
            "pill_editor_other",
            Some("other"),
        );
        seed_editor_folder_crate(
            &root,
            &format!("{GENERATED_DIRECTORY}/host_module_pill_editor_shell"),
            "host_module_pill_editor_shell",
            Some("egui"),
        );

        let modules = HostConfig::resolve_editor_modules(&root.join(EDITOR_DIRECTORY), "egui")
            .expect("a seeded editor folder resolves");

        let names: Vec<&str> = modules.iter().map(|module| module.name.as_str()).collect();
        assert_eq!(names, ["pill_editor_alpha", "pill_editor_shell"]);
        let shell = &modules[1];
        assert!(shell.is_editor_crate);
        assert_eq!(shell.folder, EDITOR_DIRECTORY);
        assert_eq!(shell.crate_directory, "editor/pill_editor_shell");
        assert_eq!(shell.watch_directory, "editor/pill_editor_shell/src");
        assert_eq!(shell.library_name, "pill_editor_shell");
        assert_eq!(shell.wrapper_library_name, "host_module_pill_editor_shell");
        assert_eq!(
            shell.wrapper_directory,
            "editor/generated/host_module_pill_editor_shell"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A workspace without an editor folder has no editor modules.
    #[test]
    fn a_missing_editor_folder_has_no_modules() {
        let root = ScratchDirectory::new("editor_modules_missing");

        let modules = HostConfig::resolve_editor_modules(&root.join(EDITOR_DIRECTORY), "egui")
            .expect("a missing folder is not an error");

        assert!(modules.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A crate in the editor folder still tagged for the `pill_ui` frontend is
    /// refused rather than skipped, like an extension's editor crate.
    #[test]
    fn refuses_the_retired_editor_frontend_tag() {
        let root = ScratchDirectory::new("editor_modules_retired");
        seed_editor_folder_crate(&root, "pill_editor_old", "pill_editor_old", Some("pill_ui"));

        let refused = HostConfig::resolve_editor_modules(&root.join(EDITOR_DIRECTORY), "egui");

        assert!(
            matches!(&refused, Err(ConfigError::RetiredEditorFrontend { name }) if name == "pill_editor_old"),
            "{refused:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The module's wrapper is written into the editor folder's `generated/`,
    /// linking the crate where it lives.
    #[test]
    fn writes_the_wrapper_into_the_editor_generated_folder() {
        let root = ScratchDirectory::new("editor_modules_wrapper");
        seed_editor_folder_crate(
            &root,
            "pill_editor_shell",
            "pill_editor_shell",
            Some("egui"),
        );
        let modules = HostConfig::resolve_editor_modules(&root.join(EDITOR_DIRECTORY), "egui")
            .expect("a seeded editor folder resolves");

        materialize_host_module_wrapper(&root, &modules[0], false).expect("the wrapper is written");

        let wrapper = root
            .join(EDITOR_DIRECTORY)
            .join(GENERATED_DIRECTORY)
            .join("host_module_pill_editor_shell");
        let manifest = std::fs::read_to_string(wrapper.join("Cargo.toml")).unwrap();
        assert!(
            manifest.contains("name = \"host_module_pill_editor_shell\""),
            "{manifest}"
        );
        assert!(
            manifest.contains("pill_editor_shell = { path ="),
            "the wrapper links the module by its path: {manifest}"
        );
        let source = std::fs::read_to_string(wrapper.join("src").join("lib.rs")).unwrap();
        assert!(
            source.contains("pill_editor_shell::__pill_module_entry_points!();"),
            "{source}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod project_editor_crate_tests {
    use super::{
        prune_stale_project_editor_wrappers, resolve_project_editor_crate,
        HOST_MODULE_MEMBER_PREFIX, PROJECT_MEMBER_DIRECTORY,
    };
    use pill_core::error::ConfigError;

    use crate::scratch::ScratchDirectory;

    /// Lays out a workspace with a project at `project/`, optionally with its
    /// editor crate at `project/project_editor/` and a frontend tag.
    fn workspace(test_name: &str, editor_tag: Option<&str>) -> ScratchDirectory {
        let root = ScratchDirectory::new(test_name);
        std::fs::create_dir_all(root.join("project").join("src")).unwrap();
        std::fs::write(
            root.join("project").join("Cargo.toml"),
            "[package]\nname = \"project\"\nversion = \"0.1.0\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
        )
        .unwrap();
        std::fs::write(root.join("project").join("src").join("lib.rs"), "").unwrap();
        if let Some(tag) = editor_tag {
            let crate_directory = root.join("project").join("project_editor");
            std::fs::create_dir_all(crate_directory.join("src")).unwrap();
            std::fs::write(
                crate_directory.join("Cargo.toml"),
                format!(
                    "[package]\nname = \"project_editor\"\nversion = \"0.1.0\"\n\n[package.metadata.pill]\neditor_frontend = \"{tag}\"\n"
                ),
            )
            .unwrap();
            std::fs::write(crate_directory.join("src").join("lib.rs"), "").unwrap();
        }
        root
    }

    /// The wrapper `resolve_project_editor_crate` writes for `project`.
    fn wrapper_directory(root: &std::path::Path) -> std::path::PathBuf {
        root.join(PROJECT_MEMBER_DIRECTORY)
            .join(format!("{HOST_MODULE_MEMBER_PREFIX}project_editor"))
    }

    #[test]
    fn an_untagged_editor_crate_is_resolved_and_wrapped() {
        let root = workspace("project_editor_resolved", Some(""));
        // A tag-less crate: write the manifest again without the metadata
        // table, which is what a project author writes by omitting the tag.
        std::fs::write(
            root.join("project")
                .join("project_editor")
                .join("Cargo.toml"),
            "[package]\nname = \"project_editor\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let configuration = resolve_project_editor_crate(&root, "project", "project", "egui")
            .unwrap()
            .expect("an untagged editor crate resolves under every frontend");

        assert_eq!(configuration.name, "project_editor");
        assert!(configuration.is_editor_crate);
        assert_eq!(
            configuration.watch_directory, "project/project_editor/src",
            "the crate is watched where it lives"
        );
        assert_eq!(
            configuration.wrapper_library_name,
            "host_module_project_editor"
        );
        let manifest =
            std::fs::read_to_string(wrapper_directory(&root).join("Cargo.toml")).unwrap();
        assert!(
            manifest.contains("name = \"host_module_project_editor\""),
            "{manifest}"
        );
        assert!(
            manifest.contains("project_editor = { path ="),
            "the wrapper links the editor crate by its resolved path: {manifest}"
        );
        assert!(wrapper_directory(&root)
            .join("src")
            .join("lib.rs")
            .is_file());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_without_an_editor_crate_has_none() {
        let root = workspace("project_editor_absent", None);

        let configuration = resolve_project_editor_crate(&root, "project", "project", "egui")
            .expect("a project without an editor part is the ordinary case");

        assert!(configuration.is_none());
        assert!(
            !wrapper_directory(&root).exists(),
            "no wrapper is written for a crate that does not exist"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_crate_tagged_for_another_frontend_is_skipped() {
        let root = workspace("project_editor_other_frontend", Some("some_other_editor"));

        let configuration = resolve_project_editor_crate(&root, "project", "project", "egui")
            .expect("another frontend's crate is not an error");

        assert!(
            configuration.is_none(),
            "a crate tagged for another frontend is left out"
        );
        assert!(!wrapper_directory(&root).exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_retired_frontend_tag_is_refused() {
        let root = workspace("project_editor_retired_frontend", Some("pill_ui"));

        let error = resolve_project_editor_crate(&root, "project", "project", "egui")
            .expect_err("the retired frontend tag is refused rather than skipped");

        match error {
            ConfigError::RetiredEditorFrontend { name } => assert_eq!(name, "project_editor"),
            other => panic!("expected the retired-frontend error, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_leftover_wrapper_is_pruned_and_the_kept_one_survives() {
        let root = workspace("project_editor_prune", Some(""));
        std::fs::write(
            root.join("project")
                .join("project_editor")
                .join("Cargo.toml"),
            "[package]\nname = \"project_editor\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        resolve_project_editor_crate(&root, "project", "project", "egui")
            .unwrap()
            .expect("the project's editor crate resolves");
        let kept = wrapper_directory(&root);
        assert!(kept.join("Cargo.toml").is_file());
        let stale = root
            .join(PROJECT_MEMBER_DIRECTORY)
            .join(format!("{HOST_MODULE_MEMBER_PREFIX}another_project_editor"));
        std::fs::create_dir_all(&stale).unwrap();
        std::fs::write(stale.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();

        prune_stale_project_editor_wrappers(&root, Some(kept.as_path()));
        assert!(
            kept.join("Cargo.toml").is_file(),
            "the wrapper of the project this host runs survives"
        );
        assert!(
            !stale.exists(),
            "a wrapper of a project the host no longer runs must be gone"
        );

        prune_stale_project_editor_wrappers(&root, None);
        assert!(
            !kept.exists(),
            "a game posture prunes every project editor wrapper"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
