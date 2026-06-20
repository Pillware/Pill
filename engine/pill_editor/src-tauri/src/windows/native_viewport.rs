//! Native child-window management for the embedded wgpu viewport (Windows).
//!
//! wgpu cannot render into the WebView DOM, so we create a borderless Win32
//! child window that is a sibling of the WebView2 host inside the same top-level
//! Tauri window. React reports the on-screen rectangle of its `#viewport-slot`
//! placeholder, and we move/resize this child window to match exactly. wgpu owns
//! a surface backed by this child window's `HWND`.
//!
//! ## Overlay windows
//!
//! Because the wgpu viewport child window sits above the WebView2 in z-order,
//! any HTML overlay elements (labels, grid, gizmo) rendered inside the
//! scene/game panel are hidden.  To fix this we create a second, transparent
//! *overlay* child window above each viewport.  The overlay uses
//! `WS_EX_LAYERED` + `LWA_COLORKEY` so its magenta background is fully
//! transparent, and it draws the overlay text &amp; gizmo geometry via GDI.

#![cfg(target_os = "windows")]
#![allow(dead_code)] // overlay code disabled until compositing issue resolved

use std::sync::Once;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreatePen, CreateSolidBrush, EndPaint, LineTo, MoveToEx, SelectObject, SetBkMode,
    SetTextColor, TextOutW, HBRUSH, HPEN, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    RegisterClassW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    GWLP_USERDATA, HWND_TOP, LWA_COLORKEY, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WINDOW_EX_STYLE,
    WM_ERASEBKGND, WM_PAINT, WNDCLASSW, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED,
    WS_EX_TRANSPARENT, WS_VISIBLE,
};

// ── Viewport window class ─────────────────────────────────────────────────

const CLASS_NAME: PCWSTR = w!("PillViewportWindowClass");

static REGISTER: Once = Once::new();

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
pub fn create_child_window(parent_hwnd: isize) -> Result<isize, String> {
    unsafe {
        let hmodule = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let hinstance = HINSTANCE(hmodule.0);
        register_class(hinstance);

        let parent = HWND(parent_hwnd);

        // windows 0.52: CreateWindowExW returns HWND directly (HWND(0) on failure).
        // Pass HWND directly for the parent, not Option<HWND>.
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            parent,    // HWND directly (no Some)
            None,      // hMenu
            hinstance, // HINSTANCE directly (no Some)
            None,      // lpParam
        );

        if hwnd.0 == 0 {
            return Err("CreateWindowExW returned null HWND".into());
        }

        Ok(hwnd.0)
    }
}

/// Move and resize the child window to the given physical-pixel rectangle,
/// relative to the parent window's client area, keeping it at the top of the
/// sibling z-order so it stays above the WebView.
pub fn set_window_rect(child_hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let hwnd = HWND(child_hwnd);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
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
#[allow(dead_code)]
pub fn set_window_visible(child_hwnd: isize, visible: bool) {
    unsafe {
        let hwnd = HWND(child_hwnd);
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
    }
}

/// Destroy the child window.
pub fn destroy_window(child_hwnd: isize) {
    unsafe {
        let hwnd = HWND(child_hwnd);
        let _ = DestroyWindow(hwnd);
    }
}

// ── Overlay window class ──────────────────────────────────────────────────
//
// NOTE: Disabled until the compositing issue with WS_EX_LAYERED child windows
// inside a layered parent is solved.  See WINDOWS_IMPL.md "Layer 3" design.

const OVERLAY_CLASS_NAME: PCWSTR = w!("PillOverlayWindowClass");
/// Magenta — used as the colour-key so the overlay background is transparent.
const KEY_COLOR: COLORREF = COLORREF(0x00FF_00FF);

static OVERLAY_REGISTER: Once = Once::new();

/// Tag stored in `GWLP_USERDATA` so the wndproc knows what to draw.
#[repr(usize)]
pub enum OverlayKind {
    Scene = 0,
    Game = 1,
}

/// Window procedure for the transparent overlay.
unsafe extern "system" fn overlay_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            // Let the class brush (magenta) fill the background so the
            // colour-key makes it transparent.
            LRESULT(0) // proceed with default erasing
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            let mut rect = RECT::default();
            let _ = GetClientRect(hwnd, &mut rect);
            let w = rect.right - rect.left;
            let h = rect.bottom - rect.top;

            let kind = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as usize;

            // ── Transparent background for text ─────────────────────
            SetBkMode(hdc, TRANSPARENT);

            // ── Scene-label in top-left ─────────────────────────────
            SetTextColor(hdc, COLORREF(0x00CC_CCCC)); // light grey
            if kind == OverlayKind::Game as usize {
                let _ = TextOutW(hdc, 8, 4, w!("Game").as_wide()).ok();
            } else {
                let _ = TextOutW(hdc, 8, 4, w!("Scene").as_wide()).ok();
            }

            // ── Simple XYZ gizmo in top-right ───────────────────────
            if w > 80 && h > 60 {
                let cx = w - 44;
                let cy = 24;
                let len: i32 = 18;

                // X axis — red
                let red_pen = CreatePen(
                    windows::Win32::Graphics::Gdi::PS_SOLID,
                    2,
                    COLORREF(0x0000_00FF),
                );
                let old_pen = SelectObject(hdc, HPEN(red_pen.0));
                let _ = MoveToEx(hdc, cx, cy, None);
                let _ = LineTo(hdc, cx + len, cy);
                SetTextColor(hdc, COLORREF(0x0000_00FF));
                let _ = TextOutW(hdc, cx + len + 2, cy - 7, w!("X").as_wide());

                // Y axis — green
                let green_pen = CreatePen(
                    windows::Win32::Graphics::Gdi::PS_SOLID,
                    2,
                    COLORREF(0x0000_FF00),
                );
                SelectObject(hdc, HPEN(green_pen.0));
                let _ = MoveToEx(hdc, cx, cy, None);
                let _ = LineTo(hdc, cx, cy + len);
                SetTextColor(hdc, COLORREF(0x0000_FF00));
                let _ = TextOutW(hdc, cx - 6, cy + len + 1, w!("Y").as_wide());

                // Z axis — blue
                let blue_pen = CreatePen(
                    windows::Win32::Graphics::Gdi::PS_SOLID,
                    2,
                    COLORREF(0x00FF_0000),
                );
                SelectObject(hdc, HPEN(blue_pen.0));
                let _ = MoveToEx(hdc, cx, cy, None);
                let _ = LineTo(hdc, cx - len / 2, cy + len / 2);
                SetTextColor(hdc, COLORREF(0x00FF_0000));
                let _ = TextOutW(hdc, cx - len / 2 - 14, cy + len / 2 - 6, w!("Z").as_wide());

                // Restore
                SelectObject(hdc, old_pen);
            }

            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn register_overlay_class(hinstance: HINSTANCE) {
    OVERLAY_REGISTER.call_once(|| unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(overlay_wndproc),
            hInstance: hinstance,
            lpszClassName: OVERLAY_CLASS_NAME,
            // Magenta background → transparent via LWA_COLORKEY.
            hbrBackground: HBRUSH(CreateSolidBrush(KEY_COLOR).0),
            ..Default::default()
        };
        RegisterClassW(&class);
    });
}

/// Create a transparent overlay child window above the viewport.
///
/// Returns the overlay `HWND`.  `kind` controls whether a "Scene" or "Game"
/// label is drawn.
pub fn create_overlay(parent_hwnd: isize, kind: OverlayKind) -> Result<isize, String> {
    unsafe {
        let hmodule = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let hinstance = HINSTANCE(hmodule.0);
        register_overlay_class(hinstance);

        let parent = HWND(parent_hwnd);

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT,
            OVERLAY_CLASS_NAME,
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            parent,
            None,
            hinstance,
            None,
        );

        if hwnd.0 == 0 {
            return Err("CreateWindowExW (overlay) returned null HWND".into());
        }

        // Make magenta pixels transparent.
        let _ = SetLayeredWindowAttributes(hwnd, KEY_COLOR, 0, LWA_COLORKEY);

        // Store the overlay kind.
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, kind as isize);

        Ok(hwnd.0)
    }
}

/// Position the overlay window to match the viewport rectangle.
/// Must be called AFTER `set_window_rect` on the viewport so the overlay
/// ends up on top (newer `HWND_TOP` call wins).
pub fn set_overlay_rect(overlay_hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    unsafe {
        let hwnd = HWND(overlay_hwnd);
        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
            x,
            y,
            width.max(1),
            height.max(1),
            SWP_NOACTIVATE,
        );
    }
}

/// Destroy the overlay window.
pub fn destroy_overlay(overlay_hwnd: isize) {
    unsafe {
        let hwnd = HWND(overlay_hwnd);
        let _ = DestroyWindow(hwnd);
    }
}
