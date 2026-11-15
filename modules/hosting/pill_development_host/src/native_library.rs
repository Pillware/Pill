//! Loading native project and extension libraries, resolving their exports
//! and keeping the temporary copies that outlive a reload.
//!
//! Each module gets a file at a path of its own before it is opened: a hard
//! link to the build output when the filesystem allows one, a copy otherwise.
//! A rebuilt DLL can then replace the build output while an older generation
//! stays mapped, because the raw pointers and vtables the engine holds still
//! point into it.
//!
//! # Responsibilities
//!
//! - Load native libraries from unique temporary paths.
//! - Check the required exports, read the ABI revision and call the
//!   registration entry point.
//! - Explain a failed load from the artifact's own import tables.
//! - Remove temporary copies left behind by earlier host processes.
//!
//! # Design
//!
//! The native ABI is one fixed export contract. A project and an extension are
//! the same kind of loadable artifact: same exports, same loader, same reload
//! transaction, same way of being retired. The loader therefore has nothing to
//! parameterise.
//!
//! - `pill_module_init(*const EngineApi) -> u32` - required: registers
//!   components and systems before the first frame and returns zero on
//!   success. Any other status fails the load transaction, and the previous
//!   generation is initialized again.
//! - `pill_module_abi_version() -> u32` - optional: read before any other
//!   function is called, so an incompatible artifact is rejected instead of
//!   invoked.
//!
//! Exports are cached at load time as raw function pointers, which stay valid
//! while the library handle keeps the image mapped. `pill_module_init` must be
//! safe to call more than once.

// Standard library
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

// External crates
#[cfg(not(windows))]
use libloading::os::unix as unix_loader;
#[cfg(windows)]
use libloading::os::windows as windows_loader;
use libloading::{Library, Symbol};
use pill_core::error::{LibraryError, ModuleError};
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, info, warn};
use pill_engine::component_registry::{
    PillFieldAccessorDescriptor, PillMethodDescriptor, PillValueTypeDescriptor,
};
use pill_engine::EngineApi;

// Current crate
use crate::analytics;
use crate::csharp::{ResolvedFieldAccessor, ResolvedMirrorMethod};

// =============================================================================
// Constants
// =============================================================================

/// The engine's shared libraries, by file stem: every module, project and
/// patch imports them, and each one exists once per process unless a module
/// needs its own copies (see [`engine_dylib_needs_isolation`]).
///
/// Each has to stay a single instance in the process, so the types that cross
/// the library boundary and the state they share are not duplicated: the UI
/// frame and draw data (`pill_ui_frame`), the in-game egui UI and its editor
/// core (`pill_project_ui`, `pill_editor_core`), and the `Theme`/`Handle`s of
/// `pill_ui_theme`. A reload must find the same instance the host loaded, so
/// the whole set is staged together.
pub(crate) const ENGINE_DYLIB_STEMS: [&str; 6] = [
    "pill_core",
    "pill_engine_core",
    "pill_ui_frame",
    "pill_project_ui",
    "pill_editor_core",
    "pill_ui_theme",
];

/// File name the loader looks up for the engine dylib `stem`.
///
/// The convention is the platform's rather than this crate's: `pill_core.dll`
/// on Windows, `libpill_core.so` on Linux, `libpill_core.dylib` on macOS.
/// `std::env::consts` spells it, and every path built for an engine dylib goes
/// through here - the build output, the host's copy, the staged copy and the
/// two paths a mismatch report names - so all of them always agree.
fn engine_dylib_file_name(stem: &str) -> String {
    format!(
        "{}{stem}{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// The engine dylib stem a library name belongs to, if any.
///
/// The evidence a failed load is explained from names a library the way its
/// own format does: a PE import table says `pill_core.dll` whichever host
/// reads it, and an ELF `DT_NEEDED` entry says `libpill_core.so`. Both
/// spellings are accepted, so evidence from either format maps to the same
/// stem.
fn engine_dylib_stem(library_name: &str) -> Option<&'static str> {
    ENGINE_DYLIB_STEMS.into_iter().find(|stem| {
        library_name.eq_ignore_ascii_case(&engine_dylib_file_name(stem))
            || library_name.eq_ignore_ascii_case(&format!("{stem}.dll"))
    })
}

/// Whether staging engine dylibs beside a module can change what the loader
/// maps.
///
/// Only a loader that searches the loaded library's own directory can be
/// steered that way, and that is what
/// [`LOAD_WITH_ALTERED_SEARCH_PATH`] asks Windows for. A Unix loader resolves
/// a `DT_NEEDED` entry by name against the objects already mapped in the
/// process *before* it searches any path, and this host maps the engine dylibs
/// at startup, so a copy beside the module is never consulted. Staging there
/// would copy hundreds of megabytes per load for nothing, and the mismatch is
/// reported instead - see [`LoadFailureExplanation::EngineDylibMismatch`].
fn engine_dylib_staging_steers_the_loader() -> bool {
    cfg!(windows)
}

/// Engine dylibs this process took from the module world because the host
/// binary does not link them, and the stems already tried.
///
/// They stay mapped for the life of the process: module code holds raw
/// pointers and vtables into them, so unloading one would leave those
/// dangling. This table owns the handles rather than leaking them.
///
/// The set cannot be decided once for the process. Which engine dylibs are
/// missing depends on which ones the binary links, but which ones a *module*
/// needs is a property of that module, so a second module can name one the
/// first never mentioned. Each load therefore adds the stems its own module
/// names, and `attempted` keeps a stem that could not be loaded from being
/// retried on every reload.
#[cfg(not(windows))]
#[derive(Default)]
struct PreloadedEngineDylibs {
    /// Handles to the dylibs this process preloaded.
    libraries: Vec<Library>,
    /// Stems whose preload has been attempted, successful or not.
    attempted: std::collections::HashSet<&'static str>,
}

/// The process-wide [`PreloadedEngineDylibs`] table.
#[cfg(not(windows))]
static PRELOADED_ENGINE_DYLIBS: std::sync::OnceLock<std::sync::Mutex<PreloadedEngineDylibs>> =
    std::sync::OnceLock::new();

/// Directory, under the workspace root, where temporary native-library copies
/// are stored.
const TEMPORARY_DIRECTORY: &str = "pill_standalone_temp";

/// How long another process's staging directory is left alone after its last
/// write.
///
/// This is the liveness check. A running host stages a fresh copy of every DLL
/// it loads or reloads, so its directory's modification time keeps moving. One
/// that has not been written to for this long belonged to a process that is
/// gone. [`process_is_alive`] answers exactly on Linux, where the probe needs
/// no process-table walk; everywhere else it reports "not alive", and the
/// clock decides.
///
/// The window is generous on purpose. Waiting costs a few megabytes of disk.
/// Guessing wrong deletes the staging of a host that is still running, and on
/// Windows that deletion only half-succeeds: it takes the files that are not
/// mapped and leaves the ones that are.
const STAGING_GRACE_PERIOD: Duration = Duration::from_secs(60 * 60);

/// Maximum number of other processes' staging directories kept.
///
/// This is the backstop for the grace period above. Without it, a developer who
/// starts many hosts in one hour keeps one directory per host until the hour is
/// up. The oldest directories are removed first. That order is also what makes
/// the limit safe to apply inside the grace period: a live host's directory is
/// the most recently written of them all, so it is the last one this limit
/// would reach.
const MAX_RETAINED_STAGING_DIRECTORIES: usize = 8;

/// Monotonic suffix ensuring temporary copies never collide, even when the
/// system clock repeats or moves backwards.
static TEMPORARY_COPY_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `LOAD_WITH_ALTERED_SEARCH_PATH`: make a module's dependency resolution start
/// from the directory that contains the module, so it loads the engine dylibs
/// staged beside it rather than the host's copies from the executable
/// directory.
#[cfg(windows)]
const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x0000_0008;

/// Required registration entry point of a loadable artifact.
///
/// One name covers both kinds. A project and an extension export the same
/// symbols, load the same way, reload through the same transaction and are
/// retired the same way. The differences that are real - a project is
/// singular where modules form an ordered list, and a module's components are
/// mirrored to C# - are policy, and live in the config and the reload
/// transaction rather than in the loader.
const MODULE_INIT_SYMBOL: &[u8] = b"pill_module_init";
/// Optional: an artifact's registration-only entry point, called instead of
/// `pill_module_init` on every generation after the first.
const MODULE_REGISTER_SYMBOL: &[u8] = b"pill_module_register";
/// Optional ABI revision export, read at load time when present.
const MODULE_ABI_VERSION_SYMBOL: &[u8] = b"pill_module_abi_version";

/// Export that lets a patch reach an artifact's redirect slots.
const PLAIN_FUNCTION_INSTALL_SYMBOL: &[u8] = b"pill_hot_resolve_install";

/// Export that returns one redirect slot to the artifact's own body.
const PLAIN_FUNCTION_RESET_SYMBOL: &[u8] = b"pill_hot_resolve_reset";

/// Export that reports any function address in this artifact.
const FUNCTION_ADDRESS_SYMBOL: &[u8] = b"pill_hot_resolve_address";

/// Export reporting how many of an artifact's functions have a known length.
const EXTENT_COVERAGE_SYMBOL: &[u8] = b"pill_hot_resolve_extent_coverage";

/// Export reporting how many value-type descriptors an artifact declares.
const VALUE_TYPE_COUNT_SYMBOL: &[u8] = b"pill_value_type_descriptor_count";

/// Export copying an artifact's value-type descriptors into a host buffer.
const VALUE_TYPE_COPY_SYMBOL: &[u8] = b"pill_copy_value_type_descriptors";

/// Export reporting how many mirrored-method descriptors an artifact declares.
const MIRROR_METHOD_COUNT_SYMBOL: &[u8] = b"pill_mirror_method_descriptor_count";

/// Export copying an artifact's mirrored-method descriptors into a host buffer.
const MIRROR_METHOD_COPY_SYMBOL: &[u8] = b"pill_copy_mirror_method_descriptors";

/// Export reporting how many heap-field accessor descriptors an artifact
/// declares.
const FIELD_ACCESSOR_COUNT_SYMBOL: &[u8] = b"pill_field_accessor_descriptor_count";

/// Export copying an artifact's heap-field accessor descriptors into a host
/// buffer.
const FIELD_ACCESSOR_COPY_SYMBOL: &[u8] = b"pill_copy_field_accessor_descriptors";

/// Whether a module build produced engine dylibs that differ from the host's
/// copies, which means the module must load against its own.
///
/// When every pair is byte-identical (a plain CLI host whose feature set
/// matches the module build), the module keeps loading the host's single
/// instances. Only a host whose dependency graph adds different features to
/// the shared engine crates (a GUI frontend) needs the isolated copies. The
/// whole set is staged together: a module's `pill_engine_core.dll` imports
/// `pill_core.dll`, and the UI dylibs share type identity across the boundary.
/// A module that resolved some engine dylibs from one build and the rest from
/// another would get mismatched symbol hashes and two `Theme` types.
fn engine_dylib_needs_isolation(workspace_root: &Path) -> bool {
    first_engine_dylib_mismatch(workspace_root).is_some()
}

/// The first engine dylib whose module-world copy differs from the host's, as
/// `(host copy, module copy)`; `None` when every module-world copy matches the
/// host's or no module build has produced one.
fn first_engine_dylib_mismatch(workspace_root: &Path) -> Option<(PathBuf, PathBuf)> {
    ENGINE_DYLIB_STEMS.iter().find_map(|stem| {
        let module_engine = module_world_engine_dylib(workspace_root, stem)?;
        let host_engine = workspace_root
            .join(crate::config::host_target_directory())
            .join(engine_dylib_file_name(stem));
        let copies_match = host_engine.is_file() && files_equal(&module_engine, &host_engine);
        (!copies_match).then_some((host_engine, module_engine))
    })
}

/// A named import the module needs that a candidate copy of its library does
/// not export, with the copy the loader would try first.
#[derive(Debug, PartialEq, Eq)]
struct UnresolvedImport {
    /// The library as the image names it: a PE import table's `pill_core.dll`,
    /// or an ELF `DT_NEEDED`'s `libpill_core.so`.
    dll: String,
    /// The symbol the module needs from it.
    symbol: String,
    /// The copy the loader would resolve for that library, when any exists.
    path: PathBuf,
}

/// What the gathered evidence says about a failed load.
#[derive(Debug, PartialEq, Eq)]
enum LoadFailureExplanation {
    /// A named import no candidate copy of its library exports; the concrete
    /// answer, naming the library and the symbol.
    MissingImport(UnresolvedImport),
    /// An engine dylib the module was linked against differs from the host's
    /// copy, and the unresolved import belongs to it (or nothing else
    /// explains the failure).
    EngineDylibMismatch {
        host_engine: PathBuf,
        module_engine: PathBuf,
    },
    /// Nothing read from the tables explains it; keep the loader's own words.
    Unknown,
}

/// The explanation the gathered evidence supports.
///
/// Usually the missing import wins, because it names both a symbol and a
/// file. It gives way to the engine-dylib explanation only when the import
/// belongs to an engine dylib whose module-world copy really differs; that
/// mismatch is then what makes the symbol unresolvable, and the answer names
/// both builds. The function is pure, so the decision can be unit-tested
/// without files on disk.
///
/// `unresolved_mismatch` is the mismatch pair for the unresolved import's own
/// engine dylib, when it is one and it differs. `fallback_mismatch` is the
/// first differing engine pair, used when there is no unresolved import.
fn choose_explanation(
    unresolved: Option<UnresolvedImport>,
    unresolved_mismatch: Option<(PathBuf, PathBuf)>,
    fallback_mismatch: Option<(PathBuf, PathBuf)>,
) -> LoadFailureExplanation {
    match unresolved {
        Some(unresolved) => match unresolved_mismatch {
            Some((host_engine, module_engine)) => LoadFailureExplanation::EngineDylibMismatch {
                host_engine,
                module_engine,
            },
            None => LoadFailureExplanation::MissingImport(unresolved),
        },
        None => match fallback_mismatch {
            Some((host_engine, module_engine)) => LoadFailureExplanation::EngineDylibMismatch {
                host_engine,
                module_engine,
            },
            None => LoadFailureExplanation::Unknown,
        },
    }
}

/// The error one explanation becomes, or `None` to keep the loader's own.
fn explanation_error(
    module_name: &str,
    explanation: LoadFailureExplanation,
) -> Option<LibraryError> {
    match explanation {
        LoadFailureExplanation::MissingImport(unresolved) => Some(LibraryError::MissingImport {
            subject: module_name.to_string(),
            dll: unresolved.dll,
            symbol: unresolved.symbol,
            path: unresolved.path.display().to_string(),
        }),
        LoadFailureExplanation::EngineDylibMismatch {
            host_engine,
            module_engine,
        } => Some(LibraryError::EngineDylibMismatch {
            subject: module_name.to_string(),
            host_engine: host_engine.display().to_string(),
            module_engine: module_engine.display().to_string(),
        }),
        LoadFailureExplanation::Unknown => None,
    }
}

/// The first import of `artifact` that a candidate copy of its library does
/// not export.
///
/// Two kinds of evidence answer, because the two image formats record
/// different things.
///
/// A PE import table attributes every symbol to the DLL it comes from, so the
/// first step reads those pairs straight out of it. An ELF image does not
/// attribute: an undefined symbol is resolved by searching `DT_NEEDED` in
/// order, so the table says which names are needed, never which library
/// defines them. The second step turns the question around for the engine
/// dylibs the artifact names - a symbol this build of the dylib exports, the
/// host's copy does not, and the artifact needs, is one that cannot resolve
/// against the copy the process mapped.
///
/// The candidates are the copies the loader would consider: the host's own
/// directory (a library the process already mapped wins by name), the
/// directory holding the loaded module, and the staged hot directory that the
/// module builds write. A library whose copies are all missing from disk is
/// skipped, because then only the loader's own message can explain the
/// failure.
fn first_unresolved_import(artifact: &Path, workspace_root: &Path) -> Option<UnresolvedImport> {
    let inspection = crate::native_binary::inspect(artifact)?;

    // Step 1: An import table that names the library each symbol comes from.
    for import in &inspection.imports {
        if import.symbols.is_empty() {
            continue;
        }
        let candidates = import_candidate_copies(&import.library, artifact, workspace_root);
        for candidate in candidates {
            if !candidate.is_file() {
                continue;
            }
            let Some(candidate_symbols) = exported_symbols(&candidate) else {
                continue;
            };
            let missing_symbol = import
                .symbols
                .iter()
                .find(|symbol| !candidate_symbols.contains(*symbol));
            if let Some(symbol) = missing_symbol {
                return Some(UnresolvedImport {
                    dll: import.library.clone(),
                    symbol: symbol.clone(),
                    path: candidate,
                });
            }
        }
    }

    // Step 2: No attribution in the image, so compare its two builds directly.
    for stem in ENGINE_DYLIB_STEMS {
        let library_name = engine_dylib_file_name(stem);
        if !inspection
            .libraries
            .iter()
            .any(|library| library.eq_ignore_ascii_case(&library_name))
        {
            continue;
        }
        let Some(module_engine) = module_world_engine_dylib(workspace_root, stem) else {
            continue;
        };
        let Some(module_symbols) = exported_symbols(&module_engine) else {
            continue;
        };
        for candidate in import_candidate_copies(&library_name, artifact, workspace_root) {
            if !candidate.is_file() {
                continue;
            }
            let Some(candidate_symbols) = exported_symbols(&candidate) else {
                continue;
            };
            let missing_symbol = inspection.required_symbols.iter().find(|symbol| {
                module_symbols.contains(*symbol) && !candidate_symbols.contains(*symbol)
            });
            if let Some(symbol) = missing_symbol {
                return Some(UnresolvedImport {
                    dll: library_name.clone(),
                    symbol: symbol.clone(),
                    path: candidate,
                });
            }
        }
    }

    None
}

/// The symbol names `path` exports, or `None` when it cannot be read.
fn exported_symbols(path: &Path) -> Option<HashSet<String>> {
    let inspection = crate::native_binary::inspect(path)?;
    Some(inspection.exported_symbols.into_iter().collect())
}

/// The copies of an imported library the loader would consider, in its own
/// order: a copy the process already maps (the host's engines), one beside
/// the loaded module, one in the staged hot directory.
fn import_candidate_copies(dll: &str, artifact: &Path, workspace_root: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    candidates.push(
        workspace_root
            .join(crate::config::host_target_directory())
            .join(dll),
    );
    if let Some(artifact_directory) = artifact.parent() {
        candidates.push(artifact_directory.join(dll));
    }
    candidates.push(
        workspace_root
            .join(crate::build_runner::PROJECT_HOT_OUTPUT_SUBDIRECTORY)
            .join(dll),
    );
    candidates
}

/// The mismatch pair for `unresolved`'s engine dylib, when the import names
/// one and its module-world copy differs from the host's.
fn unresolved_engine_mismatch(
    unresolved: &UnresolvedImport,
    workspace_root: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let stem = engine_dylib_stem(&unresolved.dll)?;
    let module_engine = module_world_engine_dylib(workspace_root, stem)?;
    let host_engine = workspace_root
        .join(crate::config::host_target_directory())
        .join(engine_dylib_file_name(stem));
    let copies_match = host_engine.is_file() && files_equal(&module_engine, &host_engine);
    (!copies_match).then_some((host_engine, module_engine))
}

/// Explains a load failure with the best evidence available.
///
/// The module's own import and export tables are read first. A symbol that no
/// candidate copy of its library exports is a concrete answer that names both
/// the library and the symbol, where the loader only said "procedure could not
/// be found" (Windows) or `undefined symbol: <name>` (Linux). The engine-dylib
/// explanation is used only for the cases it truly covers: the unresolved
/// import belongs to the mismatching dylib, or nothing else explains the
/// failure.
fn diagnose_load_failure(
    error: LibraryError,
    artifact: &Path,
    workspace_root: &Path,
    module_name: &str,
) -> LibraryError {
    // Only a load failure needs explaining. A missing export or a failed copy
    // has already said something specific and true.
    if !matches!(error, LibraryError::LoadFailed { .. }) {
        return error;
    }
    let unresolved = first_unresolved_import(artifact, workspace_root);
    let unresolved_mismatch = unresolved
        .as_ref()
        .and_then(|unresolved| unresolved_engine_mismatch(unresolved, workspace_root));
    let fallback_mismatch = first_engine_dylib_mismatch(workspace_root);
    let explanation = choose_explanation(unresolved, unresolved_mismatch, fallback_mismatch);
    match &explanation {
        LoadFailureExplanation::MissingImport(unresolved) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = module_name,
                dll = unresolved.dll.as_str(),
                symbol = unresolved.symbol.as_str(),
                path = %unresolved.path.display(),
                "load failed: the module imports a symbol the copy of its DLL does not export"
            );
        }
        LoadFailureExplanation::EngineDylibMismatch { .. } => {
            // Differing bytes are not proof. Two builds of one graph can still
            // differ, and then only the loader's own message says why. That
            // message is logged here because the returned error replaces it.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = module_name,
                error = %error,
                "load failed while the engine dylibs differ; the loader reported this"
            );
        }
        LoadFailureExplanation::Unknown => {}
    }
    explanation_error(module_name, explanation).unwrap_or(error)
}

/// Copies the module-world engine dylibs into `generation_directory`, so a
/// module loaded from there finds them next to itself instead of using the
/// host's copies.
///
/// Only called where [`engine_dylib_staging_steers_the_loader`] says a copy
/// beside the module can win; elsewhere the copies would never be consulted.
fn stage_module_engine_dylib(workspace_root: &Path, generation_directory: &Path) {
    if std::fs::create_dir_all(generation_directory).is_err() {
        return;
    }
    for stem in ENGINE_DYLIB_STEMS {
        let Some(engine_library_source) = module_world_engine_dylib(workspace_root, stem) else {
            continue;
        };
        let _ = std::fs::copy(
            &engine_library_source,
            generation_directory.join(engine_dylib_file_name(stem)),
        );

        // The build stages the engine dylib in `target/hot`, but its PDB stays
        // in the private module artifact directory beside the other
        // host-spawned outputs. Keep the engine's symbols beside the dylib that
        // was copied here too.
        let direct_symbol_source = engine_library_source.with_extension("pdb");
        let artifact_symbol_source = workspace_root
            .join(crate::config::module_build_artifact_directory())
            .join(format!("{stem}.pdb"));
        let symbol_source = if direct_symbol_source.is_file() {
            direct_symbol_source
        } else {
            artifact_symbol_source
        };
        if symbol_source.is_file() {
            let symbol_target = generation_directory.join(format!("{stem}.pdb"));
            let _ = std::fs::hard_link(&symbol_source, &symbol_target)
                .or_else(|_| std::fs::copy(symbol_source, symbol_target).map(|_| ()));
        }
    }
}

/// Locates the engine dylib `stem` that the module build produced: the staged
/// hot-load copy when a build has run, otherwise the one still in the private
/// build directory.
fn module_world_engine_dylib(workspace_root: &Path, stem: &str) -> Option<PathBuf> {
    let file_name = engine_dylib_file_name(stem);
    let staged_library_path = workspace_root
        .join(crate::build_runner::PROJECT_HOT_OUTPUT_SUBDIRECTORY)
        .join(&file_name);
    if staged_library_path.is_file() {
        return Some(staged_library_path);
    }
    let built_library_path = workspace_root
        .join(crate::config::module_build_artifact_directory())
        .join(&file_name);
    built_library_path.is_file().then_some(built_library_path)
}

/// The engine dylibs this process already has mapped.
///
/// The executable names some in its own `DT_NEEDED`, and those name others, so
/// the set is walked through the host's build tree rather than read off one
/// table. Only engine dylibs are followed: nothing else can be preloaded here,
/// and stopping at them keeps the walk to a handful of files.
#[cfg(not(windows))]
fn mapped_engine_dylibs(workspace_root: &Path) -> HashSet<&'static str> {
    let host_directory = workspace_root.join(crate::config::host_target_directory());
    let mut mapped = HashSet::new();
    let mut pending: Vec<String> = std::env::current_exe()
        .ok()
        .and_then(|executable| crate::native_binary::needed_libraries(&executable))
        .unwrap_or_default();
    while let Some(library_name) = pending.pop() {
        let Some(stem) = engine_dylib_stem(&library_name) else {
            continue;
        };
        if !mapped.insert(stem) {
            continue;
        }
        // An engine dylib names the ones below it, which were mapped with it.
        if let Some(libraries) =
            crate::native_binary::needed_libraries(&host_directory.join(&library_name))
        {
            pending.extend(libraries);
        }
    }
    mapped
}

/// Loads the module-world engine dylibs the host binary does not link.
///
/// A module's `DT_NEEDED` reaches every engine dylib it was linked against,
/// while the host binary links only the ones its own frontend needs: a project
/// that draws its own egui names `pill_project_ui`, which the editor never
/// links. That dylib is in no link map at all, so the loader would search the
/// host's target directory for it, where cargo never wrote a copy - the module
/// build is the only thing that produces one.
///
/// Loading the module build's copy here, by path and with `RTLD_GLOBAL`, puts it
/// in the scope the module's entry is resolved against. A Rust cdylib carries a
/// `DT_SONAME` equal to its file name, which is what makes the name match.
///
/// Only the stems `module` itself names are tried. A host that links none of
/// them - the renderer host maps no UI dylib - would otherwise pull the whole
/// module-world set into its process for a module that needs none of it, and a
/// second copy of `pill_ui_frame` is exactly what the one-instance rule in
/// [`ENGINE_DYLIB_STEMS`] forbids.
///
/// Best effort: a copy that cannot be loaded is logged and left to the
/// diagnosis, which names the two builds it compared.
#[cfg(not(windows))]
fn preload_unmapped_engine_dylibs(workspace_root: &Path, module: &Path) {
    // Step 1: The stems this module asks for. An image that cannot be read
    // leaves the whole set to be tried, because a load that is about to fail
    // for a missing engine dylib must not fail for a missing preload as well.
    let needed = crate::native_binary::needed_libraries(module);
    let named = |stem: &str| match &needed {
        Some(libraries) => libraries
            .iter()
            .any(|library| engine_dylib_stem(library) == Some(stem)),
        None => true,
    };

    // Step 2: The stems still worth trying. `attempted` is updated here, under
    // the lock, so a stem is tried once per process however many modules name
    // it, and a failure cannot turn into a warning on every reload. A poisoned
    // lock means an earlier preload panicked; its table is still the best
    // record there is, so it is taken back rather than abandoned.
    let wanted = {
        let mut table = match PRELOADED_ENGINE_DYLIBS.get_or_init(Default::default).lock() {
            Ok(table) => table,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mapped = mapped_engine_dylibs(workspace_root);
        let mut wanted = Vec::new();
        for stem in ENGINE_DYLIB_STEMS {
            if mapped.contains(stem) || !named(stem) {
                continue;
            }
            if table.attempted.insert(stem) {
                wanted.push(stem);
            }
        }
        wanted
    };

    // Step 3: Load each one. The lock is released first: a mapping can run
    // static initializers, and none of them may block on this table.
    let mut preloaded = Vec::new();
    for stem in wanted {
        let Some(module_engine) = module_world_engine_dylib(workspace_root, stem) else {
            continue;
        };
        // SAFETY: `module_engine` is a dylib this workspace's module build
        // just wrote, opened by absolute path. `RTLD_NOW` resolves its own
        // imports eagerly, and `RTLD_GLOBAL` puts it in the scope a later
        // module's `DT_NEEDED` entry is resolved against. The handle is kept
        // in [`PRELOADED_ENGINE_DYLIBS`], so it outlives every module that
        // resolves against it.
        let loaded = unsafe {
            unix_loader::Library::open(
                Some(module_engine.as_os_str()),
                unix_loader::RTLD_GLOBAL | unix_loader::RTLD_NOW,
            )
        };
        match loaded {
            Ok(library) => {
                debug!(
                    target: telemetry_target::HOT_RELOAD,
                    stem,
                    path = %module_engine.display(),
                    "preloaded an engine dylib this host does not link"
                );
                preloaded.push(Library::from(library));
            }
            Err(error) => warn!(
                target: telemetry_target::HOT_RELOAD,
                stem,
                path = %module_engine.display(),
                %error,
                "an engine dylib this host does not link could not be preloaded"
            ),
        }
    }

    // Step 4: Own the handles for the life of the process.
    if preloaded.is_empty() {
        return;
    }
    let mut table = match PRELOADED_ENGINE_DYLIBS.get_or_init(Default::default).lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    };
    table.libraries.append(&mut preloaded);
}

/// Byte equality for two native library files.
///
/// The length is checked first, because both files are multi-megabyte engine
/// dylibs compared on every failed `load_copy`, and a length mismatch already
/// proves inequality. Everything else goes through
/// [`files_equal_streaming`].
///
/// Timestamps cannot decide: a copied file keeps or renews its timestamp while
/// staying byte-identical, and two files can share one while differing. The
/// answer decides whether a module shares the host's engine instance or gets
/// its own staged copy. A wrong "different" would map `pill_core` twice
/// and break the one-instance contract the engine's raw pointers rely on, so
/// only exact bytes may answer "equal".
fn files_equal(left: &Path, right: &Path) -> bool {
    if let (Ok(left_metadata), Ok(right_metadata)) =
        (std::fs::metadata(left), std::fs::metadata(right))
    {
        if left_metadata.len() != right_metadata.len() {
            return false;
        }
    }
    // Metadata that cannot be read is not an answer, so fall through instead
    // of reporting "different": the streaming compare still answers, even for
    // a file that is locked against writing.
    files_equal_streaming(left, right)
}

/// Exact byte compare of two files through two reused buffers.
///
/// The fallback for [`files_equal`] once metadata has not already answered;
/// exits on the first differing chunk, so a differing pair costs one 64 KiB
/// read per side instead of a whole-file read on each.
fn files_equal_streaming(left: &Path, right: &Path) -> bool {
    use std::io::Read;

    let (Ok(mut left_file), Ok(mut right_file)) =
        (std::fs::File::open(left), std::fs::File::open(right))
    else {
        return false;
    };
    let mut left_buffer = vec![0u8; 64 * 1024];
    let mut right_buffer = vec![0u8; 64 * 1024];
    loop {
        let left_read = match left_file.read(&mut left_buffer) {
            Ok(read) => read,
            Err(_) => return false,
        };
        let right_read = match right_file.read(&mut right_buffer) {
            Ok(read) => read,
            Err(_) => return false,
        };
        if left_read != right_read {
            return false;
        }
        if left_read == 0 {
            return true;
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            return false;
        }
    }
}

// =============================================================================
// Types + Impls
// =============================================================================

/// Signature of the required registration entry point.
///
/// Returns zero on success; any non-zero status reports a registration
/// failure and keeps the previous generation loaded.
type ModuleInitFn = unsafe extern "C" fn(*const EngineApi) -> u32;

/// Signature of the `pill_hot_resolve_install` export generated by the
/// `#[pill_module]` and `#[pill_project]` macros.
///
/// Takes the function's qualified path, the replacement's address and the
/// signature text the replacement was compiled with. Returns 0 when installed,
/// 1 when this artifact declares no such function, and 2 when the signature no
/// longer matches - a non-zero result always leaves the running implementation
/// untouched.
type PlainFunctionInstallFn =
    unsafe extern "C" fn(*const std::ffi::c_char, usize, *const std::ffi::c_char) -> u32;

/// Signature of the `pill_hot_resolve_reset` export generated by the same
/// macros.
///
/// Takes the function's qualified path and returns 0 when this artifact
/// emptied its slot, 1 when it declares no such function.
type PlainFunctionResetFn = unsafe extern "C" fn(*const std::ffi::c_char) -> u32;

/// Signature of the build-script-backed address resolver.
type FunctionAddressFn =
    unsafe extern "C" fn(*const std::ffi::c_char, *mut *const u8, *mut usize) -> usize;

/// Signature of the extent-coverage diagnostic export.
type ExtentCoverageFn = unsafe extern "C" fn() -> u64;

/// Signature of the optional ABI revision export.
type ModuleAbiVersionFn = unsafe extern "C" fn() -> u32;

/// Signature of the optional value-type manifest exports.
///
/// The module artifact reports how many `#[derive(PillMirror)]` descriptors it
/// carries, then copies that many into a host-owned buffer. Both sides share
/// the `pill_engine` rlib, so `PillValueTypeDescriptor` has one layout.
type ValueTypeCountFn = unsafe extern "C" fn() -> u32;
type CopyValueTypesFn = unsafe extern "C" fn(*mut PillValueTypeDescriptor, u32) -> u32;

/// Signature of the optional mirrored-method manifest exports.
///
/// Same shape as the value-type manifest: the artifact reports how many
/// `#[pill_mirror_method]` descriptors it carries, then copies that many into
/// a host-owned buffer. Each descriptor carries the address of the
/// `#[no_mangle]` trampoline it was compiled with, so the host resolves
/// nothing by symbol.
type MirrorMethodCountFn = unsafe extern "C" fn() -> u32;
type CopyMirrorMethodsFn = unsafe extern "C" fn(*mut PillMethodDescriptor, u32) -> u32;

/// Signature of the optional heap-field accessor manifest exports.
///
/// Same shape again: the artifact reports how many accessor descriptors it
/// carries, then copies that many into a host-owned buffer. Each descriptor
/// names the `#[no_mangle]` trampolines the artifact exports for one `Vec` or
/// `String` component field, which the host resolves by symbol for the
/// callable addresses.
type FieldAccessorCountFn = unsafe extern "C" fn() -> u32;
type CopyFieldAccessorsFn = unsafe extern "C" fn(*mut PillFieldAccessorDescriptor, u32) -> u32;

/// Owns one loaded native library, either the project or an extension.
///
/// Export symbols are resolved once during loading and stored as raw function
/// pointers. The pointers stay valid for as long as the `library` field keeps
/// the module mapped, so frame-loop calls never perform a dynamic lookup or
/// panic on a missing optional export.
pub(crate) struct NativeLibrary {
    /// Loaded module handle; keeps the native library mapped in memory.
    library: Option<Library>,
    /// Declarations the process already held when this image was mapped.
    ///
    /// The entry points register declarations from this position, so a
    /// reloaded generation declares its own systems, resources and asset types
    /// and leaves the retired generation's - whose image the host keeps mapped
    /// - alone.
    declaration_baseline: pill_engine::declarations::DeclarationBaseline,
    /// Required registration entry point, resolved once at load time.
    module_init: ModuleInitFn,
    /// Optional registration-only entry point, resolved once at load time.
    ///
    /// `None` means the artifact predates the split and must be called through
    /// `pill_module_init` on every generation.
    module_register: Option<ModuleInitFn>,
    /// ABI revision the module reports, when it exports one.
    abi_version: Option<u32>,
    /// Optional value-type manifest exports (`#[derive(PillMirror)]`), used by
    /// the C# mirror codegen to resolve nested struct tags.
    value_type_count: Option<ValueTypeCountFn>,
    copy_value_types: Option<CopyValueTypesFn>,
    /// Optional mirrored-method manifest exports (`#[pill_mirror_impl]`),
    /// used by the C# mirror codegen and runtime method table.
    mirror_method_count: Option<MirrorMethodCountFn>,
    copy_mirror_methods: Option<CopyMirrorMethodsFn>,
    /// Optional heap-field accessor manifest exports (`#[derive(PillComponent)]`
    /// components with `Vec`/`String` fields), used by the C# mirror codegen
    /// and the managed runtime's method table.
    field_accessor_count: Option<FieldAccessorCountFn>,
    copy_field_accessors: Option<CopyFieldAccessorsFn>,
    /// Temporary copy backing this library; deleted when the library drops.
    temporary_path: PathBuf,
}

/// A copied artifact path that deletes itself if the load never takes it over.
///
/// `load_copy` writes the copy before the library exists, so without the guard
/// a rejected load leaves the file behind for the life of the process: the
/// `Drop` that removes it lives on `NativeLibrary`, which the failure path
/// never constructs. The guard owns the path from the moment the copy succeeds
/// and is disarmed only once the library owns the file.
struct TemporaryCopy {
    /// The copied file, or `None` once the library has taken it over.
    path: Option<PathBuf>,
}

impl TemporaryCopy {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// The copied file, which the guard still owns.
    ///
    /// # Panics
    ///
    /// Panics once the guard has handed the file to the library.
    fn path(&self) -> &Path {
        self.path.as_deref().expect("the copy is disarmed")
    }

    /// Hand the file to the library that just mapped it.
    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TemporaryCopy {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        remove_temporary_file(&path);
    }
}

impl NativeLibrary {
    /// Copy and load the built shared library from a unique temporary path.
    ///
    /// # Errors
    ///
    /// Returns [`LibraryError::TemporaryDirectory`] when a staging directory
    /// cannot be created, [`LibraryError::CopyFailed`] when the artifact cannot
    /// be staged, [`LibraryError::LoadFailed`] when the copy is not a loadable
    /// native library, and [`LibraryError::MissingExport`] when it does not
    /// export `pill_module_init`. A load failure is then re-stated as
    /// [`LibraryError::MissingImport`] or [`LibraryError::EngineDylibMismatch`]
    /// when the gathered evidence supports one of those answers.
    pub(crate) fn load_copy(
        build_output: &Path,
        workspace_root: &Path,
        module_name: &str,
    ) -> Result<Self, LibraryError> {
        // Step 1: Prepare this process's temporary directory and a unique
        // target path. Scoping the directory per process id keeps concurrent
        // host instances from touching each other's copies.
        let temporary_directory = process_temporary_directory(workspace_root);
        std::fs::create_dir_all(&temporary_directory).map_err(|source| {
            LibraryError::TemporaryDirectory {
                directory: temporary_directory.display().to_string(),
                source,
            }
        })?;

        let timestamp_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let copy_counter = TEMPORARY_COPY_COUNTER.fetch_add(1, Ordering::Relaxed);
        // One directory per generation, rather than one uniquely named file in
        // a directory shared by all of them.
        //
        // The image names its PDB by bare file name, so a debugger looks for
        // it in the directory the module was loaded from. If generations
        // shared one directory they would also have to share one PDB file,
        // and the first reload would find that file held open by the debugger
        // and impossible to replace. The new generation would then carry the
        // previous one's symbols, which do not match it.
        //
        // The module name still prefixes the directory, so several modules,
        // each reloading on its own schedule, never collide inside one process
        // directory.
        let generation_directory =
            temporary_directory.join(format!("{module_name}_{timestamp_nanos}_{copy_counter}"));
        std::fs::create_dir_all(&generation_directory).map_err(|source| {
            LibraryError::TemporaryDirectory {
                directory: generation_directory.display().to_string(),
                source,
            }
        })?;
        // Keeping the build output's own file name lets the PDB beside it keep
        // the name the image records, which is what makes the lookup work.
        let temporary_name = build_output
            .file_name()
            .map(std::ffi::OsStr::to_os_string)
            .unwrap_or_else(|| {
                std::ffi::OsString::from(format!("module{}", std::env::consts::DLL_SUFFIX))
            });
        let temporary_path = generation_directory.join(temporary_name);

        // Step 2: Give this generation its own handle on the built library.
        // A hard link is preferred. Both paths are under the workspace root, so
        // the link shares the file the staging already wrote, and a later
        // staging replaces the hot copy by rename, which leaves this
        // generation's file intact. A fresh copy would pay the platform's
        // first-access scan again on every load. A cross-volume workspace
        // falls back to that copy.
        let copy_staging_result = std::fs::hard_link(build_output, &temporary_path)
            .or_else(|_| std::fs::copy(build_output, &temporary_path).map(|_| ()));
        copy_staging_result.map_err(|source| LibraryError::CopyFailed {
            source_path: build_output.display().to_string(),
            target_path: temporary_path.display().to_string(),
            source,
        })?;
        // Step 2a: Put the module's debug symbols beside the copy.
        //
        // `rust-lld` records the PDB in the image as a bare file name, not an
        // absolute path, so a debugger looks for it in whatever directory the
        // module was loaded from: this temporary one, not the build output it
        // was copied from. Without this the loaded module has no symbols, and
        // no breakpoint in module source can bind.
        //
        // The PDB is hard linked rather than copied. It can run to tens of
        // megabytes and both paths are under the workspace root, so the link
        // costs nothing, while a copy would be paid on every reload of every
        // module.
        //
        // This is best effort by design. A build without debug info has no
        // PDB, and a link that cannot be made costs symbols but not the
        // module, so nothing here may fail the load.
        let direct_symbol_source = build_output.with_extension("pdb");
        let artifact_symbol_source = workspace_root
            .join(crate::config::module_build_artifact_directory())
            .join(
                direct_symbol_source
                    .file_name()
                    .unwrap_or_else(|| std::ffi::OsStr::new("module.pdb")),
            );
        let symbol_source = if direct_symbol_source.is_file() {
            direct_symbol_source
        } else {
            artifact_symbol_source
        };
        if let Some(symbol_name) = symbol_source.file_name() {
            if symbol_source.is_file() {
                // The image names the PDB by bare file name, so the copy has to
                // keep that name and sit in the directory the module loads
                // from - this generation's own, so reloads never contend for
                // one file the debugger may already hold open.
                let symbol_target = generation_directory.join(symbol_name);
                let symbol_staging_result = std::fs::hard_link(&symbol_source, &symbol_target)
                    .or_else(|_| std::fs::copy(&symbol_source, &symbol_target).map(|_| ()));
                match symbol_staging_result {
                    Ok(()) => debug!(
                        target: telemetry_target::HOT_RELOAD,
                        path = %symbol_target.display(),
                        "staged module debug symbols beside the loaded copy"
                    ),
                    Err(error) => debug!(
                        target: telemetry_target::HOT_RELOAD,
                        path = %symbol_source.display(),
                        %error,
                        "module debug symbols not staged; module code will have no debugger symbols"
                    ),
                }
            }
        }

        // From here the copy exists, and every path that does not hand it to a
        // `NativeLibrary` must delete it again; the guard owns that duty.
        let mut temporary_copy = TemporaryCopy::new(temporary_path);
        debug!(
            target: telemetry_target::HOT_RELOAD,
            path = %temporary_copy.path().display(),
            "copied project DLL"
        );

        // Step 2b: Decide whether this module must load against its own engine
        // dylibs instead of the host's.
        //
        // The host binary maps `pill_core` from the regular target directory.
        // When the module build produced a different engine dylib than the
        // host runs (a GUI frontend adds extra features to shared crates), the
        // module must resolve its own copy or symbol lookup fails at load
        // time. The build stages the matching copy into the hot-load
        // directory; when the two copies are byte-identical (a plain CLI host)
        // the module keeps loading the host's single instance.
        //
        // Staging is best effort, not a guarantee. If it does not help, the
        // load fails in Step 3 and the diagnosis names the engine mismatch.
        // Only the Windows loader can be pointed at the directory a library
        // was loaded from, and even there an already-mapped library of the
        // same name wins, so the staging below is a long shot by design - see
        // [`engine_dylib_staging_steers_the_loader`].
        //
        // The copies go beside the library, in this generation's own
        // directory, because the altered search path starts at the directory
        // of the library being loaded. A copy anywhere else is never
        // consulted, and a stem the host process has not mapped (the editor
        // crates in a renderer host) has no other place to be found.
        //
        // On the other platforms the copies could not be consulted at all, so
        // none are made: the load uses the host's instances, and a mismatch
        // that keeps a symbol from resolving is reported with both builds
        // named instead of failing silently inside the loader.
        let isolated_engine = engine_dylib_staging_steers_the_loader()
            && engine_dylib_needs_isolation(workspace_root);
        if isolated_engine {
            stage_module_engine_dylib(workspace_root, &generation_directory);
        }

        // Step 2c: an engine dylib the host binary does not link is in no link
        // map at all, so the loader would look for it in the host's target
        // directory, where cargo never put a copy. Load the module build's
        // copy first, which registers it under the SONAME the module names.
        #[cfg(not(windows))]
        preload_unmapped_engine_dylibs(workspace_root, temporary_copy.path());

        // Step 3: Load the copy and validate its required exports.
        let load_started = Instant::now();
        // SAFETY: `Self::load` requires `path` to name a valid native library
        // whose `pill_module_init` export uses the expected C ABI. The path is
        // this function's copy of the build output, staged in step 2, so it is
        // the module the build produced. A file that is not a loadable library
        // only makes `load` return `LoadFailed`, because the mapping and the
        // required export are both checked there.
        let native_library = unsafe {
            Self::load(
                temporary_copy.path(),
                temporary_copy.path().to_path_buf(),
                module_name,
                isolated_engine,
            )
        }
        .inspect_err(|_| {
            // The staged engine dylibs go with the copy when this attempt asked
            // for isolation. A dylib a live library maps cannot be deleted on
            // Windows, so a removal failure is ignored: another module may
            // already be using it, and the next attempt stages a fresh copy
            // either way.
            if isolated_engine {
                for stem in ENGINE_DYLIB_STEMS {
                    let _ = std::fs::remove_file(
                        generation_directory.join(engine_dylib_file_name(stem)),
                    );
                }
            }
        })
        .map_err(|error| {
            diagnose_load_failure(error, temporary_copy.path(), workspace_root, module_name)
        })?;
        temporary_copy.disarm();
        analytics::record_load(module_name, load_started.elapsed().as_secs_f64() * 1000.0);
        info!(
            target: telemetry_target::HOT_RELOAD,
            module = module_name,
            abi_version = %native_library
                .abi_version
                .map_or_else(|| "none".to_string(), |version| version.to_string()),
            "module DLL loaded successfully"
        );
        Ok(native_library)
    }

    /// ABI revision reported by the module, when it exports one.
    ///
    /// `None` means the library predates the versioned contract; the caller
    /// decides whether that is acceptable for its module kind.
    pub(crate) fn abi_version(&self) -> Option<u32> {
        self.abi_version
    }

    /// Run one descriptor table's count/copy export protocol.
    ///
    /// Every table the host fetches (value types, mirror methods, field
    /// accessors) follows the same shape: a count export, then a copy export
    /// filling a host-owned buffer of that many slots and answering how many
    /// it wrote. The one `unsafe` call pair that protocol needs lives here.
    fn fetch_descriptors<T: Clone>(
        &self,
        prototype: T,
        count_export: Option<unsafe extern "C" fn() -> u32>,
        copy_export: Option<unsafe extern "C" fn(*mut T, u32) -> u32>,
    ) -> Vec<T> {
        let (Some(count_export), Some(copy_export)) = (count_export, copy_export) else {
            return Vec::new();
        };
        // SAFETY: the count export takes no arguments and returns a plain
        // integer while the library is mapped.
        let descriptor_count = unsafe { count_export() } as usize;
        if descriptor_count == 0 {
            return Vec::new();
        }
        let mut descriptors = vec![prototype; descriptor_count];
        // SAFETY: the copy export was validated to take a host-owned buffer of
        // `descriptor_count` slots; the slice's pointer and length satisfy that
        // contract, and the library stays mapped for the call.
        let copied_count =
            unsafe { copy_export(descriptors.as_mut_ptr(), descriptor_count as u32) } as usize;
        descriptors.truncate(copied_count.min(descriptor_count));
        descriptors
    }

    /// Fetch the artifact's `#[derive(PillMirror)]` value-type descriptors.
    ///
    /// Empty when the artifact predates the exports or declares no value
    /// types. The returned descriptors reference static data inside this
    /// loaded library, which stays mapped for the library's lifetime.
    pub(crate) fn value_type_descriptors(&self) -> Vec<PillValueTypeDescriptor> {
        self.fetch_descriptors(
            PillValueTypeDescriptor {
                type_name: "",
                crate_name: "",
                size: 0,
                align: 0,
                fields: &[],
            },
            self.value_type_count,
            self.copy_value_types,
        )
    }

    /// Fetches the artifact's mirror descriptors: its mirrored functions and
    /// the rows that describe its mirrored types. Each function row carries the
    /// address of the trampoline the artifact compiled for it.
    ///
    /// Nothing is looked up by symbol, because the artifact fills the address
    /// into the descriptor itself. The list is empty when the artifact was built
    /// before these exports or mirrors nothing. A function row with no address
    /// is skipped, so a broken descriptor never gives C# a callable that points
    /// at nothing. A type row without an address (an enum's or a resource's) is
    /// kept, because the codegen needs it.
    pub(crate) fn mirror_methods(&self) -> Vec<ResolvedMirrorMethod> {
        if self.library.is_none() {
            return Vec::new();
        }
        let descriptors = self.fetch_descriptors(
            PillMethodDescriptor {
                type_name: "",
                is_free_function: false,
                crate_name: "",
                name: "",
                symbol: "",
                return_tag: "",
                arg_tags: &[],
                arg_names: &[],
                receiver: "",
                owner_kind: "",
                address: pill_engine::component_registry::ExportAddress(std::ptr::null()),
            },
            self.mirror_method_count,
            self.copy_mirror_methods,
        );

        let mut resolved_methods: Vec<ResolvedMirrorMethod> = Vec::new();
        for descriptor in descriptors {
            // The trampoline lives in this library, which stays mapped for the
            // `NativeLibrary`'s lifetime.
            let address = descriptor.address.0 as usize;
            if address == 0 && descriptor.name != pill_engine::component_registry::MIRROR_TYPE_ROW {
                continue;
            }
            resolved_methods.push(ResolvedMirrorMethod {
                type_name: descriptor.type_name.to_string(),
                method_name: descriptor.name.to_string(),
                return_tag: descriptor.return_tag.to_string(),
                arg_tags: descriptor
                    .arg_tags
                    .iter()
                    .map(|tag| tag.to_string())
                    .collect(),
                arg_names: descriptor
                    .arg_names
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
                address,
                is_free_function: descriptor.is_free_function,
                crate_name: descriptor.crate_name.to_string(),
                receiver: descriptor.receiver.to_string(),
                owner_kind: descriptor.owner_kind.to_string(),
            });
        }
        resolved_methods
    }

    /// Fetches the artifact's heap-field accessor descriptors. Each returned
    /// row carries the addresses of the `#[no_mangle]` trampolines for one
    /// field.
    ///
    /// The list is empty when the artifact was built before these exports or
    /// declares no heap fields. An operation whose trampoline the artifact does
    /// not export is reported as `None`. A descriptor whose operations all
    /// failed is skipped, so the host never publishes a row that cannot be
    /// called. Checking which operations a mirror really needs is left to the
    /// codegen, which knows the field's kind.
    pub(crate) fn field_accessors(&self) -> Vec<ResolvedFieldAccessor> {
        let Some(library) = self.library.as_ref() else {
            return Vec::new();
        };
        let descriptors = self.fetch_descriptors(
            PillFieldAccessorDescriptor {
                type_name: "",
                field_name: "",
                kind: "",
                element_tag: "",
                view_symbol: "",
                resize_symbol: "",
                set_symbol: "",
                item_symbol: "",
                set_item_symbol: "",
                push_symbol: "",
            },
            self.field_accessor_count,
            self.copy_field_accessors,
        );

        let resolve_optional = |symbol: &str| -> Option<usize> {
            if symbol.is_empty() {
                return None;
            }
            // SAFETY: `library.get` looks the exported trampoline up by name,
            // and the symbol is read as its address, never called through this
            // handle. The module stays mapped because `library` is borrowed
            // from this `NativeLibrary`.
            unsafe { library.get::<usize>(symbol.as_bytes()) }
                .ok()
                .map(|symbol| *symbol)
        };

        let mut resolved_accessors: Vec<ResolvedFieldAccessor> = Vec::new();
        for descriptor in descriptors {
            let view_address = resolve_optional(descriptor.view_symbol);
            let resize_address = resolve_optional(descriptor.resize_symbol);
            let set_address = resolve_optional(descriptor.set_symbol);
            let item_address = resolve_optional(descriptor.item_symbol);
            let set_item_address = resolve_optional(descriptor.set_item_symbol);
            let push_address = resolve_optional(descriptor.push_symbol);
            // Every operation failed, so there is nothing to call: skip the
            // descriptor instead of handing out a row of empty addresses.
            let callable = view_address.is_some()
                || resize_address.is_some()
                || set_address.is_some()
                || item_address.is_some()
                || set_item_address.is_some()
                || push_address.is_some();
            if !callable {
                continue;
            }
            resolved_accessors.push(ResolvedFieldAccessor {
                type_name: descriptor.type_name.to_string(),
                field_name: descriptor.field_name.to_string(),
                kind: descriptor.kind.to_string(),
                element_tag: descriptor.element_tag.to_string(),
                view_address,
                resize_address,
                set_address,
                item_address,
                set_item_address,
                push_address,
            });
        }
        resolved_accessors
    }

    /// Loads the staged module at `path` and resolves its required exports.
    ///
    /// `temporary_path` is that same file, kept so the returned `NativeLibrary`
    /// can delete the copy when it drops. `module_name` labels the artifact in
    /// errors. `isolated_engine` records whether the module loads against
    /// engine dylibs staged beside it.
    ///
    /// # Safety
    ///
    /// `path` must point to a valid native library whose `pill_module_init`
    /// export uses the expected C ABI.
    unsafe fn load(
        path: &Path,
        temporary_path: PathBuf,
        module_name: &str,
        isolated_engine: bool,
    ) -> Result<Self, LibraryError> {
        // Step 1: Open the native library and map it into this process.
        //
        // The declaration baseline is counted first: mapping runs the
        // artifact's constructors, which append its declarations to the
        // process-wide collections, and the entry point must stop before them.
        let declaration_baseline = pill_engine::declarations::DeclarationBaseline::current();
        // SAFETY: The `# Safety` contract of `load` guarantees `path` names a
        // valid native library. Mapping runs the module's constructors, and the
        // handle that comes back keeps the module mapped; it is stored in the
        // returned `NativeLibrary`, so it lives as long as the module does.
        let library = if isolated_engine {
            // The module was compiled against an engine dylib that differs from
            // the host's; load with the module's own directory first in the
            // dependency search order so it resolves the copy staged beside it.
            #[cfg(windows)]
            {
                // SAFETY: `path` is this process's own staged copy, written
                // just before this load, and the matching engine dylibs are
                // staged into the same directory whenever the host isolated
                // this module. The altered search path therefore starts in a
                // directory this process controls.
                unsafe {
                    // `load_with_flags` returns the platform-level handle. The
                    // crate-root `Library` that `NativeLibrary` stores is the
                    // safe wrapper around it, and `From` performs that wrap.
                    Library::from(
                        windows_loader::Library::load_with_flags(
                            path,
                            LOAD_WITH_ALTERED_SEARCH_PATH,
                        )
                        .map_err(|source| LibraryError::LoadFailed {
                            path: path.display().to_string(),
                            source: Box::new(source),
                        })?,
                    )
                }
            }
            // The other platforms' loaders do not take a search path for one
            // library the way Windows does, so this falls back to the default
            // load, which finds the module's dependencies through the
            // platform's own search rules.
            #[cfg(not(windows))]
            {
                // SAFETY: the `# Safety` contract of `load` requires `path` to
                // name a valid native module on disk, and the default load maps
                // it without changing how its dependencies are found.
                unsafe {
                    Library::new(path).map_err(|source| LibraryError::LoadFailed {
                        path: path.display().to_string(),
                        source: Box::new(source),
                    })?
                }
            }
        } else {
            // SAFETY: the `# Safety` contract of `load` requires `path` to name
            // a valid native module on disk, and this module was built against
            // the engine dylibs the host already has mapped, so the default
            // search finds those single shared instances.
            unsafe {
                Library::new(path).map_err(|source| LibraryError::LoadFailed {
                    path: path.display().to_string(),
                    source: Box::new(source),
                })?
            }
        };

        // Step 2: Resolve the required `pill_module_init` export. The error
        // carries the module's name, because a project and an extension export
        // the same symbol and the symbol alone does not say which artifact
        // lacks it.
        // SAFETY: `pill_module_init` is a mandatory export of the native ABI
        // contract, so an artifact the host accepts provides it. It is resolved
        // as a pointer with the statically known C ABI signature, and the
        // pointer stays valid because the `library` handle keeps the module
        // mapped for the lifetime of the returned `NativeLibrary`.
        let module_init: Symbol<ModuleInitFn> = unsafe {
            library
                .get(MODULE_INIT_SYMBOL)
                .map_err(|source| LibraryError::MissingExport {
                    subject: module_name.to_string(),
                    symbol: String::from_utf8_lossy(MODULE_INIT_SYMBOL).to_string(),
                    source: Box::new(source),
                })?
        };

        // Step 2b: Resolve the optional `pill_module_register` export. Its
        // presence is what lets a later generation re-register without running
        // the artifact's one-time `init`; an artifact without it keeps the one
        // entry point it always had.
        // SAFETY: the same contract as `pill_module_init` - the signature is
        // the module ABI's, and the handle keeps the image mapped.
        let module_register = unsafe {
            library
                .get::<ModuleInitFn>(MODULE_REGISTER_SYMBOL)
                .ok()
                .map(|symbol| *symbol)
        };

        // Step 3: Read the optional ABI revision before anything else calls
        // into the module, so a caller can reject an incompatible module
        // without ever handing it a pointer into engine memory.
        // SAFETY: The export is optional. When it is present it is read with the
        // statically known C ABI signature and called at once, while the
        // library is mapped. It takes no arguments and returns a plain integer,
        // so no host value is passed into it.
        let abi_version: Option<u32> =
            unsafe { library.get::<ModuleAbiVersionFn>(MODULE_ABI_VERSION_SYMBOL) }
                .ok()
                .map(|symbol| unsafe { symbol() });

        // Step 4: Resolve the optional value-type manifest exports. They are
        // optional because a library built before them does not export them,
        // and such a library simply contributes no value types.
        // SAFETY: `library.get` reads a function pointer out of the loaded
        // module using the statically known signature. The `library` handle
        // keeps that module mapped for as long as this `NativeLibrary` lives.
        let value_type_count: Option<ValueTypeCountFn> =
            unsafe { library.get(VALUE_TYPE_COUNT_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        // SAFETY: the copy export is read out of the module the `library`
        // handle keeps mapped, with the statically known signature.
        let copy_value_types: Option<CopyValueTypesFn> =
            unsafe { library.get(VALUE_TYPE_COPY_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        // Step 5: Resolve the optional mirrored-method manifest exports. Like
        // the value-type manifest, a library built before them exports none and
        // simply contributes no mirrored methods.
        // SAFETY: `library.get` reads each export with its statically known C
        // ABI signature, from a module the `library` handle keeps mapped for the
        // lifetime of the returned `NativeLibrary`.
        let mirror_method_count: Option<MirrorMethodCountFn> =
            unsafe { library.get(MIRROR_METHOD_COUNT_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        // SAFETY: the copy export is read from the module the `library` handle
        // keeps mapped, and the descriptor array it points at stays inside the
        // artifact's static data for as long as the artifact is loaded.
        let copy_mirror_methods: Option<CopyMirrorMethodsFn> =
            unsafe { library.get(MIRROR_METHOD_COPY_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);

        // Step 6: Resolve the optional heap-field accessor exports. Like the
        // manifests above, a library built before them exports none and simply
        // contributes no container accessors.
        // SAFETY: `library.get` reads each export with its statically known C
        // ABI signature, from a module the `library` handle keeps mapped for the
        // lifetime of the returned `NativeLibrary`.
        let field_accessor_count: Option<FieldAccessorCountFn> =
            unsafe { library.get(FIELD_ACCESSOR_COUNT_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);
        // SAFETY: the copy export is read from the module the `library` handle
        // keeps mapped, and its descriptor array stays inside this artifact for
        // as long as the artifact is loaded.
        let copy_field_accessors: Option<CopyFieldAccessorsFn> =
            unsafe { library.get(FIELD_ACCESSOR_COPY_SYMBOL) }
                .ok()
                .map(|symbol| *symbol);

        // Step 7: Copy the `pill_module_init` pointer out of its borrowed
        // `Symbol` wrapper. The `library` field keeps the module mapped, so this
        // raw pointer and the optional ones above stay valid for the whole
        // lifetime of the returned `NativeLibrary`.
        let module_init_pointer = *module_init;
        Ok(Self {
            library: Some(library),
            declaration_baseline,
            module_init: module_init_pointer,
            module_register,
            abi_version,
            value_type_count,
            copy_value_types,
            mirror_method_count,
            copy_mirror_methods,
            field_accessor_count,
            copy_field_accessors,
            temporary_path,
        })
    }

    /// Calls the registration-only entry point, when the artifact exports one.
    ///
    /// Returns `None` when it does not: the caller then has to use
    /// [`Self::call_init`] and accept that the artifact's one-time work runs
    /// again.
    pub(crate) fn call_register(&self, api: &EngineApi) -> Option<u32> {
        let module_register = self.module_register?;
        // SAFETY: the export was found at load time and is held with this C ABI
        // signature, which is the signature the module ABI contract defines for
        // it. The `library` field keeps the module mapped for as long as this
        // `NativeLibrary` lives, and `api` is only borrowed for the duration of
        // the call.
        Some(pill_engine::declarations::with_declaration_baseline(
            self.declaration_baseline,
            || unsafe { module_register(api as *const EngineApi) },
        ))
    }

    /// Calls the module's registration entry point.
    ///
    /// Returns the module's status code. Zero means registration succeeded.
    /// Any other value means the module failed to initialize, so the caller
    /// must keep the previous generation active.
    pub(crate) fn call_init(&self, api: &EngineApi) -> u32 {
        // SAFETY: The init export was found at load time and is held with this
        // C ABI signature, which is the signature the module ABI contract
        // defines for it. The `library` field keeps the module mapped for as
        // long as this `NativeLibrary` lives, and `api` is only borrowed for the
        // duration of the call.
        pill_engine::declarations::with_declaration_baseline(self.declaration_baseline, || unsafe {
            (self.module_init)(api as *const EngineApi)
        })
    }

    /// Rejects an artifact built against a different revision of the module ABI.
    ///
    /// An artifact may export `pill_module_abi_version`. When it does, this
    /// compares that revision with the host's before anything calls into it. A
    /// mismatch means the two sides disagree about what the entry points
    /// expect, and a call through them would use a shape one side never agreed
    /// to. An artifact with no such export was built before the contract was
    /// versioned.
    ///
    /// # Errors
    ///
    /// Returns [`ModuleError::AbiVersionMismatch`] when the revisions differ,
    /// and [`ModuleError::AbiVersionMissing`] when the export is absent.
    pub(crate) fn check_module_abi(&self, module_name: &str) -> Result<(), ModuleError> {
        let host_version = pill_engine::module_abi::MODULE_ABI_VERSION;
        match self.abi_version() {
            Some(module_version) if module_version == host_version => Ok(()),
            Some(module_version) => Err(ModuleError::AbiVersionMismatch {
                module: module_name.to_string(),
                module_version,
                host_version,
            }),
            None => Err(ModuleError::AbiVersionMissing {
                module: module_name.to_string(),
            }),
        }
    }

    /// Finds an export that only one kind of caller knows about, or `None` when
    /// the library does not export it.
    ///
    /// The loader keeps the exports every artifact may have. An export that
    /// belongs to one module's own contract, such as the renderer's attach and
    /// detach entry points or a shader reload function, is looked up here by
    /// the caller that knows its signature. That keeps `NativeLibrary` from
    /// growing a field for every such module.
    ///
    /// # Safety
    ///
    /// `T` must be the export's exact function-pointer type. The pointer is
    /// valid only while this library stays mapped.
    #[cfg_attr(not(feature = "rendering"), allow(dead_code))]
    pub(crate) unsafe fn resolve_export<T: Copy>(&self, name: &[u8]) -> Option<T> {
        let library = self.library.as_ref()?;
        // SAFETY: the caller states that `T` is the export's type. The copy of
        // the pointer that comes back stays valid while `library` keeps the
        // image mapped, which is the limit the caller's contract puts on its
        // use.
        unsafe { library.get::<T>(name) }.ok().map(|symbol| *symbol)
    }

    /// Offers a replacement implementation to this artifact's copy of one
    /// `#[pill_hot_fn]`.
    ///
    /// A crate linked into several artifacts gets compiled into each of them,
    /// so one edited function has an independent redirect slot in every artifact
    /// that links it. The host therefore offers the same replacement to every
    /// loaded library instead of assuming that one of them owns the function.
    ///
    /// Returns `Ok(true)` when this artifact installed the replacement. Returns
    /// `Ok(false)` when the artifact declares no such function, which includes a
    /// library built before this ABI that exports no resolver at all. Returns
    /// `Err` when the artifact refused because the signature no longer matches.
    pub(crate) fn install_plain_function(
        &self,
        qualified_name: &str,
        address: usize,
        signature: &str,
    ) -> Result<bool, String> {
        let Some(library) = self.library.as_ref() else {
            return Ok(false);
        };
        // Looks the export up when it is needed instead of at load time. Only
        // an artifact whose crate opted into hot patching exports it, and a
        // patch is rare enough that the extra lookup costs nothing.
        //
        // SAFETY: the symbol is looked up by the name the `#[pill_module]` and
        // `#[pill_project]` macros generate, with the signature those macros
        // define. The returned pointer stays valid because `library` keeps the
        // module mapped for this `NativeLibrary`'s lifetime.
        let install: Symbol<PlainFunctionInstallFn> =
            match unsafe { library.get(PLAIN_FUNCTION_INSTALL_SYMBOL) } {
                Ok(symbol) => symbol,
                Err(_) => return Ok(false),
            };

        let name = std::ffi::CString::new(qualified_name)
            .map_err(|_| "the function path contains an interior NUL".to_string())?;
        let signature_text = std::ffi::CString::new(signature)
            .map_err(|_| "the signature contains an interior NUL".to_string())?;

        // SAFETY: both strings are NUL-terminated and live until the call
        // returns, and `address` points into a patch image that the host never
        // unloads.
        let status = unsafe { install(name.as_ptr(), address, signature_text.as_ptr()) };
        match status {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(format!(
                "`{qualified_name}` changed shape; the running implementation was kept"
            )),
        }
    }

    /// Returns how many of this artifact's functions have a length that the
    /// exception directory records, and how many it has in total, as
    /// `(known, total)`.
    ///
    /// Only a function with a known length can be patched at its prologue, so
    /// this turns a refusal into a diagnosis: it shows whether the one function
    /// happens to be a leaf, or whether nothing in the artifact is reachable by
    /// that route. Returns `None` when the artifact exports no such report.
    pub(crate) fn extent_coverage(&self) -> Option<(usize, usize)> {
        let library = self.library.as_ref()?;
        // SAFETY: the symbol is looked up by the name the ABI macros generate,
        // with the signature they define, in a module this handle keeps mapped.
        let coverage: Symbol<ExtentCoverageFn> =
            unsafe { library.get(EXTENT_COVERAGE_SYMBOL) }.ok()?;
        // SAFETY: the export takes no arguments and only reads static tables.
        let packed = unsafe { coverage() };
        Some(((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize))
    }

    /// Returns the address of any function in this artifact, with the
    /// declaration it was built with. Both come from the inventory the build
    /// script writes, not from an attribute.
    ///
    /// The declaration is the compatibility check for the prologue route, and
    /// the only chance to refuse a reshaped function: overwriting a function's
    /// first bytes cannot check what it jumps to. An artifact whose inventory
    /// holds no declaration reports `None`, and the check refuses that case
    /// rather than treating it as nothing to verify.
    ///
    /// Returns `None` when the artifact exports no address resolver or has no
    /// entry for the name.
    pub(crate) fn function_address(&self, qualified_name: &str) -> Option<(usize, Option<String>)> {
        let library = self.library.as_ref()?;
        // SAFETY: the symbol is looked up by the name the ABI macros generate,
        // with the signature they define. The pointer stays valid because
        // `library` keeps the module mapped for this `NativeLibrary`'s lifetime.
        let resolve: Symbol<FunctionAddressFn> =
            unsafe { library.get(FUNCTION_ADDRESS_SYMBOL) }.ok()?;
        let name = std::ffi::CString::new(qualified_name).ok()?;
        let mut signature_pointer: *const u8 = std::ptr::null();
        let mut signature_length: usize = 0;
        // SAFETY: the string is NUL-terminated and lives until the call returns,
        // and both out-parameters are writable locals. On success the artifact
        // writes a pointer into its own static storage, which stays valid while
        // it is loaded.
        let address =
            unsafe { resolve(name.as_ptr(), &mut signature_pointer, &mut signature_length) };
        if address == 0 {
            return None;
        }
        let signature = if signature_pointer.is_null() {
            None
        } else {
            // SAFETY: the artifact wrote a pointer and length describing a
            // `&'static str` inside its own image.
            let bytes = unsafe { std::slice::from_raw_parts(signature_pointer, signature_length) };
            Some(String::from_utf8_lossy(bytes).into_owned())
        };
        Some((address, signature))
    }

    /// Returns this artifact's copy of one `#[pill_hot_fn]` to its own body.
    ///
    /// This is the counterpart of [`Self::install_plain_function`], and it is
    /// what a rollback uses: each artifact restores the code it was compiled
    /// with, instead of being handed an address that belongs to another image.
    ///
    /// Returns `Ok(true)` when this artifact emptied the slot. Returns
    /// `Ok(false)` when it declares no such function or exports no resolver.
    pub(crate) fn reset_plain_function(&self, qualified_name: &str) -> Result<bool, String> {
        let Some(library) = self.library.as_ref() else {
            return Ok(false);
        };
        // SAFETY: the symbol is looked up by the name the `#[pill_module]` and
        // `#[pill_project]` macros generate, with the signature those macros
        // define. The returned pointer stays valid because `library` keeps the
        // module mapped for this `NativeLibrary`'s lifetime.
        let reset: Symbol<PlainFunctionResetFn> =
            match unsafe { library.get(PLAIN_FUNCTION_RESET_SYMBOL) } {
                Ok(symbol) => symbol,
                Err(_) => return Ok(false),
            };

        let name = std::ffi::CString::new(qualified_name)
            .map_err(|_| "the function path contains an interior NUL".to_string())?;
        // SAFETY: the string is NUL-terminated and lives until the call returns.
        let status = unsafe { reset(name.as_ptr()) };
        Ok(status == 0)
    }
}

impl Drop for NativeLibrary {
    /// Unmaps the module and deletes its temporary copy together with the
    /// directory it was staged in.
    ///
    /// The library handle is dropped explicitly before the file is removed,
    /// because Windows refuses to delete a file that is still mapped into the
    /// process. A cleanup failure is logged as a warning rather than ignored.
    fn drop(&mut self) {
        // A stale call into code that is about to be freed shows up as a crash
        // in this drop, so the image is named before it is unmapped. The last
        // line printed then identifies which image the process died in.
        info!(
            target: telemetry_target::HOT_RELOAD,
            path = %self.temporary_path.display(),
            "unmapping module copy"
        );
        drop(self.library.take());
        remove_temporary_file(&self.temporary_path);
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Deletes one temporary copy and the generation directory it was staged in.
///
/// Both [`NativeLibrary`]'s `Drop` and [`TemporaryCopy`]'s call this: they
/// remove the same kind of file for the same reason, and a failure has to be
/// reported in the same way from either one.
fn remove_temporary_file(path: &Path) {
    if let Err(error) = std::fs::remove_file(path) {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            "Failed to remove temporary DLL {}: {error}",
            path.display()
        );
    }
    if let Some(directory) = path.parent() {
        remove_generation_directory(directory);
    }
}

/// Removes one generation's directory and keeps whatever the process still
/// maps.
///
/// The directory holds the copy, the module's PDB and, when the engine was
/// isolated, copies of the engine dylibs. That is one directory per load, and
/// tens of megabytes if nothing takes it away. A mapped image cannot be deleted
/// on a platform that locks its files, so whatever refuses to go is named at
/// debug level instead of warned about, and a later start removes the directory
/// once the grace period has passed.
///
/// Only a `<staging>/<pid>/<module>_<stamp>_<n>` directory is removed, never the
/// process directory itself, which holds the generated hot-patch sources and
/// images that stay for the life of the process.
fn remove_generation_directory(directory: &Path) {
    let owned_by_a_process_directory = directory
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.parse::<u32>().is_ok());
    if !owned_by_a_process_directory {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut kept = 0_usize;
    for entry in entries.filter_map(Result::ok) {
        if std::fs::remove_file(entry.path()).is_err() {
            kept += 1;
        }
    }
    let _ = std::fs::remove_dir(directory);
    if kept > 0 {
        debug!(
            target: telemetry_target::HOT_RELOAD,
            path = %directory.display(),
            kept,
            "generation directory kept the files this process still maps"
        );
    }
}

/// Directory used by this host process for temporary native-library copies.
///
/// Scoping the directory per process id means one host instance can never
/// delete or overwrite the copies of another instance running against the
/// same workspace.
pub(crate) fn process_temporary_directory(workspace_root: &Path) -> PathBuf {
    workspace_root
        .join(TEMPORARY_DIRECTORY)
        .join(std::process::id().to_string())
}

/// Whether a process with the given id is still running.
///
/// The answer is exact only where an exact one is cheap: Linux checks `/proc`
/// directly, and every other platform answers "not alive" rather than walking
/// the process table on a call that happens once at startup.
///
/// That is safe because this is not the only check. A negative answer does not
/// allow a removal; it gives the decision to [`STAGING_GRACE_PERIOD`], which
/// looks at when the directory was last written to instead of at who owns it.
/// A live host stages on every load, so the clock recognises it on every
/// platform. This function only lets Linux skip the wait.
fn process_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Removes the staging directories that earlier host processes left behind.
///
/// Three rules apply, in this order:
///
/// 1. This process's own directory always goes. A previous run under the same
///    process id left it.
/// 2. Another process's directory goes once it has been quiet for
///    [`STAGING_GRACE_PERIOD`]. That wait is how a dead host is told from a live
///    one.
/// 3. Whatever survives is capped at [`MAX_RETAINED_STAGING_DIRECTORIES`], and
///    the oldest go first.
///
/// Removal failures are reported rather than ignored: on Windows a live host
/// keeps its mapped modules locked, so a refusal means the directory is in use.
pub(crate) fn cleanup_temporary_files(workspace_root: &Path) {
    let temporary_root = workspace_root.join(TEMPORARY_DIRECTORY);
    let Ok(entries) = std::fs::read_dir(&temporary_root) else {
        return;
    };

    let own_pid = std::process::id();
    let mut candidates: Vec<(u32, SystemTime, PathBuf)> = Vec::new();

    for entry in entries.filter_map(Result::ok) {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let path = entry.path();

        if pid == own_pid {
            remove_staging_directory(&path, pid, "left by an earlier run of this process");
            continue;
        }

        // The Linux probe gives an exact answer and is asked first. Everywhere
        // else it reports "not alive" by design, and the clock decides instead.
        if process_is_alive(pid) {
            continue;
        }

        candidates.push((pid, staging_last_write(&path), path));
    }

    let candidate_times: Vec<(u32, SystemTime)> = candidates
        .iter()
        .map(|(pid, modified, _)| (*pid, *modified))
        .collect();
    for (index, reason) in staging_directories_to_evict(&candidate_times, SystemTime::now()) {
        let (pid, _, path) = &candidates[index];
        remove_staging_directory(path, *pid, reason);
    }
}

/// Chooses which of the other processes' staging directories to remove.
///
/// Two rules decide it. A directory that has been quiet for at least
/// [`STAGING_GRACE_PERIOD`] belonged to a process that is gone, so it goes.
/// Whatever is left is capped at [`MAX_RETAINED_STAGING_DIRECTORIES`], and the
/// least recently written go first: they are the least likely to belong to a
/// running host, and the ones whose disk is most worth reclaiming.
///
/// Returns each directory's index in `candidates` with the reason to log. The
/// function is pure and kept apart from the removal so it can be tested without
/// a filesystem or a second host process.
fn staging_directories_to_evict(
    candidates: &[(u32, SystemTime)],
    now: SystemTime,
) -> Vec<(usize, &'static str)> {
    let mut evictions: Vec<(usize, &'static str)> = Vec::new();
    let mut retained_candidates: Vec<(SystemTime, usize)> = Vec::new();

    for (index, (_, modified)) in candidates.iter().enumerate() {
        let quiet_for = now
            .duration_since(*modified)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if quiet_for >= STAGING_GRACE_PERIOD {
            evictions.push((index, "stale"));
        } else {
            retained_candidates.push((*modified, index));
        }
    }

    if retained_candidates.len() > MAX_RETAINED_STAGING_DIRECTORIES {
        retained_candidates.sort_by_key(|(modified, _)| *modified);
        let excess_count = retained_candidates.len() - MAX_RETAINED_STAGING_DIRECTORIES;
        for (_, index) in retained_candidates.iter().take(excess_count) {
            evictions.push((*index, "over the retained-staging bound"));
        }
    }

    evictions
}

/// Returns when a staging directory was last written to.
///
/// When the timestamp cannot be read this falls back to the epoch, which makes
/// an unreadable directory look as old as possible. That is deliberate: leaving
/// such a directory forever is worse than trying to remove it, and the attempt
/// only fails if the files are still in use.
fn staging_last_write(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Removes one staging directory and logs which one went and why.
///
/// A failure on another process's directory is expected rather than
/// exceptional, because a live host keeps its mapped modules locked. It is
/// logged as a note, not as an error.
fn remove_staging_directory(path: &Path, pid: u32, reason: &str) {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {
            if pid != std::process::id() {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    "Cleaned up temporary files from process {pid} ({reason})"
                );
            }
        }
        Err(error) => {
            if pid == std::process::id() {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    "Could not remove temporary directory {}: {error}",
                    path.display()
                );
            } else {
                // The removal can fail simply because another live host still
                // holds the files. That is why it is logged as a note here.
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    "Temporary directory {} left in place (possibly still in use): {error}",
                    path.display()
                );
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::{
        choose_explanation, engine_dylib_file_name, engine_dylib_stem, explanation_error,
        files_equal, staging_directories_to_evict, LoadFailureExplanation, TemporaryCopy,
        UnresolvedImport, MAX_RETAINED_STAGING_DIRECTORIES, STAGING_GRACE_PERIOD,
    };
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    use crate::scratch::ScratchDirectory;

    /// Engine dylibs are named the platform's way, and evidence from either
    /// image format maps back to the stem.
    ///
    /// The name is what connects the build output, the host's copy, the staged
    /// copy and the two paths a mismatch report names. A host that kept looking
    /// for `pill_core.dll` on Linux found no module-world copy at all, so a
    /// mismatch could not be detected and the loader's bare `undefined symbol`
    /// was all the reader got.
    #[test]
    fn an_engine_dylib_name_follows_the_platform_convention() {
        let name = engine_dylib_file_name("pill_core");

        assert_eq!(
            name,
            format!(
                "{}pill_core{}",
                std::env::consts::DLL_PREFIX,
                std::env::consts::DLL_SUFFIX
            )
        );
        assert_eq!(engine_dylib_stem(&name), Some("pill_core"));
        assert_eq!(
            engine_dylib_stem(&name.to_ascii_uppercase()),
            Some("pill_core"),
            "a loader lookup on a case-insensitive filesystem still matches"
        );
        assert_eq!(
            engine_dylib_stem("pill_engine_core.dll"),
            Some("pill_engine_core"),
            "a PE import table names its DLL whatever host reads it"
        );
        assert_eq!(engine_dylib_stem("not_an_engine.dll"), None);
    }

    /// A failing load where the artifact imports a symbol that `pill_ui_frame`
    /// does not export, while `pill_core`'s copies differ. The message must
    /// name `pill_ui_frame` and the symbol, so the engine-dylib answer does not
    /// win and hide the real one.
    #[test]
    fn a_missing_pill_ui_frame_import_wins_over_a_pill_core_mismatch() {
        let unresolved = UnresolvedImport {
            dll: "pill_ui_frame.dll".to_string(),
            symbol: "pill_ui_frame_frame_take".to_string(),
            path: PathBuf::from("target/hot/pill_ui_frame.dll"),
        };
        let mismatch = Some((
            PathBuf::from("target/debug/pill_core.dll"),
            PathBuf::from("target/hot/pill_core.dll"),
        ));
        let explanation = choose_explanation(Some(unresolved), None, mismatch);
        let error = explanation_error("editor_shell", explanation).expect("an explanation");
        let text = error.to_string();
        assert!(text.contains("pill_ui_frame.dll"), "{text}");
        assert!(text.contains("pill_ui_frame_frame_take"), "{text}");
        assert!(
            !text.contains("pill_core"),
            "the mismatch story must not win: {text}"
        );
    }

    /// A feature-hash mismatch whose unresolved import belongs to the
    /// mismatching dylib keeps the engine-dylib answer, with both paths.
    #[test]
    fn an_engine_import_keeps_the_mismatch_story_with_both_paths() {
        let unresolved = UnresolvedImport {
            dll: "pill_core.dll".to_string(),
            symbol: "pill_core_register".to_string(),
            path: PathBuf::from("target/hot/pill_core.dll"),
        };
        let pair = (
            PathBuf::from("target/debug/pill_core.dll"),
            PathBuf::from("target/hot/pill_core.dll"),
        );
        let explanation = choose_explanation(Some(unresolved), Some(pair.clone()), Some(pair));
        assert!(matches!(
            explanation,
            LoadFailureExplanation::EngineDylibMismatch { .. }
        ));
        let error = explanation_error("renderer", explanation).expect("an explanation");
        let text = error.to_string();
        assert!(text.contains("target/debug/pill_core.dll"), "{text}");
        assert!(text.contains("target/hot/pill_core.dll"), "{text}");
        assert!(text.contains("different engine dylib"), "{text}");
    }

    /// Without an unresolved import, the first differing pair still explains
    /// the failure. Without either, the loader's own words stand.
    #[test]
    fn the_fallback_is_the_mismatch_then_the_loader_message() {
        let pair = (
            PathBuf::from("target/debug/pill_core.dll"),
            PathBuf::from("target/hot/pill_core.dll"),
        );
        assert!(matches!(
            choose_explanation(None, None, Some(pair)),
            LoadFailureExplanation::EngineDylibMismatch { .. }
        ));
        assert_eq!(
            choose_explanation(None, None, None),
            LoadFailureExplanation::Unknown
        );
    }

    /// A directory that has been quiet longer than the grace period is stale,
    /// and one written to recently is left alone.
    ///
    /// The grace period is what says whether that process is still alive. A
    /// running host stages a copy on every load and every reload, so a recent
    /// write is the evidence that it is there. Deleting a live host's directory
    /// would take whatever it does not currently map and leave the rest.
    #[test]
    fn a_quiet_staging_directory_is_stale_and_a_recent_one_is_not() {
        let now = SystemTime::now();
        let quiet = now - STAGING_GRACE_PERIOD - Duration::from_secs(1);
        let recent = now - Duration::from_secs(1);

        let evictions = staging_directories_to_evict(&[(101, quiet), (202, recent)], now);

        assert_eq!(evictions, vec![(0, "stale")]);
    }

    /// Past the retained bound, the least recently written directories go
    /// first.
    ///
    /// Removing the oldest first is what makes the bound safe to apply inside
    /// the grace period. A live host writes to its directory on every load, so
    /// it is the most recently written of them all and the last one the bound
    /// would reach.
    #[test]
    fn the_retained_bound_evicts_the_least_recently_written_first() {
        let now = SystemTime::now();
        // Two more than the bound allows, all well inside the grace period,
        // each a second older than the next.
        let candidate_count = MAX_RETAINED_STAGING_DIRECTORIES + 2;
        let candidates: Vec<(u32, SystemTime)> = (0..candidate_count)
            .map(|index| {
                let age = Duration::from_secs((candidate_count - index) as u64);
                (index as u32, now - age)
            })
            .collect();

        let evictions = staging_directories_to_evict(&candidates, now);

        assert_eq!(
            evictions,
            vec![
                (0, "over the retained-staging bound"),
                (1, "over the retained-staging bound")
            ],
            "the two oldest go, and the most recently written - a live host's - stays"
        );
    }

    /// Inside the bound and inside the grace period, nothing is evicted.
    #[test]
    fn staging_directories_within_both_rules_are_kept() {
        let now = SystemTime::now();
        let candidates: Vec<(u32, SystemTime)> = (0..MAX_RETAINED_STAGING_DIRECTORIES)
            .map(|index| (index as u32, now - Duration::from_secs(index as u64)))
            .collect();

        assert!(staging_directories_to_evict(&candidates, now).is_empty());
    }

    /// Sets a fixed modification time on a file.
    ///
    /// Tests use it to control the timestamps exactly, so a comparison between
    /// two files is decided by their bytes and never by when they were written.
    fn set_modified(path: &std::path::Path, stamp: SystemTime) {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .expect("open to set the timestamp");
        file.set_modified(stamp).expect("set modification time");
    }

    /// The length check and the streaming compare answer one question: equal
    /// bytes are equal, and neither a length mismatch nor a same-length
    /// difference is. Timestamps must not decide it, so the identical pair
    /// below deliberately carries different modification times.
    #[test]
    fn files_equal_gates_length_and_compares_bytes() {
        let directory = ScratchDirectory::new("files_equal");

        let first = directory.join("first.dll");
        let second = directory.join("second.dll");
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        std::fs::write(&first, b"engine bytes").expect("write first");
        std::fs::write(&second, b"engine bytes").expect("write second");
        set_modified(&first, stamp);
        // A copied dylib keeps its bytes but not its timestamp. Treating the
        // two as different would stage an isolated engine and map
        // `pill_core.dll` twice.
        set_modified(&second, stamp + Duration::from_secs(1));
        assert!(
            files_equal(&first, &second),
            "identical bytes compare equal regardless of timestamps"
        );

        // Same length, one differing byte: the streaming compare has to catch
        // what the length check cannot.
        std::fs::write(&second, b"engine ByteS").expect("write one differing byte");
        set_modified(&second, stamp);
        assert!(
            !files_equal(&first, &second),
            "same length but different bytes compare unequal"
        );

        // Shorter file: the length check answers.
        std::fs::write(&second, b"engine").expect("truncate second");
        set_modified(&second, stamp);
        assert!(
            !files_equal(&first, &second),
            "a length mismatch compares unequal"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A load that never happens still removes the copy the guard owns, and a
    /// disarmed guard leaves the file for the library's own `Drop`.
    #[test]
    fn an_abandoned_copy_is_removed() {
        let directory = ScratchDirectory::new("temporary_copy");

        let abandoned_copy_path = directory.join("abandoned.dll");
        std::fs::write(&abandoned_copy_path, b"copy").expect("write abandoned copy");
        {
            let _guard = TemporaryCopy::new(abandoned_copy_path.clone());
        }
        assert!(
            !abandoned_copy_path.exists(),
            "the guard removes the copy when the load never took it over"
        );

        let kept_copy_path = directory.join("kept.dll");
        std::fs::write(&kept_copy_path, b"copy").expect("write kept copy");
        {
            let mut guard = TemporaryCopy::new(kept_copy_path.clone());
            let _ = guard.path();
            guard.disarm();
        }
        assert!(
            kept_copy_path.exists(),
            "a disarmed guard leaves the file to the library that mapped it"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A rejected load must take its whole generation directory with it.
    ///
    /// The copy is not the only thing in there. The module's PDB is staged
    /// beside it, and so are copies of the engine dylibs when the module needs
    /// its own. Removing only the copy would leave those behind, one directory
    /// per reload, for the life of the workspace.
    #[test]
    fn a_rejected_load_leaves_no_generation_directory() {
        let workspace = ScratchDirectory::new("generation_directory");
        let build_output = workspace.join("garbage.dll");
        std::fs::write(&build_output, b"not a native library").expect("write the artifact");

        let load_attempt =
            super::NativeLibrary::load_copy(&build_output, &workspace, "garbage_module");
        assert!(
            load_attempt.is_err(),
            "a file that is not a library cannot load"
        );

        let process_directory = super::process_temporary_directory(&workspace);
        let leftover_names: Vec<String> = std::fs::read_dir(&process_directory)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            leftover_names.is_empty(),
            "the generation directory outlived its copy: {leftover_names:?}"
        );
    }
}
