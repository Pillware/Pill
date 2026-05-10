use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::Rule;

/// HLSL → WGSL via the `slangc` CLI. Stage and entry-point inferred from
/// filename suffix:
///   `*_vertex.hlsl`   → `-stage vertex   -entry vs_main`
///   `*_fragment.hlsl` → `-stage fragment -entry fs_main`
///
/// Other suffixes fail fast with a clear message; add a new naming convention
/// (or a richer rule) before introducing compute / mesh stages.
pub struct HlslToWgsl;

impl Rule for HlslToWgsl {
    fn name(&self) -> &'static str {
        "hlsl_to_wgsl"
    }

    fn input_glob(&self) -> &'static str {
        // Top-level only; `shaders/include/*.hlsl` are header files (#include'd).
        "shaders/*.hlsl"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("wgsl")
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let (entry, stage) = infer_stage(input)?;

        let result = Command::new("slangc")
            .args([
                input.to_str().context("input path is not UTF-8")?,
                "-target",
                "wgsl",
                "-entry",
                entry,
                "-stage",
                stage,
                // -O0 + -g preserve intermediate variable names so the emitted
                // WGSL is debuggable in browser devtools.
                "-O0",
                "-g",
                "-o",
                output.to_str().context("output path is not UTF-8")?,
            ])
            .output();

        let out = match result {
            Ok(out) => out,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "slangc not found on PATH. Install Slang from https://github.com/shader-slang/slang/releases and add slangc to PATH."
            ),
            Err(e) => return Err(e).context("failed to spawn slangc"),
        };

        if !out.status.success() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!(
                "slangc exited {:?} for {input:?}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
                out.status.code()
            );
        }

        Ok(())
    }
}

fn infer_stage(input: &Path) -> Result<(&'static str, &'static str)> {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .with_context(|| format!("can't read filename stem of {input:?}"))?;

    if stem.ends_with("_vertex") {
        Ok(("vs_main", "vertex"))
    } else if stem.ends_with("_fragment") {
        Ok(("fs_main", "fragment"))
    } else {
        bail!(
            "{input:?}: can't infer shader stage from filename. Expected suffix `_vertex` or `_fragment`."
        )
    }
}
