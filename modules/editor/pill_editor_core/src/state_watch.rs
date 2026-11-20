//! Polling the editor's state files for changes.
//!
//! # Responsibilities
//!
//! - Notice that one of the editor's state files changed without a writer to
//!   hook: the editor's files are also written by hand and by other tools, so
//!   the file's own metadata is the signal ([`FileStamp`]).
//! - Read a file again only when that metadata moved or the regular re-read is
//!   due, and keep its text so a file that did not change costs nothing
//!   ([`StateFileWatch`]).
//!
//! # Design
//!
//! A watch re-reads its file when the file's stamp (its modification time and
//! length, see [`FileStamp`]) moved. It also re-reads every
//! [`STATE_FILE_POLL_FRAMES`] frames whatever the stamp says, which catches an
//! edit whose modification time is too coarse to show it. A caller whose apply
//! can fail calls [`StateFileWatch::poll_now`] until it has applied once, so
//! the watch does not need to remember whether the caller applied its last
//! answer.
//!
//! The editor's theme watch keeps a stamp for each file it follows. It looks
//! the theme up again instead of caching a document's text, so
//! [`StateFileWatch`] is not used there.

// Standard library
use std::path::{Path, PathBuf};

// =============================================================================
// Constants
// =============================================================================

/// The number of frames after which a state file is re-read even when its
/// stamp did not move.
///
/// A filesystem may report a modification time that changes less often than
/// the file does, so the stamp alone can miss an edit. This regular re-read
/// bounds how long the editor can miss it.
pub const STATE_FILE_POLL_FRAMES: u64 = 30;

// =============================================================================
// Types
// =============================================================================

/// The metadata of a state file, which the editor compares from frame to frame
/// to decide whether the file needs reading at all.
///
/// A missing file stamps as the default. No file that exists has that stamp,
/// so creating the file is noticed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileStamp {
    /// The file's modification time, when the filesystem reports one.
    modification_time: Option<std::time::SystemTime>,
    /// The file's length in bytes.
    length_bytes: u64,
}

impl FileStamp {
    /// Returns the stamp of `path`, or the default stamp when the file cannot
    /// be read.
    #[must_use]
    pub fn of(path: &Path) -> Self {
        std::fs::metadata(path)
            .map(|metadata| Self {
                modification_time: metadata.modified().ok(),
                length_bytes: metadata.len(),
            })
            .unwrap_or_default()
    }
}

/// One state file the editor polls.
///
/// It holds the file's path, the stamp seen when its text was last read, the
/// text that read produced, and whether the file has been read at all.
#[derive(Debug, Default)]
pub struct StateFileWatch {
    path: PathBuf,
    last_read_stamp: FileStamp,
    text: Option<String>,
    has_read_once: bool,
}

impl StateFileWatch {
    /// Starts a watch on `path` that has not read the file yet.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_read_stamp: FileStamp::default(),
            text: None,
            has_read_once: false,
        }
    }

    /// Returns the path this watch polls.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the text of the most recent read. It is `None` before the first
    /// read and when that read did not succeed.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Re-reads the file when its stamp moved or when the regular re-read is
    /// due.
    ///
    /// `frame_index` is the editor's frame counter. It decides when the
    /// regular re-read falls, once every [`STATE_FILE_POLL_FRAMES`] frames.
    ///
    /// Returns `None` when the file is unchanged, `Some(None)` when it is gone
    /// or could not be read, and `Some(Some(text))` when the text is new. The
    /// first poll always reads, whatever the stamp says.
    pub fn poll(&mut self, frame_index: u64) -> Option<Option<String>> {
        let current_stamp = FileStamp::of(&self.path);
        let is_poll_due = frame_index.is_multiple_of(STATE_FILE_POLL_FRAMES);
        if !is_poll_due && self.has_read_once && current_stamp == self.last_read_stamp {
            return None;
        }
        self.last_read_stamp = current_stamp;
        self.read()
    }

    /// Re-reads the file whatever its stamp says, and returns it only when the
    /// text changed.
    ///
    /// The answer has the same three shapes as [`poll`](Self::poll). A caller
    /// whose apply can fail calls this until it has applied once, so a file
    /// that could not be applied is retried instead of being skipped as
    /// unchanged.
    pub fn poll_now(&mut self) -> Option<Option<String>> {
        self.last_read_stamp = FileStamp::of(&self.path);
        self.read()
    }

    /// Reads the file and returns its text when that text differs from the
    /// cached one, or when this is the first read.
    ///
    /// A file that cannot be read counts as an absent one.
    fn read(&mut self) -> Option<Option<String>> {
        let current_text = std::fs::read_to_string(&self.path).ok();
        let is_first_read = !self.has_read_once;
        self.has_read_once = true;
        if !is_first_read && current_text == self.text {
            return None;
        }
        self.text = current_text;
        Some(self.text.clone())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a scratch directory under the system temp directory, named so
    /// that parallel test runs do not collide.
    fn scratch_directory(test_label: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "ecs-state-watch-{test_label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&directory).expect("the scratch directory");
        directory
    }

    /// Checks the four states a watch has to tell apart: absent, present,
    /// unchanged and edited.
    #[test]
    fn a_watch_follows_absent_present_unchanged_and_edited() {
        let directory = scratch_directory("follow");
        let path = directory.join("state.json");
        let mut watch = StateFileWatch::new(path.clone());

        // Absent: the first poll reads and reports the absence.
        assert_eq!(watch.poll(1), Some(None), "the first poll reads");
        assert_eq!(watch.text(), None);
        // Still absent: the second poll reports no change.
        assert_eq!(watch.poll(2), None);

        // Present: the stamp moved, so the poll reads the file.
        std::fs::write(&path, "{\"a\": 1}").expect("the file");
        assert_eq!(watch.poll(3), Some(Some("{\"a\": 1}".to_owned())));
        assert_eq!(watch.text(), Some("{\"a\": 1}"));

        // Unchanged: the stamp did not move, so nothing is read.
        assert_eq!(watch.poll(4), None);

        // Edited: the two documents have the same length, so a filesystem
        // whose modification time is coarse reports the same stamp and only
        // the regular re-read can see the edit. Polling on a frame that is due
        // is therefore what the contract promises, not a stamp that moves.
        std::fs::write(&path, "{\"a\": 2}").expect("the edit");
        let poll_due_frame = STATE_FILE_POLL_FRAMES * 2;
        assert_eq!(
            watch.poll(poll_due_frame),
            Some(Some("{\"a\": 2}".to_owned()))
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Checks that the regular re-read still happens when the stamp did not
    /// move, which is what catches an edit on a filesystem with a coarse
    /// modification time.
    #[test]
    fn an_unchanged_stamp_is_read_on_the_regular_reread() {
        let directory = scratch_directory("grace");
        let path = directory.join("state.json");
        std::fs::write(&path, "{\"a\": 1}").expect("the file");
        let mut watch = StateFileWatch::new(path.clone());
        assert_eq!(watch.poll(1), Some(Some("{\"a\": 1}".to_owned())));
        // A poll on the regular re-read frame reads the file even though the
        // stamp compares equal. The text is the same, so the poll reports
        // nothing.
        assert_eq!(watch.poll(STATE_FILE_POLL_FRAMES), None);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Checks that a caller that cannot apply yet can ask for a re-read
    /// regardless of the stamp, and gets an answer only when the text really
    /// changed.
    #[test]
    fn poll_now_reads_without_a_stamp_move() {
        let directory = scratch_directory("now");
        let path = directory.join("state.json");
        std::fs::write(&path, "{\"a\": 1}").expect("the file");
        let mut watch = StateFileWatch::new(path.clone());
        assert_eq!(watch.poll(1), Some(Some("{\"a\": 1}".to_owned())));
        assert_eq!(watch.poll_now(), None, "the same text is not a change");
        std::fs::write(&path, "{\"a\": 2}").expect("the edit");
        assert_eq!(watch.poll_now(), Some(Some("{\"a\": 2}".to_owned())));
        let _ = std::fs::remove_dir_all(&directory);
    }
}
