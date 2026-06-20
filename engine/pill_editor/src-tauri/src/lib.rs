mod platform;
mod viewport_store;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod orange_renderer;
#[cfg(target_os = "windows")]
mod renderer_windows;
#[cfg(target_os = "windows")]
mod windows;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn create_viewport(
    backend: tauri::State<'_, Box<dyn platform::PlatformBackend>>,
    store: tauri::State<'_, std::sync::Mutex<Option<viewport_store::ViewportStore>>>,
    id: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    backend.create_viewport(&store, id, x, y, width, height)
}

#[tauri::command]
fn viewport_resize(
    backend: tauri::State<'_, Box<dyn platform::PlatformBackend>>,
    store: tauri::State<'_, std::sync::Mutex<Option<viewport_store::ViewportStore>>>,
    id: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    backend.viewport_resize(&store, id, x, y, width, height)
}

#[tauri::command]
fn delete_viewport(
    backend: tauri::State<'_, Box<dyn platform::PlatformBackend>>,
    store: tauri::State<'_, std::sync::Mutex<Option<viewport_store::ViewportStore>>>,
    id: String,
) -> Result<(), String> {
    backend.delete_viewport(&store, id)
}

/// Close a popup GTK window by its flexlayout layout ID.
#[tauri::command]
fn close_popup_window(
    backend: tauri::State<'_, Box<dyn platform::PlatformBackend>>,
    layout_id: String,
) -> Result<(), String> {
    backend.close_popup_window(&layout_id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            create_viewport,
            viewport_resize,
            delete_viewport,
            close_popup_window,
        ])
        .setup(|app| {
            use tauri::{Manager as _, WebviewUrl, WebviewWindowBuilder};

            app.manage(std::sync::Mutex::new(
                Option::<viewport_store::ViewportStore>::None,
            ));

            let backend = platform::create_backend();
            app.manage(backend);

            // Build the main window first — linux_ui_drawer needs it.
            let app_handle = app.handle().clone();
            let mut window_builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Pill")
                    .inner_size(800.0, 600.0);

            // Transparent window so the desktop is visible behind the WebView.
            // NOTE: On Windows, WS_EX_LAYERED may interfere with HTML5
            // Drag & Drop in WebView2 (used by flexlayout tab dragging).
            // If DnD breaks, you can toggle this off with #[cfg] guards.
            window_builder = window_builder.transparent(true);

            window_builder
                .disable_drag_drop_handler() // TODO: Make sure it works on linux!
                .on_new_window(move |url, _features| {
                    use tauri::Manager;
                    eprintln!("[popup] created: {url}");
                    let b = app_handle.state::<Box<dyn platform::PlatformBackend>>();
                    b.on_popup_created(url.as_str());

                    // Delegate to platform backend for window creation.
                    // Windows: custom window with title "Pill", no URL bar.
                    // Linux:   default Allow (GTK backend renames the window).
                    #[cfg(target_os = "windows")]
                    {
                        crate::windows::handle_popup(url, &app_handle)
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        tauri::webview::NewWindowResponse::Allow
                    }
                })
                .build()
                .map_err(|e| format!("failed to create main window: {e}"))?;

            // Now that the window exists, initialise the platform backend.
            let backend = app.state::<Box<dyn platform::PlatformBackend>>();
            if let Err(e) = backend.setup(app.handle()) {
                eprintln!("[setup] backend error: {e}");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
