// This file contains the CLI entry point: a generic action dispatcher
// and shared CLI flag constructors and argument parsers used by multiple actions.
//
// Responsibilities:
// - Takes a list of Action trait objects.
// - Iterates over them, calling register() on each to build the full CLI.
// - Adds the common --action flag and -- project-args passthrough.
// - After parsing, dispatches to the matching action's run() method.

use crate::types::{BuildTarget, CompileMode};
use anyhow::{bail, Result};
use clap::{App, AppSettings, Arg, ArgMatches};

use crate::actions::Action;

/// Build the CLI from the provided actions, parse args, and dispatch.
pub(crate) fn run_app(actions: &[&dyn Action]) -> Result<()> {
    let mut app = App::new("PillLauncher")
        .about("Tool for managing Pill project projects")
        .version(env!("CARGO_PKG_VERSION"));

    // Collect all valid action names for the --action possible_values list.
    let names: Vec<&str> = actions.iter().map(|a| a.name()).collect();

    // The --action flag is the only flag defined here; everything else comes
    // from each action's register() method.
    app = app.arg(
        Arg::with_name("action")
            .short("a")
            .long("action")
            .takes_value(true)
            .possible_values(&names)
            .required(true)
            .help("Specify the action to perform"),
    );

    // Common passthrough: trailing arguments after `--` are forwarded to the
    // project or cargo command (used by "run", "build", "cargo" actions).
    app = app.arg(
        Arg::with_name("project-args")
            .help("Arguments passed through to the project or cargo (use `--` to separate)")
            .multiple(true)
            .last(true)
            .allow_hyphen_values(true),
    );

    // Register shared flags once - individual actions must NOT re-register
    // these (clap v2 panics on duplicate arg names).
    app = app
        .arg(path_flag())
        .arg(compile_mode_flag())
        .arg(target_flag())
        .arg(output_path_flag())
        .arg(clean_flag())
        .arg(features_flag())
        .arg(
            Arg::with_name("max-wasm-size")
                .long("max-wasm-size")
                .takes_value(true)
                .help("Maximum WASM binary size in KB"),
        )
        .arg(
            Arg::with_name("wasm-port")
                .long("wasm-port")
                .takes_value(true)
                .default_value("8080")
                .help("Port for the WASM dev server"),
        );

    // Let each action register its own unique flags.
    for action in actions {
        app = action.register(app);
    }

    app = app.setting(AppSettings::TrailingVarArg);
    app = app.setting(AppSettings::ArgRequiredElseHelp);

    // Use get_matches_safe so we don't exit() inside the library - important
    // for unit tests that call run_app directly.  We must handle
    // HelpDisplayed / VersionDisplayed ourselves because clap returns them
    // as errors even though the user expects exit code 0.
    let matches = match app.get_matches_safe() {
        Ok(m) => m,
        Err(e) => {
            // get_matches_safe() does NOT print to stdout/stderr — it returns
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

    // Find the action named by --action and delegate.
    let action_name = matches.value_of("action").expect("Action is required");
    for action in actions {
        if action.name() == action_name {
            return action.run(&matches);
        }
    }

    bail!("Unknown action: {}", action_name)
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

/// `--features` - comma-separated Cargo features for project.
pub(crate) fn features_flag() -> Arg<'static, 'static> {
    Arg::with_name("features")
        .long("features")
        .takes_value(true)
        .help("Cargo features to enable for project (comma-separated)")
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
