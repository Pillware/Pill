use std::sync::Mutex;
use crate::viewport_store::ViewportStore;

/// Platform-specific backend that drives the native viewport lifecycle.
///
/// One implementation exists per target OS.  Non-Linux builds use no-op stubs
/// so the Tauri commands compile and run everywhere without `#[cfg]` guards.
pub trait PlatformBackend: Send + Sync + 'static {
    /// One-time initialisation called from Tauri's `setup()`.
    fn setup(&self, app: &tauri::AppHandle) -> Result<(), String>;

    /// Create a native viewport at the given CSS-space position.
    fn create_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String>;

    /// Move / resize an existing viewport.
    fn viewport_resize(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String>;

    /// Destroy a viewport and release its resources.
    fn delete_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
    ) -> Result<(), String>;

    /// Called when a popup window is created via `window.open`.
    /// The URL contains `popout.html?id=<flexlayout_layout_id>`.
    fn on_popup_created(&self, url: &str);

    /// Close a popup GTK window by its flexlayout layout ID.
    fn close_popup_window(&self, layout_id: &str) -> Result<(), String>;
}

/// Construct the appropriate backend for the current target OS.
#[allow(unreachable_code)]
pub fn create_backend() -> Box<dyn PlatformBackend> {
    #[cfg(target_os = "linux")]
    {
        return Box::new(crate::linux::LinuxBackend::new());
    }
    #[cfg(target_os = "windows")]
    {
        return Box::new(crate::windows::WindowsBackend::new());
    }
    #[cfg(target_os = "macos")]
    {
        return Box::new(crate::macos::MacOsBackend);
    }
    unreachable!("unsupported platform — add a PlatformBackend impl")
}
