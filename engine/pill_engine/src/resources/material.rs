use crate::{
    ecs::{
        DeferredUpdateComponent, DeferredUpdateManagerPointer, DeferredUpdateResourceRequest,
        MeshRenderingComponent,
    },
    engine::Engine,
    resources::{Resource, ResourceStorage, TextureHandle, TextureType},
};

#[cfg(not(feature = "headless"))]
use crate::{
    graphics::RendererTextureHandle,
    renderer::resources::{MaterialParams, MaterialTextures, RendererMaterial, RendererTexture},
    resources::{ResourceManager, Texture},
};

use pill_core::{
    error, get_type_name, Color, EngineError, LogContext, PillSlotMapKey, PillStyle,
    PillTypeMapKey, Result,
};

const DEFERRED_REQUEST_VARIANT_PARAMETERS: usize = 0;
const DEFERRED_REQUEST_VARIANT_TEXTURES: usize = 1;

// Texture slots: (name, texture type). Color and Emissive are both sRGB color and interchangeable;
// Normal and MetallicRoughness are linear data and must match exactly.
const ALBEDO_SLOT: (&str, TextureType) = ("albedo", TextureType::Color);
const NORMAL_SLOT: (&str, TextureType) = ("normal", TextureType::Normal);
const METALLIC_ROUGHNESS_SLOT: (&str, TextureType) =
    ("metallic_roughness", TextureType::MetallicRoughness);
const EMISSIVE_SLOT: (&str, TextureType) = ("emissive", TextureType::Emissive);

fn is_color(texture_type: TextureType) -> bool {
    matches!(texture_type, TextureType::Color | TextureType::Emissive)
}

/// Renderer texture for a slot, checking the texture exists and has the slot's type.
#[cfg(not(feature = "headless"))]
fn resolve_texture(
    resource_manager: &ResourceManager,
    handle: TextureHandle,
    (slot_name, slot_type): (&str, TextureType),
) -> Result<RendererTextureHandle> {
    let texture = resource_manager.get_resource::<Texture>(&handle)?;
    let compatible = texture.texture_type == slot_type
        || (is_color(texture.texture_type) && is_color(slot_type));
    if !compatible {
        return Err(EngineError::WrongTextureType(
            format!("{:?}", texture.texture_type),
            slot_name.to_string(),
            format!("{slot_type:?}"),
        )
        .into());
    }
    resource_manager.get_resource_handle::<RendererTexture>(&texture.name)
}

// --- Material ---

pill_core::define_new_pill_slotmap_key! {
    pub struct MaterialHandle;
}

/// Physically based (metallic-roughness) material. Built by value before `add_resource`;
/// after that, use the `set_*` methods so changes reach the GPU.
#[readonly::make]
pub struct Material {
    #[readonly]
    pub name: String,
    #[readonly]
    pub albedo: Color,
    #[readonly]
    pub metallic: f32,
    #[readonly]
    pub roughness: f32,
    #[readonly]
    pub emissive: Color,
    #[readonly]
    pub albedo_texture: Option<TextureHandle>,
    #[readonly]
    pub normal_texture: Option<TextureHandle>,
    #[readonly]
    pub metallic_roughness_texture: Option<TextureHandle>,
    #[readonly]
    pub emissive_texture: Option<TextureHandle>,
    handle: Option<MaterialHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Material {
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
            handle: None,
            deferred_update_manager: None,
        }
    }

    // --- Builder (before add_resource) ---

    pub fn albedo(mut self, color: Color) -> Self {
        self.albedo = color;
        self
    }

    pub fn roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness.clamp(0.0, 1.0);
        self
    }

    pub fn metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic.clamp(0.0, 1.0);
        self
    }

    /// Emitted radiance, added after lighting. Multiplied by the emissive texture (white by default),
    /// so a black albedo plus an emissive color gives an unlit look.
    pub fn emissive(mut self, color: Color) -> Self {
        self.emissive = color;
        self
    }

    pub fn albedo_texture(mut self, handle: TextureHandle) -> Self {
        self.albedo_texture = Some(handle);
        self
    }

    pub fn normal_texture(mut self, handle: TextureHandle) -> Self {
        self.normal_texture = Some(handle);
        self
    }

    pub fn metallic_roughness_texture(mut self, handle: TextureHandle) -> Self {
        self.metallic_roughness_texture = Some(handle);
        self
    }

    pub fn emissive_texture(mut self, handle: TextureHandle) -> Self {
        self.emissive_texture = Some(handle);
        self
    }

    // --- Setters (after add_resource) ---

    pub fn set_albedo(&mut self, color: Color) {
        self.albedo = color;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETERS);
    }

    pub fn set_roughness(&mut self, roughness: f32) {
        self.roughness = roughness.clamp(0.0, 1.0);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETERS);
    }

    pub fn set_metallic(&mut self, metallic: f32) {
        self.metallic = metallic.clamp(0.0, 1.0);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETERS);
    }

    pub fn set_emissive(&mut self, color: Color) {
        self.emissive = color;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETERS);
    }

    pub fn set_albedo_texture(&mut self, handle: Option<TextureHandle>) {
        self.albedo_texture = handle;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURES);
    }

    pub fn set_normal_texture(&mut self, handle: Option<TextureHandle>) {
        self.normal_texture = handle;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURES);
    }

    pub fn set_metallic_roughness_texture(&mut self, handle: Option<TextureHandle>) {
        self.metallic_roughness_texture = handle;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURES);
    }

    pub fn set_emissive_texture(&mut self, handle: Option<TextureHandle>) {
        self.emissive_texture = handle;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURES);
    }

    /// Clears every texture slot referencing `texture_handle`; returns true if any slot changed.
    pub(crate) fn remove_texture(&mut self, texture_handle: &TextureHandle) -> bool {
        let mut changed = false;
        for slot in [
            &mut self.albedo_texture,
            &mut self.normal_texture,
            &mut self.metallic_roughness_texture,
            &mut self.emissive_texture,
        ] {
            if slot.is_some_and(|handle| handle.data() == texture_handle.data()) {
                *slot = None;
                changed = true;
            }
        }
        changed
    }

    #[cfg(not(feature = "headless"))]
    fn renderer_parameters(&self) -> MaterialParams {
        MaterialParams {
            base_color: [self.albedo.x, self.albedo.y, self.albedo.z],
            roughness: self.roughness,
            emissive: [self.emissive.x, self.emissive.y, self.emissive.z],
            metallic: self.metallic,
        }
    }

    #[cfg(not(feature = "headless"))]
    pub(crate) fn renderer_textures(
        &self,
        resource_manager: &ResourceManager,
    ) -> Result<MaterialTextures> {
        let [base_color, normal, metallic_roughness, emissive] =
            self.texture_slots().map(|(handle, slot_type)| {
                handle
                    .map(|handle| resolve_texture(resource_manager, handle, slot_type))
                    .transpose()
            });
        Ok(MaterialTextures {
            base_color: base_color?,
            normal: normal?,
            metallic_roughness: metallic_roughness?,
            emissive: emissive?,
        })
    }

    fn texture_slots(&self) -> [(Option<TextureHandle>, (&'static str, TextureType)); 4] {
        [
            (self.albedo_texture, ALBEDO_SLOT),
            (self.normal_texture, NORMAL_SLOT),
            (self.metallic_roughness_texture, METALLIC_ROUGHNESS_SLOT),
            (self.emissive_texture, EMISSIVE_SLOT),
        ]
    }

    /// Empties slots whose texture is missing or of the wrong type, so one bad `set_*_texture`
    /// call doesn't block later texture updates.
    #[cfg(not(feature = "headless"))]
    fn clear_invalid_texture_slots(&mut self, resource_manager: &ResourceManager) {
        let material_name = self.name.clone();
        let check = |slot: &mut Option<TextureHandle>, slot_info: (&str, TextureType)| {
            if let Some(handle) = *slot {
                if let Err(error) = resolve_texture(resource_manager, handle, slot_info) {
                    error!(LogContext::Engine => "Material {} {} texture cleared: {}", material_name, slot_info.0, error);
                    *slot = None;
                }
            }
        };
        check(&mut self.albedo_texture, ALBEDO_SLOT);
        check(&mut self.normal_texture, NORMAL_SLOT);
        check(
            &mut self.metallic_roughness_texture,
            METALLIC_ROUGHNESS_SLOT,
        );
        check(&mut self.emissive_texture, EMISSIVE_SLOT);
    }

    /// Rebuilds the GPU texture bind group from the current texture slots.
    #[cfg(not(feature = "headless"))]
    fn rebuild_renderer_textures(&self, engine: &mut Engine) -> Result<()> {
        let textures = self.renderer_textures(&engine.resource_manager)?;
        let bind_group = RendererMaterial::create_textures_bind_group(
            engine.renderer.get_device(),
            &engine.resource_manager,
            &self.name,
            &textures,
        )?;
        engine
            .resource_manager
            .get_resource_by_name_mut::<RendererMaterial>(&self.name)?
            .textures_bind_group = bind_group;
        Ok(())
    }

    // Only posts once the material is initialized (deferred_update_manager set by initialize)
    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if let Some(manager) = self.deferred_update_manager.as_mut() {
            let handle = self
                .handle
                .expect("Critical: Cannot post deferred update request. No Handle set in Resource");
            let request = DeferredUpdateResourceRequest::<Material>::new(handle, request_variant);
            manager.post_update_request(request);
        }
    }
}

impl PillTypeMapKey for Material {
    type Storage = ResourceStorage<Material>;
}

impl Resource for Material {
    type Handle = MaterialHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This resource is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine
            .get_global_component_mut::<DeferredUpdateComponent>()
            .expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager =
            Some(deferred_update_component.borrow_deferred_update_manager());

        // Create renderer counterpart
        #[cfg(not(feature = "headless"))]
        {
            let textures = self.renderer_textures(&engine.resource_manager)?;
            let renderer_material = RendererMaterial::new(
                engine.renderer.get_device(),
                engine.renderer.get_queue(),
                &engine.resource_manager,
                &self.name,
                &self.renderer_parameters(),
                &textures,
            )?;
            engine.resource_manager.add_resource(renderer_material)?;
        }

        Ok(())
    }

    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {
        self.handle = Some(MaterialHandle::from(self_handle.data()));
    }

    #[cfg_attr(feature = "headless", allow(unused_variables))]
    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> {
        match request {
            DEFERRED_REQUEST_VARIANT_PARAMETERS => {
                #[cfg(not(feature = "headless"))]
                engine
                    .resource_manager
                    .get_resource_by_name::<RendererMaterial>(&self.name)?
                    .write_parameters(engine.renderer.get_queue(), &self.renderer_parameters());
            }
            DEFERRED_REQUEST_VARIANT_TEXTURES => {
                #[cfg(not(feature = "headless"))]
                {
                    self.clear_invalid_texture_slots(&engine.resource_manager);
                    self.rebuild_renderer_textures(engine)?;
                }
            }
            _ => {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
            }
        }

        Ok(())
    }

    #[cfg_attr(feature = "headless", allow(unused_variables))]
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer counterpart
        #[cfg(not(feature = "headless"))]
        engine
            .resource_manager
            .remove_resource_by_name::<RendererMaterial>(&self.name)?;

        // Components using this material fall back to the default material
        for (_scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
            if let Ok(iter) = scene.get_one_component_iterator_mut::<MeshRenderingComponent>() {
                for (_entity_handle, mesh_rendering_component) in iter {
                    let uses_material = mesh_rendering_component
                        .material_handle
                        .is_some_and(|handle| handle.data() == self_handle.data());
                    if uses_material {
                        mesh_rendering_component.set_material_handle(None);
                        mesh_rendering_component
                            .update_render_queue_key(&engine.resource_manager)?;
                    }
                }
            }
        }
        Ok(())
    }
}
