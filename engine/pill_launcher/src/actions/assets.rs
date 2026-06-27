// This file implements the "assets" action: run the asset pipeline.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::utils::assets::run_asset_pipeline;

#[derive(Debug)]
pub(crate) struct Assets;

impl Action for Assets {
    fn name(&self) -> &'static str {
        "assets"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app // shared flags (--path, --clean) registered in cli::run_app()
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let clean = matches.is_present("clean");
        run_asset_pipeline(&path.join("res"), clean)
    }
}
