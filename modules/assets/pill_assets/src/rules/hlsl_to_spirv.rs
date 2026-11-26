//! HLSL/Slang source in, SPIR-V out, through the `slangc` CLI.
//!
//! # Responsibilities
//!
//! - Compile the stages WGSL cannot express: ray generation, miss, closest hit,
//!   any hit, intersection and compute.
//! - Infer the stage and the entry point from the file name, so a source that
//!   does not say which stage it is fails instead of guessing.
//! - Write `<name>.spv` beside the source, so the staleness check and a build
//!   script's `cargo:rerun-if-changed` lines work the way they do for WGSL.
//!
//! # Design
//!
//! This rule and [`super::HlslToWgsl`] split one shader tree between them: each
//! names the files it claims, and the pipeline refuses a source no rule claims.
//! A ray tracing source therefore sits in the same tree as the raster ones and
//! is cooked by the same walk - the module's `build.rs` and the development
//! host's shader watcher both run it.
//!
//! The target is `spirv_1_5`: ray tracing needs SPIR-V 1.4 or newer, and the
//! ray tracing extensions build on Vulkan 1.2, so 1.5 is the version every
//! device that can trace rays accepts. `-O0 -g` keep the debug names, as the
//! WGSL rule does.

// Standard library
use std::path::{Path, PathBuf};
use std::process::Command;

// Current crate
use crate::rules::hlsl_to_wgsl::{run_bounded_command, slangc_path, SLANGC_TIMEOUT};
use crate::{walk_files, CookError, Rule};

// =============================================================================
// Constants
// =============================================================================

/// The environment variable that names the compiler a cook runs, instead of
/// searching `PATH`.
const SLANGC_ENV: &str = "PILL_SLANGC";

/// The SPIR-V version the stages compile against.
const SPIRV_PROFILE: &str = "spirv_1_5";

/// The file suffix, the entry point and the `slangc` stage of each stage this
/// rule compiles.
///
/// Order does not matter: a file name ends with exactly one of these, because
/// no suffix is a suffix of another.
const STAGES: &[(&str, &str, &str)] = &[
    ("_raygen.hlsl", "raygen_main", "raygeneration"),
    ("_miss.hlsl", "miss_main", "miss"),
    ("_closesthit.hlsl", "closesthit_main", "closesthit"),
    ("_anyhit.hlsl", "anyhit_main", "anyhit"),
    ("_intersection.hlsl", "intersection_main", "intersection"),
    ("_compute.hlsl", "cs_main", "compute"),
];

/// The suffixes this rule accepts, as one line for an error message.
const STAGE_SUFFIXES: &str = "*_raygen.hlsl, *_miss.hlsl, *_closesthit.hlsl, *_anyhit.hlsl, \
     *_intersection.hlsl or *_compute.hlsl";

// =============================================================================
// HlslToSpirv
// =============================================================================

/// Compiles the ray tracing and compute stages to SPIR-V beside the source
/// through `slangc`.
///
/// Sources `#include` headers from an `include/` directory beside them, and
/// [`HlslToSpirv::with_include_directory`] adds directories a header is shared
/// from, exactly as the WGSL rule does. The compiler comes from `PILL_SLANGC`
/// or the first `slangc` on `PATH`.
pub struct HlslToSpirv {
    /// Glob that selects the sources, relative to the root.
    source_glob: &'static str,
    /// Directories handed to `slangc -I`, in the order they were added.
    include_directories: Vec<PathBuf>,
    pinned_slangc: Option<PathBuf>,
}

impl HlslToSpirv {
    /// Reads the sources of a `shaders/` directory below the root.
    pub fn new() -> Self {
        Self {
            source_glob: "shaders/*.hlsl",
            include_directories: Vec::new(),
            pinned_slangc: std::env::var_os(SLANGC_ENV).map(PathBuf::from),
        }
    }

    /// Reads the sources of the root itself, which is what a tree cooked per
    /// source directory needs.
    pub fn flat_tree() -> Self {
        Self {
            source_glob: "*.hlsl",
            include_directories: Vec::new(),
            pinned_slangc: std::env::var_os(SLANGC_ENV).map(PathBuf::from),
        }
    }

    /// Searches `directory` for the headers sources `#include`.
    pub fn with_include_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.include_directories.push(directory.into());
        self
    }
}

impl Default for HlslToSpirv {
    fn default() -> Self {
        Self::new()
    }
}

impl Rule for HlslToSpirv {
    fn name(&self) -> &'static str {
        "hlsl_to_spirv"
    }

    fn input_glob(&self) -> &'static str {
        self.source_glob
    }

    fn claims(&self, input: &Path) -> bool {
        stage_for(input).is_some()
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("spv")
    }

    fn extra_inputs(&self, input: &Path) -> Result<Vec<PathBuf>, CookError> {
        // Every header the sources may `#include`, for the same reason the WGSL
        // rule lists them: a header edit has to make its sources stale.
        let mut headers = Vec::new();
        if let Some(parent) = input.parent() {
            headers.extend(walk_files(&parent.join("include"))?);
        }
        for directory in &self.include_directories {
            headers.extend(walk_files(directory)?);
        }
        Ok(headers)
    }

    fn build(&self, input: &Path, output: &Path) -> Result<(), CookError> {
        // Step 1: Read the entry point and stage from the file name, and find
        // the compiler.
        let Some((entry_point, stage)) = stage_for(input) else {
            return Err(CookError::Rule {
                rule: self.name(),
                input: input.to_owned(),
                detail: format!(
                    "cannot infer a stage from {}: name it {STAGE_SUFFIXES}",
                    input.display()
                ),
            });
        };
        let compiler_path =
            slangc_path(self.pinned_slangc.as_deref()).map_err(|detail| CookError::Rule {
                rule: self.name(),
                input: input.to_owned(),
                detail,
            })?;

        // Step 2: Build the command line and run the compiler with the cook's
        // time limit.
        let mut slangc_command = Command::new(&compiler_path);
        slangc_command.arg(input).args([
            "-target",
            "spirv",
            "-profile",
            SPIRV_PROFILE,
            "-entry",
            entry_point,
            "-stage",
            stage,
        ]);
        for directory in &self.include_directories {
            slangc_command.arg("-I").arg(directory);
        }
        slangc_command.args(["-O0", "-g", "-o"]).arg(output);
        let cook_output = run_bounded_command(slangc_command, &compiler_path, SLANGC_TIMEOUT)
            .map_err(|detail| CookError::Rule {
                rule: self.name(),
                input: input.to_owned(),
                detail,
            })?;

        // Step 3: Report a failed compile with the compiler's own output.
        if !cook_output.status.success() {
            return Err(CookError::Rule {
                rule: self.name(),
                input: input.to_owned(),
                detail: format!(
                    "slangc exited with {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
                    cook_output.status.code(),
                    String::from_utf8_lossy(&cook_output.stdout).trim_end(),
                    String::from_utf8_lossy(&cook_output.stderr).trim_end(),
                ),
            });
        }
        Ok(())
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The entry point and `slangc` stage a file name declares, when it declares
/// one this rule compiles.
fn stage_for(file: &Path) -> Option<(&'static str, &'static str)> {
    let name = file.file_name()?.to_str()?;
    STAGES
        .iter()
        .find(|(suffix, _, _)| name.ends_with(suffix))
        .map(|(_, entry_point, stage)| (*entry_point, *stage))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_stage_suffix_names_its_entry_point_and_stage() {
        let cases = [
            ("shadow_raygen.hlsl", "raygen_main", "raygeneration"),
            ("shadow_miss.hlsl", "miss_main", "miss"),
            ("shadow_closesthit.hlsl", "closesthit_main", "closesthit"),
            ("gizmo_anyhit.hlsl", "anyhit_main", "anyhit"),
            (
                "gizmo_intersection.hlsl",
                "intersection_main",
                "intersection",
            ),
            ("denoise_compute.hlsl", "cs_main", "compute"),
        ];
        for (file, entry_point, stage) in cases {
            assert_eq!(
                stage_for(Path::new(file)),
                Some((entry_point, stage)),
                "{file}"
            );
        }
    }

    #[test]
    fn a_raster_or_unknown_name_is_not_claimed() {
        for file in [
            "default_vertex.hlsl",
            "pbr_fragment.hlsl",
            "shadow.hlsl",
            "common.hlsl",
        ] {
            assert!(stage_for(Path::new(file)).is_none(), "{file}");
            assert!(!HlslToSpirv::new().claims(Path::new(file)), "{file}");
        }
    }

    #[test]
    fn the_output_sits_beside_the_source_as_spirv() {
        let rule = HlslToSpirv::new();

        assert_eq!(
            rule.output_for(Path::new("shaders/shadow_raygen.hlsl")),
            PathBuf::from("shaders/shadow_raygen.spv")
        );
        assert_eq!(rule.input_glob(), "shaders/*.hlsl");
        assert_eq!(
            HlslToSpirv::flat_tree().input_glob(),
            "*.hlsl",
            "a tree cooked per directory reads the directory itself"
        );
    }
}
