#![cfg(target_os = "linux")]

//! Linux backend implementation for the platform abstraction.
//!
//! Handles:
//! - Wayland subsurface creation for wgpu viewports (via `linux_surface_utilities`)
//! - Background SHM layer and viewport-offset computation (via `linux_ui_drawer`)
//! - Popup window lifecycle: rename, store pointer, destroy on close
//! - Delegates wgpu render-thread management to `orange_renderer`

pub mod linux_surface_utilities;
pub mod linux_ui_drawer;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::viewport_store::{ViewportStore, ViewportEntry};

/// Linux implementation of [`crate::platform::PlatformBackend`].
///
/// Manages Wayland subsurfaces, wgpu render threads, and popup GTK windows.
pub struct LinuxBackend {
    /// Shared wgpu renderer — one per app, creates threads per viewport.
    renderer: crate::orange_renderer::Renderer,
    /// Map of flexlayout layout IDs → raw GTK window pointers.
    /// Used by `close_popup_window` to destroy windows after tabs are dragged out.
    popup_windows: Arc<Mutex<HashMap<String, isize>>>,
}

impl LinuxBackend {
    /// Create a new backend with an empty popup-window map.
    pub fn new() -> Self {
        Self {
            renderer: crate::orange_renderer::Renderer::new(),
            popup_windows: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl crate::platform::PlatformBackend for LinuxBackend {
    /// One-time initialisation: draws the background SHM layer and computes
    /// CSS→Wayland coordinate offsets.  Must be called AFTER the main window
    /// has been built (the `"main"` WebviewWindow must exist).
    fn setup(&self, app: &tauri::AppHandle) -> Result<(), String> {
        linux_ui_drawer::draw(app.clone()).map_err(|e| e.to_string())
    }

    /// Create a wgpu subsurface at the given CSS coordinates and spawn a
    /// dedicated render thread.  The `id` is the stable React tab ID.
    fn create_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        // ── Read offsets from ViewportStore without holding the lock across
        //     the blocking GTK-thread call.
        let (parent_usize, display_usize, wx, wy) = {
            let g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_ref().ok_or("ViewportStore not ready")?;
            (
                s.parent_surface as usize,
                s.display as usize,
                s.x_offset + x,
                s.y_offset + y,
            )
        };

        // ── Create the Wayland subsurface on the GTK main thread (required
        //     because the Wayland connection is single-threaded).
        let (tx, rx) = std::sync::mpsc::channel::<
            Result<linux_surface_utilities::WgpuSurface, String>,
        >();
        gtk::glib::MainContext::default().invoke(move || {
            let res = linux_surface_utilities::create_wgpu_subsurface(
                parent_usize as *mut std::ffi::c_void,
                display_usize as *mut std::ffi::c_void,
                wx,
                wy,
                width as i32,
                height as i32,
            );
            let _ = tx.send(res);
        });
        let surf = rx.recv().map_err(|e| e.to_string())??;

        // ── Start the render thread; the returned Sender lets us signal
        //     resizes (Some(w,h)) or shutdown (None) later.
        let resize_tx =
            self.renderer
                .register_viewport(surf.surface, surf.display, width, height);

        // ── Store the entry in ViewportStore keyed by the React tab ID.
        //     Future resize/delete commands will look it up here.
        let mut g = store.lock().map_err(|e| e.to_string())?;
        let s = g.as_mut().ok_or("ViewportStore not ready")?;
        s.viewports.insert(
            id,
            ViewportEntry {
                surface: surf.surface,
                subsurface: surf.subsurface,
                overlay: 0, // not used on Linux
                resize_tx,
            },
        );
        Ok(())
    }

    /// Move a subsurface when its tab is repositioned, and signal the render
    /// thread to reconfigure the swapchain for the new size.
    fn viewport_resize(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let (subsurface_usize, parent_usize, display_usize, wx, wy, resize_tx) = {
            let g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_ref().ok_or("ViewportStore not ready")?;
            let e = s
                .viewports
                .get(&id)
                .ok_or_else(|| format!("unknown viewport: {id}"))?;
            (
                e.subsurface as usize,
                s.parent_surface as usize,
                s.display as usize,
                s.x_offset + x,
                s.y_offset + y,
                e.resize_tx.clone(),
            )
        };

        // ── Move on the GTK main thread (fire-and-forget; errors logged).
        gtk::glib::MainContext::default().invoke(move || {
            if let Err(e) = linux_surface_utilities::move_wgpu_subsurface(
                subsurface_usize as *mut std::ffi::c_void,
                parent_usize as *mut std::ffi::c_void,
                display_usize as *mut std::ffi::c_void,
                wx,
                wy,
            ) {
                eprintln!("[viewport_resize] move failed: {e}");
            }
        });

        // ── Signal the render thread to reconfigure its swapchain.
        //     `max(1)` guards against zero-size surfaces (tab collapsed).
        let _ = resize_tx.send(Some((width.max(1), height.max(1))));
        Ok(())
    }

    /// Send a shutdown signal to the render thread, then destroy the Wayland
    /// subsurface.  Called when a Scene/Game tab is closed.
    fn delete_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
    ) -> Result<(), String> {
        // ── Remove the entry and signal the render thread to stop.
        let (surface_usize, subsurface_usize, parent_usize, display_usize) = {
            let mut g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_mut().ok_or("ViewportStore not ready")?;
            let e = s
                .viewports
                .remove(&id)
                .ok_or_else(|| format!("unknown viewport: {id}"))?;
            let _ = e.resize_tx.send(None); // None = shutdown
            (
                e.surface as usize,
                e.subsurface as usize,
                s.parent_surface as usize,
                s.display as usize,
            )
        };

        // ── Give the render thread one frame (~16ms at 60fps) to notice
        //     the shutdown signal before we destroy its Wayland objects.
        //     32ms is a generous safety margin.
        std::thread::sleep(std::time::Duration::from_millis(32));

        // ── Destroy the Wayland subsurface role and surface on the GTK main
        //     thread (the only thread allowed to touch the Wayland connection).
        gtk::glib::MainContext::default().invoke(move || {
            if let Err(e) = linux_surface_utilities::destroy_wgpu_surface(
                surface_usize as *mut std::ffi::c_void,
                subsurface_usize as *mut std::ffi::c_void,
                parent_usize as *mut std::ffi::c_void,
                display_usize as *mut std::ffi::c_void,
            ) {
                eprintln!("[delete_viewport] destroy failed: {e}");
            }
        });
        Ok(())
    }

    /// Called when flexlayout opens a popup window via `window.open`.
    ///
    /// On the next GTK event-loop iteration, finds the new window by matching
    /// `popout.html?id=<layout_id>` in the title, renames it to "Pill", and
    /// stores its raw pointer for later destruction.
    fn on_popup_created(&self, url: &str) {
        // Extract the flexlayout layout ID from the query string.
        let layout_id = url
            .split("id=")
            .nth(1)
            .unwrap_or("")
            .to_string();

        let popups_clone = self.popup_windows.clone();
        // Defer to the next GTK iteration — wry creates the window
        // synchronously in the `Allow` handler before this runs.
        gtk::glib::idle_add_local_once(move || {
            use gtk::prelude::{GtkWindowExt, Cast, ObjectType};

            // Warn if other popups already exist (WebKitGTK limitation).
            let existing = popups_clone.lock().map(|m| m.len()).unwrap_or(0);
            if existing > 0 {
                eprintln!(
                    "[popup] WARNING: Having two popups at the same time is not supported - {existing} other popup(s) already open. \
                     WebKitGTK will destroy its webview to display a new popup! This is work in progress!"
                );
            }

            // Scan all toplevel GTK windows for the new popup.
            for widget in gtk::Window::list_toplevels() {
                if let Ok(win) = widget.clone().downcast::<gtk::Window>() {
                    let title = win.title().unwrap_or_default();
                    if title.as_str().contains("popout.html")
                        && title.as_str().contains(&format!("id={layout_id}"))
                    {
                        win.set_title("Pill");
                        if let Ok(mut map) = popups_clone.lock() {
                            map.insert(layout_id.clone(), win.as_ptr() as isize);
                        }
                        eprintln!("[popup] renamed + stored: {layout_id}");
                    }
                }
            }
        });
    }

    /// Destroy a popup GTK window identified by its flexlayout layout ID.
    ///
    /// Looks up the raw pointer stored by `on_popup_created`, calls
    /// `gtk_widget_destroy`, and removes the entry from the map.
    fn close_popup_window(&self, layout_id: &str) -> Result<(), String> {
        let ptr = {
            let map = self.popup_windows.lock().map_err(|e| e.to_string())?;
            map.get(layout_id).copied()
        };
        if let Some(ptr) = ptr {
            eprintln!("[close_popup_window] destroying window for: {layout_id}");
            unsafe { gtk::ffi::gtk_widget_destroy(ptr as *mut gtk::ffi::GtkWidget); }
            self.popup_windows.lock().map_err(|e| e.to_string())?.remove(layout_id);
        }
        Ok(())
    }
}
