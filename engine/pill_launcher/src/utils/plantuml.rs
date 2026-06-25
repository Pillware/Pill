// PlantUML diagram rendering for crate documentation.
//
// Renders *.puml files under <crate>/docs/uml/ into SVGs in <crate>/docs/uml_out/
// using the plantuml CLI. Gracefully skips if plantuml is not installed.

use anyhow::{bail, Context, Result};
use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// Render all *.puml files under <crate>/docs/uml/ into SVGs via the plantuml CLI.
/// Silently skips if plantuml is not installed or no .puml files are found.
pub(crate) fn render_puml_for_crate(crate_directory: &Path) -> Result<()> {
    // Look for .puml files under docs/uml/, render each as SVG into docs/uml_out/.
    let input_directory = crate_directory.join("docs").join("uml");
    let output_directory = crate_directory.join("docs").join("uml_out");

    if !input_directory.exists() {
        return Ok(());
    }
    fs::create_dir_all(&output_directory)?;

    // Collect all .puml input files.
    let mut inputs = Vec::new();
    for entry in fs::read_dir(&input_directory)
        .with_context(|| format!("Failed to read directory: {}", input_directory.display()))?
    {
        let path = entry?.path();
        if path.extension() == Some(OsStr::new("puml")) {
            inputs.push(path);
        }
    }

    if inputs.is_empty() {
        return Ok(());
    }

    // Check if plantuml CLI is available.
    let plantuml_available = Command::new("plantuml")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();

    if !plantuml_available {
        println!("plantuml not found, skipping diagram rendering");
        return Ok(());
    }

    // Render each .puml file via plantuml -tsvg -pipe.
    for puml in &inputs {
        let svg_path = output_directory
            .join(puml.file_stem().unwrap())
            .with_extension("svg");

        let mut child = Command::new("plantuml")
            .arg("-tsvg")
            .arg("-pipe")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Spawn plantuml -pipe")?;

        {
            let mut stdin = child
                .stdin
                .take()
                .context("Failed to take plantuml stdin")?;
            let bytes =
                fs::read(puml).with_context(|| format!("Read PUML file {}", puml.display()))?;
            stdin.write_all(&bytes)?;
        }

        let output = child.wait_with_output().context("Wait plantuml")?;
        if !output.status.success() {
            bail!("plantuml failed with code {}", output.status);
        }
        fs::write(&svg_path, &output.stdout)
            .with_context(|| format!("Write SVG file {}", svg_path.display()))?;
    }

    Ok(())
}
