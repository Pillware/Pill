// This file contains the CLI entry point: a generic action dispatcher
// and shared CLI flag constructors and argument parsers used by multiple actions.
//
// Responsibilities:
// - Takes a list of Action trait objects.
// - Iterates over them, calling register() on each to build the full CLI.
// - Adds the common --action flag and -- game-args passthrough.
// - After parsing, dispatches to the matching action's run() method.

use crate::types::{BuildTarget, CompileMode};
use anyhow::{bail, Result};
use clap::{App, AppSettings, Arg, ArgMatches};

use crate::actions::Action;

/// Build the CLI from the provided actions, parse args, and dispatch.
pub(crate) fn run_app(actions: &[&dyn Action]) -> Result<()> {
    let mut app = App::new("PillLauncher")
        .about("Tool for managing Pill game projects")
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
    // game or cargo command (used by "run", "build", "cargo" actions).
    app = app.arg(
        Arg::with_name("game-args")
            .help("Arguments passed through to the game or cargo (use `--` to separate)")
            .multiple(true)
            .last(true)
            .allow_hyphen_values(true),
    );

    // Let each action register its own flags.
    for action in actions {
        app = action.register(app);
    }

    app = app.setting(AppSettings::TrailingVarArg);

    let matches = app.get_matches();

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

/// `-p` / `--path` — game project directory.
pub(crate) fn path_flag() -> Arg<'static, 'static> {
    Arg::with_name("path")
        .short("p")
        .long("path")
        .takes_value(true)
        .default_value(".")
        .help("Path to the game project")
}

/// `-c` / `--compile-mode` — debug, release, or hot-reload.
pub(crate) fn compile_mode_flag() -> Arg<'static, 'static> {
    Arg::with_name("compile-mode")
        .short("c")
        .long("compile-mode")
        .takes_value(true)
        .default_value(DEFAULT_COMPILE_MODE)
        .possible_values(&["debug", "release", "hot-reload"])
        .help("Build profile: debug, release, or hot-reload")
}

/// `-o` / `--output-path` — where to place build artifacts.
pub(crate) fn output_path_flag() -> Arg<'static, 'static> {
    Arg::with_name("output-path")
        .short("o")
        .long("output-path")
        .takes_value(true)
        .default_value(".")
        .help("Build output directory")
}

/// `-t` / `--target` — native or web (WASM).
pub(crate) fn target_flag() -> Arg<'static, 'static> {
    Arg::with_name("target")
        .short("t")
        .long("target")
        .takes_value(true)
        .default_value("native")
        .possible_values(&["native", "web"])
        .help("Build target: native executable or WASM+WebGPU")
}

/// `--clean` — force-rebuild cooked assets before building.
pub(crate) fn clean_flag() -> Arg<'static, 'static> {
    Arg::with_name("clean")
        .long("clean")
        .help("Delete all cooked asset files and rebuild from source")
}

/// `--features` — comma-separated Cargo features for pill_game.
pub(crate) fn features_flag() -> Arg<'static, 'static> {
    Arg::with_name("features")
        .long("features")
        .takes_value(true)
        .help("Cargo features to enable for pill_game (comma-separated)")
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
