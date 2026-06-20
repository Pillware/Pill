/// Shared state that maps React tab IDs to native platform surface handles.
///
/// Populated once during app setup, then mutated by the
/// `create_viewport` / `viewport_resize` / `delete_viewport` Tauri commands.
use std::collections::HashMap;
use std::sync::mpsc;

/// Everything we need to move/resize/destroy one viewport after creation.
pub struct ViewportEntry {
    /// Opaque platform surface handle (the wgpu child window HWND on Windows).
    pub surface: isize,
    /// Opaque platform subsurface handle.
    pub subsurface: isize,
    /// Transparent overlay window HWND (Windows only — 0 otherwise).
    /// Sits above the viewport; draws labels / grid / gizmo via GDI.
    pub overlay: isize,
    /// Channel to the render thread.
    /// Send `Some((w, h))` to resize; send `None` to shut the thread down.
    pub resize_tx: mpsc::Sender<Option<(u32, u32)>>,
}

/// Tauri-managed state, created during app setup and used by the viewport commands.
pub struct ViewportStore {
    /// Opaque platform parent surface handle.
    pub parent_surface: isize,
    /// Opaque platform display handle.
    pub display: isize,
    /// Add to a CSS x coordinate to get the platform-native x.
    pub x_offset: i32,
    /// Add to a CSS y coordinate to get the platform-native y.
    pub y_offset: i32,
    /// Active viewports keyed by the ID string supplied by React.
    pub viewports: HashMap<String, ViewportEntry>,
}

impl ViewportStore {
    pub fn new(parent_surface: isize, display: isize, x_offset: i32, y_offset: i32) -> Self {
        Self {
            parent_surface,
            display,
            x_offset,
            y_offset,
            viewports: HashMap::new(),
        }
    }
}
