use crate::{
    config::*,
    engine::Engine,
    graphics::{
        MaterialDesc, RendererMaterialHandle, RendererTextureHandle, RENDER_QUEUE_KEY_ORDER,
    },
    resources::{Resource, ResourceStorage, Texture, TextureHandle, TextureType},
};

use pill_core::{get_type_name, Color, EngineError, PillSlotMapKey, PillStyle, PillTypeMapKey};

use anyhow::{Context, Error, Result};

// --- Material (PBR POD) ---

pill_core::define_new_pill_slotmap_key! {
    pub struct PBRMaterialHandle;
}

#[readonly::make]
pub struct PBRMaterial {
    #[readonly]
    pub name: String,
    #[readonly]
    // PBR factors (no alpha)
    pub albedo: Color, // vec3
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Color, // vec3

    // PBR textures
    pub albedo_texture: Option<TextureHandle>, // Color
    pub normal_texture: Option<TextureHandle>, // Normal
    pub metallic_roughness_texture: Option<TextureHandle>, // Color (G=roughness, B=metallic)
    pub emissive_texture: Option<TextureHandle>, // Color

    // Dirty flag to trigger GPU updates
    pub is_dirty: bool,
    pub(crate) renderer_resource_handle: Option<RendererMaterialHandle>,
}

impl PBRMaterial {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            albedo: Color::new(1.0, 1.0, 1.0),
            metallic: 0.0,
            roughness: 0.5,
            emissive: Color::new(0.0, 0.0, 0.0),
            albedo_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            is_dirty: true,
            renderer_resource_handle: None,
        }
    }

    // Factor setters (mark dirty)
    pub fn set_base_color_factor(&mut self, value: Color) {
        self.albedo = Color::new(
            value.x.clamp(0.0, 1.0),
            value.y.clamp(0.0, 1.0),
            value.z.clamp(0.0, 1.0),
        );
        self.is_dirty = true;
    }

    pub fn set_metallic_factor(&mut self, value: f32) {
        self.metallic = value.clamp(0.0, 1.0);
        self.is_dirty = true;
    }

    pub fn set_roughness_factor(&mut self, value: f32) {
        self.roughness = value.clamp(0.0, 1.0);
        self.is_dirty = true;
    }

    pub fn set_emissive_factor(&mut self, value: Color) {
        self.emissive = Color::new(
            value.x.clamp(0.0, 1.0),
            value.y.clamp(0.0, 1.0),
            value.z.clamp(0.0, 1.0),
        );
        self.is_dirty = true;
    }

    // Texture setters (mark dirty)
    pub fn set_albedo_texture(&mut self, handle: TextureHandle) {
        self.albedo_texture = Some(handle);
        self.is_dirty = true;
    }

    pub fn set_normal_texture(&mut self, handle: TextureHandle) {
        self.normal_texture = Some(handle);
        self.is_dirty = true;
    }

    pub fn set_metallic_roughness_texture(&mut self, handle: TextureHandle) {
        self.metallic_roughness_texture = Some(handle);
        self.is_dirty = true;
    }

    pub fn set_emissive_texture(&mut self, handle: TextureHandle) {
        self.emissive_texture = Some(handle);
        self.is_dirty = true;
    }
}

impl PillTypeMapKey for PBRMaterial {
    type Storage = ResourceStorage<PBRMaterial>;
}

impl Resource for PBRMaterial {
    type Handle = PBRMaterialHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // Build initial descriptor
        let desc = {
            // Resolve renderer texture handles if available
            let to_renderer = |th: &Option<TextureHandle>| -> Option<RendererTextureHandle> {
                th.as_ref().and_then(|h| {
                    engine
                        .resource_manager
                        .get_resource::<Texture>(h)
                        .ok()
                        .and_then(|t| t.renderer_resource_handle)
                })
            };
            MaterialDesc {
                label: &self.name,
                albedo: [self.albedo.x, self.albedo.y, self.albedo.z],
                metallic: self.metallic,
                roughness: self.roughness,
                emissive: [self.emissive.x, self.emissive.y, self.emissive.z],
                albedo_tex: to_renderer(&self.albedo_texture),
                normal_tex: to_renderer(&self.normal_texture),
                metallic_roughness_tex: to_renderer(&self.metallic_roughness_texture),
                emissive_tex: to_renderer(&self.emissive_texture),
            }
        };
        let h = engine.renderer.create_material(desc)?;
        self.renderer_resource_handle = Some(h);
        Ok(())
    }
}
