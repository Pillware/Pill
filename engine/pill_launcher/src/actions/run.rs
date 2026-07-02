//! This file implements the "run" action: build and launch a project.
//!
//! Responsibilities:
//! - Parses CLI flags (shared with the "build" action via `utils::cli`).
//! - Delegates to `native_target::run_project()` for native build+run.
//! - Delegates to `web_dev_server::run()` for WASM dev server.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{
    add_build_flags, add_path_flag, parse_build_target, parse_compile_mode, project_args_flag,
    wasm_port_flag,
};
use crate::utils::common::{clean_build_cache, print_build_summary};
use crate::utils::native_target::run_project;
use crate::utils::paths::get_project_build_path;
use crate::utils::web_dev_server;

/// The `run` subcommand: build and launch a project (native or WASM dev server).
#[derive(Debug)]
pub(crate) struct Run;

impl Action for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn description(&self) -> &'static str {
        "Build and launch a project"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        let application = add_path_flag(application);
        let application = add_build_flags(application);
        application.arg(wasm_port_flag()).arg(project_args_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        // 1. Parse CLI flags.
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let additional_features = matches.value_of("additional-features");
        let passthrough: Vec<String> = matches
            .values_of("project-args")
            .map(|values| values.map(String::from).collect())
            .unwrap_or_default();
        let clean = matches.is_present("clean");

        // 2. Print a summary of what we're about to build and run.
        print_build_summary(
            "Running",
            &path,
            &target,
            &compile_mode,
            matches.value_of("output-path"),
            additional_features,
        );

        // 3. If --clean was requested, wipe the build cache and rebuild assets.
        if clean {
            clean_build_cache()?;
            crate::utils::assets::run_asset_pipeline(&path.join("res"), true)?;
        }

        // 4. Dispatch to the appropriate backend.
        match target {
            BuildTarget::Native => {
                let output_directory =
                    PathBuf::from(matches.value_of("output-path").unwrap_or("."));
                let output_directory =
                    get_project_build_path(&path, &output_directory, &compile_mode)?;
                run_project(
                    &path,
                    &output_directory,
                    &compile_mode,
                    &passthrough,
                    additional_features,
                    false,
                )?;
            }
            BuildTarget::Web => {
                let port: u16 = matches
                    .value_of("wasm-port")
                    .unwrap_or("8080")
                    .parse()
                    .unwrap_or(8080);
                web_dev_server::run(&path, &compile_mode, port)?;
            }
        }
        Ok(())
    }
}
