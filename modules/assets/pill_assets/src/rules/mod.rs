//! The cooking rules this crate ships.
//!
//! # Responsibilities
//!
//! - Re-export the rules that need no external crate.
//! - Assemble [`default_rules`], the set a new pipeline starts with.
//!
//! # Design
//!
//! Only rules that need no external crate live here. A rule that has to decode
//! a file format - mesh, texture - belongs to the crate that already has that
//! decoder. It is added to a [`crate::Pipeline`] through
//! [`crate::Pipeline::with_manifest`].

mod hlsl_to_spirv;
mod hlsl_to_wgsl;

// Current crate
use crate::Rule;
pub use hlsl_to_spirv::HlslToSpirv;
pub use hlsl_to_wgsl::HlslToWgsl;

/// Returns the rules a new pipeline starts with.
///
/// Shader cooking is always on here. The renderer embeds the cooked WGSL with
/// `include_str!` and the cooked SPIR-V with `include_bytes!`, so the files
/// must exist before it compiles. A build that skipped the cook would either
/// fail or use an old shader.
///
/// The two shader rules split one tree by file name: raster stages cook to
/// WGSL, the ray tracing and compute stages to SPIR-V.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(HlslToWgsl::new()), Box::new(HlslToSpirv::new())]
}
