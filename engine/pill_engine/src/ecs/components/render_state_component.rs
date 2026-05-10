use crate::{
    ecs::components::{GlobalComponent, GlobalComponentStorage},
    graphics::RendererTextureHandle,
};

use pill_core::PillTypeMapKey;

pub struct RenderStateComponent {
    pub boot_done: bool,
    pub background: RendererTextureHandle,
    pub ibl_diffuse: RendererTextureHandle,
    pub ibl_specular: RendererTextureHandle,
    pub ibl_brdf_lut: RendererTextureHandle,
    pub bg_color: [f32; 3], // tint multiplied against equirect sample; [1,1,1] = identity
    pub fog_density: f32,
}

impl RenderStateComponent {
    /// Creates the component in its pre-boot state; handles are replaced by `create_default_resources`.
    pub fn new() -> Self {
        Self {
            boot_done: false,
            background: RendererTextureHandle::default(),
            ibl_diffuse: RendererTextureHandle::default(),
            ibl_specular: RendererTextureHandle::default(),
            ibl_brdf_lut: RendererTextureHandle::default(),
            bg_color: [1.0, 1.0, 1.0],
            fog_density: 0.0,
        }
    }
}

impl Default for RenderStateComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl PillTypeMapKey for RenderStateComponent {
    type Storage = GlobalComponentStorage<RenderStateComponent>;
}

impl GlobalComponent for RenderStateComponent {}
