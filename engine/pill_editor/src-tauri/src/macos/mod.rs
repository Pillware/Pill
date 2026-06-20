use std::sync::Mutex;
use crate::viewport_store::ViewportStore;

/// No-op macOS backend stub.
pub struct MacOsBackend;

impl crate::platform::PlatformBackend for MacOsBackend {
    fn setup(&self, _app: &tauri::AppHandle) -> Result<(), String> {
        Ok(())
    }

    fn create_viewport(
        &self,
        _store: &Mutex<Option<ViewportStore>>,
        _id: String,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
    ) -> Result<(), String> {
        Ok(())
    }

    fn viewport_resize(
        &self,
        _store: &Mutex<Option<ViewportStore>>,
        _id: String,
        _x: i32,
        _y: i32,
        _width: u32,
        _height: u32,
    ) -> Result<(), String> {
        Ok(())
    }

    fn delete_viewport(
        &self,
        _store: &Mutex<Option<ViewportStore>>,
        _id: String,
    ) -> Result<(), String> {
        Ok(())
    }

    fn on_popup_created(&self, _url: &str) {}

    fn close_popup_window(&self, _layout_id: &str) -> Result<(), String> {
        Ok(())
    }
}
