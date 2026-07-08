//! This file implements a lightweight file-system watcher for hot-reload.
//!
//! Polls a directory tree for file additions, modifications, and deletions
//! by comparing last-modified timestamps against a stored snapshot. Skips
//! hidden files, editor temp files, and swap files to avoid false positives.
//!
//! Used by: hot_reload (create_file_watchers, FileWatchers struct)

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// Polls a directory for file additions, modifications, and deletions.
///
/// Stores a snapshot of every tracked file's last-modified timestamp and
/// compares against it on each poll.  Skips hidden files, editor temp files,
/// and swap files to avoid false positives during editing.
pub struct FileWatcher {
    // The directory being watched.
    path: PathBuf,
    // Whether to descend into subdirectories.
    recursive: bool,
    // Canonical path → last modified time for every known file.
    previous_metadata: HashMap<PathBuf, SystemTime>,
}

impl FileWatcher {
    /// Creates a new watcher for a single directory (non-recursive by default).
    /// Takes an immediate snapshot of the current file state.
    pub fn new(path: PathBuf) -> Self {
        let previous_metadata = Self::collect_file_metadata(&path, false);
        Self {
            path,
            previous_metadata,
            recursive: false,
        }
    }

    /// Enables recursive watching and immediately rescans the directory.
    /// Returns self for builder-style chaining.
    pub fn set_recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self.previous_metadata = Self::collect_file_metadata(&self.path, recursive);
        self
    }

    /// Scans a directory and returns a map of canonical path → last modified time
    /// for every non-hidden, non-temp file found.
    fn collect_file_metadata(path: &Path, recursive: bool) -> HashMap<PathBuf, SystemTime> {
        let mut file_metadata = HashMap::new();
        Self::scan_directory(path, recursive, &mut file_metadata);
        file_metadata
    }

    /// Recursively walks a directory tree, recording the last-modified
    /// timestamp for each regular file encountered.
    fn scan_directory(
        path: &Path,
        recursive: bool,
        file_metadata: &mut HashMap<PathBuf, SystemTime>,
    ) {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };

        for entry in entries.filter_map(Result::ok) {
            let entry_path = entry.path();

            if entry_path.is_dir() && recursive {
                // Descend into subdirectories when recursive mode is on.
                Self::scan_directory(&entry_path, recursive, file_metadata);
            } else if entry_path.is_file() {
                // Extract the last-modified timestamp if available.
                if let Ok(metadata) = entry_path.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        // Skip hidden files, editor temp files, and vim swap files.
                        if let Some(file_name) = entry_path.file_name() {
                            if let Some(file_name_str) = file_name.to_str() {
                                if file_name_str.starts_with('.')
                                    || file_name_str.ends_with('~')
                                    || file_name_str.ends_with(".swp")
                                {
                                    continue;
                                }
                                // Use the canonical path as the key so that
                                // comparisons survive symlinks and path normalisation.
                                let key = entry_path
                                    .canonicalize()
                                    .unwrap_or_else(|_| entry_path.clone());
                                file_metadata.insert(key, modified);
                            }
                        }
                    }
                }
            }
        }
    }

    // Compares the current filesystem state against the stored snapshot.
    // Returns every path that was added, modified, or deleted since the
    // last call, then updates the snapshot for the next poll.
    fn check_for_changes(&mut self) -> Vec<PathBuf> {
        let current_metadata = Self::collect_file_metadata(&self.path, self.recursive);
        let mut changes = Vec::new();

        // Detect new and modified files.
        for (file_path, modified_time) in &current_metadata {
            match self.previous_metadata.get(file_path) {
                Some(&previous_time) if previous_time != *modified_time => {
                    // File was modified.
                    changes.push(file_path.clone());
                }
                Some(_) => {
                    // File unchanged — nothing to report.
                }
                None => {
                    // New file appeared.
                    changes.push(file_path.clone());
                }
            }
        }

        // Detect deleted files: keys present in the old snapshot but
        // missing from the current scan.
        for file_path in self.previous_metadata.keys() {
            if !current_metadata.contains_key(file_path) {
                changes.push(file_path.clone());
            }
        }

        self.previous_metadata = current_metadata;
        changes
    }

    // Public polling interface.
    // Returns Some(list) when files have changed, None when nothing changed.
    pub fn get_changes(&mut self) -> Option<Vec<PathBuf>> {
        let changes = self.check_for_changes();
        if !changes.is_empty() {
            Some(changes.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    // Creates a unique temporary directory for a test, keyed by name and
    // nanosecond timestamp to prevent collisions between parallel test runs.
    fn unique_temporary_directory(name: &str) -> PathBuf {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("pill_filewatch_{name}_{nanoseconds}"))
    }

    // Verifies that modifying a tracked file is detected by the watcher.
    #[test]
    fn detects_modified_file() {
        let root = unique_temporary_directory("modify");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let file_path = root.join("project.rs");
        fs::write(&file_path, "v1").unwrap();

        let mut watcher = FileWatcher::new(root.clone()).set_recursive(true);

        // Sleep to ensure the filesystem mtime resolution advances
        // beyond the previous write timestamp.
        std::thread::sleep(Duration::from_millis(1100));
        fs::write(&file_path, "v2").unwrap();

        let changes = watcher.get_changes().unwrap_or_default();
        let canonical = file_path.canonicalize().unwrap_or(file_path);
        assert!(changes.contains(&canonical));
    }
}
