//! Development asset reload: watch the project's `res` folder and report which
//! source assets an edit affects.
//!
//! # Responsibilities
//!
//! - Watch the project's asset directory recursively and group a burst of
//!   events into one batch (debounce), the same way source edits are grouped.
//!   Each edited file maps to the source asset it belongs to: an asset file is
//!   its own source, and a metadata file (`a.png.meta`) belongs to the source
//!   named before `.meta`.
//! - Queue each change as an [`AssetChange`] for the main thread, which
//!   imports or reimports it through the world's
//!   [`ImportRegistry`](pill_engine::ImportRegistry).
//!
//! # Design
//!
//! The worker thread runs only host code and never calls into a module. A
//! reload can therefore unload a module at any moment and never leave a thread
//! running inside code that is going away. The main thread applies the queue
//! at the frame boundary, after the reloads, using the import registrations
//! that are current at that moment. The shader watcher splits the work the
//! same way.
//!
//! The watcher reports paths, not decisions. Whether a changed path is an
//! asset at all, whether it is loaded, and what to do about a deleted source
//! are all decided on the main thread, where the registry and the asset store
//! live.
//!
//! Dot-files are ignored. A metadata write goes through a temporary sibling
//! named `.<file name>.<pid>-<n>.tmp`, and the file it produces is what
//! counts.

// Standard library
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

// External crates
use notify::event::{EventKind, ModifyKind, RenameMode};
use pill_core::error::WatcherError;
use pill_core::info;
use pill_core::telemetry::telemetry_target;

// Current crate
use super::shared::{is_relevant_event, spawn_settled_watcher};

// =============================================================================
// Constants
// =============================================================================

/// The extension of a metadata file, appended to its source's name.
const METADATA_SUFFIX: &str = ".meta";

// =============================================================================
// Types
// =============================================================================

/// One source asset's change, worked out from the files on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssetChange {
    /// The source or its metadata file was created or edited: import it, or
    /// reimport it when it is loaded.
    Edited {
        /// The source, relative to the asset directory, `/`-separated.
        source: String,
        /// Whether the event was on the metadata file rather than the source.
        through_metadata: bool,
    },
    /// The source file is gone.
    SourceRemoved {
        /// The source, relative to the asset directory, `/`-separated.
        source: String,
    },
    /// The metadata file is gone while its source remains.
    MetadataRemoved {
        /// The source, relative to the asset directory, `/`-separated.
        source: String,
    },
}

/// A running asset watcher. Dropping it makes the worker stop when it next
/// reports a batch.
pub(crate) struct AssetWatcher {
    /// Queued changes, oldest first; at most one per source per batch.
    changes: Receiver<AssetChange>,
}

impl AssetWatcher {
    /// Watches `asset_directory` and queues every change in it.
    ///
    /// Returns `Ok(None)` when the directory does not exist: a project without
    /// `res` has nothing to reload.
    ///
    /// # Errors
    ///
    /// Returns a [`WatcherError`] when the watcher or its worker thread cannot
    /// be created, or the directory cannot be registered for watching.
    pub(crate) fn spawn(asset_directory: PathBuf) -> Result<Option<Self>, WatcherError> {
        if !asset_directory.is_dir() {
            return Ok(None);
        }

        // Step 1: Open the queue the worker fills and the main thread drains.
        let (change_sender, changes) = channel::<AssetChange>();
        info!(
            target: telemetry_target::HOT_RELOAD,
            directory = %asset_directory.display(),
            "[assets] watching the project's assets for edits"
        );

        // Step 2: Start the shared watcher worker. It accepts every relevant
        // event and any rename event: some backends report a rename as a name
        // change rather than as a create and a remove. All paths pass
        // through, because deciding which of them are assets is the main
        // thread's job.
        spawn_settled_watcher(
            asset_directory,
            "pill asset watcher",
            "asset watcher error",
            |kind: &EventKind| is_relevant_event(kind) || is_rename(kind),
            |_path: &Path| true,
            move |root, touched| {
                for change in classify_touched_paths(root, &touched) {
                    if change_sender.send(change).is_err() {
                        // The host dropped the watcher: stop.
                        return false;
                    }
                }
                true
            },
        )?;

        Ok(Some(Self { changes }))
    }

    /// Every change queued since the last call, oldest first.
    pub(crate) fn drain(&self) -> Vec<AssetChange> {
        self.changes.try_iter().collect()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns whether the event is one side of a rename. Some backends report a
/// rename this way instead of as a create and a remove.
fn is_rename(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Modify(ModifyKind::Name(
            RenameMode::Any | RenameMode::From | RenameMode::To | RenameMode::Both
        ))
    )
}

/// Turns the paths of one batch into one change per source, based on what is
/// on disk now.
///
/// What exists after the debounce is what counts, not the event kinds. When an
/// editor saves through a temporary file, one save shows up as a remove and a
/// create; the source file still being there is what makes it an edit.
fn classify_touched_paths(asset_directory: &Path, touched: &[PathBuf]) -> Vec<AssetChange> {
    // One entry per source, so several events for the same asset in one batch
    // (the source and its metadata, or one save reported three times) collapse
    // into a single change.
    let mut changes_by_source: BTreeMap<String, AssetChange> = BTreeMap::new();
    for path in touched {
        let Some(relative_path) = relative_to(asset_directory, path) else {
            continue;
        };
        let file_name = relative_path.rsplit('/').next().unwrap_or(&relative_path);
        if file_name.starts_with('.') || path.is_dir() {
            continue;
        }
        let (source, through_metadata) = match relative_path.strip_suffix(METADATA_SUFFIX) {
            Some(source) if !source.is_empty() => (source.to_owned(), true),
            _ => (relative_path.clone(), false),
        };
        let source_exists = asset_directory.join(&source).is_file();
        let change = if !source_exists {
            AssetChange::SourceRemoved {
                source: source.clone(),
            }
        } else if through_metadata && !path.is_file() {
            AssetChange::MetadataRemoved {
                source: source.clone(),
            }
        } else {
            AssetChange::Edited {
                source: source.clone(),
                through_metadata,
            }
        };
        // A source edit wins over a metadata edit for the same asset in one
        // batch. Both mean the asset must be read again, but only the source
        // edit also imports it when nothing has loaded it yet.
        let should_keep_existing = matches!(
            (changes_by_source.get(&source), &change),
            (
                Some(AssetChange::Edited {
                    through_metadata: false,
                    ..
                }),
                AssetChange::Edited { .. }
            )
        );
        if !should_keep_existing {
            changes_by_source.insert(source, change);
        }
    }
    changes_by_source.into_values().collect()
}

/// Returns `path` relative to `asset_directory`, with `/` separators. Returns
/// `None` when the path is outside `asset_directory`, is that directory itself,
/// or is not UTF-8.
fn relative_to(asset_directory: &Path, path: &Path) -> Option<String> {
    let relative_path = path.strip_prefix(asset_directory).ok()?;
    let slash_separated_path = relative_path.to_str()?.replace('\\', "/");
    (!slash_separated_path.is_empty()).then_some(slash_separated_path)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch asset directory, removed afterwards.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("pill-asset-watcher-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("textures")).unwrap();
            Self(root)
        }

        fn write(&self, relative_path: &str) -> PathBuf {
            let path = self.0.join(relative_path);
            std::fs::write(&path, b"x").unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_metadata_edit_is_a_change_to_its_source() {
        let scratch = Scratch::new("metadata-edit");
        scratch.write("textures/a.png");
        let metadata = scratch.write("textures/a.png.meta");

        assert_eq!(
            classify_touched_paths(&scratch.0, &[metadata]),
            [AssetChange::Edited {
                source: "textures/a.png".to_owned(),
                through_metadata: true,
            }]
        );
    }

    #[test]
    fn several_events_for_one_asset_become_one_change() {
        let scratch = Scratch::new("coalesce");
        let source = scratch.write("textures/a.png");
        let metadata = scratch.write("textures/a.png.meta");

        assert_eq!(
            classify_touched_paths(&scratch.0, &[source.clone(), metadata, source]),
            [AssetChange::Edited {
                source: "textures/a.png".to_owned(),
                through_metadata: false,
            }]
        );
    }

    #[test]
    fn removed_sources_and_metadata_are_told_apart() {
        let scratch = Scratch::new("removed");
        scratch.write("textures/kept.png");

        let changes = classify_touched_paths(
            &scratch.0,
            &[
                scratch.0.join("textures/gone.png"),
                scratch.0.join("textures/kept.png.meta"),
            ],
        );

        assert_eq!(
            changes,
            [
                AssetChange::SourceRemoved {
                    source: "textures/gone.png".to_owned()
                },
                AssetChange::MetadataRemoved {
                    source: "textures/kept.png".to_owned()
                },
            ]
        );
    }

    #[test]
    fn dot_files_directories_and_outside_paths_are_ignored() {
        let scratch = Scratch::new("ignored");
        let temporary = scratch.write("textures/.a.png.meta.12-0.tmp");

        let changes = classify_touched_paths(
            &scratch.0,
            &[
                temporary,
                scratch.0.join("textures"),
                std::env::temp_dir().join("x.png"),
            ],
        );

        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn a_missing_directory_starts_no_watcher() {
        let missing = std::env::temp_dir().join("pill-asset-watcher-definitely-missing");
        assert!(AssetWatcher::spawn(missing).unwrap().is_none());
    }
}
