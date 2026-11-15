//! Writes the editor's payload to the OS clipboard and reads text back from it.
//!
//! # Responsibilities
//!
//! - [`write`]: put text on the clipboard twice, once in the editor's own
//!   clipboard format and once as plain text, so another editor instance or a
//!   text editor can read it.
//! - [`read`]: return the text the clipboard carries, preferring the editor's
//!   own format and falling back to plain text.
//!
//! # Design
//!
//! The shell does not touch the OS clipboard. It queues a `ClipboardRequest` on
//! its `EditorClipboard` resource, and the runner carries the request out here
//! at the frame boundary. That way no OS clipboard handle crosses the reload
//! boundary.
//!
//! Each platform carries its own backend below.
//!
//! On Windows the Win32 clipboard is driven directly, and that code lives here
//! because only this executable names the `windows` crate: a shared dylib must
//! never carry its large `raw-dylib` import set. The write path and the read
//! path there both close the clipboard on every path, because a clipboard left
//! open blocks every other process. A read is bounded by the size the clipboard
//! reports for the block (`GlobalSize`), never by the terminator the producer
//! was supposed to write: any process can set the registered format, so a block
//! with no NUL inside its own size is refused and logged instead of being read
//! past its end into this process's heap.
//!
//! Everywhere else the session's own clipboard tool does the work -
//! `wl-copy`/`wl-paste` on Wayland, `xclip` on X11 - which keeps a second
//! clipboard implementation and the display-server libraries one needs out of
//! this crate. The editor's payload *is* the text, so writing it as plain text
//! keeps a paste from another editor decodable, exactly as the Windows path
//! does by writing both formats.

// External crates
#[cfg(windows)]
use pill_core::telemetry::telemetry_target;
#[cfg(windows)]
use pill_core::warn;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
#[cfg(windows)]
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
#[cfg(windows)]
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
#[cfg(windows)]
use windows::Win32::System::Threading::GetCurrentProcessId;

// Current crate
#[cfg(windows)]
use pill_editor_core::clipboard::FORMAT_NAME;

// =============================================================================
// Constants
// =============================================================================

/// `CF_UNICODETEXT`: the plain-text clipboard format that any application can
/// read.
#[cfg(windows)]
const CF_UNICODETEXT: u32 = 13;

/// How many attempts a read makes to open a busy clipboard before it reports
/// failure.
#[cfg(windows)]
const READ_ATTEMPTS: usize = 10;

/// The pause between clipboard read attempts.
#[cfg(windows)]
const READ_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(25);

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the clipboard format id the editor registers its copied text under.
///
/// Registering the name is idempotent, so every call returns the same id for
/// the life of the process and nothing needs caching. `None` means Windows
/// refused the name.
#[cfg(windows)]
fn payload_format_id() -> Option<u32> {
    let name: Vec<u16> = FORMAT_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `name` is a NUL-terminated UTF-16 buffer that outlives the call,
    // which is what `RegisterClipboardFormatW` requires. A zero return means
    // Windows refused the name, reported here as `None`.
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(name.as_ptr())) };
    (id != 0).then_some(id)
}

/// Puts `text` on the clipboard, in the editor's own format and as plain text.
///
/// Once the clipboard is open it is emptied before the first allocation, so a
/// later failure leaves it empty rather than holding what it held before.
///
/// # Errors
///
/// Returns a message naming the step that failed: registering the editor's
/// format, opening the clipboard, emptying it, allocating the clipboard block,
/// or handing that block to the clipboard.
#[cfg(windows)]
pub fn write(text: &str) -> Result<(), String> {
    // Step 1: find the editor's own clipboard format and open the clipboard
    // for this process.
    let format = payload_format_id().ok_or_else(|| "the payload format was refused".to_string())?;
    let owner_window = HWND::default();
    // SAFETY: `owner_window` is a null handle, which the API documents as
    // associating the clipboard with the calling process rather than a window.
    unsafe { OpenClipboard(owner_window) }.map_err(|error| format!("open clipboard: {error}"))?;
    // Step 2: write both copies while it is open, then close it whatever the
    // write did.
    let write_open_result = write_open(text, format);
    // SAFETY: the clipboard is open exactly once here, after the write path
    // above opened it; closing it is unconditional so a failure above cannot
    // leave it locked.
    let _ = unsafe { CloseClipboard() };
    write_open_result
}

/// The write path with the clipboard already open.
///
/// # Errors
///
/// Returns a message naming the failed step: emptying the clipboard, or one of
#[cfg(windows)]
/// the two copies [`set_one`] makes.
fn write_open(text: &str, format: u32) -> Result<(), String> {
    // SAFETY: the clipboard is open (the caller holds it), which is the
    // documented precondition for emptying it.
    unsafe { EmptyClipboard() }.map_err(|error| format!("empty clipboard: {error}"))?;
    // The registered format goes first, so a reader that understands it gets
    // the editor's payload. A reader that does not falls back to the plain text
    // written below.
    set_one(text, format)?;
    set_one(text, CF_UNICODETEXT)?;
    Ok(())
}

/// Puts one UTF-16 copy of `text` on the clipboard under `format`.
///
/// The memory is allocated with `GMEM_MOVEABLE` and handed to the clipboard,
/// which takes ownership on success. A failed `SetClipboardData` leaves the
/// block with this function, which frees it with `GlobalFree`.
///
/// # Errors
///
/// Returns a message when the block cannot be allocated, when it cannot be
#[cfg(windows)]
/// locked, or when the clipboard refuses it.
fn set_one(text: &str, format: u32) -> Result<(), String> {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    let bytes = units.len() * std::mem::size_of::<u16>();
    // SAFETY: `GMEM_MOVEABLE` with a byte count is the documented allocation
    // for clipboard data; a null return means the allocation failed.
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }
        .map_err(|error| format!("clipboard allocation: {error}"))?;
    // SAFETY: `handle` is the allocation just made, and the lock is taken once
    // so the buffer can be filled before the data is handed over.
    let target = unsafe { GlobalLock(HGLOBAL(handle.0)) };
    if target.is_null() {
        // SAFETY: `handle` is an allocation this call owns and no other
        // reference to it exists.
        let _ = unsafe { GlobalFree(handle) };
        return Err("clipboard allocation could not be locked".to_string());
    }
    // SAFETY: the destination is the locked allocation, `bytes` long, and the
    // source is `units`, a live buffer of exactly that size.
    unsafe { std::ptr::copy_nonoverlapping(units.as_ptr(), target.cast::<u16>(), units.len()) };
    // SAFETY: the lock was taken above on this handle and is released once.
    let _ = unsafe { GlobalUnlock(HGLOBAL(handle.0)) };
    // SAFETY: the clipboard is open (the caller holds it); on success it takes
    // ownership of `handle`, so this call must not free it afterwards.
    match unsafe { SetClipboardData(format, windows::Win32::Foundation::HANDLE(handle.0)) } {
        Ok(_) => Ok(()),
        Err(error) => {
            // SAFETY: `handle` is still this call's allocation because the
            // clipboard refused it.
            let _ = unsafe { GlobalFree(handle) };
            Err(format!("clipboard data: {error}"))
        }
    }
}

/// Returns the text the clipboard carries, preferring the editor's own format.
///
/// `Ok(None)` means the clipboard held nothing usable; the runner then shows an
/// empty-clipboard notice instead of pasting.
///
/// The clipboard is a lock shared by every process, so `OpenClipboard` can
/// fail simply because another application holds it at that moment. The
/// attempt is retried up to `READ_ATTEMPTS` times, `READ_RETRY_DELAY` apart,
/// and a clipboard that never frees is an error rather than an empty paste.
///
/// # Errors
///
#[cfg(windows)]
/// Returns the message from the last failed attempt when the clipboard cannot
/// be opened within the retries.
pub fn read() -> Result<Option<String>, String> {
    // Step 1: try to open the clipboard, pausing a little between attempts.
    let owner_window = HWND::default();
    let mut last_error = String::new();
    for attempt in 0..READ_ATTEMPTS {
        // SAFETY: `owner_window` is a null handle, the documented form for
        // reading the clipboard from a process without a window that owns the
        // data.
        match unsafe { OpenClipboard(owner_window) } {
            Ok(()) => {
                // Step 2: read while it is open, then close it on every path.
                let read_open_result = read_open();
                // SAFETY: the clipboard is open exactly once here and is closed
                // unconditionally, so a read failure cannot leave it locked.
                let _ = unsafe { CloseClipboard() };
                return read_open_result;
            }
            Err(error) => {
                last_error = format!("open clipboard: {error}");
                if attempt + 1 < READ_ATTEMPTS {
                    std::thread::sleep(READ_RETRY_DELAY);
                }
            }
        }
    }
    Err(last_error)
}

/// The read path with the clipboard already open.
///
/// The formats are tried in order: the editor's own format first, then plain
/// text. A format that is unavailable, a block that cannot be locked and a
/// block with no terminator all move on to the next format. The result is the
/// first text that could be read, or `None`.
///
/// # Errors
///
#[cfg(windows)]
/// No branch returns an error: the signature carries the type [`read`] passes
/// on, but every failure here ends in `Ok`.
fn read_open() -> Result<Option<String>, String> {
    let mut format_ids = Vec::new();
    if let Some(format) = payload_format_id() {
        format_ids.push(format);
    }
    format_ids.push(CF_UNICODETEXT);
    for format in format_ids {
        // SAFETY: the clipboard is open (the caller holds it); the query only
        // reads the format list.
        if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
            continue;
        }
        // SAFETY: the format was just reported available while the clipboard
        // is open, which is the documented precondition for fetching it.
        let handle = match unsafe { GetClipboardData(format) } {
            Ok(handle) => handle,
            Err(_) => continue,
        };
        if handle.is_invalid() {
            continue;
        }
        // SAFETY: `handle` is the clipboard's data block for an available
        // format; it stays valid until the clipboard changes, and it is not
        // freed here - the clipboard owns it.
        let block_pointer = unsafe { GlobalLock(HGLOBAL(handle.0)) }.cast::<u16>();
        if block_pointer.is_null() {
            continue;
        }
        // Read only as far as the block's own size allows. The terminator is
        // something the producer is supposed to write, not a property the block
        // guarantees. Any process can set the registered format, so a block with
        // no NUL inside its size must not be followed out of the allocation.
        // SAFETY: `handle` is the clipboard's data block; the query only reads
        // the allocation's size, and neither frees nor unlocks it.
        let bytes = unsafe { GlobalSize(HGLOBAL(handle.0)) };
        let unit_capacity = bytes / std::mem::size_of::<u16>();
        let mut unit_count = 0usize;
        // SAFETY: `block_pointer` addresses the locked block of `unit_capacity`
        // UTF-16 units, so every index below `unit_capacity` is inside the
        // allocation.
        while unit_count < unit_capacity && unsafe { *block_pointer.add(unit_count) } != 0 {
            unit_count += 1;
        }
        let unterminated = unit_count == unit_capacity;
        // SAFETY: `block_pointer` is the locked block and `unit_count` counts
        // the UTF-16 units before its terminator, so the slice stays inside the
        // block.
        let slice = unsafe { std::slice::from_raw_parts(block_pointer, unit_count) };
        let text = String::from_utf16_lossy(slice);
        // SAFETY: the lock was taken above on this handle and is released once.
        let _ = unsafe { GlobalUnlock(HGLOBAL(handle.0)) };
        if unterminated {
            warn!(
                target: telemetry_target::EDITOR,
                bytes = bytes,
                "[editor] clipboard refused: the block carries no terminator"
            );
            continue;
        }
        if !text.is_empty() {
            return Ok(Some(text));
        }
    }
    Ok(None)
}

/// Returns the id of this process, which the clipboard log lines carry.
///
#[cfg(windows)]
/// The write and paste lines name it as `writer =` and `reader =`, so a test
/// that runs more than one editor can tell which process did each step.
pub fn process_id() -> u32 {
    // SAFETY: `GetCurrentProcessId` takes no arguments and cannot fail.
    unsafe { GetCurrentProcessId() }
}

// =============================================================================
// Unix Backend
// =============================================================================

/// Which direction a clipboard command runs in.
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    /// Copy the text from this process to the clipboard.
    Write,
    /// Copy the clipboard's text into this process.
    Read,
}

/// Which command-line clipboard this session offers.
///
/// A Wayland session gets `wl-copy`/`wl-paste`; an X11 session - or a Wayland
/// session where the tool is missing - gets `xclip`. `None` means neither is
/// installed, which every operation reports rather than silently doing nothing.
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClipboardBackend {
    /// `wl-copy` and `wl-paste` from `wl-clipboard`.
    Wayland,
    /// `xclip`, which speaks to any X server, XWayland included.
    X11,
}

#[cfg(not(windows))]
impl ClipboardBackend {
    /// The program and the arguments it needs for one direction.
    fn command(self, direction: Direction) -> (&'static str, &'static [&'static str]) {
        match (self, direction) {
            (Self::Wayland, Direction::Write) => ("wl-copy", &[]),
            // Without `--no-newline` a copied payload comes back with a
            // newline this process never wrote, and the decode fails.
            (Self::Wayland, Direction::Read) => ("wl-paste", &["--no-newline"]),
            (Self::X11, Direction::Write) => ("xclip", &["-selection", "clipboard", "-i"]),
            (Self::X11, Direction::Read) => ("xclip", &["-selection", "clipboard", "-o"]),
        }
    }
}

/// Chooses a backend from what the session declares and what is installed.
///
/// Kept apart from the environment lookup so the choice can be tested without
/// a display server. Wayland wins only when the session says it is one *and*
/// both of its commands are installed; otherwise the X11 tool is the answer,
/// because it also works under XWayland.
#[cfg(not(windows))]
fn choose_backend(
    wayland_session: bool,
    have_wayland_tools: bool,
    have_xclip: bool,
) -> Option<ClipboardBackend> {
    if wayland_session && have_wayland_tools {
        Some(ClipboardBackend::Wayland)
    } else if have_xclip {
        Some(ClipboardBackend::X11)
    } else {
        None
    }
}

/// The backend this process uses, resolved once per process.
#[cfg(not(windows))]
fn backend() -> Option<ClipboardBackend> {
    static BACKEND: std::sync::OnceLock<Option<ClipboardBackend>> = std::sync::OnceLock::new();
    *BACKEND.get_or_init(|| {
        choose_backend(
            std::env::var_os("WAYLAND_DISPLAY").is_some(),
            executable_on_path("wl-copy") && executable_on_path("wl-paste"),
            executable_on_path("xclip"),
        )
    })
}

/// Whether `name` names an executable file on `PATH`.
#[cfg(not(windows))]
fn executable_on_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| {
            std::fs::metadata(directory.join(name)).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
    })
}

/// Puts `text` on the session's clipboard as plain text.
///
/// The text carries the editor's payload, so a paste from another editor
/// decodes and a paste from a text editor reads the payload text - which is
/// what the Windows path produces too, where the same text goes to both the
/// editor's format and the plain-text one.
///
/// # Errors
///
/// Returns a message naming the tool when no clipboard tool is installed, when
/// it cannot be started, or when it reports a failure.
#[cfg(not(windows))]
pub fn write(text: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let backend = backend().ok_or_else(no_clipboard_tool_message)?;
    let (program, arguments) = backend.command(Direction::Write);
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        // Neither stream is inherited: `wl-copy` forks the process that serves
        // the selection, and a pipe it inherited would keep this call waiting
        // for the whole selection instead of for the write.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program}: {error}"))?;
    // The tool finishes reading when the pipe closes, so the handle is dropped
    // as soon as the text is in it.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("{program}: {error}"))?;
    }
    let status = child
        .wait()
        .map_err(|error| format!("{program}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

/// Returns the text the session's clipboard carries.
///
/// `Ok(None)` means nothing usable was there: the selection is empty, or its
/// owner offers no text. Both tools report that as a failed exit, which is why
/// a failed exit reads as "nothing" instead of as an error - the runner shows
/// its empty-clipboard notice for it.
///
/// # Errors
///
/// Returns a message naming the tool only when it cannot be started at all.
#[cfg(not(windows))]
pub fn read() -> Result<Option<String>, String> {
    use std::process::{Command, Stdio};

    let Some(backend) = backend() else {
        return Ok(None);
    };
    let (program, arguments) = backend.command(Direction::Read);
    let output = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok((!text.is_empty()).then_some(text))
}

/// Returns the id of this process, which the clipboard log lines carry.
///
/// The write and paste lines name it as `writer =` and `reader =`, so a test
/// that runs more than one editor can tell which process did each step.
#[cfg(not(windows))]
pub fn process_id() -> u32 {
    std::process::id()
}

/// The refusal both directions report when neither clipboard tool is
/// installed. Named once so the write and the read paths cannot drift apart.
#[cfg(not(windows))]
fn no_clipboard_tool_message() -> String {
    "no clipboard tool installed: wl-clipboard (`wl-copy`/`wl-paste`) for Wayland, `xclip` for X11"
        .to_string()
}

// =============================================================================
// Unix Tests
// =============================================================================

#[cfg(all(test, not(windows)))]
mod unix_tests {
    use super::{choose_backend, ClipboardBackend};

    /// The choice follows the session first and the installed tools second.
    ///
    /// A Wayland session without `wl-clipboard` still gets a clipboard through
    /// `xclip` under XWayland; a session with no tool at all reports `None` so
    /// the paste path can say why it did nothing.
    #[test]
    fn the_backend_follows_the_session_and_the_installed_tools() {
        assert_eq!(
            choose_backend(true, true, false),
            Some(ClipboardBackend::Wayland),
            "a Wayland session with wl-clipboard uses it"
        );
        assert_eq!(
            choose_backend(true, false, true),
            Some(ClipboardBackend::X11),
            "XWayland's xclip is the fallback in a Wayland session"
        );
        assert_eq!(
            choose_backend(false, true, true),
            Some(ClipboardBackend::X11),
            "an X11 session never uses the Wayland tools"
        );
        assert_eq!(
            choose_backend(true, false, false),
            None,
            "no installed tool leaves no backend"
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The clipboard is one resource for the whole process, so the tests below
    /// take turns. A `CloseClipboard` on one thread ends an `OpenClipboard` that
    /// another thread made.
    static CLIPBOARD_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Waits until no other test is using the clipboard, and keeps it until the
    /// returned guard drops.
    fn clipboard_turn() -> std::sync::MutexGuard<'static, ()> {
        CLIPBOARD_TURN
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Puts `units` UTF-16 `A`s on the clipboard under the editor's format,
    /// with no terminator - the block shape a foreign application can hand
    /// over.
    fn set_unterminated_block(units: usize) {
        let format = payload_format_id().expect("the payload format registers");
        // SAFETY: `HWND::default()` is the null owner, the documented form for
        // a process without a clipboard-owning window.
        unsafe { OpenClipboard(HWND::default()) }.expect("the clipboard opens");
        // SAFETY: the clipboard is open and this call owns it.
        unsafe { EmptyClipboard() }.expect("the clipboard empties");
        let bytes = units * std::mem::size_of::<u16>();
        // SAFETY: `GMEM_MOVEABLE` with a byte count is the documented
        // allocation for clipboard data; a null return fails the test.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes) }.expect("the block allocates");
        // SAFETY: `handle` is the allocation just made, locked once so it can
        // be filled.
        let target = unsafe { GlobalLock(HGLOBAL(handle.0)) }.cast::<u16>();
        assert!(!target.is_null(), "the block locks");
        for unit in 0..units {
            // SAFETY: `unit` is inside the block just allocated and locked.
            unsafe { *target.add(unit) = u16::from(b'A') };
        }
        // SAFETY: the lock was taken above on this handle and is released once.
        let _ = unsafe { GlobalUnlock(HGLOBAL(handle.0)) };
        // SAFETY: the clipboard is open and takes ownership of the block.
        unsafe { SetClipboardData(format, windows::Win32::Foundation::HANDLE(handle.0)) }
            .expect("the block is accepted");
        // SAFETY: the clipboard is open exactly once here.
        let _ = unsafe { CloseClipboard() };
    }

    #[test]
    fn an_unterminated_block_is_refused_rather_than_walked_past() {
        let _turn = clipboard_turn();
        const UNITS: usize = 4096;
        set_unterminated_block(UNITS);
        match read() {
            Ok(None) => {}
            Ok(Some(text)) => panic!(
                "a block of {UNITS} units with no terminator was read as {} units",
                text.chars().count()
            ),
            Err(message) => panic!("the read reported '{message}' instead of refusing the block"),
        }
    }

    #[test]
    fn a_terminated_block_is_read_back_whole() {
        let _turn = clipboard_turn();
        write("a payload the editor wrote itself").expect("the clipboard write succeeds");
        match read() {
            Ok(Some(text)) => assert_eq!(text, "a payload the editor wrote itself"),
            Ok(None) => panic!("the block just written was not found"),
            Err(message) => panic!("the read reported '{message}'"),
        }
    }
}
