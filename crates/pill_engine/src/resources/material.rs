use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use pill_core::EngineError;
use pill_core::PillSlotMapKey;
use pill_core::na::SliceRange;

use anyhow::{Result, Context, Error};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use boolinator::*;

// --- Material parameters ---

#[derive(Debug)]
pub enum MaterialParameter {
    Scalar(Option<f32>),
    Bool(Option<bool>),
    Color(Option<cgmath::Vector3::<f32>>),
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

pub type ParameterMap = HashMap<String, MaterialParameter>;

pub trait MaterialParameterMap {
    fn get_scalar(&self, parameter_name: &str) -> Result<f32>;
    fn get_bool(&self, parameter_name: &str) -> Result<bool>;
    fn get_color(&self, parameter_name: &str) -> Result<cgmath::Vector3::<f32>>;

    fn set_parameter(&mut self, parameter_name: &str, value: MaterialParameter) -> Result<()>;
}

impl MaterialParameterMap for ParameterMap {
    fn get_scalar(&self, parameter_name: &str) -> Result<f32> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Scalar".to_string());
        match self.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Scalar(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }

    fn get_bool(&self, parameter_name: &str) -> Result<bool> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Bool".to_string());
        match self.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Bool(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }
   
    fn get_color(&self, parameter_name: &str) -> Result<cgmath::Vector3::<f32>> {
        let error = EngineError::MaterialParameterNotFound(parameter_name.to_string(), "Color".to_string());
        match self.get(parameter_name).context(format!("{}", error))? {
            MaterialParameter::Color(v) => match v {
                Some(vv) => Ok(vv.clone()),
                None => panic!(),
            },
            _ => Err(Error::new(error))
        }
    }

    fn set_parameter(&mut self, parameter_name: &str, value: MaterialParameter) -> Result<()> {
        let error = Error::new(EngineError::MaterialParameterNotFound(parameter_name.to_string(), pill_core::get_enum_variant_type_name(&value).to_string()));
        let parameter = self.get_mut(parameter_name).context(format!("{}", error))?;

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


pub type TextureMap = HashMap<String, MaterialTexture>;

// --- Material ---

#[readonly::make]
pub struct Material {
    #[readonly]
    pub name: String,
    #[readonly]
    pub textures: TextureMap,
    #[readonly]
    pub parameters: ParameterMap,
    #[readonly]
    pub order: u32,
    pub(crate) renderer_resource_handle: Option<RendererMaterialHandle>,
}

impl Material {

    pub fn g(&mut self) -> &mut TextureMap {
        &mut self.textures
    }

    pub fn new(name: &str) -> Self {  // [TODO] What if renderer fails to create material?        
        let mut textures = TextureMap::new();
        textures.insert("Color".to_string(), MaterialTexture::Color(None));
        textures.insert("Normal".to_string(), MaterialTexture::Normal(None));

        let mut parameters = ParameterMap::new();
        parameters.insert("Tint".to_string(), MaterialParameter::Color(None));
        
        Self {
            name: name.to_string(),  
            textures,
            parameters,
            order: 0,
            renderer_resource_handle: None, 
        }
    }

    pub fn set_texture(&mut self, engine: &mut Engine, texture_name: &str, texture_handle: TextureHandle) -> Result<()> {
        let texture = engine.resource_manager.get_resource::<TextureHandle, Texture>(&texture_handle)?;
        let renderer_texture_handle = texture.renderer_resource_handle.unwrap();

        let texture_entry = self.textures
            .get_mut(texture_name)
            .ok_or( Error::new(EngineError::MaterialTextureNotFound(texture_name.to_string())))?;

        // Check if texture type is valid for this material texture 
        match texture_entry {
            MaterialTexture::Color(v) => {
                match texture.texture_type {
                    TextureType::Color => { v.replace((texture_handle, renderer_texture_handle)); },
                    _ => { return Err(Error::new(EngineError::WrongTextureType(pill_core::get_enum_variant_type_name(&texture.texture_type).to_string(), "Color".to_string()))) },
                };
            },
            MaterialTexture::Normal(v) => {
                match texture.texture_type {
                    TextureType::Normal => { v.replace((texture_handle, renderer_texture_handle)); },
                    _ => { return Err(Error::new(EngineError::WrongTextureType(pill_core::get_enum_variant_type_name(&texture.texture_type).to_string(), "Normal".to_string()))) },
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

    pub fn set_order(&mut self, _engine: &mut Engine, order: u32) {
        // if order >= 2^order_mask_range.end { } // [TODO] Implement failing in not in range
        self.order = order;

        //engine.renderer.update_material(self.renderer_resource_index, self);
    }

    pub fn get_scalar(&self, parameter_name: &str) -> Result<f32> {
        self.parameters.get_scalar(parameter_name)
    }

    pub fn get_bool(&self, parameter_name: &str) -> Result<bool> {
        self.parameters.get_bool(parameter_name)
    }

    pub fn get_color(&self, parameter_name: &str) -> Result<cgmath::Vector3::<f32>> {
        self.parameters.get_color(parameter_name)
    }

    pub fn set_scalar(&mut self, engine: &mut Engine, parameter_name: &str, value: f32) -> Result<()> {
        self.set_parameter(engine, parameter_name, MaterialParameter::Scalar(Some(value)))
    }

    pub fn set_bool(&mut self, engine: &mut Engine, parameter_name: &str, value: bool) -> Result<()> {
        self.set_parameter(engine, parameter_name, MaterialParameter::Bool(Some(value)))
    }

    pub fn set_color(&mut self, engine: &mut Engine, parameter_name: &str, value: cgmath::Vector3::<f32>) -> Result<()> {
        self.set_parameter(engine, parameter_name, MaterialParameter::Color(Some(value)))
    }

    fn set_parameter(&mut self, engine: &mut Engine, parameter_name: &str, value: MaterialParameter) -> Result<()> {
        self.parameters.set_parameter(parameter_name, value)?;
        if self.renderer_resource_handle.is_some() {
            engine.renderer.update_material_parameters(self.renderer_resource_handle.unwrap(), &self.parameters)?;
        }
        Ok(())
    }
}

impl Resource for Material {
    fn initialize(&mut self, engine: &mut Engine) {

        // Set default textures if non texture is set
        // Add only if nothing else is already there 
        // (since initialization of this resource happens in moment it is being added to the engine and
        // since it is possible to set textures before that moment it may happen that textures are already
        // defined by the user so setting default ones is not needed)
        let texture_values = vec![
            ("Color", MaterialTexture::Color(None)), 
            ("Normal", MaterialTexture::Normal(None)), 
        ]; // [TODO] Move magic values
        for texture_value in texture_values {
            let texture = self.textures.get_mut(texture_value.0);
            match texture {
                Some(v) => {
                    if pill_core::enum_variant_eq::<MaterialTexture>(&texture_value.1, v) {
                        let texture_data= v.get_texture_data_mut();
                        if texture_data.is_none() {
                            let default_texture_data = engine.resource_manager.get_default_texture(texture_value.0).unwrap();
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
            ("Tint", MaterialParameter::Color(Some(cgmath::Vector3::<f32>::new(1.0, 1.0, 1.0))))
        ]; // [TODO] Move magic values
        for parameter_value in parameter_values {
            let parameter = self.parameters.get_mut(parameter_value.0);
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
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) {

        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_material(v).unwrap();
        }

        // Find mesh rendering components that use this material and update them
        let default_material = engine.resource_manager.get_default_material().unwrap();
        let active_scene = engine.scene_manager.get_active_scene_mut().unwrap();
        let mesh_rendering_component_storage = active_scene.get_component_storage_mut::<MeshRenderingComponent>().unwrap();
        for i in 0..mesh_rendering_component_storage.data.len() {
            if let Some(mesh_rendering_component) = mesh_rendering_component_storage.data.get_mut(i).unwrap().as_mut() {
                mesh_rendering_component.material_handle = Some(default_material.0);
                mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
            }
            // [TODO] Instead of this use "mesh_rendering_component.assign_material(engine, &default_material.0);". This requires component wrapped with option or refcell
        }
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

impl typemap_rev::TypeMapKey for Material {
    type Value = Option<ResourceStorage<MaterialHandle, Material>>; 
}