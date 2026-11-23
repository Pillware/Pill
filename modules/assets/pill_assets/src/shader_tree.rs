//! Cooks a crate's shader directory by convention.
//!
//! # Responsibilities
//!
//! - Find the directories under a shader root that hold stage sources, and the
//!   ones that hold headers ([`shader_tree_layout`]).
//! - Cook every stale source in one call ([`cook_shader_tree`]), with every
//!   header directory on the include path.
//! - Cook a project's tree with the header directories other trees ship
//!   ([`cook_shader_tree_with_include_directories`]), so a project shader can
//!   include a header a module ships.
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
use crate::{walk_files, CookError, HlslToSpirv, HlslToWgsl, Pipeline, Rule};

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
    cook_shader_tree_with_include_directories(root, &[])
}

/// Cooks every stale stage source under `root`, searching
/// `extra_include_directories` for the headers they include.
///
/// This is how a project's tree reaches a header a module ships: the caller
/// passes the `include` directories of the loaded modules' shader trees, so a
/// project shader can include a header a module ships by its name. The
/// extra directories are searched after the tree's own, in the order given, so
/// a project header shadows a module header of the same name. Their files
/// count as inputs, so editing one makes the sources that include it stale.
///
/// # Errors
///
/// Returns the first [`CookError`]: a source `slangc` rejects, a missing
/// `slangc`, or an I/O failure. Outputs cooked before the failure stay in
/// place, so a caller that keeps the previous shaders on failure still finds
/// them on disk.
pub fn cook_shader_tree_with_include_directories(
    root: &Path,
    extra_include_directories: &[PathBuf],
) -> Result<ShaderTreeReport, CookError> {
    // Step 1: Find the layout and list every header file, including the ones
    // the caller's directories hold.
    let layout = shader_tree_layout(root)?;
    let mut report = ShaderTreeReport::default();
    for include_directory in &layout.include_directories {
        report.header_files.extend(walk_files(include_directory)?);
    }
    for include_directory in extra_include_directories {
        report.header_files.extend(walk_files(include_directory)?);
    }

    // Step 2: Cook each source directory, with the tree's own header
    // directories first and the caller's after them, in sorted path order.
    let mut include_directories = layout.include_directories.clone();
    for include_directory in extra_include_directories {
        if !include_directories.contains(include_directory) {
            include_directories.push(include_directory.clone());
        }
    }
    for source_directory in &layout.source_directories {
        let mut wgsl_rule = HlslToWgsl::flat_tree();
        let mut spirv_rule = HlslToSpirv::flat_tree();
        for include_directory in &include_directories {
            wgsl_rule = wgsl_rule.with_include_directory(include_directory);
            spirv_rule = spirv_rule.with_include_directory(include_directory);
        }
        // Both rules over the same directory: each claims the file names it can
        // compile, and the pipeline refuses a source neither claims.
        let rules: Vec<Box<dyn Rule>> = vec![Box::new(wgsl_rule), Box::new(spirv_rule)];
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

    /// Creates a project-shaped tree and a separate module-shaped header
    /// directory outside it. Returns the project root and the module's
    /// `include/` directory.
    fn create_cook_tree(
        scenario_name: &str,
        source: &str,
        project_header: Option<&str>,
        module_header: &str,
    ) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir()
            .join("pill_assets_shader_cook")
            .join(format!("{}_{scenario_name}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let project_root = base.join("project");
        let module_include = base.join("module/include");
        fs::create_dir_all(project_root.join("shaders")).expect("scratch directory");
        fs::create_dir_all(&module_include).expect("scratch directory");
        fs::write(project_root.join("shaders/probe_fragment.hlsl"), source).expect("source");
        if let Some(header) = project_header {
            fs::create_dir_all(project_root.join("shaders/include")).expect("scratch directory");
            fs::write(
                project_root.join("shaders/include/shared_tint.hlsl"),
                header,
            )
            .expect("header");
        }
        fs::write(module_include.join("shared_tint.hlsl"), module_header).expect("header");
        (project_root, module_include)
    }

    /// A source that includes a header from another tree cooks when that
    /// directory is on the include path.
    #[test]
    fn a_source_cooks_with_an_extra_include_directory() {
        let (project_root, module_include) = create_cook_tree(
            "extra_include",
            "#include \"shared_tint.hlsl\"\n\
             float4 fs_main() : SV_TARGET { return float4(shared_tint(), 1.0); }\n",
            None,
            "float3 shared_tint() { return float3(0.25, 0.5, 0.75); }\n",
        );

        let report = cook_shader_tree_with_include_directories(
            &project_root,
            std::slice::from_ref(&module_include),
        )
        .expect("the module header is on the include path");

        let output = project_root.join("shaders/probe_fragment.wgsl");
        assert!(
            output.is_file(),
            "the cook wrote the stage beside its source"
        );
        assert!(report.rebuilt_outputs.contains(&output));
    }

    /// A ray tracing source cooks to SPIR-V and a raster source in the same
    /// directory still cooks to WGSL, because the two rules split the tree by
    /// file name.
    #[test]
    fn a_ray_tracing_source_cooks_to_spirv_beside_the_raster_ones() {
        let base = std::env::temp_dir()
            .join("pill_assets_shader_cook")
            .join(format!("{}_stages", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("project");
        fs::create_dir_all(root.join("shaders")).expect("scratch directory");
        fs::write(
            root.join("shaders/probe_vertex.hlsl"),
            "float4 vs_main() : SV_POSITION { return float4(0.0, 0.0, 0.0, 1.0); }\n",
        )
        .expect("source");
        fs::write(
            root.join("shaders/shadow_raygen.hlsl"),
            "[shader(\"raygeneration\")]\nvoid raygen_main()\n{\n}\n",
        )
        .expect("source");

        let report = cook_shader_tree(&root).expect("both stages cook");

        let wgsl_output = root.join("shaders/probe_vertex.wgsl");
        let spirv_output = root.join("shaders/shadow_raygen.spv");
        assert!(wgsl_output.is_file(), "the raster stage cooks to WGSL");
        assert!(spirv_output.is_file(), "the ray stage cooks to SPIR-V");
        assert!(report.rebuilt_outputs.contains(&wgsl_output));
        assert!(report.rebuilt_outputs.contains(&spirv_output));
    }

    /// A name neither shader rule claims is refused, and the message names
    /// both lists so the author knows what to call the file.
    #[test]
    fn a_source_no_rule_claims_is_refused_naming_both_lists() {
        let base = std::env::temp_dir()
            .join("pill_assets_shader_cook")
            .join(format!("{}_unclaimed", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("project");
        fs::create_dir_all(root.join("shaders")).expect("scratch directory");
        fs::write(root.join("shaders/shadow.hlsl"), "void main()\n{\n}\n").expect("source");

        let error = cook_shader_tree(&root).expect_err("a name with no stage is refused");
        let message = error.to_string();
        assert!(
            message.contains("*_vertex.hlsl") && message.contains("*_raygen.hlsl"),
            "the refusal names both lists: {message}"
        );
    }

    /// Editing a header the caller supplied makes the source that includes it
    /// stale, so the next cook rebuilds it.
    #[test]
    fn an_extra_header_edit_makes_the_source_stale() {
        let (project_root, module_include) = create_cook_tree(
            "extra_header_staleness",
            "#include \"shared_tint.hlsl\"\n\
             float4 fs_main() : SV_TARGET { return float4(shared_tint(), 1.0); }\n",
            None,
            "float3 shared_tint() { return float3(0.25, 0.5, 0.75); }\n",
        );
        let output = project_root.join("shaders/probe_fragment.wgsl");
        cook_shader_tree_with_include_directories(
            &project_root,
            std::slice::from_ref(&module_include),
        )
        .expect("first cook");
        assert!(output.is_file());

        // The output is written before this, so the header is the newer file.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(
            module_include.join("shared_tint.hlsl"),
            "float3 shared_tint() { return float3(0.75, 0.5, 0.25); }\n",
        )
        .expect("header edit");

        let report = cook_shader_tree_with_include_directories(
            &project_root,
            std::slice::from_ref(&module_include),
        )
        .expect("second cook");

        assert!(
            report.rebuilt_outputs.contains(&output),
            "a header the caller supplied is an input of the source"
        );
    }

    /// The project's own header wins over a module header of the same name.
    #[test]
    fn the_project_header_shadows_a_module_header_of_the_same_name() {
        let (project_root, module_include) = create_cook_tree(
            "shadowing",
            "#include \"shared_tint.hlsl\"\n\
             float4 fs_main() : SV_TARGET { return float4(shared_tint(), 1.0); }\n",
            Some("float3 shared_tint() { return float3(0.25, 0.25, 0.25); }\n"),
            "float3 shared_tint() { return float3(0.75, 0.75, 0.75); }\n",
        );

        cook_shader_tree_with_include_directories(
            &project_root,
            std::slice::from_ref(&module_include),
        )
        .expect("both headers compile");

        let cooked = fs::read_to_string(project_root.join("shaders/probe_fragment.wgsl"))
            .expect("the cook wrote the stage");
        assert!(
            cooked.contains("0.25f") && !cooked.contains("0.75f"),
            "the project's own include directory is searched first"
        );
    }
}
