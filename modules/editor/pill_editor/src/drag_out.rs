//! Dragging a project asset out to the OS.
//!
//! # Responsibilities
//!
//! - [`start`]: hands one project asset to OLE as a file drag, so it can be
//!   dropped into Explorer, another editor, or any `CF_HDROP` target.
//! - [`EditorDropSource`]: the drop source that ends the drag when the mouse
//!   button comes up and cancels it on Escape.
//!
//! # Design
//!
//! The shell cannot start the drag itself. `DoDragDrop`, OLE's drag-and-drop
//! call, runs a modal loop on the thread that calls it, and only this
//! executable depends on the `windows` crate. Instead, the shell reports the
//! asset's path in the frame it hands back, and this module starts the drag
//! from the frame boundary.
//!
//! The path becomes the shell's item list first (a PIDL, a pointer to an item
//! identifier list). `SHCreateDataObject` then wraps that list in a data object
//! which offers the file under `CF_HDROP`, the format a file drop target reads.
//!
//! The drop source always ends the drag instead of keeping it running. It drops
//! the file as soon as the left button is not held, and cancels on Escape.
//! Answering "continue" on a button it could not read would spin the modal loop
//! while the editor's frame stays frozen.
//!
//! The same rule applies before the loop. `DoDragDrop` only asks its drop
//! source whether to continue when input arrives, so a drag started with no
//! button held would leave the editor frozen until more input arrived.
//! [`start`] reads the button first and starts no drag. A drag no pointer
//! started - a scripted one - never runs the loop at all, for the same reason.
//!
//! # Platforms
//!
//! An OLE drag source is what makes a file draggable out of the editor, and it
//! exists on Windows alone. Everywhere else [`start`] answers
//! [`DragOutOutcome::Unsupported`] without starting anything, and the runner
//! logs that once per request; nothing else in the editor is affected. winit
//! offers no drag source on Unix, so a real implementation would have to speak
//! XDND or the Wayland data-device protocol itself, which is why the platform
//! split is here and not in a dependency.

// External crates
#[cfg(windows)]
use pill_core::telemetry::telemetry_target;
#[cfg(windows)]
use windows::core::implement;
#[cfg(windows)]
use windows::Win32::Foundation::{BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, S_OK};
#[cfg(windows)]
use windows::Win32::System::Com::CoTaskMemFree;
#[cfg(windows)]
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT, DROPEFFECT_COPY,
};
#[cfg(windows)]
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
#[cfg(windows)]
use windows::Win32::UI::Shell::{SHCreateDataObject, SHParseDisplayName};

// =============================================================================
// EditorDropSource
// =============================================================================

/// The drop source OLE calls while the drag runs.
#[cfg(windows)]
#[implement(IDropSource)]
pub struct EditorDropSource;

#[cfg(windows)]
impl IDropSource_Impl for EditorDropSource_Impl {
    /// Reports whether the drag goes on, ends in a drop, or is cancelled.
    ///
    /// `fescapepressed` is OLE's own reading of the keyboard, so Escape is
    /// handled from it. The left mouse button is not part of the key state OLE
    /// passes in, so this method reads it itself. A button that cannot be read
    /// counts as released: the drag ends instead of spinning the loop.
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        _grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> windows::core::HRESULT {
        if fescapepressed.as_bool() {
            return DRAGDROP_S_CANCEL;
        }
        // SAFETY: `GetAsyncKeyState` takes a virtual-key code and cannot fail.
        // The high bit of the result is set while the key is held.
        let held = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
        if held {
            S_OK
        } else {
            DRAGDROP_S_DROP
        }
    }

    /// Keeps OLE's own pointer feedback, which follows the drop target's effect.
    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> windows::core::HRESULT {
        S_OK
    }
}

// =============================================================================
// DragOutOutcome
// =============================================================================

/// How a drag out ended, for the caller's log line.
///
/// Every outcome but `Unsupported` is produced by the Windows drag source, so
/// they are unconstructed where that source does not exist. The runner still
/// matches on all of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum DragOutOutcome {
    /// The drop target took the file under this effect.
    Dropped(DropEffect),
    /// The drag ended with no target taking the file.
    Cancelled,
    /// The pointer button was not held, so no drag was started.
    NoButtonHeld,
    /// The platform has no source for a file drag, so nothing was started.
    ///
    /// Only the non-Windows [`start`] answers this, so it is unconstructed on
    /// Windows while the runner still matches on it.
    #[cfg_attr(windows, allow(dead_code))]
    Unsupported,
    /// The drag could not be prepared, or the platform refused to run it.
    Failed,
}

/// The effect a drop target reported, without naming the platform's own type.
///
/// `Ole`'s `DROPEFFECT` exists only where the `windows` crate does, and the
/// outcome has to be nameable everywhere the runner matches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum DropEffect {
    /// The target copied the file.
    Copy,
    /// The target moved the file.
    Move,
    /// The target linked to the file.
    Link,
    /// Any other effect, kept as the platform's raw code.
    Other(u32),
}

impl DropEffect {
    /// The platform's raw effect code, which the runner logs.
    pub fn code(self) -> u32 {
        match self {
            Self::Copy => 1,
            Self::Move => 2,
            Self::Link => 4,
            Self::Other(code) => code,
        }
    }

    /// The effect one raw platform code means.
    ///
    /// The codes are the documented `DROPEFFECT` values, which are the same
    /// numbers the log line has always carried.
    #[cfg(windows)]
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Copy,
            2 => Self::Move,
            4 => Self::Link,
            other => Self::Other(other),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Whether the left button is held right now.
#[cfg(windows)]
fn left_button_held() -> bool {
    // SAFETY: `GetAsyncKeyState` takes a virtual-key code, reads no pointer and
    // cannot fail. The high bit of its result is set while the key is held.
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16) & 0x8000 != 0
}

/// Hands `absolute_path` to OLE as a file drag and blocks until the drag ends.
///
/// Logs `[editor] drag out prepared` once the data object exists, and a warning
/// naming the reason when a step fails. The runner logs one line for each of
/// the other outcomes, so every way a drag can end leaves a trace.
///
/// `driven_by_pointer` says whether a person's pointer started the drag. Only a
/// pointer drag runs OLE's loop; one asked for by anything else - the scripted
/// channel, whose input never reaches the pointer - stops once the data object
/// exists, because the loop would freeze the editor until more input arrived.
/// A pointer drag started with no button held is refused for the same reason.
#[cfg(windows)]
pub fn start(absolute_path: &str, driven_by_pointer: bool) -> DragOutOutcome {
    // Step 1: make sure this thread has OLE, which `DoDragDrop` needs.
    if !initialize_ole() {
        return DragOutOutcome::Failed;
    }
    let wide_path: Vec<u16> = absolute_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut pidl = std::ptr::null_mut();
    // Step 2: turn the path into the shell's item list.
    // SAFETY: `wide_path` is a NUL-terminated UTF-16 buffer that stays alive
    // for the whole call, and `pidl` is a valid out-pointer for the item list.
    if let Err(error) = unsafe {
        SHParseDisplayName(
            windows::core::PCWSTR(wide_path.as_ptr()),
            None,
            &mut pidl,
            0,
            None,
        )
    } {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            path = absolute_path,
            reason = %error,
            "[editor] drag out target not resolvable"
        );
        return DragOutOutcome::Failed;
    }
    // Step 3: wrap the item list in the shell's data object, which offers the
    // file to every target that takes files under `CF_HDROP`.
    let data_object: windows::core::Result<windows::Win32::System::Com::IDataObject> =
        // SAFETY: `pidl` is the item list just parsed, and both it and the
        // one-element array passed here stay alive for the whole call.
        unsafe { SHCreateDataObject(None, Some(&[pidl as *const _]), None) };
    // Step 4: run the drag while the button is held, then free the item list.
    let outcome = match data_object {
        Ok(data_object) => {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                path = absolute_path,
                "[editor] drag out prepared"
            );
            // `DoDragDrop` only asks the drop source whether to continue when
            // input arrives, so a drag started without the button held would
            // freeze the editor until more input arrived.
            if driven_by_pointer && left_button_held() {
                run_modal_drag(data_object)
            } else {
                DragOutOutcome::NoButtonHeld
            }
        }
        Err(error) => {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                path = absolute_path,
                reason = %error,
                "[editor] drag out data object refused"
            );
            DragOutOutcome::Failed
        }
    };
    // SAFETY: `pidl` came from `SHParseDisplayName`, which allocates it with
    // the COM task allocator. It is freed exactly once, here.
    unsafe { CoTaskMemFree(Some(pidl as *const core::ffi::c_void)) };
    outcome
}

/// Hands `absolute_path` to the platform as a file drag.
///
/// No platform but Windows gives the editor a drag source it can drive (see
/// the module docs), so this starts nothing and answers
/// [`DragOutOutcome::Unsupported`]. The runner logs one line for it and the
/// asset stays where it is; every other shell feature is unaffected.
#[cfg(not(windows))]
pub fn start(_absolute_path: &str, _driven_by_pointer: bool) -> DragOutOutcome {
    DragOutOutcome::Unsupported
}

/// Runs the modal drag over one prepared data object.
///
/// Blocks in OLE's loop until the drop source ends the drag. The effect OLE
#[cfg(windows)]
/// reports is the drop target's answer, and zero means no target took the file.
fn run_modal_drag(data_object: windows::Win32::System::Com::IDataObject) -> DragOutOutcome {
    let drop_source: IDropSource = EditorDropSource.into();
    let mut effect = DROPEFFECT(0);
    // SAFETY: `data_object` and `drop_source` are live for the whole call, and
    // `effect` points at a valid out-parameter slot.
    let drag_result = unsafe {
        DoDragDrop(
            &data_object,
            &drop_source,
            DROPEFFECT_COPY,
            &mut effect as *mut DROPEFFECT,
        )
    };
    if drag_result.is_err() {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            reason = %windows::core::Error::from_hresult(drag_result),
            "[editor] drag out refused by OLE"
        );
        DragOutOutcome::Failed
    } else if effect.0 == 0 {
        // A zero effect means the drag was cancelled. The target refused the
        // file, or the pointer came up over nothing that takes files.
        DragOutOutcome::Cancelled
    } else {
        DragOutOutcome::Dropped(DropEffect::from_code(effect.0))
    }
}

/// Initialises OLE on the calling thread, once.
///
/// `DoDragDrop` needs an OLE-initialised thread. The editor's main thread owns
/// the window, so that is where this runs. A failure leaves drag-out
/// unavailable and is logged once.
#[cfg(windows)]
fn initialize_ole() -> bool {
    use std::sync::OnceLock;
    static INITIALISED: OnceLock<bool> = OnceLock::new();
    *INITIALISED.get_or_init(|| {
        // SAFETY: `OleInitialize` takes a reserved pointer that must be null.
        match unsafe { OleInitialize(None) } {
            Ok(()) => true,
            Err(error) => {
                // An already-initialised thread reports `S_FALSE`, which the
                // `windows` crate turns into `Ok`. Only a real failure reaches
                // this arm.
                pill_core::warn!(
                    target: telemetry_target::EDITOR,
                    reason = %error,
                    "[editor] drag out needs OLE, which this thread refused"
                );
                false
            }
        }
    })
}
