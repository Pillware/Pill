//! This file contains the CLI entry point: a generic action dispatcher
//! and shared CLI flag constructors and argument parsers used by multiple actions.
//!
//! Responsibilities:
//! - Takes a list of Action trait objects.
//! - Iterates over them, calling register() on each to build the full CLI.
//! - Builds subcommands from actions and dispatches to the matching action's
//!   run() method.
//! - After parsing, dispatches to the matching action's run() method.

use crate::types::{BuildTarget, CompileMode};
use anyhow::{bail, Result};
use clap::{App, AppSettings, Arg, ArgMatches};

use crate::actions::Action;

/// Build the CLI from the provided actions, parse args, and dispatch.
///
/// Each action becomes a clap subcommand (e.g. `PillLauncher run -p . -c release`),
/// giving context-sensitive `--help` per action.  The old `-a` / `--action` flag
/// is no longer used.
pub(crate) fn run_app(actions: &[&dyn Action]) -> Result<()> {
    let mut application = App::new("PillLauncher")
        .about("Tool for managing Pill project projects")
        .version(env!("CARGO_PKG_VERSION"))
        .setting(AppSettings::SubcommandRequiredElseHelp)
        .setting(AppSettings::TrailingVarArg);

    // Each action registers its own flags on its own subcommand.
    for action in actions {
        let sub = App::new(action.name()).about(action.description());
        let sub = action.register(sub);
        application = application.subcommand(sub);
    }

    // Use get_matches_safe so we don't exit() inside the library - important
    // for unit tests that call run_app directly.  We must handle
    // HelpDisplayed / VersionDisplayed ourselves because clap returns them
    // as errors even though the user expects exit code 0.
    let matches = match application.get_matches_safe() {
        Ok(m) => m,
        Err(e) => {
            // get_matches_safe() does NOT print to stdout/stderr - it returns
            // an Error with the message.  For --help / --version, the error's
            // Display impl has the help/version text; we must print it ourselves.
            use clap::ErrorKind;
            match e.kind {
                ErrorKind::HelpDisplayed | ErrorKind::VersionDisplayed => {
                    print!("{e}");
                    return Ok(());
                }
                _ => return Err(anyhow::anyhow!("{}", e)),
            }
        }
    };

    // Find the matched subcommand and dispatch.
    for action in actions {
        if let Some(sub_matches) = matches.subcommand_matches(action.name()) {
            return action.run(sub_matches);
        }
    }

    // Should not reach here because SubcommandRequiredElseHelp handles it,
    // but fall back to showing help.
    bail!("No valid action specified. Use --help to see available actions.")
}

// -- Named defaults (single source of truth) -------------------------------

pub(crate) const DEFAULT_COMPILE_MODE: &str = "debug";

// -- Shared flag builders ---------------------------------------------------

/// `-p` / `--path` - project directory.
pub(crate) fn path_flag() -> Arg<'static, 'static> {
    Arg::with_name("path")
        .short("p")
        .long("path")
        .takes_value(true)
        .default_value(".")
        .help("Path to the project")
}

/// `-c` / `--compile-mode` - debug, release, or hot-reload.
pub(crate) fn compile_mode_flag() -> Arg<'static, 'static> {
    Arg::with_name("compile-mode")
        .short("c")
        .long("compile-mode")
        .takes_value(true)
        .default_value(DEFAULT_COMPILE_MODE)
        .possible_values(&["debug", "release", "hot-reload"])
        .help("Build profile: debug, release, or hot-reload")
}

/// `-o` / `--output-path` - where to place build artifacts.
pub(crate) fn output_path_flag() -> Arg<'static, 'static> {
    Arg::with_name("output-path")
        .short("o")
        .long("output-path")
        .takes_value(true)
        .default_value(".")
        .help("Build output directory")
}

/// `-t` / `--target` - native or web (WASM).
pub(crate) fn target_flag() -> Arg<'static, 'static> {
    Arg::with_name("target")
        .short("t")
        .long("target")
        .takes_value(true)
        .default_value("native")
        .possible_values(&["native", "web"])
        .help("Build target: native executable or WASM+WebGPU")
}

/// `--clean` - force-rebuild cooked assets before building.
pub(crate) fn clean_flag() -> Arg<'static, 'static> {
    Arg::with_name("clean")
        .long("clean")
        .help("Delete all cooked asset files and rebuild from source")
}

/// `--additional-features` - comma-separated Cargo features for the project.
pub(crate) fn features_flag() -> Arg<'static, 'static> {
    Arg::with_name("additional-features")
        .long("additional-features")
        .takes_value(true)
        .help("Cargo features to enable for project (comma-separated)")
}

/// `--wasm-port` - port for the WASM dev server.
pub(crate) fn wasm_port_flag() -> Arg<'static, 'static> {
    Arg::with_name("wasm-port")
        .long("wasm-port")
        .takes_value(true)
        .default_value("8080")
        .help("Port for the WASM dev server")
}

/// `--max-wasm-size` - maximum WASM binary size in KB.
pub(crate) fn max_wasm_size_flag() -> Arg<'static, 'static> {
    Arg::with_name("max-wasm-size")
        .long("max-wasm-size")
        .takes_value(true)
        .help("Maximum WASM binary size in KB")
}

/// `--wasm-analyze` - run twiggy size analysis after WASM release build.
pub(crate) fn wasm_analyze_flag() -> Arg<'static, 'static> {
    Arg::with_name("wasm-analyze")
        .long("wasm-analyze")
        .help("Run twiggy size breakdown after WASM release build")
}

/// `--` passthrough for project/cargo arguments.
pub(crate) fn project_args_flag() -> Arg<'static, 'static> {
    Arg::with_name("project-args")
        .help("Arguments passed through to the project or cargo (use `--` to separate)")
        .multiple(true)
        .last(true)
        .allow_hyphen_values(true)
}

/// `--headless` - build/run without a window (native target only).
pub(crate) fn headless_flag() -> Arg<'static, 'static> {
    Arg::with_name("headless")
        .long("headless")
        .help("Build and run without a window (native only, for benchmarks/CI)")
}

// -- Flag-group helpers (so actions only register what they use) ------------

/// Add `-p` / `--path` to an app/subcommand.
pub(crate) fn add_path_flag(application: App<'static, 'static>) -> App<'static, 'static> {
    application.arg(path_flag())
}

/// Add build-related flags: `-c`, `-t`, `-o`, `--clean`, `--features`, `--headless`.
pub(crate) fn add_build_flags(application: App<'static, 'static>) -> App<'static, 'static> {
    application
        .arg(compile_mode_flag())
        .arg(target_flag())
        .arg(output_path_flag())
        .arg(clean_flag())
        .arg(features_flag())
        .arg(headless_flag())
}

// -- Shared parsers ---------------------------------------------------------

/// Extract CompileMode from parsed CLI matches.
pub(crate) fn parse_compile_mode(matches: &ArgMatches) -> CompileMode {
    match matches
        .value_of("compile-mode")
        .unwrap_or(DEFAULT_COMPILE_MODE)
    {
        "release" => CompileMode::Release,
        "hot-reload" => CompileMode::HotReload,
        _ => CompileMode::Debug,
    }
}

/// Extract BuildTarget from parsed CLI matches.
pub(crate) fn parse_build_target(matches: &ArgMatches) -> BuildTarget {
    match matches.value_of("target").unwrap_or("native") {
        "web" => BuildTarget::Web,
        _ => BuildTarget::Native,
    }
}
