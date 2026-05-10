use crate::{
    config::*,
    ecs::{
        DeferredUpdateComponent, DeferredUpdateManagerPointer, DeferredUpdateResourceRequest,
        PbrRenderableComponent,
    },
    engine::Engine,
    graphics::RENDER_QUEUE_KEY_ORDER,
    renderer::resources::{RendererMaterial, RendererShader, RendererTexture},
    resources::{Resource, ResourceStorage, Shader, ShaderHandle, Texture, TextureHandle},
};

use pill_core::{
    enum_variant_eq, get_enum_variant_type_name, get_type_name, Color, EngineError, PillSlotMapKey,
    PillStyle, PillTypeMapKey,
};

use pill_core::{ErrorContext, Result};
use std::collections::HashMap;

const DEFERRED_REQUEST_VARIANT_RENDERING_ORDER: usize = 0;
const DEFERRED_REQUEST_VARIANT_PARAMETER: usize = 1;

const DEFERRED_REQUEST_VARIANT_TEXTURE_START: usize = 2;
const DEFERRED_REQUEST_VARIANT_TEXTURE_END: usize = 10;

// --- PBRMaterial ---

pill_core::define_new_pill_slotmap_key! {
    pub struct PBRMaterialHandle;
}

#[readonly::make]
pub struct PBRMaterial {
    #[readonly]
    pub name: String,
    pub albedo: Color,
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: Color,
    pub albedo_texture: Option<TextureHandle>,
    pub normal_texture: Option<TextureHandle>,
    pub metallic_roughness_texture: Option<TextureHandle>,
    pub emissive_texture: Option<TextureHandle>,
    handle: Option<PBRMaterialHandle>,
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
            handle: None,
        }
    }

    pub fn albedo(mut self, color: Color) -> Self {
        self.albedo = color;
        self
    }

    pub fn roughness(mut self, roughness: f32) -> Self {
        self.roughness = roughness.clamp(0.0, 1.0);
        self
    }

    pub fn albedo_texture(mut self, handle: TextureHandle) -> Self {
        self.albedo_texture = Some(handle);
        self
    }

    pub fn metallic(mut self, metallic: f32) -> Self {
        self.metallic = metallic.clamp(0.0, 1.0);
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
        #[cfg(not(feature = "headless"))]
        {
            let mut parameters: HashMap<String, MaterialParameter> = HashMap::new();
            parameters.insert(
                DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME.to_string(),
                MaterialParameter::Color(self.albedo),
            );
            parameters.insert(
                DEFAULT_LIT_SHADER_SPECULARITY_PARAMETER_SLOT_NAME.to_string(),
                MaterialParameter::Scalar(1.0 - self.roughness),
            );
            parameters.insert(
                DEFAULT_LIT_SHADER_METALLIC_FACTOR_PARAMETER_SLOT_NAME.to_string(),
                MaterialParameter::Scalar(self.metallic),
            );

            let mut resolved_textures: HashMap<String, crate::graphics::RendererTextureHandle> =
                HashMap::new();

            for (opt_handle, slot_name) in [
                (
                    self.albedo_texture,
                    DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME,
                ),
                (
                    self.normal_texture,
                    DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_NAME,
                ),
                (
                    self.metallic_roughness_texture,
                    DEFAULT_LIT_SHADER_METALLIC_ROUGHNESS_TEXTURE_SLOT_NAME,
                ),
                (
                    self.emissive_texture,
                    DEFAULT_LIT_SHADER_EMISSIVE_TEXTURE_SLOT_NAME,
                ),
            ] {
                if let Some(texture_handle) = opt_handle {
                    let tex_name = engine
                        .resource_manager
                        .get_resource::<Texture>(&texture_handle)?
                        .name
                        .clone();
                    let renderer_texture_handle = engine
                        .resource_manager
                        .get_resource_handle::<RendererTexture>(&tex_name)?;
                    resolved_textures.insert(slot_name.to_string(), renderer_texture_handle);
                }
            }

            let renderer_shader_handle = engine
                .resource_manager
                .get_resource_handle::<RendererShader>(DEFAULT_LIT_SHADER_NAME)?;

            let renderer_material = RendererMaterial::new(
                engine.renderer.get_device(),
                engine.renderer.get_queue(),
                &engine.resource_manager,
                &self.name,
                renderer_shader_handle,
                &resolved_textures,
                &parameters,
            )?;
            engine.resource_manager.add_resource(renderer_material)?;
        }
        Ok(())
    }

    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {
        self.handle = Some(PBRMaterialHandle::from(self_handle.data()));
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, _self_handle: H) -> Result<()> {
        #[cfg(not(feature = "headless"))]
        engine
            .resource_manager
            .remove_resource_by_name::<RendererMaterial>(&self.name)?;
        Ok(())
    }
}

// --- Material parameters ---

#[derive(Debug)]
pub enum MaterialParameter {
    Scalar(f32),
    Bool(bool),
    Color(Color),
}

// --- Material textures ---

#[derive(Clone)]
pub struct MaterialTexture {
    pub texture_handle: TextureHandle,
}

impl MaterialTexture {
    pub fn new(texture_handle: TextureHandle) -> Self {
        Self { texture_handle }
    }
}

// --- Builder ---

pub struct MaterialBuilder {
    material: Material,
}

impl MaterialBuilder {
    pub fn new(name: &str) -> Self {
        Self {
            material: Material::new(name),
        }
    }

    pub fn shader(mut self, shader_handle: ShaderHandle) -> Result<Self> {
        self.material.shader_handle = shader_handle;
        Ok(self)
    }

    pub fn texture(mut self, slot_name: &str, texture_handle: TextureHandle) -> Result<Self> {
        self.material
            .textures
            .push((slot_name.to_string(), MaterialTexture::new(texture_handle)));
        Ok(self)
    }

    pub fn scalar_parameter(mut self, slot_name: &str, value: f32) -> Result<Self> {
        self.material
            .parameters
            .insert(slot_name.to_string(), MaterialParameter::Scalar(value));
        Ok(self)
    }

    pub fn bool_parameter(mut self, slot_name: &str, value: bool) -> Result<Self> {
        self.material
            .parameters
            .insert(slot_name.to_string(), MaterialParameter::Bool(value));
        Ok(self)
    }

    pub fn color_parameter(mut self, slot_name: &str, value: Color) -> Result<Self> {
        self.material
            .parameters
            .insert(slot_name.to_string(), MaterialParameter::Color(value));
        Ok(self)
    }

    pub fn rendering_order(mut self, order: u8) -> Result<Self> {
        self.material.rendering_order = order;
        Ok(self)
    }

    pub fn build(self) -> Material {
        self.material
    }
}

// --- Material ---

pill_core::define_new_pill_slotmap_key! {
    pub struct MaterialHandle;
}

#[readonly::make]
pub struct Material {
    #[readonly]
    pub name: String,
    #[readonly]
    pub shader_handle: ShaderHandle,
    pub(crate) textures: Vec<(String, MaterialTexture)>,
    //pub(crate) textures_mapping: Vec<String>,  // Maps index to slot name, required for deferred update requests
    #[readonly]
    pub(crate) parameters: HashMap<String, MaterialParameter>,
    #[readonly]
    pub rendering_order: u8,

    shader_name: Option<String>,
    handle: Option<MaterialHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Material {
    pub fn builder(name: &str) -> MaterialBuilder {
        MaterialBuilder::new(name)
    }

    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            shader_handle: get_default_lit_shader_handles().0,
            textures: Vec::new(),
            //textures_mapping: Vec::new(),
            parameters: HashMap::new(),
            rendering_order: RENDER_QUEUE_KEY_ORDER.max as u8,
            shader_name: None,
            handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn set_texture(&mut self, slot_name: &str, texture_handle: TextureHandle) -> Result<()> {
        // Get or insert new
        let slot_index = if let Some(pos) = self.textures.iter().position(|(k, _)| k == slot_name) {
            self.textures[pos].1.texture_handle = texture_handle;
            pos
        } else {
            self.textures
                .push((slot_name.to_string(), MaterialTexture::new(texture_handle)));
            self.textures.len() - 1
        };

        // Get texture slot
        //let texture_slot = self.textures.get_mut(slot_name)
        //    .ok_or( EngineError::MaterialTextureSlotNotFound(slot_name.to_string(), self.name.to_string()).into())?;

        // Get texture slot index
        //let texture_slot_index = self.textures_mapping.iter().position(|v| v == slot_name).expect("Critical: No mapping");

        // Set new handle but not renderer resource handle (it will be set by deferred update system)
        //  l//et _ = texture_slot.texture_handle = texture_handle;

        // Post deferred update request (only if deferred_update_manager is set (it means that material is initialized))
        if self.deferred_update_manager.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURE_START + slot_index);
        }

        Ok(())
    }

    pub fn set_rendering_order(&mut self, order: u8) -> Result<()> {
        let error = EngineError::WrongRenderingOrder(
            order.to_string(),
            format!("{}-{}", 0, RENDER_QUEUE_KEY_ORDER.max),
        );
        if order < RENDER_QUEUE_KEY_ORDER.max as u8 {
            self.rendering_order = order;
            if self.deferred_update_manager.is_some() {
                self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_RENDERING_ORDER);
            }
        } else {
            return Err(error.into());
        }
        Ok(())
    }

    pub fn get_scalar_parameter(&self, parameter_name: &str) -> Result<f32> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Scalar".to_string(),
            self.name.to_string(),
        );
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Scalar(value) => Ok(*value),
            _ => Err(error.into()),
        }
    }

    pub fn get_bool_parameter(&self, parameter_name: &str) -> Result<bool> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Bool".to_string(),
            self.name.to_string(),
        );
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Bool(value) => Ok(*value),
            _ => Err(error.into()),
        }
    }

    pub fn get_color_parameter(&self, parameter_name: &str) -> Result<Color> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Color".to_string(),
            self.name.to_string(),
        );
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Color(value) => Ok(*value),
            _ => Err(error.into()),
        }
    }

    pub fn set_scalar_parameter(&mut self, parameter_name: &str, value: f32) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Scalar".to_string(),
            self.name.to_string(),
        );
        let parameter = self
            .parameters
            .get_mut(parameter_name)
            .context(error.clone())?;
        match parameter {
            MaterialParameter::Scalar(v) => {
                if *v != value {
                    *v = value;
                    if self.deferred_update_manager.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            }
            _ => Err(error.into()),
        }
    }

    pub fn set_bool_parameter(&mut self, parameter_name: &str, value: bool) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Bool".to_string(),
            self.name.to_string(),
        );
        let parameter = self
            .parameters
            .get_mut(parameter_name)
            .context(error.clone())?;
        match parameter {
            MaterialParameter::Bool(v) => {
                if *v != value {
                    *v = value;
                    if self.deferred_update_manager.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            }
            _ => Err(error.into()),
        }
    }

    pub fn set_color_parameter(&mut self, parameter_name: &str, value: Color) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(
            parameter_name.to_string(),
            "Color".to_string(),
            self.name.to_string(),
        );
        let parameter = self
            .parameters
            .get_mut(parameter_name)
            .context(error.clone())?;
        match parameter {
            MaterialParameter::Color(v) => {
                let valid_color = Color::new(
                    value.x.clamp(0.0, 1.0),
                    value.y.clamp(0.0, 1.0),
                    value.z.clamp(0.0, 1.0),
                );
                if *v != valid_color {
                    *v = valid_color;
                    if self.deferred_update_manager.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            }
            _ => Err(error.into()),
        }
    }

    fn validate_texture(
        &self,
        engine: &mut Engine,
        texture_slot_name: &str,
        texture_slot: &MaterialTexture,
    ) -> Result<()> {
        let shader = engine.get_resource::<Shader>(&self.shader_handle)?;

        let texture = engine
            .get_resource::<Texture>(&texture_slot.texture_handle)
            .context(EngineError::InvalidTextureHandleForSlot(
                texture_slot_name.to_string(),
            ))?;

        let shader_texture_slot = shader.texture_slots.get(texture_slot_name).context(
            EngineError::MaterialTextureSlotNotFound(texture_slot_name.to_string()),
        )?;

        if !enum_variant_eq(&texture.texture_type, &shader_texture_slot.texture_type) {
            return Err(EngineError::WrongTextureType(
                get_enum_variant_type_name(&texture.texture_type),
                texture_slot_name.to_string(),
                get_enum_variant_type_name(&shader_texture_slot.texture_type),
            )
            .into());
        }

        Ok(())
    }

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        let handle = self
            .handle
            .expect("Critical: Cannot post deferred update request. No Handle set in Resource");
        let request = DeferredUpdateResourceRequest::<Material>::new(handle, request_variant);
        self.deferred_update_manager
            .as_mut()
            .expect("Critical: No DeferredUpdateManager")
            .post_update_request(request);
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
        let deferred_update_component = engine
            .get_global_component_mut::<DeferredUpdateComponent>()
            .expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager =
            Some(deferred_update_component.borrow_deferred_update_manager());

        for (texture_slot_name, texture_slot) in self.textures.iter() {
            self.validate_texture(engine, texture_slot_name, texture_slot)?;
        }

        #[cfg(not(feature = "headless"))]
        {
            let shader = engine.get_resource::<Shader>(&self.shader_handle)?;
            self.shader_name = Some(shader.name.clone());
            let shader_name = shader.name.clone();

            let mut resolved_textures: HashMap<String, crate::graphics::RendererTextureHandle> =
                HashMap::new();
            for (slot_name, tex_slot) in self.textures.iter() {
                let tex_name = engine
                    .resource_manager
                    .get_resource::<Texture>(&tex_slot.texture_handle)
                    .context(EngineError::InvalidTextureHandleForSlot(slot_name.clone()))?
                    .name
                    .clone();
                let renderer_tex_handle = engine
                    .resource_manager
                    .get_resource_handle::<RendererTexture>(&tex_name)?;
                resolved_textures.insert(slot_name.clone(), renderer_tex_handle);
            }

            let renderer_shader_handle = engine
                .resource_manager
                .get_resource_handle::<RendererShader>(&shader_name)?;

            let renderer_material = RendererMaterial::new(
                engine.renderer.get_device(),
                engine.renderer.get_queue(),
                &engine.resource_manager,
                &self.name,
                renderer_shader_handle,
                &resolved_textures,
                &self.parameters,
            )?;
            engine.resource_manager.add_resource(renderer_material)?;
        }

        Ok(())
    }

    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {
        self.handle = Some(MaterialHandle::from(self_handle.data()));
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> {
        match request {
            DEFERRED_REQUEST_VARIANT_RENDERING_ORDER => {
                for (_scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
                    for (_entity_handle, pbr_renderable_component) in
                        scene.get_one_component_iterator_mut::<PbrRenderableComponent>()?
                    {
                        if let Some(material_handle) = pbr_renderable_component.material_handle {
                            if material_handle.data() == self.handle.unwrap().data() {
                                pbr_renderable_component
                                    .update_render_queue_key(&engine.resource_manager)
                                    .unwrap();
                            }
                        }
                    }
                }
            }
            DEFERRED_REQUEST_VARIANT_PARAMETER => {
                #[cfg(not(feature = "headless"))]
                {
                    let mat_name = self.name.clone();
                    let parameter_slots = {
                        let renderer_mat = engine
                            .resource_manager
                            .get_resource_by_name::<RendererMaterial>(&mat_name)?;
                        let shader = engine
                            .resource_manager
                            .get_resource::<RendererShader>(&renderer_mat.shader_handle)?;
                        shader.parameter_slots.clone()
                    };

                    let queue = engine.renderer.get_queue();
                    let renderer_mat = engine
                        .resource_manager
                        .get_resource_by_name_mut::<RendererMaterial>(&mat_name)?;
                    if let Some(ref buffer) = renderer_mat.parameters_uniform_buffer {
                        RendererMaterial::write_parameters_to_buffer(
                            queue,
                            buffer,
                            &parameter_slots,
                            &self.parameters,
                        )?;
                    }
                }
            }
            DEFERRED_REQUEST_VARIANT_TEXTURE_START..=DEFERRED_REQUEST_VARIANT_TEXTURE_END => {
                // Check if assigned texture is of correct type
                let idx = request - DEFERRED_REQUEST_VARIANT_TEXTURE_START;
                let (texture_slot_name, texture_slot) = &self.textures[idx];
                self.validate_texture(engine, texture_slot_name, texture_slot)?;

                #[cfg(not(feature = "headless"))]
                {
                    let mat_name = self.name.clone();
                    let new_bind_group = {
                        let renderer_mat = engine
                            .resource_manager
                            .get_resource_by_name::<RendererMaterial>(&mat_name)?;
                        let renderer_shader = engine
                            .resource_manager
                            .get_resource::<RendererShader>(&renderer_mat.shader_handle)?;
                        let mut resolved: HashMap<String, crate::graphics::RendererTextureHandle> =
                            HashMap::new();
                        for (slot_name, mat_tex) in &self.textures {
                            let tex = engine
                                .resource_manager
                                .get_resource::<Texture>(&mat_tex.texture_handle)?;
                            let h = engine
                                .resource_manager
                                .get_resource_handle::<RendererTexture>(&tex.name)?;
                            resolved.insert(slot_name.clone(), h);
                        }
                        RendererMaterial::create_textures_bind_group(
                            engine.renderer.get_device(),
                            &engine.resource_manager,
                            renderer_shader.textures_bind_group_layout.as_ref().unwrap(),
                            &format!("{}_textures", mat_name),
                            &renderer_shader.texture_slots,
                            &resolved,
                        )?
                    };
                    engine
                        .resource_manager
                        .get_resource_by_name_mut::<RendererMaterial>(&mat_name)?
                        .textures_bind_group = Some(new_bind_group);
                }
            }
            _ => {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
            }
        }

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, _self_handle: H) -> Result<()> {
        #[cfg(not(feature = "headless"))]
        engine
            .resource_manager
            .remove_resource_by_name::<RendererMaterial>(&self.name)?;
        Ok(())
    }
}
