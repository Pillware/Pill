//! Test-only helpers: scratch directories and explicit file dates.
//!
//! # Responsibilities
//!
//! - Give every host test a fresh directory under the process's temporary
//!   folder, named for the test, the process id and a counter. Two test
//!   processes, such as a second checkout or a second agent, therefore never
//!   share a path.
//! - Remove that directory when its [`ScratchDirectory`] guard is dropped. A
//!   test that fails still releases its directory.
//! - Set a file's modification time by hand, for tests that ask which of two
//!   files is newer. The answer then does not depend on the filesystem's
//!   timestamp precision or on the wall clock.
//!
//! # Design
//!
//! One guard serves the crate's tests, so every directory made through it
//! follows the same naming scheme. It cleans up in `Drop`, not in a call at
//! the end of the test: a failing test never reaches the end of its body, and
//! its directory still goes away.

// Standard library
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

// =============================================================================
// Constants
// =============================================================================

/// The number given to the next scratch directory, so two directories made in
/// one process never share a name.
static NEXT_SCRATCH_DIRECTORY_INDEX: AtomicUsize = AtomicUsize::new(0);

// =============================================================================
// Types
// =============================================================================

/// An empty directory under the process's temporary folder, removed when the
/// guard is dropped.
pub(crate) struct ScratchDirectory {
    /// The directory this guard owns.
    path: PathBuf,
}

impl ScratchDirectory {
    /// Creates an empty directory whose name is `pill_host_` followed by
    /// `test_label`, the process id and a number that is new for every call.
    ///
    /// `test_label` is the test's own name. It becomes part of the directory
    /// name, so a directory left behind in the temporary folder shows which
    /// test made it.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created, for example when the
    /// temporary folder is not writable.
    pub(crate) fn new(test_label: &str) -> Self {
        let counter = NEXT_SCRATCH_DIRECTORY_INDEX.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "pill_host_{test_label}_{}_{counter}",
            std::process::id()
        ));
        // The name holds this process's id and a counter, so a directory that
        // is already there comes from an earlier run with the same id. Remove
        // it so the test starts with an empty folder.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self { path }
    }

    /// Returns the directory, for a test that hands it to the code it tests.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Writes a file inside the directory and returns its full path. Missing
    /// parent folders are created first.
    ///
    /// `relative_path` is resolved against the directory.
    ///
    /// # Panics
    ///
    /// Panics if a parent folder or the file cannot be written.
    pub(crate) fn write(&self, relative_path: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the scratch parent directory");
        }
        std::fs::write(&path, contents).expect("write the scratch file");
        path
    }
}

impl Deref for ScratchDirectory {
    type Target = Path;

    /// Returns the directory, so `root.join("x")` and passing `&root` where a
    /// `&Path` is expected both work without a call.
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for ScratchDirectory {
    /// Returns the directory, for the `AsRef<Path>` parameters of `std::fs`.
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDirectory {
    fn drop(&mut self) {
        // Best effort: a directory the test already removed is fine, and a
        // failed removal is ignored.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Sets a file's modification time, so a test can decide which of two files is
/// newer without waiting for the clock.
///
/// Setting one side of the comparison by hand makes the answer exact. This
/// works on files only: Windows cannot open a directory to set its time, so a
/// test that compares directory times cannot create subfolders.
///
/// # Panics
///
/// Panics if the file cannot be opened for writing or its time cannot be set.
pub(crate) fn date_file(path: &Path, modified_at: SystemTime) {
    let file = std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {} to date it: {error}", path.display()));
    file.set_modified(modified_at)
        .unwrap_or_else(|error| panic!("set the modification time of {}: {error}", path.display()));
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Two guards in one process never share a path, and both exist.
    #[test]
    fn guards_get_distinct_live_directories() {
        let first = ScratchDirectory::new("distinct");
        let second = ScratchDirectory::new("distinct");
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir());
        assert!(second.path().is_dir());
    }

    /// Dropping the guard removes the directory and the files inside it.
    #[test]
    fn drop_removes_the_directory() {
        let path = {
            let scratch = ScratchDirectory::new("cleanup");
            scratch.write("nested/file.txt", "contents");
            assert!(scratch.join("nested/file.txt").is_file());
            scratch.path().to_path_buf()
        };
        assert!(
            !path.exists(),
            "the guard must remove its directory on drop"
        );
    }
}
