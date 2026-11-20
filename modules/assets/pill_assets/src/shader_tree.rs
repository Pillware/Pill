//! Cooks a crate's shader directory by convention.
//!
//! # Responsibilities
//!
//! - Find the directories under a shader root that hold stage sources, and the
//!   ones that hold headers ([`shader_tree_layout`]).
//! - Cook every stale source in one call ([`cook_shader_tree`]), with every
//!   header directory on the include path.
//!
//! # Design
//!
//! A crate's shaders are cooked in two places: in its build script, before the
//! crate compiles, and in the development host's shader watcher, while the
//! crate runs. Both call this module with the same root. So a cook at runtime
//! compiles the same files, and finds the same headers, as the build did.
//!
//! # Convention
//!
//! - A directory named `include` holds headers. Every one in the tree is on
//!   every source's include path, in sorted path order.
//! - Any other directory that directly holds `.hlsl` files is a source
//!   directory. Its sources are cooked to `.wgsl` next to them, and the stage
//!   comes from the file name (see [`HlslToWgsl`]).
//! - In any other directory, only subdirectories matter: nothing it holds is
//!   cooked.

// Standard library
use std::fs;
use std::path::{Path, PathBuf};

// Current crate
use crate::{walk_files, CookError, HlslToWgsl, Pipeline, Rule};

// =============================================================================
// Constants
// =============================================================================

/// Name of a directory holding headers rather than stage sources.
const INCLUDE_DIRECTORY_NAME: &str = "include";

/// Extension of the stage source files this module looks for.
const SOURCE_EXTENSION: &str = "hlsl";

// =============================================================================
// Types
// =============================================================================

/// Where a shader tree keeps its sources and its headers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ShaderTreeLayout {
    /// Directories that directly hold stage sources, sorted.
    pub source_directories: Vec<PathBuf>,
    /// Directories named `include`, sorted.
    pub include_directories: Vec<PathBuf>,
}

impl ShaderTreeLayout {
    /// Returns every directory that holds a source or a header, which is where
    /// an edit can change what the cook produces. A build script reports them
    /// to cargo, and the development host watches the tree that contains them.
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        let mut directories = self.source_directories.clone();
        directories.extend(self.include_directories.iter().cloned());
        directories.sort();
        directories
    }
}

/// What one [`cook_shader_tree`] found and did.
#[derive(Debug, Default)]
pub struct ShaderTreeReport {
    /// The layout the cook ran against.
    pub layout: ShaderTreeLayout,
    /// Every stage source the cook matched, whether or not it was stale.
    pub discovered_sources: Vec<PathBuf>,
    /// Every file inside an `include` directory, of any extension.
    pub header_files: Vec<PathBuf>,
    /// Outputs this run rebuilt, whether or not their text changed.
    pub rebuilt_outputs: Vec<PathBuf>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Finds the directories under `root` that hold stage sources and the ones
/// that hold headers.
///
/// A missing `root` simply has no such directories, and that is not an error,
/// so a crate without shaders can still run its build script.
///
/// # Errors
///
/// Returns [`CookError::Io`] when a directory cannot be read.
pub fn shader_tree_layout(root: &Path) -> Result<ShaderTreeLayout, CookError> {
    let mut layout = ShaderTreeLayout::default();
    collect_layout(root, &mut layout)?;
    layout.source_directories.sort();
    layout.include_directories.sort();
    Ok(layout)
}

/// Cooks every stale stage source under `root`, a crate's `shaders/`
/// directory.
///
/// A source is stale when its `.wgsl` output is missing, or older than the
/// source, or older than any header. So a header edit re-cooks every source
/// that could include it, and a tree nobody edited cooks nothing.
///
/// # Errors
///
/// Returns the first [`CookError`]: a source `slangc` rejects, a missing
/// `slangc`, or an I/O failure. Outputs cooked before the failure stay in
/// place, so a caller that keeps the previous shaders on failure still finds
/// them on disk.
pub fn cook_shader_tree(root: &Path) -> Result<ShaderTreeReport, CookError> {
    // Step 1: Find the layout and list every header file.
    let layout = shader_tree_layout(root)?;
    let mut report = ShaderTreeReport::default();
    for include_directory in &layout.include_directories {
        report.header_files.extend(walk_files(include_directory)?);
    }

    // Step 2: Cook each source directory, with every header directory in the
    // tree on the include path, in sorted path order.
    for source_directory in &layout.source_directories {
        let mut rule = HlslToWgsl::flat_tree();
        for include_directory in &layout.include_directories {
            rule = rule.with_include_directory(include_directory);
        }
        let rules: Vec<Box<dyn Rule>> = vec![Box::new(rule)];
        let stats = Pipeline::with_rules(source_directory, rules).run()?;
        report.discovered_sources.extend(stats.discovered_inputs);
        report.rebuilt_outputs.extend(stats.rebuilt_outputs);
    }
    report.layout = layout;
    Ok(report)
}

/// Walks `directory` and every directory below it, recording the source
/// directories and header directories it finds in `layout`.
fn collect_layout(directory: &Path, layout: &mut ShaderTreeLayout) -> Result<(), CookError> {
    // Step 1: Read the directory's entries, remembering its subdirectories and
    // whether it directly holds `.hlsl` files. A missing directory is not an
    // error: there is nothing to record for it.
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(CookError::Io {
                path: directory.to_owned(),
                source,
            })
        }
    };

    let mut holds_sources = false;
    let mut subdirectories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| CookError::Io {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            subdirectories.push(path);
        } else if path
            .extension()
            .is_some_and(|extension| extension == SOURCE_EXTENSION)
        {
            holds_sources = true;
        }
    }

    // Step 2: Record what this directory is, then walk its subdirectories. A
    // directory named `include` is a header directory whatever it contains,
    // and the walk does not go inside it. `walk_files` reads those headers,
    // and they are never cooked.
    if directory
        .file_name()
        .is_some_and(|name| name == INCLUDE_DIRECTORY_NAME)
    {
        layout.include_directories.push(directory.to_owned());
        return Ok(());
    }
    if holds_sources {
        layout.source_directories.push(directory.to_owned());
    }
    for subdirectory in subdirectories {
        collect_layout(&subdirectory, layout)?;
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a scratch tree shaped like a renderer's `shaders/` directory: a
    /// group whose sources sit directly in it, a group with a `shaders/` level
    /// below it, a shared `include/`, and an empty group.
    fn create_scratch_tree(scenario_name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join("pill_assets_shader_tree")
            .join(format!("{}_{scenario_name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for directory in ["common/include", "pipeline/shaders", "empty_group"] {
            fs::create_dir_all(root.join(directory)).expect("scratch directory");
        }
        fs::write(root.join("common/default_vertex.hlsl"), "").expect("source");
        fs::write(root.join("common/include/shared.hlsl"), "").expect("header");
        fs::write(root.join("pipeline/shaders/lit_fragment.hlsl"), "").expect("source");
        fs::write(root.join("pipeline/readme.txt"), "").expect("other file");
        root
    }

    #[test]
    fn sources_and_headers_are_found_by_convention() {
        let root = create_scratch_tree("layout");

        let layout = shader_tree_layout(&root).expect("readable");

        assert_eq!(
            layout.source_directories,
            vec![root.join("common"), root.join("pipeline/shaders")]
        );
        assert_eq!(
            layout.include_directories,
            vec![root.join("common/include")]
        );
        assert_eq!(
            layout.watched_directories(),
            vec![
                root.join("common"),
                root.join("common/include"),
                root.join("pipeline/shaders"),
            ]
        );
    }

    #[test]
    fn a_missing_root_has_an_empty_layout() {
        let root = std::env::temp_dir().join("pill_assets_shader_tree_that_does_not_exist");

        assert_eq!(
            shader_tree_layout(&root).expect("not an error"),
            ShaderTreeLayout::default()
        );
    }
}
