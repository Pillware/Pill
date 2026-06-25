// This file manages the asset pipeline (raw → cooked asset processing).

use anyhow::{Context, Result};
use std::path::Path;

use crate::utils::files::delete_cooked_resource_files_recursive;

/// Process raw assets (models, textures) into cooked formats via pill_assets.
/// If force_rebuild is true, deletes all previously cooked files first.
pub(crate) fn run_asset_pipeline(resources_directory: &Path, force_rebuild: bool) -> Result<()> {
    // Force-rebuild: delete all previously cooked assets before processing.
    if force_rebuild {
        println!(
            "Force-rebuild: deleting cooked files under {}...",
            resources_directory.display()
        );
        delete_cooked_resource_files_recursive(resources_directory)?;
    }
    let pipeline = pill_assets::Pipeline {
        root: resources_directory.to_path_buf(),
        rules: pill_assets::default_rules(),
    };
    let stats = pipeline.run().context("Asset pipeline operation failed")?;
    println!(
        "Assets: discovered={} rebuilt={} skipped={} (root: {})",
        stats.discovered.len(),
        stats.rebuilt.len(),
        stats.skipped.len(),
        resources_directory.display()
    );
    Ok(())
}
