use crate::Rule;

pub mod equirect_to_ibl;
pub mod glb_to_cooked_mesh;
pub mod hlsl_to_wgsl;
pub mod obj_to_cooked_mesh;
pub mod png_to_cooked_tex;
pub mod procedural_equirect;

pub use equirect_to_ibl::EquirectToIBL;
pub use glb_to_cooked_mesh::GlbToCookedMesh;
pub use hlsl_to_wgsl::HlslToWgsl;
pub use obj_to_cooked_mesh::ObjToCookedMesh;
pub use png_to_cooked_tex::PngToCookedTex;
pub use procedural_equirect::ProceduralEquirect;

/// Built-in rule set used by both the cargo build-script and the launcher.
pub fn default_rules() -> Vec<Box<dyn Rule>> {
    vec![
        Box::new(HlslToWgsl),
        Box::new(PngToCookedTex),
        Box::new(ProceduralEquirect), // generates *_equirect.cooked_tex from *.procedural_equirect
        Box::new(EquirectToIBL),      // generates IBL maps from *_equirect.cooked_tex
        Box::new(ObjToCookedMesh),
        Box::new(GlbToCookedMesh),
    ]
}
