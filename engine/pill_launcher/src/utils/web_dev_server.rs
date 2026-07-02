//! This file implements a dev HTTP server for WASM targets with live reload.
//!
//! Responsibilities:
//! - run(): entry point - builds the WASM bundle, then serves build/wasm/ on
//!   the given port with long-poll reload when files change.
//! - Injects a small <script> into HTML responses that polls /__reload.
//! - Watches the build output directory for file changes (polling every 500ms).
//! - Depends on: utils::wasm (for the initial bundle build), types::CompileMode.

use std::fs::{self, File};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Error, Result};

use crate::types::CompileMode;
use crate::utils::common::get_latest_mtime_in_directory;
use crate::utils::wasm_target;

const ADDRESS_HOST: &str = "127.0.0.1";
const WATCH_POLL: Duration = Duration::from_millis(500);
const LONG_POLL_TIMEOUT: Duration = Duration::from_secs(30);

// Long-poll client script injected into HTML responses.
const RELOAD_SCRIPT: &str = concat!(
    "<script>(async function reloadLoop(){for(;;){try{",
    "const r=await fetch('/__reload?v='+Date.now(),{cache:'no-store'});",
    "if(r.status===200){location.reload();return;}",
    "}catch(_){await new Promise(r=>setTimeout(r,500));}}})();</script>"
);

type Subscribers = Arc<Mutex<Vec<mpsc::Sender<()>>>>;

/// Build the WASM bundle and start a dev HTTP server.
///
/// 1. Builds the WASM bundle via `wasm_target::build_project`.
/// 2. Starts a tiny_http server on `127.0.0.1:<port>`.
/// 3. In hot-reload mode, spawns a file watcher and injects a live-reload
///    `<script>` into HTML responses.
pub fn run(project_directory_path: &Path, compile_mode: &CompileMode, port: u16) -> Result<()> {
    // 1. Build the WASM bundle first, then serve it.
    wasm_target::build_project(project_directory_path, compile_mode, None, false)?;

    let build_wasm_directory = project_directory_path.join("build").join("wasm");
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));
    let address = format!("{ADDRESS_HOST}:{port}");

    let hot_reload_enabled = *compile_mode == CompileMode::HotReload;

    // 2. Start the file watcher in hot-reload mode only.
    if hot_reload_enabled {
        spawn_watcher(build_wasm_directory.clone(), Arc::clone(&subscribers));
    }

    // 3. Start the HTTP server and handle requests in worker threads.
    let server =
        tiny_http::Server::http(&address).map_err(|error| Error::msg(error.to_string()))?;
    println!();
    println!(
        "Serving {} at http://{}",
        build_wasm_directory.display(),
        address
    );
    if hot_reload_enabled {
        println!("Live reload enabled - the page will refresh on WASM rebuilds.");
    }
    println!("Ctrl+C to stop.");

    for request in server.incoming_requests() {
        let subscribers = Arc::clone(&subscribers);
        let build_wasm_directory = build_wasm_directory.clone();
        thread::spawn(move || {
            if let Err(error) = handle_request(
                request,
                &build_wasm_directory,
                subscribers,
                hot_reload_enabled,
            ) {
                eprintln!("HTTP request error: {:#}", error);
            }
        });
    }

    Ok(())
}

/// Spawn a background thread that polls `watch_dir` for file changes every 500ms.
/// When a change is detected, notifies all connected live-reload subscribers.
fn spawn_watcher(watch_directory: std::path::PathBuf, subscribers: Subscribers) {
    let mut last_mtime = get_latest_mtime_in_directory(&watch_directory);
    thread::spawn(move || loop {
        thread::sleep(WATCH_POLL);
        let current_mtime = get_latest_mtime_in_directory(&watch_directory);
        if current_mtime > last_mtime && current_mtime.is_some() {
            last_mtime = current_mtime;
            let mut subscriptions = subscribers.lock().unwrap_or_else(|e| e.into_inner());
            subscriptions.retain(|sender| sender.send(()).is_ok());
        }
    });
}

/// Handle a single HTTP request: serve static files, inject live-reload script,
/// or respond to the `/__reload` long-poll endpoint.
fn handle_request(
    request: tiny_http::Request,
    build_wasm_directory: &Path,
    subscribers: Subscribers,
    hot_reload_enabled: bool,
) -> Result<()> {
    let url_path = request.url().split('?').next().unwrap_or("/").to_string();

    // /__reload - long-poll endpoint for live-reload clients (hot-reload only).
    if hot_reload_enabled && url_path == "/__reload" {
        return handle_reload(request, subscribers);
    }

    // Map URL path to a file under build_wasm_directory; default to index.html.
    let relative_path = url_path.trim_start_matches('/');
    let relative_path = if relative_path.is_empty() {
        "index.html"
    } else {
        relative_path
    };

    // Reject directory-traversal attempts.
    if relative_path
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return respond(request, 400, "bad path");
    }

    let file_path = build_wasm_directory.join(relative_path);

    // Canonicalize and verify the resolved path stays within the served root.
    let canonical = match file_path.canonicalize() {
        Ok(path) => path,
        Err(_) => return respond(request, 403, "forbidden"),
    };
    let canonical_root = match build_wasm_directory.canonicalize() {
        Ok(path) => path,
        Err(_) => return respond(request, 500, "server misconfigured"),
    };
    if !canonical.starts_with(&canonical_root) {
        return respond(request, 403, "forbidden");
    }
    if !file_path.is_file() {
        return respond(request, 404, "not found");
    }

    // Determine content type from file extension.
    let content_type = content_type_for(&file_path);
    let content_type_header = tiny_http::Header::from_bytes("Content-Type", content_type)
        .map_err(|_| Error::msg("invalid content-type header"))?;

    // Inject the live-reload <script> into HTML responses (hot-reload only).
    if hot_reload_enabled && content_type.starts_with("text/html") {
        let mut html = fs::read_to_string(&file_path)?;
        if let Some(index) = html.rfind("</body>") {
            html.insert_str(index, RELOAD_SCRIPT);
        } else {
            html.push_str(RELOAD_SCRIPT);
        }
        let response = tiny_http::Response::from_string(html).with_header(content_type_header);
        request.respond(response)?;
        return Ok(());
    }

    // Serve the file as-is.
    let file = File::open(&file_path)?;
    let response = tiny_http::Response::from_file(file).with_header(content_type_header);
    request.respond(response)?;
    Ok(())
}

/// Handle the `/__reload` long-poll request.  Waits up to `LONG_POLL_TIMEOUT`
/// for a file-change notification, then returns 200 (reload) or 204 (timeout).
fn handle_reload(request: tiny_http::Request, subscribers: Subscribers) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    subscribers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(sender);
    let signaled = receiver.recv_timeout(LONG_POLL_TIMEOUT).is_ok();
    respond(request, if signaled { 200 } else { 204 }, "")
}

/// Send a minimal HTTP response with the given status code and body.
fn respond(request: tiny_http::Request, status_code: u16, body: &str) -> Result<()> {
    let response = tiny_http::Response::from_string(body.to_string()).with_status_code(status_code);
    request.respond(response)?;
    Ok(())
}

/// Map a file extension to an HTTP Content-Type string.
fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("mjs") => "application/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("css") => "text/css; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}
