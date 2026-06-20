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
//! Tauri-level setup for the transparent **editor WebView** (Layer 3).
//!
//! Creates a second, borderless, transparent `WebviewWindow` (label `editor`)
//! that loads `editor.html`, then hands its `HWND` to [`crate::editor_window`]
//! to be reparented as an *owned* window of the main window so it is always
//! composited above the native viewport.
//!
//! The editor's screen rectangle is driven from JS (`EditorViewportSync`) via
//! the `set_editor_rect` / `set_editor_visible` commands in `lib.rs`. The last
//! reported (client-space) rectangle is cached here so the editor can be
//! repositioned when the *owner* window itself moves or its DPI changes — owned
//! windows do not auto-follow their owner.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

/// Shared state for the editor window. Managed by Tauri (as `Arc<EditorState>`)
/// and also captured by the main window's move/resize event handler.
pub struct EditorState {
    /// `HWND` of the editor window (as `isize`). `0` when unavailable.
    pub editor_hwnd: isize,
    /// `HWND` of the main window (as `isize`), used as the `ClientToScreen` base.
    pub main_hwnd: isize,
    /// Last rectangle reported by JS, in physical pixels relative to the main
    /// window's client area: `(x, y, width, height)`.
    pub last_rect: Mutex<(i32, i32, i32, i32)>,
    /// Whether the editor is currently meant to be visible.
    pub visible: AtomicBool,
}

impl EditorState {
    /// State for platforms/cases where no editor window exists.
    pub fn disabled() -> Self {
        Self {
            editor_hwnd: 0,
            main_hwnd: 0,
            last_rect: Mutex::new((0, 0, 1, 1)),
            visible: AtomicBool::new(false),
        }
    }
}

/// Create the editor `WebviewWindow`, own it to the main window, and return the
/// resulting [`EditorState`].
#[cfg(target_os = "windows")]
pub fn create_editor_webview(app: &tauri::App, main_hwnd: isize) -> EditorState {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    println!("Creating editor webview, main_hwnd = {main_hwnd:#x}");

    let builder = WebviewWindowBuilder::new(app, "editor", WebviewUrl::App("editor.html".into()))
        .title("editor")
        // Transparent + borderless so only the editor's React elements paint.
        .transparent(true)
        .decorations(false)
        .shadow(false)
        // Never appears in the taskbar / alt-tab; never grabs focus.
        .skip_taskbar(true)
        .focused(false)
        .resizable(false)
        // Start hidden at 1x1; JS supplies the real rectangle once laid out.
        .visible(false)
        .inner_size(1.0, 1.0);

    match builder.build() {
        Ok(win) => {
            let editor_hwnd = win.hwnd().map(|h| h.0 as isize).unwrap_or(0);
            if editor_hwnd != 0 {
                crate::editor_window::configure_editor(editor_hwnd, main_hwnd);
                println!("Editor webview configured successfully, hwnd = {editor_hwnd:#x}");
            }

            println!("Editor webview created successfully, hwnd = {editor_hwnd:#x}");
            EditorState {
                editor_hwnd,
                main_hwnd,
                last_rect: Mutex::new((0, 0, 1, 1)),
                visible: AtomicBool::new(false),
            }
        }
        Err(err) => {
            eprintln!("failed to create editor webview: {err}");
            EditorState::disabled()
        }
    }
}

/// Reposition the editor using its last reported client rectangle. Called when
/// the owner window moves/resizes/changes DPI (owned windows don't auto-follow).
#[cfg(target_os = "windows")]
pub fn reposition_to_last(state: &EditorState) {
    use std::sync::atomic::Ordering;

    if state.editor_hwnd == 0 || !state.visible.load(Ordering::Relaxed) {
        return;
    }
    let (x, y, w, h) = *state.last_rect.lock().unwrap();
    crate::editor_window::set_editor_rect(state.editor_hwnd, state.main_hwnd, x, y, w, h);
}
// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

mod editor;
#[cfg(target_os = "windows")]
mod editor_window;
#[cfg(target_os = "windows")]
mod viewport;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tauri::{Manager, State};

use editor::EditorState;

/// Surface size shared between the Tauri command thread (writer) and the
/// dedicated render thread (reader). Physical pixels.
#[derive(Default)]
struct ViewportSize {
    width: AtomicU32,
    height: AtomicU32,
}

/// Application state for the embedded native viewport.
struct ViewportState {
    /// `HWND` of the native child window (as `isize`). `0` when unsupported.
    #[allow(dead_code)]
    child_hwnd: isize,
    /// Shared, lock-free target size consumed by the render thread.
    size: Arc<ViewportSize>,
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Reposition and resize the native viewport window to match the React
/// placeholder. All values are **physical** pixels relative to the window's
/// client area (CSS pixels multiplied by `devicePixelRatio` on the JS side).
#[tauri::command]
fn set_viewport_rect(state: State<'_, ViewportState>, x: i32, y: i32, width: i32, height: i32) {
    let width = width.max(0);
    let height = height.max(0);

    // Publish the new size first so the render thread reconfigures the surface
    // to match before/while the window is moved.
    state
        .size
        .width
        .store(width.max(1) as u32, Ordering::Relaxed);
    state
        .size
        .height
        .store(height.max(1) as u32, Ordering::Relaxed);

    #[cfg(target_os = "windows")]
    if state.child_hwnd != 0 {
        viewport::set_window_rect(state.child_hwnd, x, y, width, height);
    }

    println!(
        "Set viewport rect: client=({}, {}, {}, {}), published size=({}, {})",
        x,
        y,
        width,
        height,
        state.size.width.load(Ordering::Relaxed),
        state.size.height.load(Ordering::Relaxed)
    );

    let _ = (x, y);
}

/// Show or hide the native viewport (e.g. when its host panel is collapsed).
#[tauri::command]
fn set_viewport_visible(state: State<'_, ViewportState>, visible: bool) {
    #[cfg(target_os = "windows")]
    if state.child_hwnd != 0 {
        viewport::set_window_visible(state.child_hwnd, visible);
    }

    println!("Set viewport visible: {}", visible);

    let _ = visible;
}

/// Set the native viewport's opacity (`0.0` transparent .. `1.0` opaque).
/// Used to "ghost" the surface while its dock tab is being dragged.
#[tauri::command]
fn set_viewport_opacity(state: State<'_, ViewportState>, opacity: f32) {
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    #[cfg(target_os = "windows")]
    if state.child_hwnd != 0 {
        viewport::set_window_opacity(state.child_hwnd, alpha);
    }

    println!("Set viewport opacity: {} -> alpha={}", opacity, alpha);

    let _ = alpha;
}

/// Position and size the editor window over the native viewport. Coordinates
/// match `set_viewport_rect` (physical pixels relative to the main window's
/// client area); Rust converts them to screen space for the top-level editor.
#[tauri::command]
fn set_editor_rect(state: State<'_, Arc<EditorState>>, x: i32, y: i32, width: i32, height: i32) {
    let width = width.max(0);
    let height = height.max(0);

    *state.last_rect.lock().unwrap() = (x, y, width, height);

    println!(
        "Set editor rect: client=({}, {}, {}, {})",
        x, y, width, height
    );

    #[cfg(target_os = "windows")]
    if state.editor_hwnd != 0 {
        editor_window::set_editor_rect(state.editor_hwnd, state.main_hwnd, x, y, width, height);
    }

    let _ = (x, y, width, height);
}

/// Show or hide the editor window (mirrors the native viewport's visibility).
#[tauri::command]
fn set_editor_visible(state: State<'_, Arc<EditorState>>, visible: bool) {
    state.visible.store(visible, Ordering::Relaxed);

    #[cfg(target_os = "windows")]
    if state.editor_hwnd != 0 {
        editor_window::set_editor_visible(state.editor_hwnd, visible);
        if visible {
            editor::reposition_to_last(&state);
        }
    }

    println!("Set editor visible: {}", visible);

    let _ = visible;
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            set_viewport_rect,
            set_viewport_visible,
            set_viewport_opacity,
            set_editor_rect,
            set_editor_visible
        ])
        .setup(|app| {
            let size = Arc::new(ViewportSize::default());
            size.width.store(1, Ordering::Relaxed);
            size.height.store(1, Ordering::Relaxed);

            #[allow(unused_mut, unused_assignments)]
            let mut child_hwnd: isize = 0;
            #[allow(unused_mut, unused_assignments)]
            let mut editor_state = Arc::new(EditorState::disabled());

            #[cfg(target_os = "windows")]
            {
                use little_shader_display::EmbeddedRenderer;

                let window = app
                    .get_webview_window("main")
                    .expect("main window must exist");
                let parent_hwnd = window.hwnd().expect("failed to get HWND").0 as isize;

                let child = viewport::create_child_window(parent_hwnd)
                    .expect("failed to create native viewport window");
                child_hwnd = child.hwnd;

                let render_size = size.clone();
                let child_for_thread = child;

                // Dedicated render thread: owns all wgpu state. The window itself
                // lives on the main thread (its messages are pumped by Tauri's
                // event loop); only the HWND value is moved here.
                let (init_w, init_h) = (
                    render_size.width.load(Ordering::Relaxed).max(1),
                    render_size.height.load(Ordering::Relaxed).max(1),
                );
              
                std::thread::spawn(move || {
                  
                    let mut renderer = EmbeddedRenderer::new(
                        child_for_thread.hwnd,
                        child_for_thread.hinstance,
                        init_w,
                        init_h,
                    );

                    loop {
                        let w = render_size.width.load(Ordering::Relaxed).max(1);
                        let h = render_size.height.load(Ordering::Relaxed).max(1);

                        // Skip work while the viewport is effectively invisible
                        // (panel collapsed or not yet laid out) to avoid burning
                        // a core spinning on a 1x1 surface.
                        if w <= 1 && h <= 1 {
                            std::thread::sleep(std::time::Duration::from_millis(32));
                            continue;
                        }

                        renderer.resize(w, h);
                        // Fifo present mode blocks to vsync, pacing the loop.
                        renderer.render();
                    }
                
                });

                println!(
                    "Viewport thread spawned, created child window with hwnd = {child_hwnd:#x}, initial size = ({}, {})",
                    init_w, init_h
                );

                // Layer 3: the transparent editor WebView, owned by the main
                // window so it always composites above the viewport child HWND.
                editor_state = Arc::new(editor::create_editor_webview(app, parent_hwnd));

                // Owned top-level windows do not auto-follow their owner, so keep
                // the editor glued whenever the main window moves / resizes / its
                // DPI changes by re-applying the last reported rectangle.
                let editor_for_events = editor_state.clone();
                window.on_window_event(move |event| match event {
                    tauri::WindowEvent::Moved(_)
                    | tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::ScaleFactorChanged { .. } => {
                        editor::reposition_to_last(&editor_for_events);
                    }
                    _ => {}
                });

                println!("Editor webview setup complete, hwnd = {child_hwnd:#x}");
            }

            app.manage(ViewportState { child_hwnd, size });
            app.manage(editor_state);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
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
import { useEffect, useState } from "react";

export default function EditorWebView() {
    const [tick, setTick] = useState(0);

    useEffect(() => {
        const id = setInterval(() => {
            setTick((prev) => prev + 1);
        }, 1000);

        return () => clearInterval(id);
    }, []);

    useEffect(() => {
        if (tick > 0) {
            console.log("hello editor");
        }
    }, [tick]);

    return (
        <div className="editor-webview-root">
            <div style={{ marginTop: "200px", color: "white", fontSize: "24px", textAlign: "center" }}>
                hellooxxxo editor webview! tick: {tick}
            </div>
        </div>
    );
}
import { invoke } from "@tauri-apps/api/core";

/**
 * Rectangle reported to Rust, in **physical pixels** relative to the window's
 * client (content) area — exactly the coordinate space of a Win32 child window.
 */
interface PhysicalRect {
    x: number;
    y: number;
    width: number;
    height: number;
    [key: string]: number;
}

function rectsEqual(a: PhysicalRect, b: PhysicalRect): boolean {
    return (
        a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height
    );
}

/** Opacity of the native viewport while *its own* dock tab is being dragged. */
const DRAG_OPACITY = 0.55;

/**
 * Opacity of the native viewport while *another* tab is being dragged. The
 * native surface sits on top of the WebView, so golden-layout's drop indicator
 * and floating tab preview (both DOM elements behind the surface) would be
 * hidden by it. Going fully transparent lets that preview show through, so the
 * user can see exactly where/how the dragged tab will dock.
 */
const OTHER_DRAG_OPACITY = 0;

/**
 * Keeps the native wgpu child window perfectly aligned with a single DOM
 * element (the golden-layout "viewport" tab's content element).
 *
 * The element is never rendered into — it only provides layout. Whenever it
 * moves or resizes (panel resize, dock rearrange, window resize, DPI change,
 * **and tab dragging**) we recompute its on-screen rectangle and forward it to
 * Rust, which repositions the native surface.
 *
 * Tab dragging is handled specially: golden-layout physically re-parents the
 * tracked element into a floating `.lm_dragProxy` while dragging, so simply
 * polling the element's rectangle every frame makes the native surface follow
 * the tab. During the drag we also ghost the surface (reduced opacity).
 *
 * This is a plain controller (not a React hook) because golden-layout creates
 * and destroys components imperatively.
 */
export class NativeViewportSync {
    private element: HTMLElement;
    private lastRect: PhysicalRect | null = null;
    private frame = 0;
    private dragRaf = 0;
    private disposed = false;
    private visible = true;
    private draggingSelf = false;
    private draggingOther = false;

    private resizeObserver: ResizeObserver;
    private dragObserver: MutationObserver;

    constructor(element: HTMLElement) {
        this.element = element;

        // Element size changes (splitter drag, dock rearrange, maximise).
        this.resizeObserver = new ResizeObserver(() => this.schedule());
        this.resizeObserver.observe(element);

        // Detect when golden-layout starts/stops dragging *our* element by watching
        // for the floating drag proxy entering/leaving the DOM and containing it.
        this.dragObserver = new MutationObserver(() => this.checkDrag());
        this.dragObserver.observe(document.body, { childList: true, subtree: true });

        // Ambient layout shifts that don't resize the element itself.
        window.addEventListener("resize", this.schedule);
        window.addEventListener("scroll", this.schedule, true);
        window.addEventListener("resize", this.scheduleEditor);

        this.schedule();
        this.pushEditorRect();
        invoke("set_editor_visible", { visible: true }).catch(() => { });
    }

    private computeRect(): PhysicalRect {
        const r = this.element.getBoundingClientRect();
        // CSS px -> physical px (handles HiDPI and runtime DPI changes).
        const dpr = window.devicePixelRatio || 1;
        return {
            x: Math.round(r.left * dpr),
            y: Math.round(r.top * dpr),
            width: Math.round(r.width * dpr),
            height: Math.round(r.height * dpr),
        };
    }

    private pushRect = () => {
        if (this.disposed) return;
        const rect = this.computeRect();
        if (this.lastRect && rectsEqual(this.lastRect, rect)) return;
        this.lastRect = rect;
        invoke("set_viewport_rect", rect).catch(() => {
            /* command may be unavailable on unsupported platforms */
        });
        this.pushEditorRect();
    };

    private pushEditorRect = () => {
        if (this.disposed) return;
        const dpr = window.devicePixelRatio || 1;
        const rect = {
            x: 0,
            y: 0,
            width: Math.round(window.innerWidth * dpr),
            height: Math.round(window.innerHeight * dpr),
        };
        invoke("set_editor_rect", rect).catch(() => { });
    };

    private editorFrame = 0;

    /** Coalesce window resize bursts into one editor-rect update per frame. */
    private scheduleEditor = () => {
        if (this.editorFrame || this.disposed) return;
        this.editorFrame = requestAnimationFrame(() => {
            this.editorFrame = 0;
            this.pushEditorRect();
        });
    };

    /** Coalesce bursts of layout events into one update per animation frame. */
    private schedule = () => {
        if (this.frame || this.disposed) return;
        this.frame = requestAnimationFrame(() => {
            this.frame = 0;
            this.pushRect();
        });
    };

    private checkDrag = () => {
        // A `.lm_dragProxy` exists in the DOM only while a tab is being dragged.
        const proxy = document.querySelector(".lm_dragProxy");
        const self = !!proxy && proxy.contains(this.element);
        const other = !!proxy && !self;

        // Case 1: our own viewport tab is being dragged -> ghost it and make the
        // native surface follow the floating proxy frame by frame.
        if (self !== this.draggingSelf) {
            this.draggingSelf = self;
            if (self) this.startDragFollow();
            else this.stopDragFollow();
        }

        // Case 2: some *other* tab is being dragged -> get out of the way so the
        // DOM drop preview underneath the native surface is visible.
        if (other !== this.draggingOther) {
            this.draggingOther = other;
            if (other) {
                this.setOpacity(OTHER_DRAG_OPACITY);
            } else if (!this.draggingSelf) {
                // Drag finished; restore and resync to the (possibly new) layout.
                this.setOpacity(1);
                this.lastRect = null;
                this.schedule();
            }
        }
    };

    private startDragFollow() {
        this.setOpacity(DRAG_OPACITY);
        // The proxy moves via inline styles (no resize/scroll events), so poll the
        // rectangle every frame to keep the native surface glued to the tab.
        const follow = () => {
            if (this.disposed || !this.draggingSelf) return;
            this.pushRect();
            this.dragRaf = requestAnimationFrame(follow);
        };
        this.dragRaf = requestAnimationFrame(follow);
    }

    private stopDragFollow() {
        if (this.dragRaf) cancelAnimationFrame(this.dragRaf);
        this.dragRaf = 0;
        this.setOpacity(1);
        // Element has been re-docked; resync to its final resting place.
        this.lastRect = null;
        this.schedule();
    }

    private setOpacity(opacity: number) {
        invoke("set_viewport_opacity", { opacity }).catch(() => { });
    }

    /** Show/hide the native surface (e.g. when the tab's stack is hidden). */
    setVisible(visible: boolean) {
        if (visible === this.visible) return;
        this.visible = visible;
        invoke("set_viewport_visible", { visible }).catch(() => { });
        invoke("set_editor_visible", { visible }).catch(() => { });
        if (visible) {
            this.lastRect = null;
            this.schedule();
        }
    }

    /** Force a resync (call from golden-layout `resize`/`show` events). */
    refresh() {
        this.lastRect = null;
        this.schedule();
    }

    dispose() {
        this.disposed = true;
        if (this.frame) cancelAnimationFrame(this.frame);
        if (this.dragRaf) cancelAnimationFrame(this.dragRaf);
        if (this.editorFrame) cancelAnimationFrame(this.editorFrame);
        this.resizeObserver.disconnect();
        this.dragObserver.disconnect();
        window.removeEventListener("resize", this.schedule);
        window.removeEventListener("scroll", this.schedule, true);
        window.removeEventListener("resize", this.scheduleEditor);
        // Hide the surface; there is no longer an element to track.
        invoke("set_viewport_visible", { visible: false }).catch(() => { });
        invoke("set_editor_visible", { visible: false }).catch(() => { });
    }
}
import React from "react";
import ReactDOM from "react-dom/client";
import "./background-styles.css";
import BackgroundWebView from "./BackgroundWebView";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <BackgroundWebView />
  </React.StrictMode>,
);
import { useEffect, useRef, useState } from "react";
import { NativeViewportSync } from "./nativeViewport";
import "./editor-styles.css";

export default function BackgroundWebView() {
    const hostRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const host = hostRef.current;
        if (!host) return;
        let viewportSync: NativeViewportSync | null = null;
        const sync = new NativeViewportSync(document.getElementsByClassName("viewport")[0] as HTMLElement);
        viewportSync = sync;
    }, []);

    const [tick, setTick] = useState(0);

    useEffect(() => {
        const id = setInterval(() => {
            setTick((prev) => prev + 1);
        }, 1000);

        return () => clearInterval(id);
    }, []);

    useEffect(() => {
        if (tick > 0) {
            console.log("hello background webview!");
        }
    }, [tick]);

    return <div className="checker-bg" ref={hostRef}>
        <div className="viewport" style={{ width: "300px", height: "300px" }}>
            xxx
        </div>
        <div>
            tick: {tick}
        </div>
    </div>;
}
<!doctype html>
<html lang="en">

<head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Editor</title>
</head>

<body>
    <div id="editor-webview-root"></div>
    <script type="module" src="/src/editor-webview-main.tsx"></script>
</body>

</html><!doctype html>
<html lang="en">

<head>
  <meta charset="UTF-8" />
  <link rel="icon" type="image/svg+xml" href="/vite.svg" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Tauri + React + Typescript</title>
</head>

<body>
  <div id="root"></div>
  <script type="module" src="/src/background-webview-main.tsx"></script>
</body>

</html>
