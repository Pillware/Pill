use crate::{
    config::*,
    ecs::{
        DeferredUpdateComponent,
        DeferredUpdateManagerPointer,
        DeferredUpdateResourceRequest,
        MeshRenderingComponent
    },
    engine::Engine,
    graphics::{
        RendererMaterialHandle,
        RendererTextureHandle,
        RENDER_QUEUE_KEY_ORDER
    },
    resources::{
        Resource,
        ResourceStorage,
        Shader,
        ShaderHandle,
        Texture,
        TextureHandle}
};

use pill_core::{
    enum_variant_eq,
    get_enum_variant_type_name,
    get_type_name,
    Color,
    EngineError,
    PillSlotMapKey,
    PillStyle,
    PillTypeMapKey
};

use anyhow::{ Result, Context, Error };
use std::{
    collections::{HashMap},
};
use indexmap::IndexMap;

const DEFERRED_REQUEST_VARIANT_RENDERING_ORDER: usize = 0;
const DEFERRED_REQUEST_VARIANT_PARAMETER: usize = 1;

const DEFERRED_REQUEST_VARIANT_TEXTURE_START: usize = 2;
const DEFERRED_REQUEST_VARIANT_TEXTURE_END: usize = 10;

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
    //pub texture_type: TextureType,
    pub texture_handle: TextureHandle,
    pub(crate) renderer_resource_handle: Option<RendererTextureHandle>,
}

impl MaterialTexture {
    pub fn new(texture_handle: TextureHandle) -> Self {
        Self {
            //texture_type,
            texture_handle,
            renderer_resource_handle: None,
        }
    }
}

// This needed so that renderer can get renderer texture handle from material texture while it is still hidden in game API
pub fn get_renderer_texture_handle_from_material_texture(material_texture: &MaterialTexture) -> &Option<RendererTextureHandle> {
    &material_texture.renderer_resource_handle
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
        self.material.textures.insert(slot_name.to_string() , MaterialTexture::new(texture_handle));
        Ok(self)
    }

    pub fn scalar_parameter(mut self, slot_name: &str, value: f32) -> Result<Self> {
        self.material.parameters.insert(slot_name.to_string(), MaterialParameter::Scalar(value));
        Ok(self)
    }

    pub fn bool_parameter(mut self, slot_name: &str, value: bool) -> Result<Self> {
        self.material.parameters.insert(slot_name.to_string(), MaterialParameter::Bool(value));
        Ok(self)
    }

    pub fn color_parameter(mut self, slot_name: &str, value: Color) -> Result<Self> {
        self.material.parameters.insert(slot_name.to_string(), MaterialParameter::Color(value));
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
    pub(crate) textures: IndexMap<String, MaterialTexture>,
    //pub(crate) textures_mapping: Vec<String>,  // Maps index to slot name, required for deferred update requests
    #[readonly]
    pub(crate) parameters: HashMap<String, MaterialParameter>,
    #[readonly]
    pub rendering_order: u8,

    pub(crate) renderer_resource_handle: Option<RendererMaterialHandle>,
    shader_name: Option<String>,
    handle: Option<MaterialHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Material {
    pub fn builder(name: &str) -> MaterialBuilder {
        MaterialBuilder::new(name)
    }

    // Creates default lit material with default shader and textures
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            shader_handle: get_default_lit_shader_handles().0,
            textures: IndexMap::new(),
            //textures_mapping: Vec::new(),
            parameters: HashMap::new(),
            rendering_order: RENDER_QUEUE_KEY_ORDER.max as u8,
            renderer_resource_handle: None,
            shader_name: None,
            handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn set_texture(&mut self, slot_name: &str, texture_handle: TextureHandle) -> Result<()> {

        // Get or insert new
        match self.textures.entry(slot_name.to_string()) {
            indexmap::map::Entry::Occupied(mut entry) => {
                entry.get_mut().texture_handle = texture_handle;
            }
            indexmap::map::Entry::Vacant(entry) => {
                entry.insert(MaterialTexture::new(texture_handle));
                //self.textures_mapping.push(slot_name.to_string());
            }
        }

        let slot_index = self.textures.get_index_of(slot_name).unwrap();

        // Get texture slot
        //let texture_slot = self.textures.get_mut(slot_name)
        //    .ok_or( Error::new(EngineError::MaterialTextureSlotNotFound(slot_name.to_string(), self.name.to_string())))?;

        // Get texture slot index
        //let texture_slot_index = self.textures_mapping.iter().position(|v| v == slot_name).expect("Critical: No mapping");

        // Set new handle but not renderer resource handle (it will be set by deferred update system)
      //  l//et _ = texture_slot.texture_handle = texture_handle;

        // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
        if self.renderer_resource_handle.is_some() {
            self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURE_START + slot_index);     // texture_slot_index);
        }

        Ok(())
    }

    // pub fn remove_texture(&mut self, slot_name: &str) -> Result<()> {
    //     // Get texture slot
    //     let texture_slot = self.textures.data.get_mut(slot_name)
    //         .ok_or( Error::new(EngineError::MaterialTextureSlotNotFound(slot_name.to_string(), self.name.to_string())))?;

    //     // Get texture slot index
    //     let texture_slot_index = self.textures.mapping.iter().position(|v| v == slot_name).expect("Critical: No mapping");

    //     // Set new handle and renderer resource handle
    //     texture_slot.texture_handle = None;
    //     texture_slot.renderer_texture_handle = None;

    //     // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
    //     if self.renderer_resource_handle.is_some() {
    //         self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_TEXTURE_START + texture_slot_index);
    //     }

    //     Ok(())
    // }

    pub fn set_rendering_order(&mut self, order: u8) -> Result<()> {
        let error = EngineError::WrongRenderingOrder(order.to_string(), format!("{}-{}", 0, RENDER_QUEUE_KEY_ORDER.max));
        if order < RENDER_QUEUE_KEY_ORDER.max as u8 {
            self.rendering_order = order;
            // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
            if self.renderer_resource_handle.is_some() {
                self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_RENDERING_ORDER);
            }
        }
        else {
            return Err(Error::new(error));
        }

        Ok(())
    }

    pub fn get_scalar_parameter(&self, parameter_name: &str) -> Result<f32> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Scalar".to_string(), self.name.to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Scalar(value) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn get_bool_parameter(&self, parameter_name: &str) -> Result<bool> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Bool".to_string(), self.name.to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Bool(value) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn get_color_parameter(&self, parameter_name: &str) -> Result<Color> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Color".to_string(), self.name.to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Color(value) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_scalar_parameter(&mut self, parameter_name: &str, value: f32) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Scalar".to_string(), self.name.to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Scalar(v) => {
                if *v != value {
                    *v = value;
                    // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
                    if self.renderer_resource_handle.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_bool_parameter(&mut self, parameter_name: &str, value: bool) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Bool".to_string(), self.name.to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Bool(v) => {
                if *v != value {
                    *v = value;
                    // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
                    if self.renderer_resource_handle.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_color_parameter(&mut self, parameter_name: &str, value: Color) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Color".to_string(), self.name.to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Color(v) => {
                // Clamp color channel values between 0.0 and 1.0
                let valid_color = Color::new(value.x.clamp(0.0, 1.0), value.y.clamp(0.0, 1.0), value.z.clamp(0.0, 1.0));
                if *v != valid_color {
                    *v = valid_color;
                    // Post deferred update request (only if renderer resource handle is set (it means that material is initialized))
                    if self.renderer_resource_handle.is_some() {
                        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_PARAMETER);
                    }
                }
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    fn validate_texture(&self, engine: &mut Engine, texture_slot_name: &str, texture_slot: &MaterialTexture) -> Result<()> {

        let shader = engine.get_resource::<Shader>(&self.shader_handle)?;

        // Get texture to be set
        let texture = engine.get_resource::<Texture>(&texture_slot.texture_handle)
            .context(EngineError::InvalidTextureHandleForSlot(texture_slot_name.to_string()))?;

        // Get texture slot from shader
        let shader_texture_slot = shader.texture_slots.get(texture_slot_name)
            .context(EngineError::MaterialTextureSlotNotFound(texture_slot_name.to_string()))?;

        // Check if slots are of the same type
        if !enum_variant_eq(&texture.texture_type,&shader_texture_slot.texture_type) {
            return Err(Error::new(EngineError::WrongTextureType(
                get_enum_variant_type_name(&texture.texture_type),
                texture_slot_name.to_string(),
                get_enum_variant_type_name(&shader_texture_slot.texture_type)
            )));
        }

        Ok(())
    }

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        let handle = self.handle.expect("Critical: Cannot post deferred update request. No Handle set in Resource");
        let request = DeferredUpdateResourceRequest::<Material>::new(handle, request_variant);
        self.deferred_update_manager.as_mut().expect("Critical: No DeferredUpdateManager").post_update_request(request);
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
        let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

        // Check if assigned textures are of correct type declared in shader
        for (texture_slot_name, texture_slot) in self.textures.iter() {
            self.validate_texture(engine, texture_slot_name, texture_slot)?;
        }

        // Assign renderer resource handle to texture slot
        for (texture_slot_name, texture_slot) in self.textures.iter_mut () {
            let texture = engine.get_resource::<Texture>(&texture_slot.texture_handle)
                .context(EngineError::InvalidTextureHandleForSlot(texture_slot_name.to_string()))?;
            texture_slot.renderer_resource_handle = Some(texture.renderer_resource_handle.unwrap());
        }

        // Create new renderer material resource
        let shader = engine.get_resource::<Shader>(&self.shader_handle)?;
        self.shader_name = Some(shader.get_name());
        let shader_renderer_resource_handle = shader.renderer_resource_handle.unwrap();
        let renderer_resource_handle = engine.renderer.create_material(&self.name, shader_renderer_resource_handle, &self.textures, &self.parameters)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {
        self.handle = Some(MaterialHandle::from(self_handle.data()));
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> {
        match request {
            DEFERRED_REQUEST_VARIANT_RENDERING_ORDER =>
            {
                // Find mesh rendering components that use this material and update them
                for (_scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
                    for (_entity_handle, mesh_rendering_component) in scene.get_one_component_iterator_mut::<MeshRenderingComponent>()? {
                        if let Some(material_handle) = mesh_rendering_component.material_handle {
                            // If mesh rendering component has handle to this material
                            if material_handle.data() == self.handle.unwrap().data() {
                                mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
                            }
                        }
                    }
                }
            },
            DEFERRED_REQUEST_VARIANT_PARAMETER =>
            {
                // Update renderer counterpart
                engine.renderer.update_material_parameters(self.renderer_resource_handle.unwrap(), &self.parameters)?;
            },
            DEFERRED_REQUEST_VARIANT_TEXTURE_START..=DEFERRED_REQUEST_VARIANT_TEXTURE_END =>
            {
                // Check if assigned texture is of correct type
                let (texture_slot_name, texture_slot) = self.textures.get_index(request - DEFERRED_REQUEST_VARIANT_TEXTURE_START).unwrap();
                self.validate_texture(engine, texture_slot_name, texture_slot)?;

                // Assign renderer resource handle to texture slot
                let (texture_slot_name, texture_slot) = self.textures.get_index_mut(request - DEFERRED_REQUEST_VARIANT_TEXTURE_START).unwrap();
                let texture = engine.get_resource::<Texture>(&texture_slot.texture_handle)
                    .context(EngineError::InvalidTextureHandleForSlot(texture_slot_name.to_string()))?;
                texture_slot.renderer_resource_handle = Some(texture.renderer_resource_handle.unwrap());

                // Update renderer counterpart
                engine.renderer.update_material_textures(self.renderer_resource_handle.unwrap(), &self.textures)?;
            },
            _ =>
            {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
            }
        }

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, _self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_material(v).unwrap();
        }

        // Find mesh rendering components that use this material and update them



        //for (_scene_handle, _scene) in engine.scene_manager.scenes.iter_mut() {
        //    let x = &engine.resource_manager;

            // for (entity_handle, mesh_rendering_component) in engine.iterate_one_component::<MeshRenderingComponent>()? {
            //     if let Some(material_handle) = mesh_rendering_component.material_handle {
            //         // If mesh rendering component has handle to this material
            //         if material_handle.data() == self_handle.data() {
            //             mesh_rendering_component.set_material_handle(Option::<MaterialHandle>::None);
            //             mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
            //         }
            //     }
            // }
        //}

        Ok(())
    }
}
