use crate::resources::RendererTexture;

// COLD: construction-only helper; no allocations, just references bundled for factory use.
pub struct RendererMaterialTextures<'a> {
    pub albedo: &'a RendererTexture,
    pub normal: &'a RendererTexture,
    pub metallic_roughness: &'a RendererTexture,
    pub emissive: &'a RendererTexture,
}

#[repr(C)]
#[derive(Copy, Clone)]
// HOT(ish): per-material UBO bound during draws; keep compact (<= 64B ideal).
// Layout is 3x vec3+pads → 48 bytes total, 16-byte aligned to match WGSL/std140 rules.
pub struct RendererMaterialParamsStd140 {
    pub albedo: [f32; 3],
    pub _pad0: f32,
    pub metallic: f32,
    pub roughness: f32,
    pub _pad1: [f32; 2],
    pub emissive: [f32; 3],
    pub _pad2: f32,
}
unsafe impl bytemuck::Zeroable for RendererMaterialParamsStd140 {}
unsafe impl bytemuck::Pod for RendererMaterialParamsStd140 {}

// COLD: GPU handles (bind groups/buffer) created infrequently and reused across many draws.
pub struct RendererMaterial {
    pub name: String,
    pub texture_bind_group: wgpu::BindGroup,
    pub param_buffer: wgpu::Buffer,
    pub param_bind_group: wgpu::BindGroup,
}
