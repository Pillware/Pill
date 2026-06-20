
//! Native child-window management for the embedded wgpu viewport (Windows).
//!
//! wgpu cannot render into the WebView DOM, so we create a borderless Win32
//! child window that is a sibling of the WebView2 host inside the same top-level
//! Tauri window. React reports the on-screen rectangle of its `#viewport-slot`
//! placeholder, and we move/resize this child window to match exactly. wgpu owns
//! a surface backed by this child window's `HWND`.

#![cfg(target_os = "windows")]

use std::ffi::c_void;
use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowLongPtrW, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOP,
    LWA_ALPHA, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WINDOW_EX_STYLE, WM_ERASEBKGND, WNDCLASSW,
    WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_VISIBLE,
};

const CLASS_NAME: PCWSTR = w!("PillViewportWindowClass");

static REGISTER: Once = Once::new();

/// A native child window handle pair, stored as integers so it is `Send`.
#[derive(Clone, Copy)]
pub struct ChildWindow {
    pub hwnd: isize,
    pub hinstance: isize,
}

/// Minimal window procedure. We suppress background erasing to avoid white/black
/// flashes during resize because wgpu fully repaints the surface every frame.
unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register_class(hinstance: HINSTANCE) {
    REGISTER.call_once(|| unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            lpszClassName: CLASS_NAME,
            // Opaque black background until the first frame is presented.
            hbrBackground: HBRUSH(CreateSolidBrush(COLORREF(0x0000_0000)).0),
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

/// Create the borderless child window parented to the main Tauri window.
///
/// `parent_hwnd` is the top-level window `HWND` (as `isize`) obtained from
/// `WebviewWindow::hwnd()`.
pub fn create_child_window(parent_hwnd: isize) -> Result<ChildWindow, String> {
    unsafe {
        let hmodule = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let hinstance = HINSTANCE(hmodule.0);
        register_class(hinstance);

        let parent = HWND(parent_hwnd as *mut c_void);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!(""),
            // Child, clipped against its siblings (the WebView2 host) so it is
            // never overpainted. Starts at 1x1; React supplies the real rect.
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            Some(parent),
            None,
            Some(hinstance),
            None,
        )
        .map_err(|e| e.to_string())?;

        Ok(ChildWindow {
            hwnd: hwnd.0 as isize,
            hinstance: hinstance.0 as isize,
        })
    }
}

/// Move and resize the child window to the given physical-pixel rectangle,
/// relative to the parent window's client area, keeping it at the top of the
/// sibling z-order so it stays above the WebView.
pub fn set_window_rect(child_hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let hwnd = HWND(child_hwnd as *mut c_void);
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOP),
            x,
            y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the child window (e.g. when a panel holding the viewport is
/// collapsed). Uses `SW_SHOWNA` so it never steals focus from the WebView.
pub fn set_window_visible(child_hwnd: isize, visible: bool) {
    unsafe {
        let hwnd = HWND(child_hwnd as *mut c_void);
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// Set the per-pixel alpha of the child window (`0` = fully transparent,
/// `255` = fully opaque). Used to "ghost" the viewport while its tab is being
/// dragged. The `WS_EX_LAYERED` extended style is toggled on demand: it is
/// removed at full opacity so the common case keeps the GPU fast path.
pub fn set_window_opacity(child_hwnd: isize, alpha: u8) {
    unsafe {
        let hwnd = HWND(child_hwnd as *mut c_void);
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let layered = WS_EX_LAYERED.0 as isize;

        if alpha >= 255 {
            // Drop the layered style so the window composites normally again.
            if ex_style & layered != 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style & !layered);
            }
        } else {
            if ex_style & layered == 0 {
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | layered);
            }
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA);
        }
    }
}

/// Destroy the child window on shutdown.
#[allow(dead_code)]
pub fn destroy_window(child_hwnd: isize) {
    unsafe {
        let hwnd = HWND(child_hwnd as *mut c_void);
        let _ = DestroyWindow(hwnd);
    }
}
/// Make `editor_hwnd` an *owned* window of `owner_hwnd` so it is always
/// composited above the owner and all of the owner's child windows (including
/// the native viewport). Also marks it no-activate / tool-window so it never
/// steals focus from the editor and stays off the taskbar/alt-tab list.
pub fn configure_editor(editor_hwnd: isize, owner_hwnd: isize) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);

        // Establish ownership: the editor now floats above the owner's z-band.
        SetWindowLongPtrW(editor, GWLP_HWNDPARENT, owner_hwnd);

        // Add no-activate + tool-window extended styles (preserving whatever
        // Tauri already set, e.g. transparency/layered).
        let ex = GetWindowLongPtrW(editor, GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if ex & extra != extra {
            SetWindowLongPtrW(editor, GWL_EXSTYLE, ex | extra);
        }
    }
}

/// Position and size the editor window over the native viewport.
///
/// `x`, `y`, `width`, `height` are **physical pixels relative to the main
/// window's client area** (identical to what the viewport child window
/// receives). They are converted to screen coordinates here because the editor
/// is a top-level window. `SWP_NOACTIVATE` keeps editor focus intact.
pub fn set_editor_rect(
    editor_hwnd: isize,
    main_hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let main = HWND(main_hwnd as *mut c_void);

        // Client (content-area) origin -> screen origin.
        let mut origin = POINT { x, y };
        let _ = ClientToScreen(main, &mut origin);

        println!(
            "Setting editor rect: client=({}, {}, {}, {}), screen origin=({}, {})",
            x, y, width, height, origin.x, origin.y
        );

        let _ = SetWindowPos(
            editor,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the editor window without taking focus.
pub fn set_editor_visible(editor_hwnd: isize, visible: bool) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let _ = ShowWindow(editor, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
//! Native window management for the transparent **editor WebView** (Windows).
//!
//! The editor is a second, borderless, transparent top-level Tauri window. To
//! make it act as **Layer 3** — always composited above both the native wgpu
//! viewport child window (Layer 2) and the main editor WebView (Layer 1) — we
//! reparent it as an *owned* window of the main top-level window.
//!
//! ## Why an owned top-level window (and not a child or a topmost window)?
//!
//! * A WebView2-backed window does not survive being turned into a `WS_CHILD`
//!   of another window cleanly, so we must keep it top-level.
//! * Win32 guarantees that an **owned** window is always shown above its owner
//!   *and above every child window of that owner* — which is exactly the
//!   "editor above the viewport child HWND" ordering we need, with no per-frame
//!   z-order fighting against the viewport (whose own `SetWindowPos(HWND_TOP)`
//!   only reorders it among the main window's *children*, never above an owned
//!   window).
//! * Unlike `HWND_TOPMOST`, an owned window only floats above *its owner*, so it
//!   correctly drops behind other applications when the editor loses focus.
//!
//! Ownership is established by storing the owner's `HWND` in the editor's
//! `GWLP_HWNDPARENT` slot. The owner does not auto-move the editor, so the JS
//! side reports the viewport rectangle and Rust converts it from client to
//! screen coordinates (`ClientToScreen`) on every update and on owner move.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE,
    HWND_TOP, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Make `editor_hwnd` an *owned* window of `owner_hwnd` so it is always
/// composited above the owner and all of the owner's child windows (including
/// the native viewport). Also marks it no-activate / tool-window so it never
/// steals focus from the editor and stays off the taskbar/alt-tab list.
pub fn configure_editor(editor_hwnd: isize, owner_hwnd: isize) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);

        // Establish ownership: the editor now floats above the owner's z-band.
        SetWindowLongPtrW(editor, GWLP_HWNDPARENT, owner_hwnd);

        // Add no-activate + tool-window extended styles (preserving whatever
        // Tauri already set, e.g. transparency/layered).
        let ex = GetWindowLongPtrW(editor, GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if ex & extra != extra {
            SetWindowLongPtrW(editor, GWL_EXSTYLE, ex | extra);
        }
    }
}

/// Position and size the editor window over the native viewport.
///
/// `x`, `y`, `width`, `height` are **physical pixels relative to the main
/// window's client area** (identical to what the viewport child window
/// receives). They are converted to screen coordinates here because the editor
/// is a top-level window. `SWP_NOACTIVATE` keeps editor focus intact.
pub fn set_editor_rect(
    editor_hwnd: isize,
    main_hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let main = HWND(main_hwnd as *mut c_void);

        // Client (content-area) origin -> screen origin.
        let mut origin = POINT { x, y };
        let _ = ClientToScreen(main, &mut origin);

        println!(
            "Setting editor rect: client=({}, {}, {}, {}), screen origin=({}, {})",
            x, y, width, height, origin.x, origin.y
        );

        let _ = SetWindowPos(
            editor,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the editor window without taking focus.
pub fn set_editor_visible(editor_hwnd: isize, visible: bool) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let _ = ShowWindow(editor, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
//! Native window management for the transparent **editor WebView** (Windows).
//!
//! The editor is a second, borderless, transparent top-level Tauri window. To
//! make it act as **Layer 3** — always composited above both the native wgpu
//! viewport child window (Layer 2) and the main editor WebView (Layer 1) — we
//! reparent it as an *owned* window of the main top-level window.
//!
//! ## Why an owned top-level window (and not a child or a topmost window)?
//!
//! * A WebView2-backed window does not survive being turned into a `WS_CHILD`
//!   of another window cleanly, so we must keep it top-level.
//! * Win32 guarantees that an **owned** window is always shown above its owner
//!   *and above every child window of that owner* — which is exactly the
//!   "editor above the viewport child HWND" ordering we need, with no per-frame
//!   z-order fighting against the viewport (whose own `SetWindowPos(HWND_TOP)`
//!   only reorders it among the main window's *children*, never above an owned
//!   window).
//! * Unlike `HWND_TOPMOST`, an owned window only floats above *its owner*, so it
//!   correctly drops behind other applications when the editor loses focus.
//!
//! Ownership is established by storing the owner's `HWND` in the editor's
//! `GWLP_HWNDPARENT` slot. The owner does not auto-move the editor, so the JS
//! side reports the viewport rectangle and Rust converts it from client to
//! screen coordinates (`ClientToScreen`) on every update and on owner move.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE,
    HWND_TOP, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Make `editor_hwnd` an *owned* window of `owner_hwnd` so it is always
/// composited above the owner and all of the owner's child windows (including
/// the native viewport). Also marks it no-activate / tool-window so it never
/// steals focus from the editor and stays off the taskbar/alt-tab list.
pub fn configure_editor(editor_hwnd: isize, owner_hwnd: isize) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);

        // Establish ownership: the editor now floats above the owner's z-band.
        SetWindowLongPtrW(editor, GWLP_HWNDPARENT, owner_hwnd);

        // Add no-activate + tool-window extended styles (preserving whatever
        // Tauri already set, e.g. transparency/layered).
        let ex = GetWindowLongPtrW(editor, GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if ex & extra != extra {
            SetWindowLongPtrW(editor, GWL_EXSTYLE, ex | extra);
        }
    }
}

/// Position and size the editor window over the native viewport.
///
/// `x`, `y`, `width`, `height` are **physical pixels relative to the main
/// window's client area** (identical to what the viewport child window
/// receives). They are converted to screen coordinates here because the editor
/// is a top-level window. `SWP_NOACTIVATE` keeps editor focus intact.
pub fn set_editor_rect(
    editor_hwnd: isize,
    main_hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let main = HWND(main_hwnd as *mut c_void);

        // Client (content-area) origin -> screen origin.
        let mut origin = POINT { x, y };
        let _ = ClientToScreen(main, &mut origin);

        println!(
            "Setting editor rect: client=({}, {}, {}, {}), screen origin=({}, {})",
            x, y, width, height, origin.x, origin.y
        );

        let _ = SetWindowPos(
            editor,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the editor window without taking focus.
pub fn set_editor_visible(editor_hwnd: isize, visible: bool) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let _ = ShowWindow(editor, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
//! Native window management for the transparent **editor WebView** (Windows).
//!
//! The editor is a second, borderless, transparent top-level Tauri window. To
//! make it act as **Layer 3** — always composited above both the native wgpu
//! viewport child window (Layer 2) and the main editor WebView (Layer 1) — we
//! reparent it as an *owned* window of the main top-level window.
//!
//! ## Why an owned top-level window (and not a child or a topmost window)?
//!
//! * A WebView2-backed window does not survive being turned into a `WS_CHILD`
//!   of another window cleanly, so we must keep it top-level.
//! * Win32 guarantees that an **owned** window is always shown above its owner
//!   *and above every child window of that owner* — which is exactly the
//!   "editor above the viewport child HWND" ordering we need, with no per-frame
//!   z-order fighting against the viewport (whose own `SetWindowPos(HWND_TOP)`
//!   only reorders it among the main window's *children*, never above an owned
//!   window).
//! * Unlike `HWND_TOPMOST`, an owned window only floats above *its owner*, so it
//!   correctly drops behind other applications when the editor loses focus.
//!
//! Ownership is established by storing the owner's `HWND` in the editor's
//! `GWLP_HWNDPARENT` slot. The owner does not auto-move the editor, so the JS
//! side reports the viewport rectangle and Rust converts it from client to
//! screen coordinates (`ClientToScreen`) on every update and on owner move.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE,
    HWND_TOP, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Make `editor_hwnd` an *owned* window of `owner_hwnd` so it is always
/// composited above the owner and all of the owner's child windows (including
/// the native viewport). Also marks it no-activate / tool-window so it never
/// steals focus from the editor and stays off the taskbar/alt-tab list.
pub fn configure_editor(editor_hwnd: isize, owner_hwnd: isize) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);

        // Establish ownership: the editor now floats above the owner's z-band.
        SetWindowLongPtrW(editor, GWLP_HWNDPARENT, owner_hwnd);

        // Add no-activate + tool-window extended styles (preserving whatever
        // Tauri already set, e.g. transparency/layered).
        let ex = GetWindowLongPtrW(editor, GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if ex & extra != extra {
            SetWindowLongPtrW(editor, GWL_EXSTYLE, ex | extra);
        }
    }
}

/// Position and size the editor window over the native viewport.
///
/// `x`, `y`, `width`, `height` are **physical pixels relative to the main
/// window's client area** (identical to what the viewport child window
/// receives). They are converted to screen coordinates here because the editor
/// is a top-level window. `SWP_NOACTIVATE` keeps editor focus intact.
pub fn set_editor_rect(
    editor_hwnd: isize,
    main_hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let main = HWND(main_hwnd as *mut c_void);

        // Client (content-area) origin -> screen origin.
        let mut origin = POINT { x, y };
        let _ = ClientToScreen(main, &mut origin);

        println!(
            "Setting editor rect: client=({}, {}, {}, {}), screen origin=({}, {})",
            x, y, width, height, origin.x, origin.y
        );

        let _ = SetWindowPos(
            editor,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the editor window without taking focus.
pub fn set_editor_visible(editor_hwnd: isize, visible: bool) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let _ = ShowWindow(editor, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
//! Native window management for the transparent **editor WebView** (Windows).
//!
//! The editor is a second, borderless, transparent top-level Tauri window. To
//! make it act as **Layer 3** — always composited above both the native wgpu
//! viewport child window (Layer 2) and the main editor WebView (Layer 1) — we
//! reparent it as an *owned* window of the main top-level window.
//!
//! ## Why an owned top-level window (and not a child or a topmost window)?
//!
//! * A WebView2-backed window does not survive being turned into a `WS_CHILD`
//!   of another window cleanly, so we must keep it top-level.
//! * Win32 guarantees that an **owned** window is always shown above its owner
//!   *and above every child window of that owner* — which is exactly the
//!   "editor above the viewport child HWND" ordering we need, with no per-frame
//!   z-order fighting against the viewport (whose own `SetWindowPos(HWND_TOP)`
//!   only reorders it among the main window's *children*, never above an owned
//!   window).
//! * Unlike `HWND_TOPMOST`, an owned window only floats above *its owner*, so it
//!   correctly drops behind other applications when the editor loses focus.
//!
//! Ownership is established by storing the owner's `HWND` in the editor's
//! `GWLP_HWNDPARENT` slot. The owner does not auto-move the editor, so the JS
//! side reports the viewport rectangle and Rust converts it from client to
//! screen coordinates (`ClientToScreen`) on every update and on owner move.

#![cfg(target_os = "windows")]

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWLP_HWNDPARENT, GWL_EXSTYLE,
    HWND_TOP, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Make `editor_hwnd` an *owned* window of `owner_hwnd` so it is always
/// composited above the owner and all of the owner's child windows (including
/// the native viewport). Also marks it no-activate / tool-window so it never
/// steals focus from the editor and stays off the taskbar/alt-tab list.
pub fn configure_editor(editor_hwnd: isize, owner_hwnd: isize) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);

        // Establish ownership: the editor now floats above the owner's z-band.
        SetWindowLongPtrW(editor, GWLP_HWNDPARENT, owner_hwnd);

        // Add no-activate + tool-window extended styles (preserving whatever
        // Tauri already set, e.g. transparency/layered).
        let ex = GetWindowLongPtrW(editor, GWL_EXSTYLE);
        let extra = (WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0) as isize;
        if ex & extra != extra {
            SetWindowLongPtrW(editor, GWL_EXSTYLE, ex | extra);
        }
    }
}

/// Position and size the editor window over the native viewport.
///
/// `x`, `y`, `width`, `height` are **physical pixels relative to the main
/// window's client area** (identical to what the viewport child window
/// receives). They are converted to screen coordinates here because the editor
/// is a top-level window. `SWP_NOACTIVATE` keeps editor focus intact.
pub fn set_editor_rect(
    editor_hwnd: isize,
    main_hwnd: isize,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let main = HWND(main_hwnd as *mut c_void);

        // Client (content-area) origin -> screen origin.
        let mut origin = POINT { x, y };
        let _ = ClientToScreen(main, &mut origin);

        println!(
            "Setting editor rect: client=({}, {}, {}, {}), screen origin=({}, {})",
            x, y, width, height, origin.x, origin.y
        );

        let _ = SetWindowPos(
            editor,
            Some(HWND_TOP),
            origin.x,
            origin.y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Show or hide the editor window without taking focus.
pub fn set_editor_visible(editor_hwnd: isize, visible: bool) {
    unsafe {
        let editor = HWND(editor_hwnd as *mut c_void);
        let _ = ShowWindow(editor, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}
