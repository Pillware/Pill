// This file implements the "run" action: build and launch a native project.
//
// Responsibilities:
// - Parses CLI flags (shared with the "build" action via native_target).
// - Delegates to native_target::run_project() for the actual build+run logic.
// - Supports WASM target by delegating to web_dev_server.

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
use crate::utils::common::print_build_summary;
use crate::utils::native_target::run_project;
use crate::utils::paths::get_project_build_path;
use crate::utils::web_dev_server;

#[derive(Debug)]
pub(crate) struct Run;

impl Action for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn description(&self) -> &'static str {
        "Build and launch a project"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        let app = add_path_flag(app);
        let app = add_build_flags(app);
        app.arg(wasm_port_flag()).arg(project_args_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let features = matches.value_of("features");
        let passthrough: Vec<String> = matches
            .values_of("project-args")
            .map(|v| v.map(String::from).collect())
            .unwrap_or_default();
        let clean = matches.is_present("clean");

        print_build_summary(
            "Running",
            &path,
            &target,
            &compile_mode,
            matches.value_of("output-path"),
            features,
        );

        if clean {
            crate::utils::assets::run_asset_pipeline(&path.join("res"), true)?;
        }

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
                    features,
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
