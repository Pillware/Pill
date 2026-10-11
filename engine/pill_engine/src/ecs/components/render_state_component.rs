use crate::{
    ecs::components::{GlobalComponent, GlobalComponentStorage},
    graphics::PassTextureHandle,
};

use pill_core::PillTypeMapKey;

pub struct RenderStateComponent {
    pub boot_done: bool,
    /// Scene color target (HDR_TARGET_FORMAT) shared by scene passes; resolved by the tonemap pass.
    pub hdr_target: PassTextureHandle,
    /// Scene depth target (DEPTH_TARGET_FORMAT) shared by scene passes.
    pub depth_target: PassTextureHandle,
    /// Environment (equirect) image drawn behind the scene; `None` draws `background_color`.
    pub background: Option<PassTextureHandle>,
    /// Solid background color used when `background` is `None` (linear RGB).
    pub background_color: [f32; 3],
    pub ibl_diffuse: PassTextureHandle,
    pub ibl_specular: PassTextureHandle,
    pub ibl_brdf_lut: PassTextureHandle,
    /// Exponential-squared fog: color is mixed toward `fog_color` by 1 - exp(-density² · distance²);
    /// density 0 disables fog.
    pub fog_color: [f32; 3],
    pub fog_density: f32,
}

impl RenderStateComponent {
    /// Creates the component in its pre-boot state; handles are replaced by `create_default_resources`.
    pub fn new() -> Self {
        Self {
            boot_done: false,
            hdr_target: PassTextureHandle::default(),
            depth_target: PassTextureHandle::default(),
            background: None,
            background_color: [0.15, 0.15, 0.15],
            ibl_diffuse: PassTextureHandle::default(),
            ibl_specular: PassTextureHandle::default(),
            ibl_brdf_lut: PassTextureHandle::default(),
            fog_color: [0.0, 0.0, 0.0],
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
