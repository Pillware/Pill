//! The editor's scripted test channel. It reads the commands a devops suite
//! appends to a file, and it reads the auto-quit limit.
//!
//! # Responsibilities
//!
//! - [`TestChannel`]: reads the file named by `PILL_EDITOR_TEST_COMMANDS` and
//!   returns only the lines appended since the last poll.
//! - [`seconds_from_environment`]: the `PILL_EDITOR_TEST_SECONDS` auto-quit
//!   delay, so a run that gets stuck still ends.
//!
//! # Design
//!
//! The channel only reads the file. A suite appends one command per line, and
//! the runner executes the lines it has not seen yet.
//!
//! Only complete lines are handed over. A line whose newline has not arrived
//! yet waits for the next poll, so a write that arrives in two pieces never
//! runs half a command. A `#` line is a comment the channel skips, so a suite
//! can annotate its command file. The runner executes the commands itself,
//! quit and dock rearrangement included, so this module does not need any
//! window or engine type.

// Standard library
use std::path::PathBuf;

// =============================================================================
// TestChannel
// =============================================================================

/// Reads the command lines a test suite appends to a file.
pub struct TestChannel {
    /// Path of the file the test suites append to.
    file_path: PathBuf,
    /// Number of bytes of the file whose lines have already been handed over.
    consumed_byte_count: usize,
}

impl TestChannel {
    /// Opens the channel when `PILL_EDITOR_TEST_COMMANDS` is set, and returns
    /// `None` when it is not.
    pub fn from_environment() -> Option<TestChannel> {
        let commands_file_path = std::env::var_os("PILL_EDITOR_TEST_COMMANDS")?;
        Some(TestChannel {
            file_path: PathBuf::from(commands_file_path),
            consumed_byte_count: 0,
        })
    }

    /// Returns the command lines appended since the last poll.
    ///
    /// Only complete lines come back. A trailing piece without its newline
    /// waits until the writer finishes it, so a command written in two pieces
    /// never runs its first piece alone. A file that got shorter (the suite
    /// rewrote it) is read again from the first byte, so nothing in the new
    /// contents is skipped. Blank lines and `#` comments are not commands and
    /// never reach the runner.
    pub fn poll(&mut self) -> Vec<String> {
        // Step 1: read the whole file. If the file cannot be read, there are
        // no new commands; the next poll tries again.
        let file_text = match std::fs::read_to_string(&self.file_path) {
            Ok(file_text) => file_text,
            Err(_) => return Vec::new(),
        };
        // Step 2: find where the last complete line ends. A file that got
        // shorter was rewritten, so reading starts again from the first byte.
        if file_text.len() < self.consumed_byte_count {
            self.consumed_byte_count = 0;
        }
        if file_text.len() == self.consumed_byte_count {
            return Vec::new();
        }
        // The last newline marks the end of the last complete line. Anything
        // after it is a write still in progress, so it waits for the next poll.
        let Some(last_newline_index) = file_text[self.consumed_byte_count..].rfind('\n') else {
            return Vec::new();
        };
        let next_read_offset = self.consumed_byte_count + last_newline_index + 1;
        let complete_lines_text = &file_text[self.consumed_byte_count..next_read_offset];
        self.consumed_byte_count = next_read_offset;
        // Step 3: hand over the complete lines that are not blank or comments.
        complete_lines_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(String::from)
            .collect()
    }
}

// =============================================================================
// Auto-quit
// =============================================================================

/// Returns how many seconds the editor runs before quitting gracefully, when
/// `PILL_EDITOR_TEST_SECONDS` holds a number.
pub fn seconds_from_environment() -> Option<f64> {
    std::env::var("PILL_EDITOR_TEST_SECONDS")
        .ok()
        .and_then(|seconds_text| seconds_text.trim().parse().ok())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Standard library
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    // Current crate
    use super::TestChannel;

    /// Makes each test's temporary file name unique, because the tests run in
    /// parallel in one process.
    static NEXT_FILE_INDEX: AtomicU64 = AtomicU64::new(0);

    fn unique_temporary_file_path(name_prefix: &str) -> PathBuf {
        let index = NEXT_FILE_INDEX.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pill_test_channel_{name_prefix}_{}_{index}.txt",
            std::process::id()
        ))
    }

    fn open_channel(path: &Path) -> TestChannel {
        TestChannel {
            file_path: path.to_path_buf(),
            consumed_byte_count: 0,
        }
    }

    fn append_to_file(path: &Path, text: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("the test channel file opens");
        file.write_all(text.as_bytes())
            .expect("the test channel write succeeds");
    }

    #[test]
    fn a_partial_line_waits_for_its_newline() {
        let path = unique_temporary_file_path("partial");
        let mut channel = open_channel(&path);
        append_to_file(&path, "clic");
        assert!(
            channel.poll().is_empty(),
            "a half-written line is not a verb"
        );
        append_to_file(&path, "k 10 20\n");
        assert_eq!(channel.poll(), vec!["click 10 20".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn comments_and_blank_lines_are_not_commands() {
        let path = unique_temporary_file_path("comments");
        let mut channel = open_channel(&path);
        append_to_file(&path, "# the scene step\n\n   \nclick 1 2\n");
        assert_eq!(channel.poll(), vec!["click 1 2".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn only_appended_lines_run() {
        let path = unique_temporary_file_path("appended");
        let mut channel = open_channel(&path);
        append_to_file(&path, "quit\n");
        assert_eq!(channel.poll(), vec!["quit".to_string()]);
        assert!(channel.poll().is_empty(), "a handled line runs once");
        append_to_file(&path, "tab scene\n");
        assert_eq!(channel.poll(), vec!["tab scene".to_string()]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_truncated_file_restarts_from_the_top() {
        let path = unique_temporary_file_path("truncated");
        let mut channel = open_channel(&path);
        append_to_file(&path, "click 1 2\nquit\n");
        assert_eq!(channel.poll().len(), 2);
        std::fs::write(&path, "tab scene\n").expect("the test channel file rewrites");
        assert_eq!(channel.poll(), vec!["tab scene".to_string()]);
        let _ = std::fs::remove_file(&path);
    }
}
