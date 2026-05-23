use crate::Rule;

pub mod hlsl_to_wgsl;
pub mod obj_to_cooked_mesh;
pub mod png_to_cooked_tex;

pub use hlsl_to_wgsl::HlslToWgsl;
pub use obj_to_cooked_mesh::ObjToCookedMesh;
pub use png_to_cooked_tex::PngToCookedTex;

/// Built-in rule set used by both the cargo build-script and the launcher.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(HlslToWgsl),
        Box::new(PngToCookedTex),
        Box::new(ObjToCookedMesh),
    ]
}
