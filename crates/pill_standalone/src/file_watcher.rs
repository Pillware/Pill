use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

// Struct to watch a directory for file changes.
pub struct FileWatcher {
    path: PathBuf, // The directory path being watched
    previous_metadata: HashMap<PathBuf, SystemTime>, // Tracks file paths and their last modified time
}

impl FileWatcher {
    // Create a new FileWatcher for a given directory.
    pub fn new(path: PathBuf) -> Self {
        // Initialize with the current metadata of files in the directory
        let previous_metadata = Self::get_file_metadata(&path);
        Self { path, previous_metadata }
    }

    // Retrieve metadata (modified times) for all files in a given directory.
    fn get_file_metadata(path: &Path) -> HashMap<PathBuf, SystemTime> {
        let mut file_metadata = HashMap::new();

        // Attempt to read the directory entries
        if let Ok(entries) = fs::read_dir(path) {
            // Iterate through each directory entry
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                // Try to retrieve file metadata
                if let Ok(metadata) = path.metadata() {
                    // Get the last modified time of the file
                    if let Ok(modified) = metadata.modified() {
                        // Optional check: ensure the file name is valid UTF-8
                        if let Some(file_name) = path.file_name() {
                            if let Some(file_name_str) = file_name.to_str() {
                                // Skip hidden or temp files
                                if file_name_str.starts_with('.') || file_name_str.ends_with("~") || file_name_str.ends_with(".swp") {
                                    continue;
                                }
                                // Store the file path and its modified time
                                file_metadata.insert(path, modified);
                            }
                        }
                    }
                }
            }
        }

        file_metadata
    }

    // Check for changes compared to the last known state.
    // Returns a list of paths that have been added, modified, or deleted.
    fn check_for_changes(&mut self) -> Vec<PathBuf> {
        // Get the current state of the directory
        let current_metadata = Self::get_file_metadata(&self.path);
        let mut changes = Vec::new();

        // Detect modified or newly added files
        for (file_path, modified_time) in &current_metadata {
            match self.previous_metadata.get(file_path) {
                // File exists, but was modified
                Some(&prev_time) if prev_time != *modified_time => {
                    changes.push(file_path.clone());
                }
                // File exists and has not changed
                Some(_) => {
                    // No action required
                }
                // New file
                None => {
                    changes.push(file_path.clone());
                }
            }
        }

        // Detect deleted files
        for file_path in self.previous_metadata.keys() {
            if !current_metadata.contains_key(file_path) {
                changes.push(file_path.clone());
            }
        }

        // Update the stored metadata for next comparison
        self.previous_metadata = current_metadata;

        changes
    }

    // Public interface to retrieve changed files.
    // Returns `Some(Vec<PathBuf>)` if there are changes, otherwise `None`.
    pub fn get_changes(&mut self) -> Option<Vec<PathBuf>> {
        let changes = self.check_for_changes();
        if !changes.is_empty() {
            Some(changes.clone())
        } else {
            None
        }
    }
}