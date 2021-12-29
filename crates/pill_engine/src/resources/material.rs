use crate::config::*;
use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use pill_core::Color;
use pill_core::EngineError;
use pill_core::PillSlotMapKey;

use anyhow::{Result, Context, Error};
use typemap_rev::TypeMapKey;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use boolinator::*;

// --- Material parameters ---

#[derive(Debug)]
pub enum MaterialParameter {
    Scalar(Option<f32>),
    Bool(Option<bool>),
    Color(Option<Color>),
}

impl MaterialParameter {
    pub fn is_some(&self) -> bool {
        match self {
            MaterialParameter::Scalar(v) => v.is_some(),
            MaterialParameter::Bool(v) => v.is_some(),
            MaterialParameter::Color(v) => v.is_some(),
        }
    }
}

pub struct MaterialParameterMap {
    pub(crate) data: HashMap<String, MaterialParameter>,
    pub(crate) mapping: Vec<String>, // Maps index to slot name
}

impl MaterialParameterMap {
    pub fn new() -> Self {
        Self {
            data: HashMap::<String, MaterialParameter>::new(),
            mapping: Vec::<String>::new(),
        }
    }

    pub fn get_scalar(&self, parameter_name: &str) -> Result<f32> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Scalar".to_string());
        match self.data.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Scalar(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }

    pub fn get_bool(&self, parameter_name: &str) -> Result<bool> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Bool".to_string());
        match self.data.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Bool(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }
   
    pub fn get_color(&self, parameter_name: &str) -> Result<Color> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Color".to_string());
        match self.data.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Color(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }

    pub fn set_parameter(&mut self, parameter_name: &str, value: MaterialParameter) -> Result<()> {
        let error = Error::new(EngineError::MaterialParameterNotFound(parameter_name.to_string(), pill_core::get_enum_variant_type_name(&value).to_string()));
        let parameter = self.data.get_mut(parameter_name).context(format!("{}", error))?;

        if pill_core::enum_variant_eq::<MaterialParameter>(&parameter, &value) {
            *parameter = value; 
        }
          
        Ok(())
    }
}

// --- Material textures ---

#[derive(Debug)]
pub enum MaterialTexture {
    Color(Option<(TextureHandle, RendererTextureHandle)>),
    Normal(Option<(TextureHandle, RendererTextureHandle)>),
}

impl MaterialTexture {
    pub fn get_type(&self) -> TextureType {
        match self {
            MaterialTexture::Color(_) => TextureType::Color,
            MaterialTexture::Normal(_) => TextureType::Normal,
        }
    }
}

impl MaterialTexture {
    pub fn get_texture_data(&self) -> &Option<(TextureHandle, RendererTextureHandle)> {
        match self {
            MaterialTexture::Color(v) => v,
            MaterialTexture::Normal(v) => v,
        }
    }
    pub fn get_texture_data_mut(&mut self) -> &mut Option<(TextureHandle, RendererTextureHandle)> {
        match self {
            MaterialTexture::Color(v) => v,
            MaterialTexture::Normal(v) => v,
        }
    }
}

pub struct MaterialTextureMap {
    pub data: HashMap<String, MaterialTexture>,
    pub(crate) mapping: Vec<String>, // Maps index to slot name
}

impl MaterialTextureMap {
    pub fn new() -> Self {
        Self {
            data: HashMap::<String, MaterialTexture>::new(),
            mapping: Vec::<String>::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&MaterialTexture> {
        self.data.get(name)
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
    textures: MaterialTextureMap,
    #[readonly]
    parameters: MaterialParameterMap,
    #[readonly]
    pub rendering_order: u8,
    pub renderer_resource_handle: Option<RendererMaterialHandle>,
   
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Material {

    pub fn new(name: &str) -> Self {  // [TODO] What if renderer fails to create material?        
        let mut textures = MaterialTextureMap::new();
        textures.data.insert(MASTER_SHADER_COLOR_TEXTURE_SLOT.to_string(), MaterialTexture::Color(None));
        textures.mapping.insert(0, MASTER_SHADER_COLOR_TEXTURE_SLOT.to_string());
        textures.data.insert(MASTER_SHADER_NORMAL_TEXTURE_SLOT.to_string(), MaterialTexture::Normal(None));
        textures.mapping.insert(1, MASTER_SHADER_NORMAL_TEXTURE_SLOT.to_string());

        let mut parameters = MaterialParameterMap::new();
        parameters.data.insert(MASTER_SHADER_TINT_PARAMETER_SLOT.to_string(), MaterialParameter::Color(None));
        textures.mapping.insert(0, MASTER_SHADER_TINT_PARAMETER_SLOT.to_string());
        
        Self {
            name: name.to_string(),  
            textures,
            parameters,
            rendering_order: RENDER_QUEUE_KEY_ORDER.max as u8,
            renderer_resource_handle: None, 
            deferred_update_manager: None,
        }
    }

    pub fn set_texture(&mut self, engine: &mut Engine, slot_name: &str, texture_handle: TextureHandle) -> Result<()> {

        let texture = engine.resource_manager.get_resource::<Texture>(&texture_handle)?;
        let renderer_texture_handle = texture.renderer_resource_handle.unwrap();

        let texture_entry = self.textures.data
            .get_mut(slot_name)
            .ok_or( Error::new(EngineError::MaterialTextureNotFound(slot_name.to_string())))?;

        // Check if texture type is valid for this material texture 
        match texture_entry {
            MaterialTexture::Color(v) => {
                match texture.texture_type {
                    TextureType::Color => { v.replace((texture_handle, renderer_texture_handle)); },
                    _ => { return Err(Error::new(EngineError::WrongTextureType(pill_core::get_enum_variant_type_name(&texture.texture_type).to_string(), pill_core::get_enum_variant_type_name(texture_entry).to_string()))) },
                };
            },
            MaterialTexture::Normal(v) => {
                match texture.texture_type {
                    TextureType::Normal => { v.replace((texture_handle, renderer_texture_handle)); },
                    _ => { return Err(Error::new(EngineError::WrongTextureType(pill_core::get_enum_variant_type_name(&texture.texture_type).to_string(), pill_core::get_enum_variant_type_name(texture_entry).to_string()))) },
                };
            },
        }
       
        // Check if material is added to engine, if so then also its renderer resource has to be updated
        if self.renderer_resource_handle.is_some() {
            engine.renderer.update_material_textures(self.renderer_resource_handle.unwrap(), &self.textures)?;
        }

        Ok(())

        //[TODO] Revert material if failed to update in renderer ??
    }

    pub fn set_order(&mut self, engine: &mut Engine, order: u8) -> Result<()> {
        match order > RENDER_QUEUE_KEY_ORDER.max as u8 {
            true => {
                return Err(Error::new(EngineError::WrongRenderingOrder(order.to_string(), format!("{}-{}", 0, RENDER_QUEUE_KEY_ORDER.max.to_string()))));
            },
            false => {
                self.rendering_order = order;

                let self_handle = engine.get_resource_handle::<Material>(&self.name)?;

                // Update render queue keys in all mesh rendering components that use this material
                for scene in engine.scene_manager.scenes.iter_mut() { // [TODO] Getting user, duplicated code, create iterator for that
                    let mesh_rendering_component_storage = scene.1.get_component_storage_mut::<MeshRenderingComponent>().unwrap();
                    for i in 0..mesh_rendering_component_storage.data.len() {
                        if let Some(mesh_rendering_component) = mesh_rendering_component_storage.data.get_mut(i).unwrap().as_mut() {
                            if let Some(material_handle) = mesh_rendering_component.material_handle {
                                if material_handle.data() == self_handle.data() {
                                    mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
                                }
                            }
                        }
                    }
                }

                Ok(())
            },
        }
      

      

        //engine.renderer.update_material(self.renderer_resource_index, self);
    }

    pub fn get_scalar(&self, parameter_name: &str) -> Result<f32> {
        self.parameters.get_scalar(parameter_name)
    }

    pub fn get_bool(&self, parameter_name: &str) -> Result<bool> {
        self.parameters.get_bool(parameter_name)
    }

    pub fn get_color(&self, parameter_name: &str) -> Result<Color> {
        self.parameters.get_color(parameter_name)
    }

    pub fn set_scalar(&mut self, engine: &mut Engine, parameter_name: &str, value: f32) -> Result<()> {
        self.set_parameter(engine, parameter_name, MaterialParameter::Scalar(Some(value)))
    }

    pub fn set_bool(&mut self, engine: &mut Engine, parameter_name: &str, value: bool) -> Result<()> {
        self.set_parameter(engine, parameter_name, MaterialParameter::Bool(Some(value)))
    }

    pub fn set_color(&mut self, engine: &mut Engine, parameter_name: &str, value: Color) -> Result<()> {
        // Clamp color channel values between 0.0 and 1.0
        let valid_color = Color::new(value.x.clamp(0.0, 1.0), value.y.clamp(0.0, 1.0), value.z.clamp(0.0, 1.0));
        self.set_parameter(engine, parameter_name, MaterialParameter::Color(Some(valid_color)))
    }

    fn set_parameter(&mut self, engine: &mut Engine, parameter_name: &str, value: MaterialParameter) -> Result<()> {
        self.parameters.set_parameter(parameter_name, value)?;

        //self.deferred_update_manager.post_request();

        if self.renderer_resource_handle.is_some() {
            engine.renderer.update_material_parameters(self.renderer_resource_handle.unwrap(), &self.parameters)?;
        }
        Ok(())
    }

    pub(crate) fn get_textures(&mut self) -> &mut MaterialTextureMap {
        &mut self.textures
    }
}

impl Resource for Material {
    type Handle = MaterialHandle;

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {

        // TODO: REPLACE WITH PROPER GLOBAL COMPONENTS IMPLEMENTATION
        // This resource is using DeferredUpdateSystem so keep DeferredUpdateManager
        self.deferred_update_manager = Some(engine.TEMP_deferred_component.borrow_deferred_update_manager());

        // Set default textures if non texture is set
        // Add only if nothing else is already there 
        // (since initialization of this resource happens in moment it is being added to the engine and
        // since it is possible to set textures before that moment it may happen that textures are already
        // defined by the user so setting default ones is not needed)

        // 0 - Name of slot, 1 - Type of texture in it, 2 - Type of texture in general
        let texture_values = vec![
            (MASTER_SHADER_COLOR_TEXTURE_SLOT, MaterialTexture::Color(None)), 
            (MASTER_SHADER_NORMAL_TEXTURE_SLOT, MaterialTexture::Normal(None)), 
        ];
        for texture_value in texture_values {
            let texture = self.textures.data.get_mut(texture_value.0);
            match texture {
                Some(v) => {
                    if pill_core::enum_variant_eq::<MaterialTexture>(&texture_value.1, v) {
                        let texture_data= v.get_texture_data_mut();
                        if texture_data.is_none() {
                            let default_texture_data = engine.resource_manager.get_default_texture(texture_value.1.get_type()).expect("Critical: No default Resource");
                            *texture_data = Some(default_texture_data);  
                        }
                    }
                    else {
                        panic!();
                    }
                },
                None => panic!(),
            }
        }

        // Set default parameters if not already set
        let parameter_values = vec![
            (MASTER_SHADER_TINT_PARAMETER_SLOT, MaterialParameter::Color(Some(Color::new(1.0, 1.0, 1.0))))
        ];
        for parameter_value in parameter_values {
            let parameter = self.parameters.data.get_mut(parameter_value.0);
            match parameter {
                Some(v) => {
                    if pill_core::enum_variant_eq::<MaterialParameter>(&parameter_value.1, v) {
                        if !v.is_some() {
                            *v = parameter_value.1;
                        }
                    }
                },
                None => panic!(),
            }
        }

        // Create new renderer material resource
        let renderer_resource_handle = engine.renderer.create_material(
            &self.name,
            &self.textures,
            &self.parameters,
        ).unwrap();

        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    // fn deferred_update(&mut self, request: DeferredUpdateRequest) {
    //     match request {
    //         DeferredUpdateRequest::MaterialOrder => todo!(),
    //     }
    // }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) {

        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_material(v).unwrap();
        }

        // Find mesh rendering components that use this material and update them
        let default_material = engine.resource_manager.get_default_material().unwrap();
        for scene in engine.scene_manager.scenes.iter_mut() {
            let mesh_rendering_component_storage = scene.1.get_component_storage_mut::<MeshRenderingComponent>().unwrap();
            for i in 0..mesh_rendering_component_storage.data.len() {
                if let Some(mesh_rendering_component) = mesh_rendering_component_storage.data.get_mut(i).unwrap().as_mut() {
                    if let Some(material_handle) = mesh_rendering_component.material_handle {
                        if material_handle.data() == self_handle.data() {
                            mesh_rendering_component.material_handle = Some(default_material.0);
                            mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
                            // [TODO] Instead of this use "mesh_rendering_component.set_material(engine, &default_material.0);". This requires component wrapped with option or refcell
                        }
                    }
                }
            }
        }
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

// pub enum MaterialDeferredUpdateRequests {
//     OrderUpdated,

// }



impl TypeMapKey for Material {
    type Value = ResourceStorage<Material>; 
}