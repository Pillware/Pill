//! This file implements the "build" action: compile a native or WASM project.
//!
//! Responsibilities:
//! - Parses CLI flags (shared with the "run" action via `utils::cli`).
//! - Delegates to `native_target::build_project()` for native builds.
//! - Delegates to `wasm_target::build_project()` for WASM targets.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{
    add_build_flags, add_path_flag, max_wasm_size_flag, parse_build_target, parse_compile_mode,
    wasm_analyze_flag,
};
use crate::utils::common::{clean_build_cache, print_build_summary};
use crate::utils::native_target::build_project;
use crate::utils::paths::get_project_build_path;
use crate::utils::wasm_target;

/// The `build` subcommand: compile a project to native or WASM.
#[derive(Debug)]
pub(crate) struct Build;

impl Action for Build {
    fn name(&self) -> &'static str {
        "build"
    }

    fn description(&self) -> &'static str {
        "Compile a project (native or WASM)"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        let application = add_path_flag(application);
        let application = add_build_flags(application);
        application
            .arg(max_wasm_size_flag())
            .arg(wasm_analyze_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        // 1. Parse CLI flags.
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let additional_features = matches.value_of("additional-features");
        let headless = matches.is_present("headless");
        let clean = matches.is_present("clean");
        let maximum_wasm_size: Option<u64> = matches
            .value_of("max-wasm-size")
            .and_then(|size| size.parse().ok());
        let wasm_analyze = matches.is_present("wasm-analyze");

        // 2. Print a summary of what we're about to build.
        print_build_summary(
            "Building project",
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

        // 4. Dispatch to the appropriate build backend.
        match target {
            BuildTarget::Native => {
                let output_directory =
                    PathBuf::from(matches.value_of("output-path").unwrap_or("."));
                let output_directory =
                    get_project_build_path(&path, &output_directory, &compile_mode)?;
                build_project(
                    &path,
                    &output_directory,
                    &compile_mode,
                    additional_features,
                    headless,
                )?;
            }
            BuildTarget::Web => {
                if headless {
                    println!("Note: `--headless` is ignored for WASM builds; headless mode only applies to native targets.");
                }
                if matches.occurrences_of("output-path") > 0 {
                    println!("Note: `-o/--output-path` is ignored with `-t wasm`; output is fixed at <project>/build/wasm/");
                }
                wasm_target::build_project(&path, &compile_mode, maximum_wasm_size, wasm_analyze)?;

                let build_wasm_directory = path.join("build").join("wasm");
                println!("Done! Serve with:");
                println!("  PillLauncher run -t web -p {:?}", &path);
                println!(
                    "  (or any static server pointed at {:?})",
                    build_wasm_directory
                );
            }
        }
        Ok(())
    }
}
