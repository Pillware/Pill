use crate::ecs::RenderStateComponent;

/// Scene globals of the PBR passes (group 0, binding 7): fog, IBL intensity and the three
/// directional lights from `RenderStateComponent`. All float4 rows so the std140 layout is
/// unambiguous; mirrors `Scene` in pbr_opaque_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SceneUniform {
    fog_color_density: [f32; 4], // xyz = fog color, w = fog density
    ibl_intensity: [f32; 4],     // x
    // Per light: direction (xyz) row, then color (rgb) + intensity (w) row
    lights: [[f32; 4]; 6],
}

impl SceneUniform {
    pub(crate) fn from_render_state(render_state: &RenderStateComponent) -> Self {
        let [r, g, b] = render_state.fog_color;
        let mut lights = [[0.0; 4]; 6];
        for (index, light) in render_state.lights.iter().enumerate() {
            let [x, y, z] = light.direction;
            let [red, green, blue] = light.color;
            lights[index * 2] = [x, y, z, 0.0];
            lights[index * 2 + 1] = [red, green, blue, light.intensity];
        }
        Self {
            fog_color_density: [r, g, b, render_state.fog_density],
            ibl_intensity: [render_state.ibl_intensity, 0.0, 0.0, 0.0],
            lights,
        }
    }
}
