//! Dev HTTP server for `-a run -t wasm`. Serves <game>/build/wasm/ on
//! localhost:8080 and pushes long-poll reloads whenever any file in the dir
//! changes (wasm rebuilds trigger browser refresh).

use std::fs::{self, File};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{Error, Result};

use crate::{wasm_build, CompileMode};

const ADDR: &str = "127.0.0.1:8080";
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

pub fn run(game_project_directory_path: &Path, compile_mode: &CompileMode) -> Result<()> {
    wasm_build::build(game_project_directory_path, compile_mode, None)?;

    let build_wasm_dir = game_project_directory_path.join("build").join("wasm");
    let subscribers: Subscribers = Arc::new(Mutex::new(Vec::new()));

    spawn_watcher(build_wasm_dir.clone(), Arc::clone(&subscribers));

    let server = tiny_http::Server::http(ADDR).map_err(|e| Error::msg(e.to_string()))?;
    println!();
    println!("Serving {} at http://{}", build_wasm_dir.display(), ADDR);
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
            let mut subs = subscribers.lock().unwrap();
            for tx in subs.drain(..) {
                let _ = tx.send(());
            }
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

    if url_path == "/__reload" {
        return handle_reload(request, subscribers);
    }

    // Map URL to file under build_wasm_dir; reject `..` segments.
    let rel = url_path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.split('/').any(|seg| seg == "..") {
        return respond(request, 400, "bad path");
    }
    let path = build_wasm_dir.join(rel);
    if !path.is_file() {
        return respond(request, 404, "not found");
    }

    let content_type = content_type_for(&path);
    let ct_header = tiny_http::Header::from_bytes("Content-Type", content_type)
        .map_err(|_| Error::msg("invalid content-type header"))?;

    if content_type.starts_with("text/html") {
        let mut html = fs::read_to_string(&path)?;
        if let Some(idx) = html.rfind("</body>") {
            html.insert_str(idx, RELOAD_SCRIPT);
        } else {
            html.push_str(RELOAD_SCRIPT);
        }
        let resp = tiny_http::Response::from_string(html).with_header(ct_header);
        request.respond(resp)?;
        return Ok(());
    }

    let file = File::open(&path)?;
    let resp = tiny_http::Response::from_file(file).with_header(ct_header);
    request.respond(resp)?;
    Ok(())
}

fn handle_reload(request: tiny_http::Request, subscribers: Subscribers) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    subscribers.lock().unwrap().push(tx);
    let signaled = rx.recv_timeout(LONG_POLL_TIMEOUT).is_ok();
    respond(request, if signaled { 200 } else { 204 }, "")
}

fn respond(request: tiny_http::Request, status: u16, body: &str) -> Result<()> {
    let resp = tiny_http::Response::from_string(body.to_string()).with_status_code(status);
    request.respond(resp)?;
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
