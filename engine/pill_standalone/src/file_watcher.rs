use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

// Struct to watch a directory for file changes.
pub struct FileWatcher {
    path: PathBuf,                                   // The directory path being watched
    recursive: bool,                                 // Whether to watch subfolders recursively
    previous_metadata: HashMap<PathBuf, SystemTime>, // Tracks file paths and their last modified time
}

impl FileWatcher {
    // Create a new FileWatcher for a given directory.
    pub fn new(path: PathBuf) -> Self {
        // Initialize with the current metadata of files in the directory
        let previous_metadata = Self::get_file_metadata(&path, false);
        Self {
            path,
            previous_metadata,
            recursive: false,
        }
    }

    pub fn set_recursive(mut self, recursive: bool) -> Self {
        self.recursive = recursive;
        self.previous_metadata = Self::get_file_metadata(&self.path, recursive);
        self
    }

    // Retrieve metadata (modified times) for all files in a given directory.
    fn get_file_metadata(path: &Path, recursive: bool) -> HashMap<PathBuf, SystemTime> {
        let mut file_metadata = HashMap::new();
        Self::scan_directory(path, recursive, &mut file_metadata);
        file_metadata
    }

    // Recursively scan directory and collect file metadata
    fn scan_directory(
        path: &Path,
        recursive: bool,
        file_metadata: &mut HashMap<PathBuf, SystemTime>,
    ) {
        // Attempt to read the directory entries
        if let Ok(entries) = fs::read_dir(path) {
            // Iterate through each directory entry
            for entry in entries.filter_map(Result::ok) {
                let entry_path = entry.path();

                // If it's a directory and we're in recursive mode, scan it
                if entry_path.is_dir() && recursive {
                    Self::scan_directory(&entry_path, recursive, file_metadata);
                } else if entry_path.is_file() {
                    // Try to retrieve file metadata
                    if let Ok(metadata) = entry_path.metadata() {
                        // Get the last modified time of the file
                        if let Ok(modified) = metadata.modified() {
                            // Optional check: ensure the file name is valid UTF-8
                            if let Some(file_name) = entry_path.file_name() {
                                if let Some(file_name_str) = file_name.to_str() {
                                    // Skip hidden or temp files
                                    if file_name_str.starts_with('.')
                                        || file_name_str.ends_with("~")
                                        || file_name_str.ends_with(".swp")
                                    //  TODO: skip also the
                                    //  new dll file
                                    {
                                        continue;
                                    }
                                    // Store the file path and its modified time
                                    file_metadata.insert(entry_path, modified);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Check for changes compared to the last known state.
    // Returns a list of paths that have been added, modified, or deleted.
    fn check_for_changes(&mut self) -> Vec<PathBuf> {
        // Get the current state of the directory
        let current_metadata = Self::get_file_metadata(&self.path, self.recursive);
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
