// This file implements the "docs" action: generating rustdoc for engine crates.
//
// Responsibilities:
// - Generates two doc sets: project_dev (public API) and engine_dev (private items).
// - Temporarily rewrites the Cube example's Cargo.toml and pill_native's Cargo.toml
//   to point at absolute engine paths so cargo doc resolves dependencies correctly.
// - Pre-renders PlantUML diagrams before doc generation.

use anyhow::{bail, Context, Error, Result};
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::{env, fs, path::PathBuf, process::Command};

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::output_path_flag;
use crate::utils::files::modify_file;
use crate::utils::paths::*;
use crate::utils::plantuml::render_puml_for_crate;

#[derive(Debug)]
pub(crate) struct Docs;

impl Action for Docs {
    fn name(&self) -> &'static str {
        "docs"
    }

    fn description(&self) -> &'static str {
        "Generate rustdoc for engine crates"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(output_path_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let out = PathBuf::from(matches.value_of("output-path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        generate_docs(&out)
    }
}

/// Generate rustdoc for engine crates into two sets: project_dev and engine_dev.
/// Temporarily rewrites Cargo.toml files so cargo doc resolves path dependencies.
/// Restores original manifests on exit even if generation fails.
///
/// WARNING: Not safe for concurrent use. If two `pill_launcher -a docs` processes run
/// simultaneously, they race on the same Cargo.toml files and may produce
/// incorrect documentation or leave manifests in a modified state.
pub(crate) fn generate_docs(output_directory_path: &PathBuf) -> Result<()> {
    // The Cube example serves as a workspace anchor so cargo doc can resolve deps.
    let cube_example_project_path = get_path(Location::EngineProjectRoot)
        .join("examples")
        .join("Cube");
    if !cube_example_project_path.exists() {
        return Err(Error::msg("Cannot find Cube project in examples directory"));
    }

    let cube_cargo_toml = cube_example_project_path.join("Cargo.toml");
    let native_cargo_toml = get_path(Location::PillNativeCrate).join("Cargo.toml");

    // Snapshot original manifests so we can restore them on exit.
    let original_cube = fs::read_to_string(&cube_cargo_toml)
        .with_context(|| format!("Failed to read {}", cube_cargo_toml.display()))?;
    let original_native = fs::read_to_string(&native_cargo_toml)
        .with_context(|| format!("Failed to read {}", native_cargo_toml.display()))?;

    // 1. Point the Cube example's Cargo.toml at the absolute engine path
    // so cargo doc can resolve the pill_engine dependency.  Strip the
    // workspace = "NO_PATH" line entirely - the Cube example isn't a
    // workspace member, and leaving a valid workspace path would cause
    // "package believes it's in a workspace when it's not" errors.
    modify_file(
        &cube_cargo_toml,
        &cube_cargo_toml,
        |line: String| -> String {
            if line.trim_start().starts_with("workspace") {
                return String::new(); // remove - not a workspace member
            }
            if line.contains("pill_engine") {
                return format!(
                    "pill_engine = {{path = \"{}\", features = [\"all\"]}}",
                    get_path(Location::PillEngineCrate)
                        .to_str()
                        .unwrap()
                        .replace("\\", "/")
                );
            }
            line
        },
    )?;

    // 2. Point pill_native's Cargo.toml at the Cube example so it can be
    // used as a workspace anchor for doc generation.
    modify_file(
        &native_cargo_toml,
        &native_cargo_toml,
        |line: String| -> String {
            if line.contains("project") {
                return format!(
                    "project = {{path = \"{}\"}}",
                    cube_example_project_path
                        .to_string_lossy()
                        .replace("\\", "/")
                );
            }
            line
        },
    )?;

    // Run the main doc generation in a closure so manifests are always restored.
    let result = (|| -> Result<()> {
        // 3. Determine where to write the docs output. Defaults to ./docs/.
        let output_path = if output_directory_path.as_os_str() == "." {
            env::current_dir().context("Failed to get current directory")?
        } else {
            output_directory_path
                .absolutize()
                .context("Failed to absolutize output path")?
                .to_path_buf()
        };

        let docs_path = output_path.join("generated");

        // 4. Clean any previous doc output so stale files don't persist.
        if docs_path.exists() {
            fs::remove_dir_all(&docs_path).with_context(|| {
                format!("Cannot clear output directory: {}", docs_path.display())
            })?;
        }

        // Prepare two output directories: project_dev (public API) and engine_dev (all items).
        let output_project_dev_path = docs_path.join("project_dev");
        let output_engine_dev_path = docs_path.join("engine_dev");

        fs::create_dir_all(&docs_path)?;
        fs::create_dir_all(&output_project_dev_path)?;
        fs::create_dir_all(&output_engine_dev_path)?;

        let engine_crate_manifest_path = get_path(Location::PillEngineCrate).join("Cargo.toml");
        let full_engine_manifest_path = cube_example_project_path.join("Cargo.toml");

        // 5. Pre-render PlantUML diagrams so they appear in the generated docs.
        let pill_engine_dir = get_path(Location::PillEngineCrate);
        if let Err(e) = render_puml_for_crate(&pill_engine_dir) {
            eprintln!("Warning: skipping PlantUML render ({})", e);
        }

        // 6. Generate project_dev docs: public API surface (project + internal
        // features).  Features are enabled via the pill_engine dependency spec
        // in the Cube example's Cargo.toml (set above), not via --features on
        // the cargo doc CLI (which would apply to the wrong crate).
        let manifest = full_engine_manifest_path.to_string_lossy();
        let target = output_project_dev_path.to_string_lossy();
        let arguments = vec![
            "doc",
            "--no-deps",
            "--manifest-path",
            &*manifest,
            "--target-dir",
            &*target,
            "--release",
        ];
        let status = Command::new("cargo")
            .args(arguments)
            .status()
            .context("Failed to execute command for generating project dev docs")?;

        if !status.success() {
            bail!(
                "project_dev docs failed to generate (exit {:?})",
                status.code()
            );
        }
        println!("project_dev docs generated successfully!");

        // 7. Generate engine_dev docs: pill_core first (no dependencies), private items included.
        let core_crate_manifest_path = get_path(Location::PillCoreCrate).join("Cargo.toml");
        let manifest = core_crate_manifest_path.to_string_lossy();
        let target = output_engine_dev_path.to_string_lossy();
        let arguments = vec![
            "doc",
            "--no-deps",
            "--document-private-items",
            "--manifest-path",
            &*manifest,
            "--target-dir",
            &*target,
            "--release",
        ];
        let status = Command::new("cargo")
            .args(arguments)
            .status()
            .context("Failed to execute command for generating core dev docs")?;

        // Core docs are optional - non-fatal if they fail.
        if status.success() {
            println!("Core dev docs generated successfully!");
        }

        // Generate engine_dev docs: pill_engine with all features, private items included.
        let manifest = engine_crate_manifest_path.to_string_lossy();
        let target = output_engine_dev_path.to_string_lossy();
        let arguments = vec![
            "doc",
            "--no-deps",
            "--document-private-items",
            "--features",
            "all",
            "--manifest-path",
            &*manifest,
            "--target-dir",
            &*target,
            "--release",
        ];
        let status = Command::new("cargo")
            .args(arguments)
            .status()
            .context("Failed to execute command for generating engine dev docs")?;

        if !status.success() {
            bail!(
                "engine_dev docs failed to generate (exit {:?})",
                status.code()
            );
        }
        println!("engine_dev docs generated successfully!");

        Ok(())
    })();

    // Restore original manifests regardless of success/failure.
    fs::write(&cube_cargo_toml, &original_cube)
        .with_context(|| format!("Failed to restore {}", cube_cargo_toml.display()))?;
    fs::write(&native_cargo_toml, &original_native)
        .with_context(|| format!("Failed to restore {}", native_cargo_toml.display()))?;

    result?;

    Ok(())
}
