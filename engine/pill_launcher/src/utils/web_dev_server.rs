// This file implements a dev HTTP server for WASM targets with live reload.
//
// Responsibilities:
// - run(): entry point — builds the WASM bundle, then serves build/wasm/ on
//   localhost:8080 with long-poll reload when files change.
// - Injects a small <script> into HTML responses that polls /__reload.
// - Watches the build output directory for file changes (polling every 500ms).
// - Depends on: utils::wasm_build (for the initial bundle build), types::CompileMode.

use std::fs::{self, File};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Error, Result};

use crate::types::CompileMode;
use crate::utils::wasm;

const ADDRESS: &str = "127.0.0.1:8080";
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

/// Build the WASM bundle and start a dev HTTP server on localhost:8080.
/// Injects a live-reload script into HTML responses; watches for file changes.
pub fn run(game_project_directory_path: &Path, compile_mode: &CompileMode) -> Result<()> {
    // Build the WASM bundle first, then serve it.
    wasm::build(game_project_directory_path, compile_mode, None)?;

    let build_wasm_dir = game_project_directory_path.join("build").join("wasm");
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));

    // Start a background watcher that notifies long-poll clients on file changes.
    spawn_watcher(build_wasm_dir.clone(), Arc::clone(&subscribers));

    let server = tiny_http::Server::http(ADDRESS).map_err(|e| Error::msg(e.to_string()))?;
    println!();
    println!("Serving {} at http://{}", build_wasm_dir.display(), ADDRESS);
    println!("Live reload enabled — the page will refresh on wasm rebuilds.");
    println!("Ctrl+C to stop.");

    for request in server.incoming_requests() {
        let subscribers = Arc::clone(&subscribers);
        let build_wasm_dir = build_wasm_dir.clone();
        thread::spawn(move || {
            if let Err(e) = handle_request(request, &build_wasm_dir, subscribers) {
                eprintln!("http request error: {:#}", e);
            }
        });
    }

    Ok(())
}

fn spawn_watcher(watch_dir: std::path::PathBuf, subscribers: Subscribers) {
    let mut last = latest_mtime(&watch_dir);
    thread::spawn(move || loop {
        thread::sleep(WATCH_POLL);
        let cur = latest_mtime(&watch_dir);
        if cur > last && cur.is_some() {
            last = cur;
            let mut subs = subscribers.lock().unwrap_or_else(|e| e.into_inner());
            subs.retain(|tx| tx.send(()).is_ok());
        }
    });
}

// Max mtime among regular files in `dir` (shallow, skipping dotfiles/.build scratch).
fn latest_mtime(dir: &Path) -> Option<SystemTime> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                return None;
            }
            let md = e.metadata().ok()?;
            if !md.is_file() {
                return None;
            }
            md.modified().ok()
        })
        .max()
}

fn handle_request(
    request: tiny_http::Request,
    build_wasm_dir: &Path,
    subscribers: Subscribers,
) -> Result<()> {
    let url_path = request.url().split('?').next().unwrap_or("/").to_string();

    // /__reload is the long-poll endpoint for live-reload clients.
    if url_path == "/__reload" {
        return handle_reload(request, subscribers);
    }

    // Map URL path to a file under build_wasm_dir; reject directory traversal.
    let relative_path = url_path.trim_start_matches('/');
    let relative_path = if relative_path.is_empty() {
        "index.html"
    } else {
        relative_path
    };
    if relative_path.split('/').any(|seg| seg == "..") {
        return respond(request, 400, "bad path");
    }
    let path = build_wasm_dir.join(relative_path);
    if !path.is_file() {
        return respond(request, 404, "not found");
    }

    let content_type = content_type_for(&path);
    let content_type_header = tiny_http::Header::from_bytes("Content-Type", content_type)
        .map_err(|_| Error::msg("invalid content-type header"))?;

    // Inject the live-reload <script> into HTML responses before </body>.
    if content_type.starts_with("text/html") {
        let mut html = fs::read_to_string(&path)?;
        if let Some(idx) = html.rfind("</body>") {
            html.insert_str(idx, RELOAD_SCRIPT);
        } else {
            html.push_str(RELOAD_SCRIPT);
        }
        let response = tiny_http::Response::from_string(html).with_header(content_type_header);
        request.respond(response)?;
        return Ok(());
    }

    let file = File::open(&path)?;
    let response = tiny_http::Response::from_file(file).with_header(content_type_header);
    request.respond(response)?;
    Ok(())
}

fn handle_reload(request: tiny_http::Request, subscribers: Subscribers) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    subscribers
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(tx);
    let signaled = rx.recv_timeout(LONG_POLL_TIMEOUT).is_ok();
    respond(request, if signaled { 200 } else { 204 }, "")
}

fn respond(request: tiny_http::Request, status: u16, body: &str) -> Result<()> {
    let response = tiny_http::Response::from_string(body.to_string()).with_status_code(status);
    request.respond(response)?;
    Ok(())
}

fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("css") => "text/css; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("json") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}
