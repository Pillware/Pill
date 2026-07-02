//! This file implements the "assets" action: run the asset pipeline.
//!
//! Responsibilities:
//! - Registers the `assets` subcommand with `-p`/`--path` and `--clean` flags.
//! - Delegates to `utils::assets::run_asset_pipeline` to process raw assets
//!   (models, textures, shaders) into cooked formats.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::utils::assets::run_asset_pipeline;
use crate::utils::cli::{clean_flag, path_flag};

#[derive(Debug)]
pub(crate) struct Assets;

impl Action for Assets {
    fn name(&self) -> &'static str {
        "assets"
    }

    fn description(&self) -> &'static str {
        "Run the asset pipeline (raw → cooked assets)"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        application.arg(path_flag()).arg(clean_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let clean = matches.is_present("clean");
        // Run the pipeline on the project's res/ directory.
        // When --clean is set, previously cooked files are deleted first.
        run_asset_pipeline(&path.join("res"), clean)
    }
}
