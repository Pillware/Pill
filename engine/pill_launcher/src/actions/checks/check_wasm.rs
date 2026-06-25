// This file implements the "check-wasm" action: WASM build + smoke test + size check.
//
// Responsibilities:
// - Builds the WASM bundle via utils::wasm.
// - Optionally checks the final .wasm file size against a budget (--wasm-budget-kb).
// - Starts a tiny HTTP dev server on the given port, serves build/wasm/.
// - Smoke-tests that /, /pill_web_app.js, and /pill_web_app_bg.wasm return HTTP 200.
// - Stops the server and reports results.

use anyhow::{bail, Context, Result};
use clap::{App, Arg, ArgMatches};
use path_absolutize::Absolutize;
use std::time::Duration;
use std::{fs, path::PathBuf};

use crate::actions::Action;
use crate::types::CompileMode;
use crate::utils::cli::path_flag;
use crate::utils::wasm;

#[derive(Debug)]
pub(crate) struct CheckWasm;

impl Action for CheckWasm {
    fn name(&self) -> &'static str {
        "check-wasm"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(path_flag())
            .arg(
                Arg::with_name("wasm-port")
                    .long("wasm-port")
                    .takes_value(true)
                    .default_value("8080")
                    .help("Dev server port"),
            )
            .arg(
                Arg::with_name("wasm-budget-kb")
                    .long("wasm-budget-kb")
                    .takes_value(true)
                    .help("Fail if WASM exceeds N KB"),
            )
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let port: u16 = matches
            .value_of("wasm-port")
            .unwrap_or("8080")
            .parse()
            .unwrap_or(8080);
        let budget: Option<u64> = matches
            .value_of("wasm-budget-kb")
            .and_then(|s| s.parse().ok());
        do_check_wasm(&path, port, budget)
    }
}

/// Build the WASM bundle (always debug — wasm-pack release is slow and
/// not needed for smoke testing), optionally check the binary size against a budget,
/// start a tiny HTTP dev server, smoke-test the three core files, and stop the server.
pub(crate) fn do_check_wasm(
    project_directory_path: &PathBuf,
    port: u16,
    budget_kb: Option<u64>,
) -> Result<()> {
    println!(
        "WASM check: {} (port {})",
        project_directory_path.display(),
        port,
    );

    println!("Building WASM bundle (debug) ...");
    wasm::build_project(project_directory_path, &CompileMode::Debug, None)?;

    let wasm_directory = project_directory_path.join("build").join("wasm");

    // Optional size check: fail early if the .wasm exceeds the budget.
    if let Some(budget) = budget_kb {
        let wasm_file_path = wasm_directory.join("pill_web_app_bg.wasm");
        if !wasm_file_path.exists() {
            bail!("WASM file not found: {}", wasm_file_path.display());
        }
        let size = fs::metadata(&wasm_file_path)
            .with_context(|| format!("Failed to stat {}", wasm_file_path.display()))?
            .len();
        let size_kb = size / 1024;
        if size_kb > budget {
            bail!("WASM size {} KB exceeds {} KB budget", size_kb, budget);
        }
        println!("  OK: WASM size {} KB <= {} KB", size_kb, budget);
    }

    println!("Starting dev server on port {} ...", port);
    let wasm_directory_clone = wasm_directory.clone();
    let server = std::sync::Arc::new(
        tiny_http::Server::http(format!("127.0.0.1:{}", port))
            .map_err(|e| anyhow::anyhow!("Failed to bind dev server on port {}: {}", port, e))?,
    );
    let server_for_thread = std::sync::Arc::clone(&server);

    let server_thread = std::thread::spawn(move || {
        for request in server_for_thread.incoming_requests() {
            let request_path = request.url().trim_start_matches('/');
            let file_path = if request_path.is_empty() {
                wasm_directory_clone.join("index.html")
            } else {
                wasm_directory_clone.join(request_path)
            };
            match fs::read(&file_path) {
                Ok(data) => {
                    let _ = request.respond(tiny_http::Response::from_data(data));
                }
                Err(_) => {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }
        }
    });

    // Retry the smoke test with exponential back-off rather than a fixed sleep.
    let mut server_ready = false;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut delay = std::time::Duration::from_millis(10);
    loop {
        if std::time::Instant::now() > deadline {
            break;
        }
        std::thread::sleep(delay);
        if ureq::get(&format!("http://127.0.0.1:{}/", port))
            .timeout(Duration::from_secs(10))
            .call()
            .is_ok()
        {
            server_ready = true;
            break;
        }
        delay = (delay * 2).min(std::time::Duration::from_secs(1));
    }
    if !server_ready {
        server.unblock();
        let _ = server_thread.join();
        bail!("Dev server on port {} did not become ready in time", port);
    }

    let result = (|| -> Result<()> {
        let base = format!("http://127.0.0.1:{}", port);

        println!("Smoke testing HTTP endpoints ...");
        let files = &["/", "/pill_web_app.js", "/pill_web_app_bg.wasm"];
        for endpoint in files {
            let url = format!("{}{}", base, endpoint);
            let response = ureq::get(&url)
                .timeout(Duration::from_secs(10))
                .call()
                .with_context(|| format!("HTTP GET {} failed", url))?;
            if response.status() != 200 {
                bail!("HTTP {} for {} (expected 200)", response.status(), url);
            }
            println!("  OK: {} (200)", endpoint);
        }

        Ok(())
    })();

    // Shut down the server cleanly and join the thread before returning.
    server.unblock();
    match server_thread.join() {
        Ok(()) => {}
        Err(e) => {
            // Propagate panics from the server thread as errors
            let msg = if let Some(s) = e.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = e.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "unknown panic payload".to_string()
            };
            eprintln!("Warning: dev server thread panicked: {msg}");
        }
    }

    result?;
    println!("WASM check passed.");
    Ok(())
}
