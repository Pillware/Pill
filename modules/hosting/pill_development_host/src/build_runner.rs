//! Build execution, artifact staging and output-path resolution.
//!
//! Build output reaches the host's standard streams, so compiler progress and
//! diagnostics stay visible in the terminal that launched the host.
//!
//! # Responsibilities
//!
//! - Run each backend's build command from the workspace root.
//! - Find the artifact each backend is expected to produce.
//! - Copy built artifacts into a private hot-load directory and stamp them, so
//!   a later process can tell them from the output of another build.
//! - Check that an artifact exists before the host tries to load it.
//!
//! # Design
//!
//! [`run_build_command`] treats a build as an opaque process: it does not read
//! the compiler's output and decides success from the child's exit status
//! alone. Cancellation works by agreement: a newer source save increases a
//! generation counter, and the watchdog loop stops the child - and the
//! [`BuildProcessTree`] it started - when it sees the counter change. The
//! startup batch ([`build_extension_batch`]) builds every module with one cargo
//! command, and each module then takes its share through
//! [`take_batch_validation`].

// Standard library
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

// External crates
use pill_core::debug;
use pill_core::error::BuildError;
use pill_core::info;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

// Current crate
use crate::analytics::{self, ModuleKind};
use crate::{ExtensionConfig, ProjectModuleBackend, ProjectModuleConfig};

// =============================================================================
// Build output
// =============================================================================

/// The last lines the most recent build wrote to stderr, for the editor's
/// Problems panel.
///
/// It is process-global because `pill_development_host.dll` exists once per
/// process. Builds run one at a time - an in-flight patch defers reloads and
/// managed builds - so this ring buffer still holds that build's output when a
/// failure reads it.
static BUILD_OUTPUT: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// How many stderr lines the ring buffer keeps.
const BUILD_OUTPUT_LINES: usize = 400;

/// Resets the ring buffer for one build and writes its name as the first line.
fn begin_build_output(build_name: &str) {
    let mut ring = BUILD_OUTPUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ring.clear();
    ring.push_back(format!("--- build: {build_name}"));
}

/// Adds one stderr line to the ring buffer, dropping the oldest line once the
/// cap is reached.
fn push_build_output(line: &str) {
    let mut ring = BUILD_OUTPUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ring.push_back(line.to_string());
    while ring.len() > BUILD_OUTPUT_LINES {
        ring.pop_front();
    }
}

/// Returns everything in the ring buffer, oldest line first: what the last
/// build printed.
///
/// A failed build's notice carries this text, and the editor turns it into the
/// rows of the Problems panel.
pub(crate) fn recent_build_output() -> String {
    let ring = BUILD_OUTPUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ring.iter().cloned().collect::<Vec<String>>().join("\n")
}

/// Reads `child`'s piped stderr on its own thread and copies every line to the
/// host's stderr and into [`BUILD_OUTPUT`].
///
/// The caller joins the handle before reading the ring buffer, so a failed
/// build's diagnostics are complete by the time it reports them.
fn tee_stderr(child: &mut Child) -> Option<std::thread::JoinHandle<()>> {
    let stderr = child.stderr.take()?;
    std::thread::Builder::new()
        .name("pill-build-output".to_string())
        .spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let mut reader = BufReader::new(stderr);
            let mut line = Vec::new();
            // Read bytes instead of using `lines()`, because compiler output
            // is not always valid UTF-8.
            while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                let _ = std::io::stderr().write_all(&line);
                push_build_output(String::from_utf8_lossy(&line).trim_end());
                line.clear();
            }
            let _ = std::io::stderr().flush();
        })
        .ok()
}

// =============================================================================
// Constants
// =============================================================================

/// Longest time one module's build command may run.
///
/// A module builds incrementally in the private build tree, so this is enough
/// for one crate and the dependencies that changed.
const BUILD_TIMEOUT: Duration = Duration::from_secs(120);

/// Longest time the startup batch may run.
///
/// The batch builds every module, the native project and the renderer with one
/// cargo command, and on a fresh checkout the private build tree is empty. That
/// is the one case where a single command may legitimately take minutes. The
/// per-module limit would kill it, and the modules it had not finished would
/// then be built again one by one.
const STARTUP_BATCH_TIMEOUT: Duration = Duration::from_secs(600);

/// How often the watchdog looks for build completion or cancellation.
///
/// The interval is short on purpose: a module that needs no work finishes in a
/// few hundred milliseconds, so a coarse interval adds dead time to every
/// module load. A hundred milliseconds here means a hundred milliseconds per
/// module, multiplied by the number of modules. One check is a `try_wait` and
/// one memory sample, which is cheap enough to run this often, and the shorter
/// interval also makes a cancellation during a reload arrive sooner.
const WATCHDOG_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Private directory the host copies the project's loadable artifacts into.
/// It is also the output directory an extension's configuration defaults to.
///
/// This gives a project the same protection an extension's output has. Cargo
/// writes `project.dll` into a shared per-crate slot that any other
/// `cargo build` of the same package overwrites, possibly with a different
/// feature set and therefore a differently configured `pill_engine` compiled
/// into it. Loading such a DLL crashes with an access violation inside
/// `LoadLibrary`, before any of the host's own diagnostics can run.
pub(crate) const PROJECT_HOT_OUTPUT_SUBDIRECTORY: &str = "target/hot";

/// Directory holding one stamp per module, recording what the host built.
const ARTIFACT_STAMP_DIRECTORY: &str = "pill_standalone_temp/artifact_stamps";

/// The host's feature set, which module builds must reproduce.
///
/// When the host is rebuilt with a different feature set, every cached module
/// artifact becomes invalid. `development` is passed into every module and
/// project build, so it changes the engine each module links. `rendering` does
/// not change the module's own engine - the renderer lives outside
/// `pill_engine` - but it still belongs here: a windowed host loads the wgpu
/// renderer beside its engine dylibs, and a cached artifact is only trusted
/// for the posture it was built for.
const HOST_MODULE_FEATURE_SET: &str = if cfg!(feature = "rendering") {
    "rendering+development"
} else {
    "no-rendering+development"
};

/// The host's profiling level.
///
/// It is part of [`host_build_identity`] for the same reason as
/// [`HOST_MODULE_FEATURE_SET`]: it changes `pill_core`'s resolved features, and
/// every spawned build mirrors it through [`host_engine_features`].
const HOST_PROFILING_FEATURE_SET: &str = if cfg!(feature = "profiling-fine") {
    "profiling-fine"
} else if cfg!(feature = "profiling") {
    "profiling"
} else {
    "no-profiling"
};

/// The directory, relative to the workspace root, where a host-spawned build
/// writes the artifacts it just compiled.
///
/// Every cargo build the host spawns uses a `CARGO_TARGET_DIR` inside the
/// private module build tree ([`crate::config::MODULE_BUILD_TARGET_DIRECTORY`]).
/// It never writes to the shared `target/<profile>` that the running binary
/// maps its engine dylibs from: a GUI frontend's module build needs a
/// different engine variant than the host runs, and rewriting that shared slot
/// would make cargo delete a DLL the host has loaded. The artifacts are then
/// copied into the module's private hot-load directory.
fn cargo_module_output_subdirectory() -> String {
    crate::config::module_build_artifact_directory()
}

/// The host's build identity: feature set, cargo profile, target, and the
/// environment every spawned build runs with.
///
/// The profile and the target belong here for the same reason as the feature
/// set: both change the crate-metadata hash on both sides of the DLL boundary,
/// so an artifact built under one and loaded under the other cannot resolve its
/// exports. The environment decides whether module builds drop
/// `-C prefer-dynamic` and whether they mirror the dioxus CLI's
/// `RUSTC_WORKSPACE_WRAPPER`, and each of those changes every member crate's
/// metadata hash. Any mismatch shows up as `LoadLibrary` error 127, which names
/// no cause.
fn host_build_identity() -> String {
    let mut spawned_environment = crate::config::spawned_build_environment();
    // The pairs are compared as a set, so ordering never matters.
    spawned_environment.sort();
    let spawned_environment = spawned_environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{HOST_MODULE_FEATURE_SET}\n{HOST_PROFILING_FEATURE_SET}\nprofile={}\ntarget={}\nbuild_tree={}\nengine_features={}\nspawned_env={spawned_environment}",
        crate::config::host_profile_name(),
        crate::config::host_target_triple().unwrap_or("native"),
        crate::config::MODULE_BUILD_TARGET_DIRECTORY,
        host_engine_features().join(",")
    )
}

/// The engine features every host-spawned build must copy from the running
/// host.
///
/// `pill_core.dll` and `pill_engine_core.dll` load once per process, and their
/// exported names hash the features their dependency trees resolved. A module
/// or project compiled with a different engine feature set therefore imports
/// names the loaded copies do not export, and loading fails with
/// "The specified procedure could not be found" (os error 127). Every `cfg!`
/// name checked here is a declared `pill_development_host` feature, so each one
/// reports exactly the posture the running binary was built with.
fn host_engine_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    features.push("pill_engine/development");
    if cfg!(feature = "profiling-fine") {
        features.push("pill_engine/profiling-fine");
    } else if cfg!(feature = "profiling-verify") {
        features.push("pill_engine/profiling-verify");
    } else if cfg!(feature = "profiling") {
        features.push("pill_engine/profiling");
    }
    if cfg!(feature = "metrics") {
        features.push("pill_engine/metrics");
    }
    if cfg!(feature = "dev-logs") {
        features.push("pill_core/dev-logs");
    }
    features
}

// =============================================================================
// Build Identity and Artifact Stamps
// =============================================================================

/// The running host's toolchain version line followed by
/// [`host_build_identity`].
///
/// It is resolved once per process: running `rustc` for every stamp write would
/// cost more than the stamps save.
fn current_build_info() -> String {
    static CURRENT_BUILD_INFO: OnceLock<String> = OnceLock::new();
    CURRENT_BUILD_INFO
        .get_or_init(|| {
            // `rustc -vV` prints a version line first, for example
            // `rustc 1.95.0 (59807616e 2026-04-14)`, then a configuration
            // table. Cargo folds the same version into every crate's metadata
            // hash, so the first line is enough to notice a toolchain change
            // that would break symbol lookup at load.
            let rustc_version_line = Command::new("rustc")
                .arg("-vV")
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|stdout| stdout.lines().next().map(str::to_string))
                .unwrap_or_default();
            format!("{rustc_version_line}\n{}", host_build_identity())
        })
        .clone()
}

/// Path of the stamp recording what the host built for one module.
fn artifact_stamp_path(workspace_root: &Path, module_name: &str) -> PathBuf {
    workspace_root
        .join(ARTIFACT_STAMP_DIRECTORY)
        .join(format!("{module_name}.txt"))
}

/// Describes the artifacts a build produced, together with the host identity,
/// the sources and the command that produced them.
///
/// `build_identity` is [`current_build_info`] and `source_identity` the watch
/// directory. Both stay in the stamp because two hosts - or two projects sharing
/// a package name, an output path and a stamp file - would otherwise both
/// accept the same stamp, and one would load the other's artifact and fail
/// inside `LoadLibrary` with no explanation.
///
/// Returns `None` when an artifact is missing or unreadable. The callers treat
/// that as "not host-built", which means they run cargo.
fn artifact_stamp(
    build_identity: &str,
    source_identity: &str,
    build_command: &[String],
    artifacts: &[PathBuf],
) -> Option<String> {
    // A configured build command never contains the separator, so two
    // different argument lists cannot produce the same line.
    let mut lines = vec![
        build_identity.to_string(),
        source_identity.to_string(),
        build_command.join("\u{1}"),
    ];
    for path in artifacts {
        let metadata = std::fs::metadata(path).ok()?;
        let modified_since_epoch = metadata
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        lines.push(format!(
            "{}|{}|{}",
            path.display(),
            metadata.len(),
            modified_since_epoch.as_nanos()
        ));
    }
    Some(lines.join("\n"))
}

/// Records that the host itself produced these artifacts with this command.
///
/// The write is best-effort: if it fails, the stamp is missing, and the next
/// run rebuilds instead of trusting an artifact it cannot identify.
fn record_artifact_stamp(
    workspace_root: &Path,
    module_name: &str,
    source_identity: &str,
    build_command: &[String],
    artifacts: &[PathBuf],
) {
    let Some(stamp) = artifact_stamp(
        &current_build_info(),
        source_identity,
        build_command,
        artifacts,
    ) else {
        return;
    };
    let path = artifact_stamp_path(workspace_root, module_name);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, stamp);
}

/// Checks whether the artifacts on disk are the ones this host built with this
/// command.
///
/// A modification-time check alone cannot tell a host-built artifact from one
/// an unrelated `cargo build` wrote to the same path moments later: both are
/// newer than every source file. The recorded command, size and time close that
/// gap. The recorded `source_identity` closes the rest: package names are not
/// unique across projects, so two projects can build `project.dll` into the
/// same slot, and every timestamp then matches the wrong sources.
fn artifacts_are_host_built(
    workspace_root: &Path,
    module_name: &str,
    source_identity: &str,
    build_command: &[String],
    artifacts: &[PathBuf],
) -> bool {
    let Some(computed_stamp) = artifact_stamp(
        &current_build_info(),
        source_identity,
        build_command,
        artifacts,
    ) else {
        return false;
    };
    let recorded = std::fs::read_to_string(artifact_stamp_path(workspace_root, module_name));
    recorded.is_ok_and(|recorded_stamp| recorded_stamp == computed_stamp)
}

/// Confirms that a staged artifact set still matches the stamp just written for
/// it.
///
/// The build that produced these files has exited, but the staging paths are
/// shared with anything else that stages the same crate's artifacts, so a file
/// can be replaced between the copy and the load. When the stamp no longer
/// matches, the copy about to be loaded is not the one this host built, and the
/// build fails here instead of loading a mismatched artifact as if nothing had
/// changed.
///
/// # Errors
///
/// Returns [`BuildError::StagedArtifactChanged`] when the stamp no longer
/// matches the files on disk.
fn confirm_staged_artifacts(
    workspace_root: &Path,
    module_name: &str,
    watch_directory: &str,
    build_command: &[String],
    produced: &[PathBuf],
) -> Result<(), BuildError> {
    if artifacts_are_host_built(
        workspace_root,
        module_name,
        watch_directory,
        build_command,
        produced,
    ) {
        return Ok(());
    }
    Err(BuildError::StagedArtifactChanged {
        name: module_name.to_string(),
        path: produced
            .first()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
    })
}

// =============================================================================
// Free Functions
// =============================================================================

/// How many directory levels below the workspace root a member manifest can
/// sit. Five is the depth of
/// `extensions/pill_master_renderer/modules/runtime/data`, the deepest member
/// layout this search has to reach.
const MEMBER_SEARCH_DEPTH: usize = 5;

/// Applies the environment and argument overrides that every host-spawned
/// cargo build needs, whichever path starts it.
///
/// The full module-reload path and the per-function hot-patch path both run the
/// module's configured `cargo build`, and both must agree with the running host
/// binary:
///
/// - `CARGO_TARGET_DIR` points into the private module build tree, so cargo
///   never has to delete a DLL the host has mapped (see
///   [`crate::config::MODULE_BUILD_TARGET_DIRECTORY`]).
/// - The engine features ([`host_engine_features`]) are passed explicitly. When
///   the host was started by the dioxus CLI, or is an editor, they are instead
///   reproduced by anchoring the build to the running host's own package: the
///   shared dylibs' exported names hash their dependency trees' features, so a
///   module built with different features cannot resolve its imports against
///   them.
/// - A profile injected by the launcher (the dioxus CLI's `desktop-dev`) is not
///   declared in the module workspaces, so the spawned build defines it as a
///   profile that inherits `dev`. The profile name is part of cargo's metadata
///   hash. Built-in profiles need no definition.
///
/// Only cargo builds take these overrides; `dotnet` module builds ignore them.
pub(crate) fn apply_cargo_host_overrides(command: &mut Command, workspace_root: &Path) {
    command.env(
        "CARGO_TARGET_DIR",
        workspace_root.join(crate::config::MODULE_BUILD_TARGET_DIRECTORY),
    );
    let anchor_package = host_anchor_package();
    let is_editor_host = anchor_package.as_deref().is_some_and(is_editor_anchor);
    // The windowed standalone links winit and wgpu, whose feature unions also
    // reach the engine dylibs' symbol hashes.
    let is_rendering_standalone =
        cfg!(feature = "rendering") && anchor_package.as_deref() == Some("pill_standalone");
    if crate::config::running_under_dioxus_cli() || is_editor_host || is_rendering_standalone {
        // An editor host keeps the cargo anchor, and so does a host started by
        // the dioxus CLI. For an editor, the renderer dependencies change the
        // shared engine dylibs even though the project itself depends only on
        // renderer data. A reload builds one module while the startup batch
        // builds them all; without the anchor the two produce different feature
        // unions on `pill_core.dll`, and the reloaded artifact cannot import
        // against the running host. The anchor also reproduces the editor's
        // macro graph, whose proc-macro feature unions (`proc-macro2`, `quote`,
        // `syn`, the derive crates) reach the exported symbol hashes of
        // `pill_core` and `pill_engine_core`. Selecting the editor package is
        // the only way to reproduce that graph exactly; its unions cannot be
        // listed one by one.
        if let Some(anchor_package) = anchor_package {
            command.arg("--package").arg(&anchor_package);
            // Select the anchor with the same features the running host was
            // built with, not just the same package. The anchor's own defaults
            // are dropped first, because they change the metadata hash of every
            // crate the two share when the host was built without them.
            if let Some(declared_feature_names) = declared_features(workspace_root, &anchor_package)
            {
                command.arg("--no-default-features");
                for feature in host_posture_features() {
                    if declared_feature_names.iter().any(|name| name == feature) {
                        command
                            .arg("--features")
                            .arg(format!("{anchor_package}/{feature}"));
                    }
                }
            } else {
                // The anchor's manifest was not found, so keep its defaults
                // and add the features the host is known to need.
                for feature in host_posture_features() {
                    if feature != "dev" && feature != "development" {
                        command
                            .arg("--features")
                            .arg(format!("{anchor_package}/{feature}"));
                    }
                }
            }
        }
    } else {
        // Every other host receives its engine features explicitly, so the
        // spawned build resolves `pill_core.dll` and `pill_engine_core.dll`
        // exactly as the loaded host copies did, and nothing extra. See
        // [`host_engine_features`].
        for feature in host_engine_features() {
            command.arg("--features").arg(feature);
        }
    }
    // Mirror the host's own `--target` when a launcher (the dioxus CLI) built
    // it with one. Cargo folds the target into every crate's metadata hash,
    // so a module built natively against a `--target` host cannot resolve its
    // dynamic imports against the host's loaded engine dylibs (os error 127
    // at `LoadLibrary`). Cargo writes the mirrored build under
    // `<CARGO_TARGET_DIR>/<triple>/<profile>`, which
    // [`crate::config::module_build_artifact_directory`] already accounts for.
    if let Some(triple) = crate::config::host_target_triple() {
        command.arg("--target").arg(triple);
    }
    let profile = crate::config::host_profile_name();
    if !matches!(profile, "dev" | "release" | "test" | "bench") {
        command
            .arg("--config")
            .arg(format!("profile.{profile}.inherits=\"dev\""));
    }
}

/// Returns the running host's workspace package name, which an anchored build
/// selects with `-p`.
///
/// Cargo unifies features across every selected package, so an anchored module
/// build selects the host's own package (`-p editor` or `-p pill_editor` for an
/// editor) and forces the shared engine crates onto that host's exact feature
/// set. The package name is normally the executable's file stem, but the Dioxus
/// CLI (`dx`) stores the built binary under a name with a cargo metadata hash,
/// such as `editor-d6d95e94.exe`. The trailing `-<hex>` part is trimmed so the
/// anchor still names the real package.
fn host_anchor_package() -> Option<String> {
    let stem = std::env::current_exe()
        .ok()?
        .file_stem()?
        .to_str()?
        .to_owned();
    if let Some((package_name, hash_suffix)) = stem.rsplit_once('-') {
        let is_metadata_hash = !hash_suffix.is_empty()
            && hash_suffix.len() <= 16
            && hash_suffix
                .chars()
                .all(|character| character.is_ascii_hexdigit());
        if is_metadata_hash {
            return Some(package_name.to_owned());
        }
    }
    Some(stem)
}

/// Checks whether an executable's package is an editor anchor: the Dioxus
/// frontend (`editor`) or the egui editor (`pill_editor`).
///
/// Their dependency graphs carry the renderer and UI unions a reload build must
/// reproduce; a plain game host needs only the explicit engine features.
fn is_editor_anchor(package_name: &str) -> bool {
    matches!(package_name, "editor" | "pill_editor")
}

/// Returns the frontend features the running host was built with.
///
/// They pick the anchor's feature selection for a build that is anchored: an
/// editor host, or a host started by the dioxus CLI.
fn host_posture_features() -> Vec<&'static str> {
    let mut features = vec!["dev", "development"];
    if cfg!(feature = "rendering") {
        features.push("rendering");
    }
    if cfg!(feature = "profiling-fine") {
        features.push("profiling-fine");
    } else if cfg!(feature = "profiling") {
        features.push("profiling");
    }
    features
}

/// Returns the feature names the package declares, read from its manifest among
/// the workspace members up to [`MEMBER_SEARCH_DEPTH`] directories below
/// `workspace_root`, or `None` when no manifest names the package.
///
/// The result is cached per package, because the members do not change while a
/// host runs. The host's wrapper generation uses it to copy the extension's
/// declared features onto the wrapper's dependency edge.
pub(crate) fn declared_features(workspace_root: &Path, package_name: &str) -> Option<Vec<String>> {
    static DECLARED: OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Option<Vec<String>>>>,
    > = OnceLock::new();
    let mut cache = DECLARED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(package_name.to_string())
        .or_insert_with(|| read_declared_features(workspace_root, package_name))
        .clone()
}

/// Reads a package's declared features without caching them, for
/// [`declared_features`].
///
/// It first tries the manifest of the extension crate the package names
/// ([`crate::config::extension_crate_directory`]), then every `Cargo.toml` up
/// to [`MEMBER_SEARCH_DEPTH`] levels under `workspace_root`, and returns the
/// features of the first manifest whose package name matches.
fn read_declared_features(workspace_root: &Path, package_name: &str) -> Option<Vec<String>> {
    let mut manifests =
        vec![
            crate::config::extension_crate_directory(workspace_root, package_name)
                .join("Cargo.toml"),
        ];
    std::fs::read_dir(workspace_root).ok()?;
    collect_member_manifests(workspace_root, MEMBER_SEARCH_DEPTH, &mut manifests);
    manifests.into_iter().find_map(|manifest| {
        let text = std::fs::read_to_string(&manifest).ok()?;
        let document = text.parse::<toml_edit::DocumentMut>().ok()?;
        let declared_package_name = document.get("package")?.get("name")?.as_str()?;
        if declared_package_name != package_name {
            return None;
        }
        let features = document
            .get("features")
            .and_then(|item| item.as_table_like())
            .map(|table| table.iter().map(|(key, _)| key.to_string()).collect())
            .unwrap_or_default();
        Some(features)
    })
}

/// Adds the candidate `Cargo.toml` path of every directory below `directory`,
/// down to `depth` levels, in breadth-first order so shallower members come
/// first.
///
/// Cargo's `target` directory and hidden directories are skipped: neither holds
/// a workspace member, and `target` alone holds thousands of entries.
fn collect_member_manifests(directory: &Path, depth: usize, manifests: &mut Vec<PathBuf>) {
    let mut level = vec![directory.to_path_buf()];
    for _ in 0..depth {
        let mut next_level = Vec::new();
        for parent in &level {
            let Ok(entries) = std::fs::read_dir(parent) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !path.is_dir() || name == "target" || name.starts_with('.') {
                    continue;
                }
                manifests.push(path.join("Cargo.toml"));
                next_level.push(path);
            }
        }
        level = next_level;
    }
}

// =============================================================================
// Piggybacked flag capture
// =============================================================================

/// Harvests the module crate's `rustc` invocation out of a build the host was
/// going to run anyway.
///
/// A fast patch replays the exact compiler line cargo uses for the module, so
/// the patch links the identical dependency closure. Discovering that line
/// costs a `cargo build -v`, and cargo prints the invocation only when it
/// actually compiles - so an on-demand probe would have to touch the crate to
/// force a rebuild, once per reload, because a reload is what makes the cached
/// line stale. Taking the line from the build the host just ran makes the
/// discovery free.
///
/// `-v` is appended at spawn time only, never to the configured build command:
/// that command is part of the cache key and of the artifact stamp. Stderr is
/// piped so it can be scanned, and every line is written straight back out so
/// the console still streams - minus the `Running`, `Fresh` and `Dirty` lines
/// `-v` adds. Colour is requested explicitly when the host's own stderr is a
/// terminal, because cargo turns it off for a pipe.
struct VerboseCapture {
    /// The crate names to look for: the modules that one invocation builds.
    crate_names: Vec<String>,
    /// The workspace the build runs in. The flags cache lands under that
    /// workspace, not in a shared temporary directory.
    workspace_root: PathBuf,
    /// The reader thread and the lines it found, available once joined.
    reader: Option<std::thread::JoinHandle<Vec<(String, crate::hot_patch::CargoRustcLine)>>>,
}

impl VerboseCapture {
    /// Makes `command` a verbose build whose stderr is read through a pipe, for
    /// cargo only.
    ///
    /// Returns `None` for a build that is not cargo (the managed backend's
    /// `dotnet`): it has no rustc line to harvest and must keep its inherited
    /// streams. It also returns `None` when nothing asked for a capture.
    fn arm(
        command: &mut Command,
        program: &str,
        crate_names: &[&str],
        workspace_root: &Path,
    ) -> Option<Self> {
        if program != "cargo" || crate_names.is_empty() {
            return None;
        }
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            command.arg("--color").arg("always");
        }
        command.arg("-v").stderr(std::process::Stdio::piped());
        Some(Self {
            crate_names: crate_names.iter().map(|name| (*name).to_string()).collect(),
            workspace_root: workspace_root.to_path_buf(),
            reader: None,
        })
    }

    /// Starts reading the child's stderr on a thread.
    fn start(mut self, child: &mut std::process::Child) -> Self {
        let Some(stderr) = child.stderr.take() else {
            return self;
        };
        let crate_names = self.crate_names.clone();
        self.reader = std::thread::Builder::new()
            .name("pill-build-capture".to_string())
            .spawn(move || {
                use std::io::{BufRead, BufReader, Write};
                let mut found: Vec<(String, crate::hot_patch::CargoRustcLine)> = Vec::new();
                let mut reader = BufReader::new(stderr);
                let mut line = Vec::new();
                // Read bytes instead of using `lines()`: compiler output is not
                // always valid UTF-8, and a decode error must not cut the build
                // log short.
                while reader.read_until(b'\n', &mut line).unwrap_or(0) > 0 {
                    let text = String::from_utf8_lossy(&line);
                    if found.len() < crate_names.len() {
                        for name in &crate_names {
                            if found.iter().any(|(found_name, _)| found_name == name) {
                                continue;
                            }
                            if let Some(found_line) =
                                crate::hot_patch::parse_rustc_line(&text, name)
                            {
                                found.push((name.clone(), found_line));
                            }
                        }
                    }
                    if !is_verbose_only_line(&text) {
                        let _ = std::io::stderr().write_all(&line);
                        push_build_output(text.trim_end());
                    }
                    line.clear();
                }
                let _ = std::io::stderr().flush();
                found
            })
            .ok();
        self
    }

    /// Joins the reader and caches every line it found.
    ///
    /// A build that recompiled nothing prints no invocation. That is not a
    /// failure: the line cached when the crate was last compiled still
    /// describes it, and the patch pipeline's own freshness check decides
    /// whether that line still holds. `commands` pairs each requested name with
    /// the build command its flags cache records.
    fn finish(self, commands: &[(&str, &[String])]) {
        let Some(reader) = self.reader else {
            return;
        };
        let Ok(found) = reader.join() else {
            return;
        };
        for (name, line) in found {
            let Some((_, build_command)) = commands
                .iter()
                .find(|(command_name, _)| *command_name == name)
            else {
                continue;
            };
            let cache_path = crate::hot_patch::flags_cache_path(&self.workspace_root, &name);
            match line.save(&cache_path, build_command) {
                Ok(()) => debug!(
                    target: telemetry_target::HOT_RELOAD,
                    module = name.as_str(),
                    cache = %cache_path.display(),
                    "captured the patch compiler flags from this build"
                ),
                Err(error) => debug!(
                    target: telemetry_target::HOT_RELOAD,
                    module = name.as_str(),
                    error = %error,
                    "could not cache the patch compiler flags; the next patch will re-capture"
                ),
            }
        }
    }

    /// Joins the reader without caching anything.
    ///
    /// A failed build's stderr still has to finish draining before the ring
    /// buffer ([`recent_build_output`]) is read for the failure notice. The
    /// harvested rustc lines are dropped because no artifact appeared to
    /// describe.
    fn abandon(self) {
        if let Some(reader) = self.reader {
            let _ = reader.join();
        }
    }
}

/// Checks whether a line exists only because the build was asked for `-v`.
///
/// Such lines are filtered out of the console and out of the ring buffer, so
/// the reload output reads as it does without the capture.
///
/// Cargo colours its status words, so ANSI escape sequences sit around the
/// label whenever the host's stderr is a terminal. Matching the raw prefix
/// would silently stop filtering in exactly the case a person is watching, and
/// dump every multi-kilobyte `rustc` command line into the console.
fn is_verbose_only_line(line: &str) -> bool {
    let plain = without_ansi(line);
    let label = plain.trim_start();
    // The three status words `-v` adds: the invocation itself, the crates cargo
    // skipped, and the reason it did not skip the others.
    label.starts_with("Running `") || label.starts_with("Fresh ") || label.starts_with("Dirty ")
}

/// Returns the line with every ANSI escape sequence removed.
///
/// Cargo colours the status word alone, so escapes sit both before and
/// immediately after the word being matched. Dropping only the leading ones
/// still leaves `Running<ESC>[0m `, which matches nothing.
fn without_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(escape_index) = rest.find('\u{1b}') {
        plain.push_str(&rest[..escape_index]);
        let after = &rest[escape_index..];
        // A CSI sequence: ESC '[' <parameters> <final byte in @..~>. A lone ESC
        // that does not start one is kept verbatim, so unusual compiler output
        // survives; a sequence cut off before its final byte ends the scan.
        let Some(parameters) = after.strip_prefix("\u{1b}[") else {
            plain.push('\u{1b}');
            rest = &after[1..];
            continue;
        };
        match parameters.find(|character: char| ('@'..='~').contains(&character)) {
            Some(end) => rest = &parameters[end + 1..],
            None => return plain,
        }
    }
    plain.push_str(rest);
    plain
}

// =============================================================================
// Build Process Tree
// =============================================================================

/// How long a stopped build's process tree gets to disappear.
///
/// Termination is asynchronous, and the next build starts against the same
/// `CARGO_TARGET_DIR` right afterwards. This wait keeps a dying compiler from
/// holding cargo's package lock through that handover.
const TREE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: closing the job's last handle kills
/// every process still in it.
#[cfg(windows)]
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

/// `JobObjectExtendedLimitInformation`, the information class used to set the
/// kill-on-close limit.
#[cfg(windows)]
const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;

/// `JobObjectBasicAccountingInformation`, the information class that reports
/// how many processes a job holds right now.
#[cfg(windows)]
const JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION: u32 = 1;

/// A build's whole process tree, on platforms that provide one.
///
/// Killing the direct child reaches cargo alone. The rustc and linker
/// grandchildren survive, keep writing into the shared module build tree, and
/// can hold cargo's package lock. The next build then blocks for its whole
/// timeout and is reported as `TimedOut`, which looks like an edit that was
/// ignored. A Windows job object makes the tree the unit of termination: every
/// process cargo starts inherits membership, and
/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes the tree die even when the last
/// handle is closed rather than a termination being requested.
#[cfg(windows)]
pub(crate) struct BuildProcessTree {
    /// The job object's handle, closed on drop.
    job: isize,
}

/// A build's whole process tree, where the platform has no job object.
///
/// Only the direct child is killed, which is all those platforms offer here.
/// The type exists so the call sites need no platform `cfg` of their own.
#[cfg(not(windows))]
pub(crate) struct BuildProcessTree;

// The `kernel32` entry points that make a job object the unit of build
// cancellation. They are declared here rather than taken from a crate such as
// `windows-sys`, whose features would join the modules' dependency graph and
// split the shared engine crates into differently featured variants. That is
// the same reason the process-memory calls are declared by hand in `analytics`.
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attributes: *mut std::ffi::c_void, name: *const u16) -> isize;
    fn SetInformationJobObject(
        job: isize,
        information_class: u32,
        information: *const std::ffi::c_void,
        length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: isize, process: isize) -> i32;
    fn TerminateJobObject(job: isize, exit_code: u32) -> i32;
    fn QueryInformationJobObject(
        job: isize,
        information_class: u32,
        information: *mut std::ffi::c_void,
        length: u32,
        returned_length: *mut u32,
    ) -> i32;
    fn OpenProcess(access: u32, inherit: i32, process_id: u32) -> isize;
    fn CloseHandle(handle: isize) -> i32;
}

/// `JOBOBJECT_BASIC_LIMIT_INFORMATION`, the limits part of the job's extended
/// limit information.
///
/// Only `limit_flags` is ever set. The rest has to be there because the kernel
/// reads the whole structure through the pointer.
#[cfg(windows)]
#[repr(C)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

/// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, the memory layout that the
/// extended-limit information class expects.
#[cfg(windows)]
#[repr(C)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    /// `IO_COUNTERS`: six `ULONGLONG`s, always zeroed.
    io_info: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

/// `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`; the drain wait polls its
/// `active_processes` field.
#[cfg(windows)]
#[repr(C)]
struct JobObjectBasicAccountingInformation {
    total_user_time: i64,
    total_kernel_time: i64,
    this_period_total_user_time: i64,
    this_period_total_kernel_time: i64,
    total_page_fault_count: u32,
    total_processes: u32,
    active_processes: u32,
    total_terminated_processes: u32,
}

#[cfg(windows)]
impl BuildProcessTree {
    /// Creates a job object that kills whatever is still in it when its handle
    /// closes.
    ///
    /// Returns `None` when Windows refuses. The caller then kills the direct
    /// child instead, which is what happens on platforms without job objects.
    fn create() -> Option<Self> {
        // SAFETY: a null security descriptor and an unnamed job are both
        // valid; the call returns a handle this code owns on success.
        let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
        if job == 0 {
            return None;
        }
        let limits = JobObjectExtendedLimitInformation {
            basic_limit_information: JobObjectBasicLimitInformation {
                per_process_user_time_limit: 0,
                per_job_user_time_limit: 0,
                limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                minimum_working_set_size: 0,
                maximum_working_set_size: 0,
                active_process_limit: 0,
                affinity: 0,
                priority_class: 0,
                scheduling_class: 0,
            },
            io_info: [0; 6],
            process_memory_limit: 0,
            job_memory_limit: 0,
            peak_process_memory_used: 0,
            peak_job_memory_used: 0,
        };
        // SAFETY: a live job handle, a structure whose layout matches the
        // named information class, and that structure's own size.
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                std::ptr::addr_of!(limits).cast(),
                std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };
        if configured == 0 {
            // A job that cannot promise to kill its tree is worse than none:
            // the caller would stop using `Child::kill` for no benefit, so the
            // handle is closed and the job treated as unavailable.
            // SAFETY: the handle is live and owned by this function.
            unsafe { CloseHandle(job) };
            return None;
        }
        Some(Self { job })
    }

    /// Adds a freshly spawned process to the job, so the compiler and linker
    /// invocations it starts inherit membership.
    ///
    /// A process started before the join would sit outside the tree, which is
    /// why the caller creates the job before spawning and joins immediately
    /// after.
    fn join(&self, process_id: u32) -> bool {
        // PROCESS_TERMINATE and PROCESS_SET_QUOTA: the two rights assigning a
        // process to a job requires.
        const ASSIGN_ACCESS: u32 = 0x0001 | 0x0100;
        // SAFETY: an access mask, no handle inheritance, and the process id of
        // a child the caller has not yet waited on.
        let process_handle = unsafe { OpenProcess(ASSIGN_ACCESS, 0, process_id) };
        if process_handle == 0 {
            return false;
        }
        // SAFETY: both handles are live; the job handle is owned by `self` and
        // the job keeps its own reference to the process.
        let assigned = unsafe { AssignProcessToJobObject(self.job, process_handle) } != 0;
        // SAFETY: the process handle is owned by this function.
        unsafe { CloseHandle(process_handle) };
        assigned
    }

    /// Terminates every process still in the job.
    fn terminate(&self) {
        // SAFETY: a live job handle, and a non-zero exit code because zero
        // would read as a successful exit to anything that reaps one of these
        // processes.
        unsafe { TerminateJobObject(self.job, 1) };
    }

    /// Returns how many processes the job still holds.
    fn active_processes(&self) -> u32 {
        let mut accounting = JobObjectBasicAccountingInformation {
            total_user_time: 0,
            total_kernel_time: 0,
            this_period_total_user_time: 0,
            this_period_total_kernel_time: 0,
            total_page_fault_count: 0,
            total_processes: 0,
            active_processes: 0,
            total_terminated_processes: 0,
        };
        // SAFETY: a live job handle, a structure whose layout matches the
        // named information class, and that structure's own size; the optional
        // returned-length pointer is null.
        let queried = unsafe {
            QueryInformationJobObject(
                self.job,
                JOB_OBJECT_BASIC_ACCOUNTING_INFORMATION,
                std::ptr::addr_of_mut!(accounting).cast(),
                std::mem::size_of::<JobObjectBasicAccountingInformation>() as u32,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            // A query that fails is treated as "something may still be
            // running". The drain wait then times out and warns, which is the
            // safe reading for a function about to hand the staging directory
            // to the next build.
            return 1;
        }
        accounting.active_processes
    }

    /// Waits until the job reports no live process or the deadline passes.
    ///
    /// Returns whether the tree emptied.
    fn wait_until_empty(&self, deadline: Instant) -> bool {
        loop {
            if self.active_processes() == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(windows)]
impl Drop for BuildProcessTree {
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateJobObjectW` and is closed
        // exactly once, here. Closing the last handle is what
        // `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes lethal to anything still
        // in the job.
        unsafe { CloseHandle(self.job) };
    }
}

/// Creates the process tree for a build that is about to be spawned.
///
/// It must run before the spawn: a process started before it joins sits outside
/// the tree and survives a cancellation.
#[cfg(windows)]
pub(crate) fn create_process_tree() -> Option<BuildProcessTree> {
    BuildProcessTree::create()
}

/// Platforms without job objects have nothing to create; `Child::kill` is all
/// they offer.
#[cfg(not(windows))]
pub(crate) fn create_process_tree() -> Option<BuildProcessTree> {
    None
}

/// Adds a freshly spawned child to its build's process tree.
///
/// A refusal is reported but not fatal: the build still runs, with the weaker
/// guarantee that a cancellation reaches only the direct child.
#[cfg(windows)]
pub(crate) fn join_process_tree(tree: Option<&BuildProcessTree>, child: &Child) {
    let Some(tree) = tree else {
        return;
    };
    if !tree.join(child.id()) {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            process_id = child.id(),
            "could not join the build process to its job object; a cancelled build will only stop the direct child"
        );
    }
}

/// Nothing to join on platforms without job objects.
#[cfg(not(windows))]
pub(crate) fn join_process_tree(_tree: Option<&BuildProcessTree>, _child: &Child) {}

/// Stops a build and its whole process tree.
///
/// The tree is terminated first and then waited for, because the next build
/// starts against the same `CARGO_TARGET_DIR` as soon as this returns. An
/// orphan still holding cargo's package lock would block that build for its
/// whole timeout, and an orphan still writing into the build tree would race
/// the copy into the staging directory.
#[cfg(windows)]
pub(crate) fn stop_process_tree(tree: Option<&BuildProcessTree>, child: &mut Child) {
    let Some(tree) = tree else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    tree.terminate();
    let _ = child.wait();
    if !tree.wait_until_empty(Instant::now() + TREE_DRAIN_TIMEOUT) {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            active_processes = tree.active_processes(),
            "a stopped build's process tree is still running; the next build may block on cargo's package lock or see its staged artifacts rewritten"
        );
    }
}

/// Kill the direct child: without a job object, a grandchild cannot be reached
/// at all.
#[cfg(not(windows))]
pub(crate) fn stop_process_tree(_tree: Option<&BuildProcessTree>, child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Runs one module's build command to completion.
///
/// Every host-spawned build - the project module, the extensions, the startup
/// batch and the C# compiler - goes through here, so they share the process
/// handling, the watchdog, cancellation and failure reporting. Finding the
/// produced artifact is left to the caller. `timeout` is the caller's limit:
/// [`BUILD_TIMEOUT`] for one module, [`STARTUP_BATCH_TIMEOUT`] for the batch.
///
/// With the `development` feature this also takes the compiler flags the fast
/// patch pipeline needs out of the build it runs anyway (see
/// [`VerboseCapture`]); `capture_entries` names the crates to harvest.
///
/// # Errors
///
/// Returns an error if the command is empty, if it fails to spawn, exits with a
/// non-zero status, times out, or is cancelled by a newer source change.
pub(crate) fn run_build_command(
    workspace_root: &Path,
    name: &str,
    build_command: &[String],
    build_environment: &[(String, String)],
    capture_entries: Option<&[(&str, &[String])]>,
    cancel_flag: Option<(&AtomicU64, u64)>,
    timeout: Duration,
) -> Result<(), BuildError> {
    // Step 1: Split the configured command into its executable and arguments.
    //
    // Module configuration keeps commands as owned strings, so callers can
    // define Cargo and dotnet builds without shell-specific quoting. The first
    // item is always the executable; every other item is passed word for word.
    let (program, arguments) = build_command
        .split_first()
        .ok_or(BuildError::EmptyCommand)?;

    // Step 2: Spawn the child process from the workspace root.
    //
    // The workspace root is the working directory because configured paths and
    // Cargo package selection are relative to it. A child that needs no reader
    // inherits the host's stdout and stderr, so compiler progress, warnings and
    // errors stay visible during startup and hot reload. A cargo build whose
    // stderr has to be scanned gets a pipe instead (below). Configured
    // environment overrides win over anything the host itself inherited.
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(workspace_root)
        .envs(build_environment.iter().map(|(key, value)| (key, value)));
    // Redirect the artifact tree, align the feature set with the running host,
    // and define the launcher-injected profile. The helper's docs explain why
    // each override exists; `dotnet` builds ignore them all.
    if program == "cargo" {
        apply_cargo_host_overrides(&mut command, workspace_root);
    }
    // Ask this build to say which rustc invocation it used, so the fast patch
    // pipeline never has to run a build of its own to find out.
    let capture_crate_names: Vec<&str> = capture_entries
        .into_iter()
        .flatten()
        .map(|(captured_name, _)| *captured_name)
        .collect();
    let verbose_capture =
        VerboseCapture::arm(&mut command, program, &capture_crate_names, workspace_root);
    // A cargo build without the capture still copies its stderr into the ring
    // buffer, because a failed build's notice is built from it. A child that
    // inherits the stream outright loses the text once the terminal scrolls.
    let stderr_reader_needed = verbose_capture.is_none() && program == "cargo";
    if stderr_reader_needed {
        command.stderr(std::process::Stdio::piped());
    }
    begin_build_output(name);
    // The tree is created before the spawn, so the child can never run outside
    // it, and joined immediately after. A process started between the two would
    // survive a cancellation of the rest.
    let process_tree = create_process_tree();
    let mut child = command.spawn().map_err(|source| BuildError::SpawnFailed {
        name: name.to_string(),
        source,
    })?;
    join_process_tree(process_tree.as_ref(), &child);
    // Started before the watchdog loop, because cargo's stderr has to be
    // drained while the build runs; otherwise the pipe fills and the compiler
    // blocks forever.
    let verbose_capture = verbose_capture.map(|verbose_capture| verbose_capture.start(&mut child));
    let stderr_reader = if stderr_reader_needed {
        tee_stderr(&mut child)
    } else {
        None
    };

    // Step 3: Poll for completion, cancellation or timeout under a watchdog.
    //
    // The host frame loop must never block indefinitely on a hung compiler or
    // an interactive prompt, so the build is polled against a deadline and a
    // cancellation signal raised by newer source saves. The build's wall time
    // and the child process's peak working set are sampled for the analytics
    // report.
    let build_started_at = Instant::now();
    let mut build_peak_working_set_bytes: u64 = 0;
    let deadline = Instant::now() + timeout;
    let exit_status = loop {
        // A source save during the build moves the generation past the
        // baseline taken when the reload started, which cancels this attempt.
        // The caller keeps the old module, and the next frame rebuilds with the
        // newer sources.
        if cancel_flag
            .is_some_and(|(generation, baseline)| generation.load(Ordering::Acquire) != baseline)
        {
            stop_process_tree(process_tree.as_ref(), &mut child);
            return Err(BuildError::Cancelled);
        }
        if let Some(child_exit_status) =
            child.try_wait().map_err(|source| BuildError::WaitFailed {
                name: name.to_string(),
                source,
            })?
        {
            break child_exit_status;
        }
        // The OS reports a peak that only grows, so the latest sample is
        // already the peak.
        if let Some((_, peak_working_set_bytes)) = analytics::process_memory(Some(child.id())) {
            build_peak_working_set_bytes = build_peak_working_set_bytes.max(peak_working_set_bytes);
        }
        if Instant::now() >= deadline {
            stop_process_tree(process_tree.as_ref(), &mut child);
            return Err(BuildError::TimedOut {
                name: name.to_string(),
                seconds: timeout.as_secs(),
            });
        }
        std::thread::sleep(WATCHDOG_POLL_INTERVAL);
    };

    // Step 4: Reject a non-zero exit status.
    //
    // A failed compiler must stop the load. During hot reload the caller keeps
    // the current project module when this error comes back. The stderr readers
    // are joined first, so the ring buffer behind `recent_build_output` holds
    // every line the build printed before the failure is reported.
    if !exit_status.success() {
        if let Some(stderr_reader) = stderr_reader {
            let _ = stderr_reader.join();
        }
        if let Some(verbose_capture) = verbose_capture {
            verbose_capture.abandon();
        }
        return Err(BuildError::CommandFailed {
            name: name.to_string(),
            status: exit_status,
        });
    }
    if let Some(stderr_reader) = stderr_reader {
        let _ = stderr_reader.join();
    }
    // Cache the compiler line only after a successful build. A failed build may
    // have recompiled the crate with flags that never produced the artifact now
    // on disk.
    if let Some(verbose_capture) = verbose_capture {
        verbose_capture.finish(capture_entries.unwrap_or(&[]));
    }
    analytics::record_build_command(
        name,
        build_started_at.elapsed().as_millis() as u64,
        build_peak_working_set_bytes,
    );
    Ok(())
}

/// Adds the compiler-command-line capture to a managed project's build.
///
/// A native build comes back unchanged. For a C# build this appends the two
/// MSBuild properties that make the build report its own `csc` invocation:
/// `ProvideCommandLineArgs` fills the item group, and the injected targets file
/// writes it out. Neither property changes what is compiled; they only make the
/// build leave a record of how it compiled.
///
/// When the targets file is missing, the command is returned unchanged. The
/// fast reload path then costs more, and nothing else changes.
fn with_compiler_argument_capture(
    workspace_root: &Path,
    project_config: &ProjectModuleConfig,
) -> Vec<String> {
    let mut build_command = project_config.build_command.clone();
    if !matches!(&project_config.backend, ProjectModuleBackend::CSharp(_)) {
        return build_command;
    }
    let targets_path = workspace_root.join(crate::config::CSHARP_COMPILER_ARGUMENTS_TARGETS);
    if !targets_path.is_file() {
        return build_command;
    }
    build_command.push("-p:ProvideCommandLineArgs=true".to_string());
    build_command.push(format!(
        "-p:CustomAfterMicrosoftCSharpTargets={}",
        targets_path.display()
    ));
    build_command
}

/// Builds the managed assembly that compiles C# projects in-process.
///
/// It is built separately from every managed project because nothing references
/// it. Roslyn is large and does not suit NativeAOT, so keeping it out of each
/// project's reference graph is what keeps it out of a shipping bundle.
/// MSBuild's own incremental check makes an unchanged compiler project cheap to
/// "build", and the host does not second-guess it.
///
/// # Errors
///
/// Returns [`BuildError::OutputMissing`] when the compiler project or its output
/// assembly is absent, or whatever [`run_build_command`] reports.
pub(crate) fn build_csharp_compiler(workspace_root: &Path) -> Result<PathBuf, BuildError> {
    let manifest_path = workspace_root.join(crate::config::CSHARP_COMPILER_MANIFEST);
    if !manifest_path.is_file() {
        return Err(BuildError::OutputMissing {
            path: manifest_path.display().to_string(),
        });
    }
    let output_path = workspace_root
        .join(crate::config::CSHARP_COMPILER_OUTPUT_SUBDIRECTORY)
        .join(format!(
            "{}.dll",
            crate::config::CSHARP_COMPILER_ASSEMBLY_NAME
        ));
    info!(
        target: telemetry_target::HOT_RELOAD,
        "building the in-process C# compiler"
    );
    let build_command = vec![
        "dotnet".to_string(),
        "build".to_string(),
        crate::config::CSHARP_COMPILER_MANIFEST.to_string(),
        "-c".to_string(),
        "Release".to_string(),
        "--nologo".to_string(),
    ];
    run_build_command(
        workspace_root,
        crate::config::CSHARP_COMPILER_ASSEMBLY_NAME,
        &build_command,
        &[],
        None,
        None,
        BUILD_TIMEOUT,
    )?;
    if !output_path.exists() {
        return Err(BuildError::OutputMissing {
            path: output_path.display().to_string(),
        });
    }
    Ok(output_path)
}

/// Builds the selected project module and returns the artifact the host loads.
///
/// # Errors
///
/// Returns an error when the build fails, during the staging step, or when the
/// expected artifact does not exist at the configured path. [`run_build_command`]
/// describes the build failures; [`stage_build_outputs`] and
/// [`confirm_staged_artifacts`] describe the staging ones.
pub(crate) fn build_project_module(
    workspace_root: &Path,
    project_config: &ProjectModuleConfig,
    cancel_flag: Option<(&AtomicU64, u64)>,
) -> Result<PathBuf, BuildError> {
    // A startup build is reported through the announced build plan; a reload
    // build is logged here instead, and the suites wait for that line.
    if !crate::build_progress::announce_build(&project_config.name) {
        info!(
            target: telemetry_target::HOT_RELOAD,
            module = project_config.name.as_str(),
            "building project module"
        );
    }

    // Step 1: Find the artifact paths for this backend.
    //
    // The build command is the same for every backend, but each backend names
    // and locates its loadable artifact differently. Native outputs use the
    // platform's naming convention; managed outputs are always an assembly
    // `.dll`.
    //
    // A native project has two paths: the slot cargo writes into, and the
    // private copy the host loads from. They are separate for the same reason
    // extension outputs are. Any other `cargo build` of the same package
    // overwrites cargo's slot, and for the project that means a DLL carrying a
    // differently configured `pill_engine`, which crashes inside `LoadLibrary`.
    // The managed backend has no such collision and loads from where its build
    // wrote.
    let (build_output, output_path) = match &project_config.backend {
        ProjectModuleBackend::NativeLibrary {
            library_name,
            output_subdirectory,
        } => (
            workspace_root
                .join(output_subdirectory)
                .join(native_library_filename(library_name)),
            workspace_root
                .join(PROJECT_HOT_OUTPUT_SUBDIRECTORY)
                .join(native_library_filename(library_name)),
        ),
        ProjectModuleBackend::CSharp(csharp_config) => {
            let path = workspace_root
                .join(&csharp_config.project_output_subdirectory)
                .join(format!("{}.dll", csharp_config.project_assembly_name));
            (path.clone(), path)
        }
    };

    // The crate's `rlib`, which a generated patch links to reach the project's
    // types. It is staged beside the library so a patch cannot link a copy that
    // another build replaced. Such a copy was compiled against a differently
    // configured engine and gives every type a different `TypeId`.
    let (rlib_build_output, rlib_output) = (
        workspace_root
            .join(crate::config::module_build_artifact_directory())
            .join(format!("lib{}.rlib", project_config.name)),
        workspace_root
            .join(PROJECT_HOT_OUTPUT_SUBDIRECTORY)
            .join(format!("lib{}.rlib", project_config.name)),
    );

    // Step 2: Run cargo and let it decide what to rebuild. Cargo owns
    // freshness: every build runs unless the startup batch already ran it (the
    // token below), and a fresh workspace answers in a few hundred milliseconds
    // with `Finished`. The host has no freshness model of its own, so features,
    // targets and environment overrides need no second model.

    // A managed build also records the compiler command line MSBuild computed,
    // so a later hot reload can replay it in-process instead of running MSBuild
    // again. See `crate::csharp::fast_compile`.
    let build_command = with_compiler_argument_capture(workspace_root, project_config);

    // The startup batch may already have built this project with the same flags
    // (see [`build_extension_batch`]). The token covers that first load only: a
    // later reload always builds. A managed project never joins the batch,
    // because its `dotnet` build shares none of cargo's fixed cost, and the
    // token must not skip that build either.
    let is_batch_validated = matches!(
        &project_config.backend,
        ProjectModuleBackend::NativeLibrary { .. }
    ) && take_batch_validation(
        &project_config.name,
        &workspace_root.join(&project_config.watch_directory),
    );
    if !is_batch_validated {
        run_build_command(
            workspace_root,
            &project_config.name,
            &build_command,
            &project_config.build_environment,
            Some(&[(project_config.name.as_str(), build_command.as_slice())]),
            cancel_flag,
            BUILD_TIMEOUT,
        )?;
    }

    // Step 3: Copy the freshly built files into the private hot-load directory,
    // so the host never loads from a slot other builds write to. The managed
    // backend loads from where its build wrote, so its two paths are the same
    // and the copy is skipped.
    let mut stage_milliseconds = 0.0;
    if matches!(
        &project_config.backend,
        ProjectModuleBackend::NativeLibrary { .. }
    ) {
        let stage_started_at = Instant::now();
        // The project always produces an rlib, so a missing one is an error
        // rather than a crate that simply has none.
        let staged_artifacts = stage_build_outputs(
            workspace_root,
            (&build_output, &output_path),
            Some((&rlib_build_output, &rlib_output)),
            true,
        )?;
        stage_milliseconds = stage_started_at.elapsed().as_secs_f64() * 1000.0;

        // Stamp the staged copies so a later run can tell them apart from
        // anything another build writes to the same paths.
        record_artifact_stamp(
            workspace_root,
            &project_config.name,
            &project_config.watch_directory,
            &project_config.build_command,
            &staged_artifacts,
        );

        // Then check that the stamp still describes what is on disk. A compiler
        // orphaned by an earlier cancellation writes into these shared slots,
        // and what it writes must not be loaded as this build's output.
        confirm_staged_artifacts(
            workspace_root,
            &project_config.name,
            &project_config.watch_directory,
            &project_config.build_command,
            &staged_artifacts,
        )?;
    }

    // Step 4: Check that the resolved artifact exists before reporting success.
    //
    // A successful process exit does not prove that the configuration points at
    // the artifact the build produced. Checking the path here makes a loading
    // failure name an output-directory mismatch instead of an opaque DLL or
    // runtime failure later in the startup sequence.
    if !output_path.exists() {
        return Err(BuildError::OutputMissing {
            path: output_path.display().to_string(),
        });
    }

    analytics::record_module_artifact(
        &project_config.name,
        ModuleKind::Project,
        stage_milliseconds,
        workspace_root,
        &output_path,
    );

    Ok(output_path)
}

/// Copies one freshly built artifact into its private hot-load location.
///
/// # Errors
///
/// Returns [`BuildError::OutputMissing`] when the build did not produce the
/// source artifact, or when the destination has no parent directory. Returns
/// [`BuildError::HotArtifactCopyFailed`] when the copy itself fails.
fn stage_artifact(build_output: &Path, hot_output: &Path) -> Result<(), BuildError> {
    if !build_output.exists() {
        return Err(BuildError::OutputMissing {
            path: build_output.display().to_string(),
        });
    }
    let Some(hot_directory) = hot_output.parent() else {
        return Err(BuildError::OutputMissing {
            path: hot_output.display().to_string(),
        });
    };
    std::fs::create_dir_all(hot_directory).map_err(|source| BuildError::HotArtifactCopyFailed {
        source_path: build_output.display().to_string(),
        target_path: hot_output.display().to_string(),
        source,
    })?;
    // Skip the copy when the destination already holds this build's output.
    // Copying again renews the destination's timestamp, and on Windows opening
    // a freshly written DLL pays the platform's first-access scan. Every load of
    // an unchanged module would pay that cost again. Unchanged builds leave the
    // source's timestamps alone, so the steady state is a pair of `stat` calls.
    if staged_copy_is_current(build_output, hot_output) {
        return Ok(());
    }
    // Replace the file through a private staging file and a rename. A previous
    // generation may still have the destination mapped, and a load copy shares
    // its file. Windows refuses to open a mapped image for writing, while a
    // rename replaces the directory entry and leaves the mapped file intact.
    let staged_output = hot_output.with_extension("staged");
    std::fs::copy(build_output, &staged_output).map_err(|source| {
        BuildError::HotArtifactCopyFailed {
            source_path: build_output.display().to_string(),
            target_path: hot_output.display().to_string(),
            source,
        }
    })?;
    std::fs::rename(&staged_output, hot_output).map_err(|source| {
        let _ = std::fs::remove_file(&staged_output);
        BuildError::HotArtifactCopyFailed {
            source_path: build_output.display().to_string(),
            target_path: hot_output.display().to_string(),
            source,
        }
    })?;
    Ok(())
}

/// Copies the module-world engine dylibs beside the hot-load copies.
///
/// Every native module, project and patch imports the engine dylibs
/// ([`crate::native_library::ENGINE_DYLIB_STEMS`]). The copies a host-spawned
/// build produces, in the private module build tree, can differ from the ones the
/// host binary maps from the regular target directory, because a GUI frontend
/// adds extra features to shared crates. The loader then gives modules their
/// matching copies. When the copies are identical (a plain CLI host), these
/// staged files stay unused and the loader keeps the host's single instances.
fn stage_engine_dylib(workspace_root: &Path) {
    for stem in crate::native_library::ENGINE_DYLIB_STEMS {
        let file_name = format!("{stem}.dll");
        let source = workspace_root
            .join(crate::config::module_build_artifact_directory())
            .join(&file_name);
        if !source.is_file() {
            continue;
        }
        let destination = workspace_root
            .join(PROJECT_HOT_OUTPUT_SUBDIRECTORY)
            .join(&file_name);
        // Copy only when the staged copy is out of date. Copying again renews
        // the destination's timestamp, and on Windows opening a freshly written
        // DLL pays the platform's first-access scan. The isolation check would
        // pay that cost on every module load. The source changes only when a
        // build produced a new engine, so the steady state is a pair of `stat`
        // calls.
        if staged_copy_is_current(&source, &destination) {
            continue;
        }
        if let Err(error) = std::fs::copy(source, destination) {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                error = %error,
                dylib = %file_name,
                "could not stage the module-world engine dylib into the hot-load directory"
            );
        }
    }
}

/// Copies everything one build produced into the host's private hot-load paths.
///
/// Both build paths end the same way, so the sequence lives here once. It
/// handles the shared-slot collision that makes the host load a
/// differently-featured artifact, and it copies the dependency rlibs a later
/// patch links before another build can overwrite them.
///
/// `rlib` is `None` for a crate that produces none. `require_rlib` says whether a
/// missing one is an error. The project always produces an rlib, so a missing one
/// means something is wrong. An extension that declares only a `cdylib`
/// legitimately has none, which leaves the fast path idle.
///
/// Returns the staged paths in the order a stamp should record them.
///
/// # Errors
///
/// Returns [`BuildError::OutputMissing`] when the build produced no library to
/// stage, and [`BuildError::HotArtifactCopyFailed`] when a copy into the
/// hot-load directory fails.
fn stage_build_outputs(
    workspace_root: &Path,
    library_paths: (&Path, &Path),
    rlib: Option<(&Path, &Path)>,
    require_rlib: bool,
) -> Result<Vec<PathBuf>, BuildError> {
    let (build_output, hot_output) = library_paths;
    stage_artifact(build_output, hot_output)?;

    // The build just produced a consistent set in `deps`. Take the copies now,
    // before anything else can write to those shared per-crate slots.
    stage_shared_dependency_rlibs(workspace_root);

    let mut staged_artifacts = vec![hot_output.to_path_buf()];

    if let Some((rlib_build_output, rlib_output)) = rlib {
        if require_rlib || rlib_build_output.is_file() {
            stage_artifact(rlib_build_output, rlib_output)?;
            staged_artifacts.push(rlib_output.to_path_buf());
        }
    }

    // Stage the module-world engine dylib beside the hot copies. Modules import
    // `pill_core.dll`, and when the host runs a different engine variant than
    // this build produced, the loader gives the module this matching copy
    // instead of the host's.
    stage_engine_dylib(workspace_root);

    Ok(staged_artifacts)
}

/// Returns the newest rlib cargo wrote for `package_name` into
/// `build_directory`.
///
/// A wrapper build compiles the extension as a dependency, so its rlib sits under
/// the build's `deps` directory. It is either metadata-hashed or in the unhashed
/// workspace-member slot, whichever this cargo version uses, and some
/// configurations also leave a copy in the profile root. The host stages the
/// newest one, which the last build that compiled the crate produced; a patch
/// links it to reach the module's types. Returns `None` when the build wrote
/// none, which leaves the per-function fast path idle for that module.
pub(crate) fn newest_extension_rlib(build_directory: &Path, package_name: &str) -> Option<PathBuf> {
    let library_name = package_name.replace('-', "_");
    let mut newest_rlib: Option<(std::time::SystemTime, PathBuf)> = None;
    let mut consider_candidate = |path: PathBuf| {
        let Ok(modified) = std::fs::metadata(&path).and_then(|metadata| metadata.modified()) else {
            return;
        };
        if newest_rlib
            .as_ref()
            .is_none_or(|(time, _)| modified > *time)
        {
            newest_rlib = Some((modified, path));
        }
    };
    if let Ok(entries) = std::fs::read_dir(build_directory.join("deps")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let stem = file_name
                .strip_prefix("lib")
                .unwrap_or(file_name)
                .strip_suffix(".rlib")
                .unwrap_or(file_name);
            if stem == library_name || stem.starts_with(&format!("{library_name}-")) {
                consider_candidate(path);
            }
        }
    }
    consider_candidate(build_directory.join(format!("lib{library_name}.rlib")));
    newest_rlib.map(|(_, path)| path)
}

/// The directory where the shared per-crate dependency rlibs are staged for
/// patch linking.
///
/// It is a subdirectory of the hot-load directory, private to the host for the
/// same reason the staged artifacts are.
pub(crate) const STAGED_DEPENDENCY_SUBDIRECTORY: &str = "target/hot/deps";

/// Copies every shared per-crate rlib out of cargo's `deps` directory.
///
/// Cargo gives a workspace crate's rlib an unhashed, per-crate path such as
/// `deps/libpill_test.rlib`, and overwrites it in place on every build.
/// Third-party crates get a metadata hash and are safe. A workspace crate shares
/// one slot per crate name across every feature configuration. The host builds
/// modules with `--features pill_engine/development`, while a plain `cargo build`
/// writes a differently-featured artifact to the same path. A generated patch
/// then links a module rlib built against one variant while its dependency's
/// `--extern` names the other, which fails as `error[E0463]: can't find crate`
/// and names the module rather than the dependency.
///
/// Copying them right after a host build produced a consistent set makes the
/// patch link closure immune to anything written afterwards. It is the protection
/// [`stage_artifact`] gives the module's own rlib, extended to what it links
/// against. Only out-of-date copies are made, and a failed copy is skipped rather
/// than treated as fatal.
pub(crate) fn stage_shared_dependency_rlibs(workspace_root: &Path) -> usize {
    let source_directory = workspace_root
        .join(cargo_module_output_subdirectory())
        .join("deps");
    let staged_directory = workspace_root.join(STAGED_DEPENDENCY_SUBDIRECTORY);
    let Ok(entries) = std::fs::read_dir(&source_directory) else {
        return 0;
    };
    if std::fs::create_dir_all(&staged_directory).is_err() {
        return 0;
    }

    let mut copied_count = 0usize;
    for entry in entries.flatten() {
        let source_path = entry.path();
        let Some(file_name) = source_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_shared_slot_rlib(file_name) {
            continue;
        }
        // Generated member crates write unhashed rlibs too, but nothing links
        // them. A wrapper's rlib is not a patch target, and a generated project
        // member's rlib is rebuilt by every project build anyway.
        if is_generated_member_rlib(file_name) {
            continue;
        }
        let staged_path = staged_directory.join(file_name);
        if staged_copy_is_current(&source_path, &staged_path) {
            continue;
        }
        if std::fs::copy(&source_path, &staged_path).is_ok() {
            copied_count += 1;
        }
    }
    if copied_count > 0 {
        debug!(
            target: telemetry_target::HOT_RELOAD,
            copied = copied_count,
            directory = %staged_directory.display(),
            "staged shared dependency rlibs for patch linking"
        );
    }
    copied_count
}

/// Checks whether a `deps` filename is a shared per-crate slot rather than a
/// hash-qualified artifact.
///
/// Cargo appends `-<16 hex digits>` to a crate's filename when the artifact is
/// specific to one resolved configuration. A name without that suffix is the
/// shared slot that every build of that crate name writes to, and it is the only
/// kind another build can silently replace. The patch linker
/// (`hot_patch::compile`) and [`stage_shared_dependency_rlibs`] must agree about
/// which files are shared. Otherwise a file can be staged and never linked, or
/// linked and never staged, so both call this one implementation.
pub(crate) fn is_shared_slot_rlib(file_name: &str) -> bool {
    let Some(stem) = file_name.strip_suffix(".rlib") else {
        return false;
    };
    match stem.rsplit_once('-') {
        Some((_, suffix)) => {
            !(suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit()))
        }
        None => true,
    }
}

/// Checks whether a `deps` filename is the rlib of a host-generated member
/// crate.
///
/// Their crate names all carry a generated prefix. Nothing links the rlib of a
/// wrapper or of a generated project member, so the dependency staging skips them
/// instead of copying files no patch can use.
fn is_generated_member_rlib(file_name: &str) -> bool {
    let stem = file_name.strip_suffix(".rlib").unwrap_or(file_name);
    let stem = stem.strip_prefix("lib").unwrap_or(stem);
    stem.starts_with(crate::config::HOST_PROJECT_MEMBER_PREFIX)
        || stem.starts_with(crate::config::HOST_MODULE_MEMBER_PREFIX)
}

/// Checks whether a staged copy already matches its source.
///
/// Same size and no older. The shared-rlib staging, the engine dylib staging and
/// the patch session's rlib refresh all use it, so a copy either path leaves
/// behind satisfies the other. Anything unknown counts as out of date, so the copy
/// happens.
pub(crate) fn staged_copy_is_current(source_path: &Path, staged_path: &Path) -> bool {
    let (Ok(source_metadata), Ok(staged_metadata)) = (
        std::fs::metadata(source_path),
        std::fs::metadata(staged_path),
    ) else {
        return false;
    };
    if source_metadata.len() != staged_metadata.len() {
        return false;
    }
    match (source_metadata.modified(), staged_metadata.modified()) {
        (Ok(source_time), Ok(staged_time)) => staged_time >= source_time,
        _ => false,
    }
}

/// What one startup batch invocation validated, and when it finished.
#[derive(Debug)]
struct BatchValidation {
    /// When the batch invocation returned successfully.
    ///
    /// A token is used only while no file under the module's watch directory is
    /// newer than this instant. The batch validated exactly the sources cargo
    /// saw, so a save after it must rebuild.
    completed_at: SystemTime,
    /// Module names whose artifacts the batch built or confirmed.
    names: std::collections::HashSet<String>,
}

/// One startup batch invocation's validation, consumed per module.
///
/// One batch runs per process, before anything loads; each name is consumed
/// by its startup load, so a later reload always builds on its own.
static BATCH_VALIDATION: OnceLock<std::sync::Mutex<Option<BatchValidation>>> = OnceLock::new();

/// Runs `callback` with exclusive access to the batch validation state.
fn with_batch_validation<T>(callback: impl FnOnce(&mut Option<BatchValidation>) -> T) -> T {
    callback(
        &mut BATCH_VALIDATION
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// Builds every module of the startup plan in one cargo invocation.
///
/// Every module build otherwise pays cargo's fixed cost once: process start,
/// workspace resolve and fingerprint scan. The extensions' wrappers, the native
/// project's member and the renderer's wrapper (windowed postures only) differ
/// only in `--package`, so one invocation selects them all with flags that match
/// a single module's build. That keeps every unit's fingerprint identical to a
/// per-module build's. The modules it validated skip their own build when they
/// load (see [`take_batch_validation`]), and an edit later still rebuilds a
/// module on its own.
///
/// A managed (C#) project is never selected, because its `dotnet` build shares
/// none of cargo's fixed cost. The renderer's wrapper joins the batch because it
/// is an ordinary wrapper build. Only its load waits for the window (see
/// `crate::renderer_module`), and a rebuild request drops its token.
///
/// Feature unification is the one difference: a selected package that depends on
/// another selected package receives the union of both feature requests.
///
/// Returns whether the batch ran and validated the modules. A failure is not an
/// error: the modules then build individually.
pub(crate) fn build_extension_batch(
    workspace_root: &Path,
    configs: &[ExtensionConfig],
    project: Option<&ProjectModuleConfig>,
    renderer_name: Option<&str>,
    project_editor: Option<&ExtensionConfig>,
) -> bool {
    // Only a native project shares cargo's fixed cost. A managed build's
    // `dotnet` invocation is its own machinery, and selecting its member here
    // would fail on a missing cargo package.
    let project_package = project.and_then(|project| match &project.backend {
        ProjectModuleBackend::NativeLibrary { .. } => Some(format!(
            "{}{}",
            crate::config::HOST_PROJECT_MEMBER_PREFIX,
            project.name
        )),
        ProjectModuleBackend::CSharp(_) => None,
    });
    // The renderer's wrapper is built exactly like an extension's. It is
    // constructed from the name and the folder holding it, which is all
    // `RendererModule` needs to load it.
    let renderer_config = renderer_name.map(|renderer_name| {
        ExtensionConfig::located_workspace_member(workspace_root, renderer_name)
    });
    let package_count = configs.len()
        + usize::from(project_package.is_some())
        + usize::from(renderer_config.is_some())
        + usize::from(project_editor.is_some());
    // One package has no fixed cost to share; its own step builds it.
    if package_count < 2 {
        return false;
    }

    let mut packages: Vec<String> = configs
        .iter()
        .map(|extension_config| extension_config.wrapper_library_name.clone())
        .collect();
    if let Some(package) = &project_package {
        packages.push(package.clone());
    }
    if let Some(renderer_extension_config) = &renderer_config {
        packages.push(renderer_extension_config.wrapper_library_name.clone());
    }
    // The project's own editor crate shares the batch: it is an ordinary
    // wrapper build with a different root, and building it alone would pay
    // cargo's fixed cost a second time.
    if let Some(project_editor_config) = project_editor {
        packages.push(project_editor_config.wrapper_library_name.clone());
    }
    let command = crate::config::host_cargo_command(&packages);
    // One capture entry per selected package, so the patch pipeline harvests
    // every module's `rustc` line from this single invocation.
    let mut capture_entries: Vec<(&str, &[String])> = configs
        .iter()
        .map(|extension_config| {
            (
                extension_config.name.as_str(),
                extension_config.build_command.as_slice(),
            )
        })
        .collect();
    if let (Some(project), Some(_)) = (project, project_package.as_ref()) {
        capture_entries.push((project.name.as_str(), project.build_command.as_slice()));
    }
    if let Some(renderer_extension_config) = &renderer_config {
        capture_entries.push((
            renderer_extension_config.name.as_str(),
            renderer_extension_config.build_command.as_slice(),
        ));
    }
    if let Some(project_editor_config) = project_editor {
        capture_entries.push((
            project_editor_config.name.as_str(),
            project_editor_config.build_command.as_slice(),
        ));
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        modules = package_count,
        "building every module in one cargo invocation"
    );
    // A cold private build tree is why the batch gets minutes rather than the
    // single-module cap. The log line says so, which turns a slow first start
    // into an explained wait instead of a suspected hang.
    let artifact_directory = workspace_root.join(crate::config::module_build_artifact_directory());
    if !artifact_directory.exists() {
        info!(
            target: telemetry_target::HOT_RELOAD,
            directory = %artifact_directory.display(),
            "cold module build tree: the startup batch compiles every module from scratch"
        );
    }
    match run_build_command(
        workspace_root,
        "startup batch",
        &command,
        &crate::config::spawned_build_environment(),
        Some(&capture_entries),
        None,
        STARTUP_BATCH_TIMEOUT,
    ) {
        Ok(()) => {
            let mut validation = BatchValidation {
                completed_at: SystemTime::now(),
                names: Default::default(),
            };
            for extension_config in configs {
                validation.names.insert(extension_config.name.clone());
            }
            if let (Some(project), Some(_)) = (project, project_package.as_ref()) {
                validation.names.insert(project.name.clone());
            }
            if let Some(renderer_extension_config) = &renderer_config {
                validation
                    .names
                    .insert(renderer_extension_config.name.clone());
            }
            if let Some(project_editor_config) = project_editor {
                validation.names.insert(project_editor_config.name.clone());
            }
            with_batch_validation(|validation_slot| *validation_slot = Some(validation));
            true
        }
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                error = %error,
                "the startup batch build failed; each module builds on its own"
            );
            false
        }
    }
}

/// Checks whether the startup batch already validated `module_name`'s artifact.
///
/// Consumed per module: the token belongs to the startup load, so a reload of
/// the same module builds on its own. Any file under `watch_directory` newer
/// than the batch also invalidates it. The batch validated exactly the sources
/// cargo saw, and a save after it must rebuild rather than load stale code. The
/// window between the batch and a load is largest for the renderer, which loads
/// when the window opens.
fn take_batch_validation(module_name: &str, watch_directory: &Path) -> bool {
    with_batch_validation(|validation_slot| {
        let Some(validation) = validation_slot.as_mut() else {
            return false;
        };
        if !validation.names.remove(module_name) {
            return false;
        }
        if source_newer_than(watch_directory, validation.completed_at) {
            pill_core::debug!(
                target: telemetry_target::HOT_RELOAD,
                module = module_name,
                "source changed after the startup batch; the module builds on its own"
            );
            return false;
        }
        true
    })
}

/// Drops `module_name`'s batch token, so its next load builds instead of
/// skipping.
///
/// The renderer's rebuild request uses this. A reloaded data crate makes the
/// artifact the batch built stale even though no renderer source changed.
#[cfg(feature = "rendering")]
pub(crate) fn forget_batch_validation(module_name: &str) {
    with_batch_validation(|validation_slot| {
        if let Some(validation) = validation_slot.as_mut() {
            validation.names.remove(module_name);
        }
    });
}

/// Checks whether any entry under `directory` was modified after
/// `reference_time`.
///
/// Watch directories hold module sources and are small, so the walk is cheap per
/// load. Anything unreadable counts as newer, which costs one build and can never
/// load stale code.
fn source_newer_than(directory: &Path, reference_time: SystemTime) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return true;
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return true;
        };
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            return true;
        };
        let was_modified = metadata
            .modified()
            .ok()
            .is_none_or(|modified| modified > reference_time);
        if metadata.is_dir() {
            if was_modified || source_newer_than(&path, reference_time) {
                return true;
            }
            continue;
        }
        if was_modified {
            return true;
        }
    }
    false
}

/// Builds one extension and returns the artifact the host loads.
///
/// Extensions are workspace members, so their output always follows the
/// platform's native-library naming inside the configured output directory.
///
/// # Errors
///
/// Returns an error when the build fails, during the staging step, or when the
/// built library is missing afterwards. [`run_build_command`] describes the build
/// failures; [`stage_build_outputs`] and [`confirm_staged_artifacts`] describe the
/// staging ones.
pub(crate) fn build_extension(
    workspace_root: &Path,
    extension_config: &ExtensionConfig,
    cancel_flag: Option<(&AtomicU64, u64)>,
) -> Result<PathBuf, BuildError> {
    // A startup build is reported through the announced build plan; a reload
    // build is logged here instead, and the suites wait for that line.
    if !crate::build_progress::announce_build(&extension_config.name) {
        info!(
            target: telemetry_target::HOT_RELOAD,
            module = extension_config.name.as_str(),
            "building extension"
        );
    }

    // Cargo writes the freshly compiled cdylib into the shared per-crate output
    // slot, while the host loads the private hot-load copy. Keeping the paths
    // separate fixes the collision between one crate name and two feature sets.
    // A project build may overwrite the shared slot with the extension's
    // export-stripped dependency variant, but the loaded generation always comes
    // from the untouched hot copy.
    //
    // The built crate is the generated wrapper. The staged artifact carries the
    // extension's own name, which is how the host addresses the module.
    let build_output = workspace_root
        .join(cargo_module_output_subdirectory())
        .join(native_library_filename(
            &extension_config.wrapper_library_name,
        ));
    let hot_output = workspace_root
        .join(&extension_config.output_subdirectory)
        .join(native_library_filename(&extension_config.library_name));

    // The extension's `rlib`, staged for the same reason the project's is: a
    // generated patch links it to reach the module's types. The wrapper build
    // compiles the extension as a dependency, so its rlib sits in the build's
    // `deps` directory under its metadata hash or, for a workspace member, its
    // plain name. The newest one comes from the build that just ran.
    let rlib_build_output = newest_extension_rlib(
        &workspace_root.join(cargo_module_output_subdirectory()),
        &extension_config.name,
    );
    let rlib_output = workspace_root
        .join(&extension_config.output_subdirectory)
        .join(format!("lib{}.rlib", extension_config.name));

    // Extensions have no per-module environment of their own, but they need the
    // same profile-driven `RUSTFLAGS` handling the project gets. An optimized
    // build must not inherit `-C prefer-dynamic`.
    //
    // One batch invocation may already have built this module with the same
    // flags (see [`build_extension_batch`]). The token covers that one startup
    // load only. A reload finds no token and always builds, and a save newer
    // than the batch invalidates it.
    if !take_batch_validation(
        &extension_config.name,
        &workspace_root.join(&extension_config.watch_directory),
    ) {
        run_build_command(
            workspace_root,
            &extension_config.name,
            &extension_config.build_command,
            &crate::config::spawned_build_environment(),
            Some(&[(
                extension_config.name.as_str(),
                extension_config.build_command.as_slice(),
            )]),
            cancel_flag,
            BUILD_TIMEOUT,
        )?;
    }

    // Stage the freshly built wrapper library into the hot-load directory under
    // the extension's name. The shared per-crate slot may later hold a
    // differently featured copy of the extension's dependency rlib, which is why
    // the loadable artifact lives apart from it.
    //
    // The extension's rlib is staged beside it for the per-function fast path.
    // A build that produced none is not an error; it only leaves the fast path
    // idle for that module.
    let stage_started_at = Instant::now();
    let staged_artifacts = stage_build_outputs(
        workspace_root,
        (&build_output, &hot_output),
        rlib_build_output
            .as_deref()
            .map(|built| (built, rlib_output.as_path())),
        false,
    )?;
    let stage_milliseconds = stage_started_at.elapsed().as_secs_f64() * 1000.0;

    // Stamp the staged copies so a later run can tell them apart from anything
    // another build writes to the same paths.
    record_artifact_stamp(
        workspace_root,
        &extension_config.name,
        &extension_config.watch_directory,
        &extension_config.build_command,
        &staged_artifacts,
    );

    // The same recheck as the project path. The staged copy is loadable only if
    // it is still the one this build wrote.
    confirm_staged_artifacts(
        workspace_root,
        &extension_config.name,
        &extension_config.watch_directory,
        &extension_config.build_command,
        &staged_artifacts,
    )?;

    if !hot_output.exists() {
        return Err(BuildError::OutputMissing {
            path: hot_output.display().to_string(),
        });
    }
    analytics::record_module_artifact(
        &extension_config.name,
        ModuleKind::Extension,
        stage_milliseconds,
        workspace_root,
        &hot_output,
    );
    Ok(hot_output)
}

/// Returns the platform-specific file name of a native library.
fn native_library_filename(library_name: &str) -> String {
    // Cargo follows each platform's conventional dynamic-library prefix and
    // extension. Keeping the mapping here stops backend code from collecting
    // operating-system branches of its own.
    if cfg!(target_os = "windows") {
        format!("{library_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{library_name}.dylib")
    } else {
        format!("lib{library_name}.so")
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::scratch::{date_file, ScratchDirectory};

    /// The batch token: a file written after the batch invalidates it, a file
    /// written before it does not, and a missing directory counts as changed.
    #[test]
    fn source_newer_than_watches_file_times() {
        let directory = ScratchDirectory::new("batch_guard");
        let batch_time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // The walk also compares a subdirectory's own time, so the fixture stays
        // flat. Only the file then carries a dated time.
        let edited = directory.join("edited.rs");
        std::fs::write(&edited, "// edited").expect("write the test file");
        date_file(&edited, batch_time + Duration::from_secs(1));
        assert!(
            source_newer_than(&directory, batch_time),
            "a file dated after the batch must invalidate the token"
        );
        let after_write = batch_time + Duration::from_secs(2);
        assert!(
            !source_newer_than(&directory, after_write),
            "files dated before the batch keep the token valid"
        );
        assert!(
            source_newer_than(&directory.join("missing"), after_write),
            "a missing watch directory counts as changed"
        );
    }

    /// The token itself: it is refused once the sources move on, and used once.
    #[test]
    fn batch_validation_is_consumed_once() {
        let directory = ScratchDirectory::new("batch_token");
        let stale_moment = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let source = directory.join("lib.rs");
        std::fs::write(&source, "// module").expect("write the test file");
        date_file(&source, stale_moment + Duration::from_secs(1));
        with_batch_validation(|validation_slot| {
            *validation_slot = Some(BatchValidation {
                completed_at: stale_moment,
                names: ["batch_guard_expiring".to_string()].into_iter().collect(),
            });
        });
        assert!(
            !take_batch_validation("batch_guard_expiring", &directory),
            "a source newer than the batch must force the module's own build"
        );
        with_batch_validation(|validation_slot| {
            *validation_slot = Some(BatchValidation {
                completed_at: SystemTime::now(),
                names: ["batch_guard_once".to_string()].into_iter().collect(),
            });
        });
        assert!(take_batch_validation("batch_guard_once", &directory));
        assert!(
            !take_batch_validation("batch_guard_once", &directory),
            "the token belongs to one load only"
        );
    }

    /// An artifact another build wrote after the host's own must not be taken
    /// for host-built, however fresh its timestamp looks.
    ///
    /// This is the scenario behind an access violation during a load. The host
    /// builds an artifact, an unrelated `cargo build` overwrites it moments later
    /// with a different feature set, and every timestamp still looks fresh. The
    /// stamp is what tells the two apart.
    #[test]
    fn an_artifact_another_build_overwrote_is_not_host_built() {
        let (root, _) = test_workspace();
        let output = root.join("target/debug/module.dll");
        let command = vec![
            "cargo".to_string(),
            "build".to_string(),
            "--features".to_string(),
            "pill_engine/development".to_string(),
        ];

        record_artifact_stamp(
            &root,
            "module",
            "module/src",
            &command,
            std::slice::from_ref(&output),
        );
        assert!(
            artifacts_are_host_built(
                &root,
                "module",
                "module/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "the host's own artifact must be recognized"
        );

        // Another build writes a different library to the same path. It is
        // dated ahead of every source, so the modification-time check alone
        // would accept it.
        date_file(&output, SystemTime::now() + Duration::from_secs(3600));
        fs_write(&output, "a differently configured native library");
        assert!(
            !artifacts_are_host_built(
                &root,
                "module",
                "module/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "an artifact the host did not write must be rebuilt, not loaded"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The same artifact built under a different host identity is a different
    /// artifact, which is what makes the check feature-aware per module.
    ///
    /// The identity lives in each module's own stamp rather than in one shared
    /// marker. A host feature change must leave every module's stamp different.
    /// Otherwise a module keeps an artifact built against the engine variant the
    /// change just replaced, and loads it as os error 127 with no name in the
    /// message.
    #[test]
    fn a_different_host_identity_is_a_different_stamp() {
        let (root, _) = test_workspace();
        let output = root.join("target/debug/module.dll");
        let command = vec!["cargo".to_string(), "build".to_string()];
        let artifacts = std::slice::from_ref(&output);

        let windowed = artifact_stamp(
            "rustc 1.95.0
rendering+development",
            "module/src",
            &command,
            artifacts,
        )
        .expect("the artifact exists");
        let headless = artifact_stamp(
            "rustc 1.95.0
no-rendering+development",
            "module/src",
            &command,
            artifacts,
        )
        .expect("the artifact exists");

        assert_ne!(
            windowed, headless,
            "a host feature change must not leave a module's stamp unchanged"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The same artifact built by a different command is a different artifact,
    /// which is what makes the check feature-aware.
    #[test]
    fn a_different_build_command_is_not_host_built() {
        let (root, _) = test_workspace();
        let output = root.join("target/debug/module.dll");
        let with_feature = vec![
            "cargo".to_string(),
            "build".to_string(),
            "--features".to_string(),
            "pill_engine/development".to_string(),
        ];
        let without_feature = vec!["cargo".to_string(), "build".to_string()];

        record_artifact_stamp(
            &root,
            "module",
            "module/src",
            &with_feature,
            std::slice::from_ref(&output),
        );
        assert!(!artifacts_are_host_built(
            &root,
            "module",
            "module/src",
            &without_feature,
            std::slice::from_ref(&output)
        ));

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Two projects can share a package name, and therefore an output path, a
    /// stamp file and a build command. Only the watched sources differ.
    ///
    /// The test gives the same module name, command and artifact two different
    /// source folders. Without the source identity, one host would adopt an
    /// artifact it never built.
    #[test]
    fn a_different_project_with_the_same_package_name_is_not_host_built() {
        let root = ScratchDirectory::new("stamp_other_project");
        let output = root.join("project.dll");
        std::fs::write(&output, b"artifact").unwrap();
        let command = vec![
            "cargo".to_string(),
            "build".to_string(),
            "--package".to_string(),
            "project".to_string(),
        ];

        record_artifact_stamp(
            &root,
            "project",
            "../examples/project_rs/src",
            &command,
            std::slice::from_ref(&output),
        );

        assert!(
            artifacts_are_host_built(
                &root,
                "project",
                "../examples/project_rs/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "the project that produced the artifact must still recognise it"
        );
        assert!(
            !artifacts_are_host_built(
                &root,
                "project",
                "../devops/tests/project/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "a different project must not adopt an artifact it did not build"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A missing stamp or a missing artifact can never be host-built. Both fall
    /// through to a real build instead of being trusted.
    #[test]
    fn a_missing_stamp_or_artifact_is_not_host_built() {
        let (root, _) = test_workspace();
        let output = root.join("target/debug/module.dll");
        let command = vec!["cargo".to_string(), "build".to_string()];

        assert!(
            !artifacts_are_host_built(
                &root,
                "module",
                "module/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "no stamp has been recorded yet"
        );

        record_artifact_stamp(
            &root,
            "module",
            "module/src",
            &command,
            std::slice::from_ref(&output),
        );
        std::fs::remove_file(&output).unwrap();
        assert!(
            !artifacts_are_host_built(
                &root,
                "module",
                "module/src",
                &command,
                std::slice::from_ref(&output)
            ),
            "a stamped artifact that no longer exists must be rebuilt"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Every artifact a build produced is covered, not just the first. The same
    /// builds that overwrite the project's library also overwrite its `rlib`, and
    /// a patch that links the wrong one compiles against a differently configured
    /// engine.
    #[test]
    fn every_recorded_artifact_is_checked() {
        let (root, _) = test_workspace();
        let library = root.join("target/debug/module.dll");
        let rlib = root.join("target/debug/libmodule.rlib");
        fs_write(&rlib, "fake rlib");
        let command = vec!["cargo".to_string(), "build".to_string()];
        let artifacts = vec![library.clone(), rlib.clone()];

        record_artifact_stamp(&root, "module", "module/src", &command, &artifacts);
        assert!(artifacts_are_host_built(
            &root,
            "module",
            "module/src",
            &command,
            &artifacts
        ));

        // Only the second artifact moves, dated ahead of the stamp.
        fs_write(&rlib, "a differently configured rlib");
        date_file(&rlib, SystemTime::now() + Duration::from_secs(3600));
        assert!(
            !artifacts_are_host_built(&root, "module", "module/src", &command, &artifacts),
            "a replaced rlib must invalidate the build as surely as a replaced library"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A cancelled build must stop its whole tree, not just cargo.
    ///
    /// Cargo is the direct child, but the compiler and linker it starts are
    /// grandchildren. Killing the child leaves them running against the same
    /// shared build tree. There they hold cargo's package lock, so the next build
    /// blocks until its timeout and reads as an ignored edit, and they can write
    /// over an artifact between staging and loading. The job object makes the
    /// tree the unit of termination, so this test drives one through the same
    /// helpers `run_build_command` uses.
    #[cfg(windows)]
    #[test]
    fn cancelled_build_kills_the_whole_tree() {
        // `cmd /C` starts a long-running grandchild and waits for it. That gives
        // one direct child with one process below, the shape of a cargo build.
        let mut command = Command::new("cmd");
        command
            .args(["/C", "ping", "-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());

        let tree = create_process_tree();
        assert!(tree.is_some(), "a job object must be creatable here");
        let mut child = command.spawn().expect("cmd must spawn");
        join_process_tree(tree.as_ref(), &child);

        // Wait for the grandchild to join the job. That inheritance is the
        // property under test, so a tree of one process would prove nothing.
        let deadline = Instant::now() + Duration::from_secs(10);
        while tree.as_ref().expect("created above").active_processes() < 2 {
            assert!(
                Instant::now() < deadline,
                "the grandchild must join the job"
            );
            // A bounded wait between polls. This one waits on a live process,
            // not on a timestamp, so no explicit date can replace it.
            std::thread::park_timeout(Duration::from_millis(10));
        }

        stop_process_tree(tree.as_ref(), &mut child);

        let deadline = Instant::now() + Duration::from_secs(10);
        while tree.as_ref().expect("created above").active_processes() != 0 {
            assert!(Instant::now() < deadline, "the tree must be gone");
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }

    /// Creates a throwaway workspace containing one module, a path dependency,
    /// and a freshly produced artifact, under a scratch guard. Returns the guard,
    /// which removes the tree on drop, plus the module's manifest path.
    ///
    /// The artifact's modification time is dated explicitly, ahead of every
    /// input, instead of pausing between the writes. The stamp tests rest on
    /// "newer than the inputs", and dating the file makes that true without
    /// relying on clock resolution.
    fn test_workspace() -> (ScratchDirectory, PathBuf) {
        let root = ScratchDirectory::new("build_runner");

        // Workspace-level inputs shared by every member build.
        fs_write(&root.join("Cargo.toml"), "[workspace]\nmembers = []\n");
        fs_write(&root.join("Cargo.lock"), "# lockfile\n");
        fs_write(&root.join(".cargo/config.toml"), "");

        // The module itself, plus a path dependency it links.
        fs_write(&root.join("module/Cargo.toml"), DEPENDENT_MANIFEST);
        fs_write(
            &root.join("module/src/lib.rs"),
            "pub fn answer() -> u32 { 42 }\n",
        );
        fs_write(
            &root.join("dependency/Cargo.toml"),
            "[package]\nname = \"dependency\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        fs_write(
            &root.join("dependency/src/lib.rs"),
            "pub fn helper() -> u32 { 7 }\n",
        );

        // The artifact is produced last and dated ahead of every input.
        let output = root.join("target/debug/module.dll");
        fs_write(&output, "fake native library");
        date_file(&output, SystemTime::now() + Duration::from_secs(3600));

        let module_manifest = root.join("module/Cargo.toml");
        (root, module_manifest)
    }

    /// Writes a file, creating parent directories as needed.
    fn fs_write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// A module manifest that links the local path dependency.
    const DEPENDENT_MANIFEST: &str = r#"
[package]
name = "module"
version = "0.1.0"
edition = "2021"

[dependencies]
dependency = { path = "../dependency" }
"#;

    // =========================================================================
    // Flag capture tests
    // =========================================================================

    /// The three status lines `-v` adds are recognised with and without colour.
    ///
    /// Cargo colours its status words whenever stderr is a terminal, which is
    /// exactly when a person is reading the console. A filter that matched only
    /// the uncoloured spelling would dump every multi-kilobyte `rustc` command
    /// line in front of the person it was meant to spare.
    #[test]
    fn verbose_only_lines_are_recognised_with_and_without_colour() {
        for line in [
            "     Running `rustc --crate-name project`\n",
            "\u{1b}[0m\u{1b}[1m\u{1b}[32m     Running\u{1b}[0m `rustc --crate-name project`\n",
            "       Fresh pill_core v0.1.0\n",
            "\u{1b}[1m\u{1b}[32m       Fresh\u{1b}[0m pill_core v0.1.0\n",
            "       Dirty pill_spline v0.1.0: the list of features changed\n",
            "\u{1b}[1m\u{1b}[32m       Dirty\u{1b}[0m pill_spline v0.1.0: the file changed\n",
        ] {
            assert!(is_verbose_only_line(line), "must be filtered: {line:?}");
        }
        for line in [
            "   Compiling pill_core v0.1.0\n",
            "\u{1b}[1m\u{1b}[32m   Compiling\u{1b}[0m pill_core v0.1.0\n",
            "error[E0433]: failed to resolve\n",
            "warning: unused variable `x`\n",
            "\n",
        ] {
            assert!(
                !is_verbose_only_line(line),
                "must reach the console: {line:?}"
            );
        }
    }

    /// The module crate's invocation is picked out of cargo's verbose stream.
    /// Every other crate's invocation is ignored, because a patch must replay
    /// the flags of the crate it is patching, not of whatever else the build
    /// recompiled.
    #[test]
    fn the_harvest_picks_only_the_module_crate() {
        let other = r"     Running `C:\rustc.exe --crate-name pill_core --edition=2021 x.rs`";
        let wanted = r"     Running `C:\rustc.exe --crate-name project --edition=2021 y.rs`";
        assert!(crate::hot_patch::parse_rustc_line(other, "project").is_none());
        assert!(
            crate::hot_patch::parse_rustc_line("   Compiling project v0.1.0", "project").is_none()
        );
        let line = crate::hot_patch::parse_rustc_line(wanted, "project")
            .expect("the module crate's invocation");
        assert_eq!(line.program, r"C:\rustc.exe");
        assert!(line.args.contains(&"--edition=2021".to_string()));
    }

    /// The startup batch compiles several crates in one invocation, cold on a
    /// first start, so it gets more wall-clock time than one module's incremental
    /// build. One cap for both would kill a fresh checkout's batch.
    #[test]
    fn the_startup_batch_is_given_more_time_than_one_module() {
        assert!(STARTUP_BATCH_TIMEOUT > BUILD_TIMEOUT);
    }
}
