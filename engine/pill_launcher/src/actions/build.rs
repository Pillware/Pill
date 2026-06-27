// This file implements the "build" action: compile a native or WASM project.
//
// Responsibilities:
// - Parses CLI flags (shared with the "run" action via native_target).
// - Delegates to native_target::build_project() for native builds.
// - Delegates to wasm_target::build_project() for WASM targets.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{parse_build_target, parse_compile_mode};
use crate::utils::native_target::{build_project, register_build_flags};
use crate::utils::paths::get_project_build_path;
use crate::utils::wasm;

#[derive(Debug)]
pub(crate) struct Build;

impl Action for Build {
    fn name(&self) -> &'static str {
        "build"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        register_build_flags(app)
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let features = matches.value_of("features");
        let clean = matches.is_present("clean");
        let maximum_wasm_size: Option<u64> = matches
            .value_of("max-wasm-size")
            .and_then(|s| s.parse().ok());

        if clean {
            crate::utils::assets::run_asset_pipeline(&path.join("res"), true)?;
        }

        match target {
            BuildTarget::Native => {
                let output_directory =
                    PathBuf::from(matches.value_of("output-path").unwrap_or("."));
                let output_directory =
                    get_project_build_path(&path, &output_directory, &compile_mode)?;
                build_project(&path, &output_directory, &compile_mode, features)?;
            }
            BuildTarget::Web => {
                if matches.occurrences_of("output-path") > 0 {
                    println!("Note: `-o/--output-path` is ignored with `-t wasm`; output is fixed at <project>/build/wasm/");
                }
                wasm_target::build_project(&path, &compile_mode, maximum_wasm_size)?;
            }
        }
        Ok(())
    }
}
