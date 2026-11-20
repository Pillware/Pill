//! Watch the configured source tree for file changes and tell the main
//! thread to reload.
//!
//! # Responsibilities
//!
//! - Spawn a worker thread that watches the source tree for changes.
//! - Decide which file events and paths count as a source edit, so only a
//!   real edit starts a reload.
//! - Bump a reload generation counter when a relevant event arrives;
//!   the main thread reloads when it sees a generation it has not handled.
//! - Record when each signal fired, so the frame loop can time a reload from
//!   the save that caused it.
//! - Report which files changed, so a reload can be traced back to a save.
//! - Let `notify` handle the differences between the platforms' file
//!   notifications.
//!
//! # Design
//!
//! The watcher runs on its own thread, so watching never blocks the main
//! loop. Several notifications for one save are joined into one signal: the
//! worker waits out the debounce window, collects the paths it saw and
//! reports them. The main thread compares the generation counter against the
//! last value it handled on every loop, so a save that arrives during a
//! reload is never lost.

// Standard library
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

// External crates
use notify::event::EventKind;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use pill_core::error::WatcherError;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};

// =============================================================================
// Constants
// =============================================================================

/// File events that arrive within this window are joined into one reload.
///
/// The wait is pure delay: every edit sleeps for this long before any work
/// starts. It is needed because one save produces several notifications -
/// editors write, rename and touch metadata - and without the wait each one
/// would start its own rebuild. A burst lasts a few milliseconds, so the
/// window only has to outlast that, and it stays far below what a developer
/// would notice. Override it with `PILL_WATCH_DEBOUNCE_MS` when an editor's
/// save pattern needs more room.
const DEFAULT_DEBOUNCE_MILLISECONDS: u64 = 60;

/// Environment variable overriding the debounce window.
const DEBOUNCE_OVERRIDE_VARIABLE: &str = "PILL_WATCH_DEBOUNCE_MS";

/// Directory names that never contain source code worth rebuilding for.
const IGNORED_DIRECTORY_NAMES: [&str; 3] = ["target", "bin", "obj"];

/// Suffixes editors use for temporary and swap files written around saves.
const EDITOR_TEMPORARY_FILE_SUFFIXES: [&str; 3] = ["~", ".swp", ".swx"];

/// A file whose name starts with this is hidden, and never holds source code.
const HIDDEN_FILE_PREFIX: &str = ".";

/// Maximum number of changed paths printed per reload report.
const REPORTED_PATH_LIMIT: usize = 5;

/// How many recent triggers [`TriggerLog`] keeps for the frame loop.
///
/// The loop consumes the generation it acts on, so the bound only has to
/// cover signals that were superseded before they were ever read.
const TRIGGER_LOG_CAPACITY: usize = 16;

// =============================================================================
// Types
// =============================================================================

/// What one debounce window produced: when its signal fired, and how long the
/// save had waited before that.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SourceTrigger {
    /// The generation this trigger published, which is the value the frame
    /// loop compares its own counter against.
    pub(crate) generation: u64,
    /// Monotonic instant the signal was published.
    pub(crate) fired_at: Instant,
    /// Milliseconds from the newest edited file's modification time to the
    /// signal, debounce included; `NaN` when no timestamp could be read.
    pub(crate) detection_delay_ms: f64,
}

/// The project watcher's most recent triggers.
///
/// This is what lets the frame loop measure a reload from the save that caused
/// it: the signal's instant marks the end of the wait, and the delay from the
/// file's modification time marks the start. Only the project watcher records
/// here; the module and renderer watchers pass `None`, because nothing times a
/// reload from their signals.
pub(crate) struct TriggerLog {
    /// Newest last. Guarded because the watcher thread writes while the frame
    /// thread reads.
    entries: Mutex<Vec<SourceTrigger>>,
}

impl TriggerLog {
    /// An empty log.
    pub(crate) fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Record one fired trigger, evicting the oldest past the bound.
    fn record(&self, trigger: SourceTrigger) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if entries.len() >= TRIGGER_LOG_CAPACITY {
            entries.remove(0);
        }
        entries.push(trigger);
    }

    /// Take the trigger that published `generation`, if it is still logged.
    ///
    /// The entry is taken out rather than read, so one signal is counted
    /// against exactly one reload: the reload that handled its generation.
    pub(crate) fn take(&self, generation: u64) -> Option<SourceTrigger> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = entries
            .iter()
            .position(|entry| entry.generation == generation)?;
        Some(entries.remove(index))
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns how long a burst of file events is joined into one signal, read
/// once per process.
pub(crate) fn debounce_duration() -> Duration {
    static RESOLVED: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *RESOLVED.get_or_init(|| {
        let milliseconds = std::env::var(DEBOUNCE_OVERRIDE_VARIABLE)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_DEBOUNCE_MILLISECONDS);
        Duration::from_millis(milliseconds)
    })
}

/// Whether an event kind is a source change worth rebuilding for.
///
/// Content changes, file creations and file deletions all matter. A rename
/// arrives as a remove/create pair or as a name modification, depending on the
/// platform, so the same three kinds cover it. Every `Modify` kind counts,
/// metadata-only included, because an editor's save can arrive as one. Access
/// notifications never do.
pub(crate) fn is_relevant_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Whether a changed absolute path should start a rebuild.
///
/// Both sides are turned into their real paths first, so a symlinked directory
/// cannot pass events from outside the watch tree. The remaining rules are
/// then applied to the relative path. A deleted path resolves through its
/// nearest ancestor that still exists (see
/// [`canonicalize_through_existing_ancestor`]), because a file that was just
/// removed cannot be resolved at all.
fn is_relevant_path(path: &Path, watch_root: &Path) -> bool {
    let Ok(canonical_root) = watch_root.canonicalize() else {
        return false;
    };
    let Some(canonical_path) = canonicalize_through_existing_ancestor(path) else {
        return false;
    };
    if !canonical_path.starts_with(&canonical_root) {
        return false;
    }
    // The `starts_with` check above already verified the prefix, so this
    // strip cannot fail.
    let relative_path = canonical_path.strip_prefix(&canonical_root).unwrap();
    is_relevant_relative_path(relative_path)
}

/// Resolve a path that may no longer exist.
///
/// `Path::canonicalize` needs every part of the path to exist, and a file that
/// was just deleted cannot meet that. This walks up through the path's
/// ancestors to the first one that resolves, then puts the missing parts back
/// as plain names. A path that still exists resolves exactly as `canonicalize`
/// would, and a removed one still resolves to the place it used to occupy. A
/// `..` among the missing parts is refused: the walk up already resolved the
/// directory, and a component that climbed back out of it would hand the
/// watch-root check a path it would then trust.
fn canonicalize_through_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut missing_tail: Vec<&OsStr> = Vec::new();
    for ancestor in path.ancestors() {
        if let Ok(mut resolved) = ancestor.canonicalize() {
            for component in missing_tail.iter().rev() {
                if *component == OsStr::new("..") {
                    return None;
                }
                resolved.push(component);
            }
            return Some(resolved);
        }
        if let Some(name) = ancestor.file_name() {
            missing_tail.push(name);
        }
    }
    None
}

/// Whether a path relative to the watch root should start a rebuild.
///
/// A build output directory is ignored only at the top of the watch tree;
/// deeper directories may legitimately use the same names. Hidden files and
/// directories, and editor temporary files, are excluded at any depth.
fn is_relevant_relative_path(relative_path: &Path) -> bool {
    // Build outputs are anchored to the watch root: `target/...` directly
    // beneath it is compiler output, while `src/target_logic/` is source.
    if let Some(first) = relative_path.components().next() {
        if let Some(name) = first.as_os_str().to_str() {
            if IGNORED_DIRECTORY_NAMES.contains(&name) {
                return false;
            }
        }
    }

    // Hidden entries are excluded at any depth.
    for component in relative_path.components() {
        if let Some(name) = component.as_os_str().to_str() {
            if name.starts_with(HIDDEN_FILE_PREFIX) {
                return false;
            }
        }
    }

    // Editor temporary files appear and disappear around every save and
    // would otherwise start a rebuild of half-written content. A file name
    // that is not valid UTF-8 counts as relevant: it cannot match any ignore
    // suffix, and filtering it out would stop hot reload without a word.
    match relative_path.file_name().and_then(|name| name.to_str()) {
        Some(name) => !EDITOR_TEMPORARY_FILE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix)),
        None => true,
    }
}

/// Spawn the notify watcher and its debounce worker, the part the source,
/// asset and shader watchers share.
///
/// The callback keeps every path that `accepts_path` allows out of every event
/// that `accepts_event` allows. The worker waits out the debounce window,
/// collects the paths the burst produced and hands them to `on_settled_burst`,
/// which returns whether the worker should keep running. The watcher itself
/// moves into the worker, so its OS handles stay registered exactly as long as
/// the worker lives. `callback_error_message` is the message logged when the
/// backend hands the callback an error; watching must never panic inside it,
/// because some backends run it on their own threads.
///
/// # Errors
///
/// Returns [`WatcherError::CreationFailed`] when the platform watcher or the
/// worker thread cannot be created, and
/// [`WatcherError::RegistrationFailed`] when `root` cannot be watched.
pub(crate) fn spawn_settled_watcher<F>(
    root: PathBuf,
    thread_name: &str,
    callback_error_message: &'static str,
    accepts_event: impl Fn(&EventKind) -> bool + Send + 'static,
    accepts_path: impl Fn(&Path) -> bool + Send + 'static,
    mut on_settled_burst: F,
) -> Result<(), WatcherError>
where
    F: FnMut(&Path, Vec<PathBuf>) -> bool + Send + 'static,
{
    // Step 1: Create the watcher whose callback filters events and forwards
    // the accepted paths to the worker.
    let (path_sender, paths) = std::sync::mpsc::channel::<PathBuf>();
    let mut watcher = RecommendedWatcher::new(
        move |result: Result<Event, notify::Error>| match result {
            Ok(event) => {
                if accepts_event(&event.kind) {
                    for path in event.paths {
                        if accepts_path(&path) {
                            // A failed send only means the worker has shut
                            // down, and the callback can do nothing about it.
                            let _ = path_sender.send(path);
                        }
                    }
                }
            }
            // Never panic inside the callback: some backends run it on their
            // own threads. Log the error and carry on.
            Err(error) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    error = %error,
                    "{}",
                    callback_error_message
                );
            }
        },
        Config::default(),
    )
    .map_err(|source| WatcherError::CreationFailed {
        source: Box::new(source),
    })?;

    // Step 2: Register the watch as recursive, which covers the whole tree
    // without every caller listing its own directories.
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|source| WatcherError::RegistrationFailed {
            path: root.display().to_string(),
            source: Box::new(source),
        })?;

    // Step 3: Spawn the worker thread and hand the watcher over to it.
    std::thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            // RecommendedWatcher unregisters its OS handles when dropped.
            // Move it into the worker even though the loop never calls it
            // directly, keeping those handles alive for exactly as long as
            // the worker lives.
            let _watcher = watcher;
            while let Ok(first_path) = paths.recv() {
                // One save can produce several notifications for the same
                // file, so wait for the burst to stop before collecting it.
                std::thread::sleep(debounce_duration());
                let mut touched_paths = vec![first_path];
                touched_paths.extend(paths.try_iter());
                if !on_settled_burst(&root, touched_paths) {
                    // The callback asked the worker to stop, so it ends.
                    return;
                }
            }
        })
        .map_err(|source| WatcherError::CreationFailed {
            source: Box::new(source),
        })?;
    Ok(())
}

/// Watch the configured source tree and ask for a reload from a worker
/// thread.
///
/// # Errors
///
/// Returns [`WatcherError::WatchDirectoryMissing`] when the joined watch path
/// does not exist, and otherwise the failures [`spawn_settled_watcher`]
/// reports: creating the watcher or the worker, and registering the watch.
pub(crate) fn spawn_source_watcher(
    workspace_root: PathBuf,
    module_name: &str,
    watch_directory: &str,
    reload_generation: Arc<AtomicU64>,
    triggers: Option<Arc<TriggerLog>>,
) -> Result<(), WatcherError> {
    // Step 1: Resolve and validate the configured watch directory.
    // Watch paths are configured relative to the workspace root so the same
    // configuration works regardless of the process's current directory.
    let watch_path = workspace_root.join(watch_directory);

    // Fail during host setup instead of running the whole session without hot
    // reload when a module configuration names a source path that no longer
    // exists.
    if !watch_path.exists() {
        return Err(WatcherError::WatchDirectoryMissing {
            path: watch_path.display().to_string(),
        });
    }

    info!(
        target: telemetry_target::HOT_RELOAD,
        module = module_name,
        // The configured form, relative to the workspace: shorter than the
        // joined absolute path, and with one kind of separator.
        watch_directory = %watch_directory.replace('\\', "/"),
        "watching for source changes"
    );

    // Step 2: Spawn the shared watcher worker with the filters this watcher
    // needs (paths resolved under the watch root, relative paths that matter)
    // and the handler for one settled burst: report the paths, log the change,
    // add one to the reload generation and record the trigger.
    let accept_root = watch_path.clone();
    let report_root = watch_path.clone();
    spawn_settled_watcher(
        watch_path,
        "pill source watcher",
        "file watcher error",
        is_relevant_event,
        move |path| is_relevant_path(path, &accept_root),
        move |_root, touched_paths| {
            // One source save can produce several notifications for the same
            // file, so the burst is deduplicated before it is reported.
            let changed_paths: HashSet<PathBuf> = touched_paths.into_iter().collect();

            // Build a short report of the changed paths for the console. Paths
            // are printed relative to the watched directory.
            let mut changed_path_report: String = changed_paths
                .iter()
                .take(REPORTED_PATH_LIMIT)
                .map(|path| {
                    path.strip_prefix(&report_root)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                })
                .collect::<Vec<_>>()
                .join(", ");
            if changed_paths.len() > REPORTED_PATH_LIMIT {
                changed_path_report.push_str(&format!(
                    " (+{} more)",
                    changed_paths.len() - REPORTED_PATH_LIMIT
                ));
            }
            // How long the edit waited before this thread saw it, measured from
            // the file's own modification time. Logged at INFO because a slow
            // save-to-live is dominated by this number rather than by the patch
            // that follows it, and nothing outside this process can measure it
            // without timing the whole span.
            //
            // The debounce window above is part of the number, so a healthy
            // reading is a little over that window, not zero.
            let detection_delay_ms = changed_paths
                .iter()
                .filter_map(|path| std::fs::metadata(path).ok())
                .filter_map(|metadata| metadata.modified().ok())
                .max()
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .map(|elapsed| elapsed.as_secs_f64() * 1000.0)
                .unwrap_or(f64::NAN);
            info!(
                target: telemetry_target::HOT_RELOAD,
                changed_paths = %changed_path_report,
                detection_delay_ms = format!("{detection_delay_ms:.0}").as_str(),
                debounce_ms = debounce_duration().as_millis() as u64,
                "source change detected"
            );

            // Add one to the reload generation. The main loop compares the
            // value against the last one it handled, so a signal is never lost.
            // Release publishing makes every earlier write on this thread
            // visible to the Acquire read on the frame loop.
            let generation = reload_generation.fetch_add(1, Ordering::Release) + 1;
            // Publish the signal's instant beside the counter, so the frame
            // loop can count the reload it starts as caused by this save rather
            // than by the frame that noticed the save.
            if let Some(triggers) = triggers.as_ref() {
                triggers.record(SourceTrigger {
                    generation,
                    fired_at: Instant::now(),
                    detection_delay_ms,
                });
            }
            true
        },
    )?;

    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // External crates
    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode,
    };

    /// Checks that content, creation and removal events count as a source
    /// change, while access and other events do not.
    #[test]
    fn relevant_events_cover_changes_and_exclude_noise() {
        assert!(is_relevant_event(&EventKind::Create(CreateKind::File)));
        assert!(is_relevant_event(&EventKind::Modify(ModifyKind::Data(
            DataChange::Any
        ))));
        // A rename arrives as a name modification on some platforms.
        assert!(is_relevant_event(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
        assert!(is_relevant_event(&EventKind::Remove(RemoveKind::File)));
        assert!(!is_relevant_event(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(!is_relevant_event(&EventKind::Other));
    }

    /// Checks that a build output directory is skipped only at the top of the
    /// watch tree, while a deeper source directory with the same name still
    /// counts.
    #[test]
    fn build_output_directories_are_filtered_only_at_the_watch_root() {
        assert!(!is_relevant_relative_path(Path::new(
            "target/debug/libproject.so"
        )));
        assert!(!is_relevant_relative_path(Path::new(
            "bin/Release/project_cs.dll"
        )));
        assert!(!is_relevant_relative_path(Path::new(
            "obj/x64/project.cache"
        )));
        // A deeper directory may legitimately use one of these names.
        assert!(is_relevant_relative_path(Path::new(
            "src/target_logic/module.rs"
        )));
        assert!(is_relevant_relative_path(Path::new("scripts/bin/run.rs")));
        assert!(is_relevant_relative_path(Path::new("src/main.rs")));
        assert!(is_relevant_relative_path(Path::new("Bird.cs")));
    }

    /// Checks that a hidden file or directory is skipped at any depth.
    #[test]
    fn hidden_files_and_directories_are_filtered_out() {
        assert!(!is_relevant_relative_path(Path::new(".hidden_file")));
        assert!(!is_relevant_relative_path(Path::new(
            "src/.hidden_dir/file.rs"
        )));
    }

    /// Checks that an editor's temporary and swap files never start a rebuild.
    #[test]
    fn editor_temporary_and_swap_files_are_filtered_out() {
        assert!(!is_relevant_relative_path(Path::new("src/main.rs~")));
        assert!(!is_relevant_relative_path(Path::new("src/.main.rs.swp")));
        assert!(!is_relevant_relative_path(Path::new("src/main.rs.swx")));
    }

    /// Checks that deleting a source file inside the watch tree still counts as
    /// a change, which a plain `canonicalize` cannot express, while deleting
    /// editor noise or a file outside the root does not.
    #[test]
    fn deleted_source_files_stay_relevant() {
        let base = std::env::temp_dir().join(format!("pill_watcher_delete_{}", std::process::id()));
        let watch_root = base.join("watch");
        std::fs::create_dir_all(watch_root.join("src")).unwrap();

        let deleted = watch_root.join("src").join("project.rs");
        std::fs::write(&deleted, "// test").unwrap();
        std::fs::remove_file(&deleted).unwrap();
        assert!(
            is_relevant_path(&deleted, &watch_root),
            "a removed source file must still be relevant"
        );

        let outside = base.join("outside.rs");
        assert!(!is_relevant_path(&outside, &watch_root));

        let noise = watch_root.join("src").join("main.rs.swx");
        assert!(!is_relevant_path(&noise, &watch_root));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Checks that a file outside the watch root is rejected and one inside it
    /// is accepted. Both sides are resolved to their real paths before the
    /// comparison, which is also what stops a symlink from leading out of the
    /// tree.
    #[test]
    fn paths_outside_the_watch_root_are_filtered_out() {
        let base = std::env::temp_dir().join(format!("pill_watcher_test_{}", std::process::id()));
        let watch_root = base.join("watch");
        let outside = base.join("outside");
        std::fs::create_dir_all(&watch_root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let outside_file = outside.join("project.rs");
        std::fs::write(&outside_file, "// test").unwrap();
        assert!(!is_relevant_path(&outside_file, &watch_root));

        let inside_file = watch_root.join("main.rs");
        std::fs::write(&inside_file, "// test").unwrap();
        assert!(is_relevant_path(&inside_file, &watch_root));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Checks that the trigger log hands each generation back exactly once,
    /// and that it stays within its bound when nobody reads it.
    #[test]
    fn trigger_log_takes_each_generation_once_and_stays_bounded() {
        let log = TriggerLog::new();
        log.record(SourceTrigger {
            generation: 1,
            fired_at: Instant::now(),
            detection_delay_ms: 5.0,
        });
        let taken = log.take(1).expect("the recorded trigger is handed back");
        assert_eq!(taken.generation, 1);
        assert_eq!(taken.detection_delay_ms, 5.0);
        assert!(
            log.take(1).is_none(),
            "a taken generation cannot be taken twice"
        );

        for generation in 2..=(TRIGGER_LOG_CAPACITY as u64 + 4) {
            log.record(SourceTrigger {
                generation,
                fired_at: Instant::now(),
                detection_delay_ms: f64::NAN,
            });
        }
        assert!(
            log.take(2).is_none(),
            "entries beyond the bound are evicted oldest first"
        );
        assert!(log.take(TRIGGER_LOG_CAPACITY as u64 + 4).is_some());
    }

    /// Checks that a file name that is not valid UTF-8 still counts, instead of
    /// being dropped without a word. Unix-only, because a non-UTF-8 `OsStr`
    /// cannot be built through stable APIs on Windows.
    #[cfg(unix)]
    #[test]
    fn non_utf8_file_names_stay_relevant() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        assert!(is_relevant_relative_path(Path::new(OsStr::from_bytes(
            b"src/caf\xe9.rs"
        ))));
    }
}
