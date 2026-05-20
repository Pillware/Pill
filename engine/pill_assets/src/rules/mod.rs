use crate::Rule;

pub mod hlsl_to_wgsl;
pub mod obj_to_runtime_mesh;
pub mod png_to_rtex;

pub use hlsl_to_wgsl::HlslToWgsl;
pub use obj_to_runtime_mesh::ObjToRuntimeMesh;
pub use png_to_rtex::PngToRtex;

/// Built-in rule set used by both the cargo build-script and the launcher.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(HlslToWgsl),
        Box::new(PngToRtex),
        Box::new(ObjToRuntimeMesh),
    ]
}
