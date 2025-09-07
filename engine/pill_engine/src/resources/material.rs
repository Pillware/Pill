use crate::{
    config::*, ecs::{
        DeferredOperationComponent, DeferredOperationManagerPointer, MeshRenderingComponent 
    }, engine::{self, Engine}, game::ShaderValueParameterType, graphics::{ 
        RendererMaterialHandle, 
        RendererTextureHandle, 
        RENDER_QUEUE_KEY_ORDER 
    }, internal::{MaterialParameter, ValueParameter}, resources::{ 
        resource::ResourceDeferredOperation, MaterialParametersStore, Resource, ResourceStorage, Shader, ShaderHandle, Texture, TextureHandle, TextureParameter, TextureType }
};

use pill_core::{ 
    debug, 
    enum_variant_eq, 
    get_enum_variant_type_name, 
    get_type_name, 
    Color, 
    EngineError, 
    LogContext, 
    PillSlotMapKey,
    PillStyle, 
    PillTypeMapKey, Vector2f 
};

use anyhow::{ Result, Context, Error };
use boolinator::*;
use std::{ 
    collections::{hash_map::Entry, HashMap}, 
    ops::{Range, RangeInclusive}, 
    path::{ Path, PathBuf }, sync::Arc 
};
use indexmap::IndexMap;

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
        self.material.set_shader(shader_handle)?;
        Ok(self)
    }

    pub fn rendering_order(mut self, order: u8) -> Result<Self> {
        self.material.set_rendering_order(order)?;
        Ok(self)
    }

    pub fn float_parameter(mut self, slot_name: &str, value: f32) -> Result<Self> {
        self.material.set_float_parameter(slot_name, value)?;
        Ok(self)
    }

    pub fn bool_parameter(mut self, slot_name: &str, value: bool) -> Result<Self> {
        self.material.set_bool_parameter(slot_name, value)?;
        Ok(self)
    }

    pub fn color_parameter(mut self, slot_name: &str, value: Color) -> Result<Self> {
        self.material.set_color_parameter(slot_name, value)?;
        Ok(self)
    }

    pub fn vector2_parameter(mut self, slot_name: &str, value: Vector2f) -> Result<Self> {
        self.material.set_vector2_parameter(slot_name, value)?;
        Ok(self)
    }

    pub fn texture(mut self, slot_name: &str, texture_handle: TextureHandle) -> Result<Self> {
        self.material.set_texture_parameter(slot_name, texture_handle)?;
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

// Material is always created in context of the shader it uses
// This means that all parameters slots are created by the shader

#[readonly::make]
pub struct Material {
    #[readonly]
    pub name: String,
    #[readonly]
    pub shader_handle: ShaderHandle,
    #[readonly]
    pub rendering_order: u8,

    pub(crate) parameters_store: MaterialParametersStore,

    pub(crate) renderer_resource_handle: Option<RendererMaterialHandle>,
    shader_name: Option<String>,

    handle: Option<MaterialHandle>,
    deferred_operation_manager: Option<DeferredOperationManagerPointer>,

    renderer_resource_update_operation_scheduled: bool,
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
            rendering_order: RENDER_QUEUE_KEY_ORDER.max as u8,
            parameters_store: MaterialParametersStore::default(),
            renderer_resource_handle: None, 
            shader_name: None,
            handle: None,
            deferred_operation_manager: None,
            renderer_resource_update_operation_scheduled: false,
        }
    }

    pub fn set_shader(&mut self, shader_handle: ShaderHandle) -> Result<()> {
        if !self.is_initialized() {
           self.shader_handle = shader_handle;
        }
        else {
            self.schedule_deferred_operation(Box::new(move |self_material: &mut Material, engine: &mut Engine| {
                unimplemented!("Changing shader of an initialized material is not yet supported");
            }));
        }

        Ok(())
    }

    pub fn set_rendering_order(&mut self, order: u8) -> Result<()> {
        let error = EngineError::WrongRenderingOrder(order.to_string(), format!("{}-{}", 0, RENDER_QUEUE_KEY_ORDER.max.to_string()));
        if order < RENDER_QUEUE_KEY_ORDER.max as u8 {
            self.rendering_order = order;

            if self.is_initialized() {
                self.schedule_deferred_operation(Box::new(move |self_material: &mut Material, engine: &mut Engine| {
                    // Find mesh rendering components that use this material and update them
                    for (scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
                        for (entity_handle, mesh_rendering_component) in scene.get_one_component_iterator_mut::<MeshRenderingComponent>()? {
                            if let Some(material_handle) = mesh_rendering_component.material_handle {
                                // If mesh rendering component has handle to this material 
                                if material_handle.data() == self_material.handle.unwrap().data() {
                                    mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
                                }
                            }
                        }
                    }

                    Ok(())
                }));
            }
        }
        else {
            return Err(Error::new(error));
        }

        Ok(())
    }

    pub fn get_float_parameter(&self, parameter_name: &str) -> Result<f32> {
        self.parameters_store.get_float_parameter(parameter_name).context(EngineError::FailedToGetMaterialParameter(parameter_name.to_string(), "Float".to_string(), self.name.to_string()))
    }

    pub fn set_float_parameter(&mut self, parameter_name: &str, value: f32) -> Result<()> {

        // Before initialization, parameter slots are not yet created, so we need to allow setting values without any validation
        if !self.is_initialized() {
            if self.parameters_store.does_float_parameter_exist(parameter_name) {
                self.parameters_store.set_float_parameter(parameter_name, value)?;
            } else {
                self.parameters_store.add_float_parameter(parameter_name, value)?;
            }
        }
        else {
            self.parameters_store.set_float_parameter(parameter_name, value).context(EngineError::FailedToSetMaterialParameter(parameter_name.to_string(), "Float".to_string(), self.name.to_string()));
            self.schedule_deferred_post_parameter_set_update_operation();
        }

        Ok(())
    }

    pub fn get_bool_parameter(&self, parameter_name: &str) -> Result<bool> {
        self.parameters_store.get_bool_parameter(parameter_name).context(EngineError::FailedToGetMaterialParameter(parameter_name.to_string(), "Bool".to_string(), self.name.to_string()))
    }

    pub fn set_bool_parameter(&mut self, parameter_name: &str, value: bool) -> Result<()> {

        // Before initialization, parameter slots are not yet created, so we need to allow setting values without any validation
        if !self.is_initialized() {
            if self.parameters_store.does_bool_parameter_exist(parameter_name) {
                self.parameters_store.set_bool_parameter(parameter_name, value)?;
            } else {
                self.parameters_store.add_bool_parameter(parameter_name, value)?;
            }
        }
        else {
            self.parameters_store.set_bool_parameter(parameter_name, value).context(EngineError::FailedToSetMaterialParameter(parameter_name.to_string(), "Bool".to_string(), self.name.to_string()));
            self.schedule_deferred_post_parameter_set_update_operation();
        }

        Ok(())
    }

    pub fn get_color_parameter(&self, parameter_name: &str) -> Result<Color> {
        self.parameters_store.get_color_parameter(parameter_name).context(EngineError::FailedToGetMaterialParameter(parameter_name.to_string(), "Color".to_string(), self.name.to_string()))
    }

    pub fn set_color_parameter(&mut self, parameter_name: &str, value: Color) -> Result<()> {

        // Before initialization, parameter slots are not yet created, so we need to allow setting values without any validation
        if !self.is_initialized() {
            if self.parameters_store.does_color_parameter_exist(parameter_name) {
                self.parameters_store.set_color_parameter(parameter_name, value)?;
            } else {
                self.parameters_store.add_color_parameter(parameter_name, value)?;
            }
        }
        else {
            self.parameters_store.set_color_parameter(parameter_name, value).context(EngineError::FailedToSetMaterialParameter(parameter_name.to_string(), "Color".to_string(), self.name.to_string()));
            self.schedule_deferred_post_parameter_set_update_operation();
        }

        Ok(())
    }

    pub fn get_vector2_parameter(&self, parameter_name: &str) -> Result<Vector2f> {
        self.parameters_store.get_vector2_parameter(parameter_name).context(EngineError::FailedToGetMaterialParameter(parameter_name.to_string(), "Vector2".to_string(), self.name.to_string()))
    }

    pub fn set_vector2_parameter(&mut self, parameter_name: &str, value: Vector2f) -> Result<()> {

        // Before initialization, parameter slots are not yet created, so we need to allow setting values without any validation
        if !self.is_initialized() {
            if self.parameters_store.does_vector2_parameter_exist(parameter_name) {
                self.parameters_store.set_vector2_parameter(parameter_name, value)?;
            } else {
                self.parameters_store.add_vector2_parameter(parameter_name, value)?;
            }
        }
        else {
            self.parameters_store.set_vector2_parameter(parameter_name, value).context(EngineError::FailedToSetMaterialParameter(parameter_name.to_string(), "Vector2".to_string(), self.name.to_string()));
            self.schedule_deferred_post_parameter_set_update_operation();
        }

        Ok(())
    }

    pub fn get_texture_parameter(&self, parameter_name: &str) -> Result<&TextureParameter> {
        self.parameters_store.get_texture_parameter(parameter_name).context(EngineError::FailedToGetMaterialParameter(parameter_name.to_string(), "Texture".to_string(), self.name.to_string()))
    }

    pub fn set_texture_parameter(&mut self, parameter_name: &str, texture_handle: TextureHandle) -> Result<()> {

        // Before initialization, parameter slots are not yet created, so we need to allow setting values without any validation
        if !self.is_initialized() {
            if self.parameters_store.does_texture_parameter_exist(parameter_name) {
                self.parameters_store.set_texture_parameter(parameter_name, TextureParameter::new(texture_handle))?;
            } else {
                self.parameters_store.add_texture_parameter(parameter_name, TextureParameter::new(texture_handle))?;
            }
        }
        else {
            self.parameters_store.set_texture_parameter(parameter_name, TextureParameter::new(texture_handle))?;

            let parameter_name = parameter_name.to_string();
            self.schedule_deferred_operation(Box::new(move |self_material: &mut Material, engine: &mut Engine| {
                // First get the texture handle to avoid borrowing conflicts
                let texture_handle = {
                    let texture_parameter = self_material.parameters_store.get_texture_parameter(&parameter_name).unwrap();
                    self_material.validate_texture(engine, &parameter_name, &texture_parameter)?;
                    texture_parameter.texture_handle.unwrap()
                };

                // Get texture and its renderer handle
                let texture = engine.get_resource::<Texture>(&texture_handle)
                    .context(EngineError::InvalidTextureHandleForTextureParameterSlot(parameter_name.to_string()))?;
                let renderer_texture_handle = texture.renderer_resource_handle.unwrap();

                // Now assign renderer resource handle to texture slot
                let texture_parameter = self_material.parameters_store.get_texture_parameter_mut(&parameter_name).unwrap();
                texture_parameter.renderer_texture_handle = Some(renderer_texture_handle);

                // Update renderer counterpart
                engine.renderer.update_material_parameters(self_material.renderer_resource_handle.unwrap(), &self_material.parameters_store.parameters)?;
                self_material.parameters_store.is_dirty = false;
                self_material.renderer_resource_update_operation_scheduled = false;

                Ok(())
            }));

            self.renderer_resource_update_operation_scheduled = true;
        }

        Ok(())
    }

    pub fn reset_texture_parameter(&mut self, parameter_name: &str) -> Result<()> {
        // Get texture slot
        let texture_parameter_slot = self.parameters_store.get_texture_parameter_mut(parameter_name)
            .context(format!("Failed to reset texture from {0} parameter", parameter_name.name_style()))?;

        // Set new handle and renderer resource handle
        texture_parameter_slot.texture_handle = None;
        texture_parameter_slot.renderer_texture_handle = None;

        let parameter_name = parameter_name.to_string();
        self.schedule_deferred_operation(Box::new(move |self_material: &mut Material, engine: &mut Engine| {
            // Check if assigned texture is of correct type
            let texture_parameter = self_material.parameters_store.get_texture_parameter_mut(&parameter_name).unwrap();

            // Update renderer counterpart
            engine.renderer.update_material_parameters(self_material.renderer_resource_handle.unwrap(), &self_material.parameters_store.parameters)?;
            self_material.parameters_store.is_dirty = false;
            self_material.renderer_resource_update_operation_scheduled = false;

            Ok(())
        }));

        Ok(())
    }

    fn schedule_deferred_post_parameter_set_update_operation(&mut self) {
         if !self.renderer_resource_update_operation_scheduled {
            self.schedule_deferred_operation(Box::new(move |self_material: &mut Material, engine: &mut Engine| {
                // Update renderer counterpart
                engine.renderer.update_material_parameters(self_material.renderer_resource_handle.unwrap(), &self_material.parameters_store.parameters)?;
                self_material.parameters_store.is_dirty = false;
                self_material.renderer_resource_update_operation_scheduled = false;
                Ok(())
            }));
            self.renderer_resource_update_operation_scheduled = true;
        }
    }

    // Validates if texture assigned to the slot is of correct type declared in shader
    fn validate_texture(&self, engine: &mut Engine, texture_parameter_slot_name: &str, texture_parameter: &TextureParameter) -> Result<()> {
        let shader = engine.get_resource::<Shader>(&self.shader_handle)?;

        // Get texture to be set
        let texture_handle = texture_parameter.texture_handle.unwrap();
        let texture = engine.get_resource::<Texture>(&texture_handle)
            .context(EngineError::WrongTextureHandleForTextureParameterSlot(texture_parameter_slot_name.to_string()))?;

        // Get texture slot from shader
        let shader_texture_slot = shader.texture_parameters_slots.get(texture_parameter_slot_name)
            .context(EngineError::MaterialTextureSlotNotFound(texture_parameter_slot_name.to_string()))?; 

        // Check if slots are of the same type
        if !enum_variant_eq(&texture.texture_type,&shader_texture_slot.texture_type) {
            return Err(Error::new(EngineError::WrongTextureType(
                get_enum_variant_type_name(&texture.texture_type), 
                texture_parameter_slot_name.to_string(), 
                get_enum_variant_type_name(&shader_texture_slot.texture_type)
            )));
        }

        Ok(())
    }

    fn schedule_deferred_operation(&mut self, operation: Box<dyn Fn(&mut Material, &mut Engine) -> Result<()> + Send>) {
        let handle = self.handle.expect("Critical: Cannot schedule deferred operation. No Handle set in Resource");
        let operation_to_schedule = ResourceDeferredOperation::<Material>::new(handle, operation);
        self.deferred_operation_manager.as_mut().expect("Critical: No DeferredOperationManager").schedule_deferred_operation(operation_to_schedule);
    }

    fn check_if_has_parameters_which_are_not_defined_in_shader() -> Result<()> {
        unimplemented!("Not yet implemented");
    }

    // Match shader parameters slots with material parameters (both value and texture parameters)
    // If material parameter is missing, create it with default value
    // If material parameter does not exist in shader, return error
    // If material parameter does not match type declared in shader, return error
    fn match_parameters_against_shader(&self, engine: &mut Engine) -> Result<()> {
        let shader: &Shader = engine.get_resource::<Shader>(&self.shader_handle)?;

        // Check all parameters against the shader
        for (parameter_name, parameter) in self.parameters_store.parameters.iter() {
            match parameter {
                MaterialParameter::Value(value_parameter) => {
                    match value_parameter {
                        ValueParameter::Float(_) => {
                            let shader_value_parameter_slot = shader.value_parameters_slots.get(parameter_name)
                                .context(EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Float".to_string()))?;
                            if !enum_variant_eq(&ShaderValueParameterType::Float, &shader_value_parameter_slot.parameter_type) {
                                return Err(Error::new(EngineError::WrongValueParameterTypeForValueParameterSlot(
                                    parameter_name.to_string(), 
                                    get_enum_variant_type_name(&ShaderValueParameterType::Float), 
                                    get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                                )));
                            }
                        }
                        ValueParameter::Bool(_) => {
                            let shader_value_parameter_slot = shader.value_parameters_slots.get(parameter_name)
                                .context(EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Bool".to_string()))?;
                            if !enum_variant_eq(&ShaderValueParameterType::Bool, &shader_value_parameter_slot.parameter_type) {
                                return Err(Error::new(EngineError::WrongValueParameterTypeForValueParameterSlot(
                                    parameter_name.to_string(), 
                                    get_enum_variant_type_name(&ShaderValueParameterType::Bool), 
                                    get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                                )));
                            }
                        }
                        ValueParameter::Color(_) => {
                            let shader_value_parameter_slot = shader.value_parameters_slots.get(parameter_name)
                                .context(EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Color".to_string()))?;
                            if !enum_variant_eq(&ShaderValueParameterType::Color, &shader_value_parameter_slot.parameter_type) {
                                return Err(Error::new(EngineError::WrongValueParameterTypeForValueParameterSlot(
                                    parameter_name.to_string(), 
                                    get_enum_variant_type_name(&ShaderValueParameterType::Color), 
                                    get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                                )));
                            }
                        }
                        ValueParameter::Vector2(_) => {
                            let shader_value_parameter_slot = shader.value_parameters_slots.get(parameter_name)
                                .context(EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Vector2".to_string()))?;
                            if !enum_variant_eq(&ShaderValueParameterType::Vector2, &shader_value_parameter_slot.parameter_type) {
                                return Err(Error::new(EngineError::WrongValueParameterTypeForValueParameterSlot(
                                    parameter_name.to_string(), 
                                    get_enum_variant_type_name(&ShaderValueParameterType::Vector2), 
                                    get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                                )));
                            }
                        }
                    }
                }
                MaterialParameter::Texture(texture_parameter) => {
                    let shader_texture_parameter_slot = shader.texture_parameters_slots.get(parameter_name)
                        .context(EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Texture".to_string()))?;
                    
                    if !enum_variant_eq(&texture_parameter., &shader_texture_parameter_slot.parameter_type) {
                        return Err(Error::new(EngineError::WrongMaterialTextureParameterType(
                            parameter_name.to_string(),
                            get_enum_variant_type_name(&ShaderTextureParameterType::Texture),
                            get_enum_variant_type_name(&shader_texture_parameter_slot.parameter_type)
                        )));
                    }
                }
            }
        }

        Ok(())
    }

      // Check if slots are of the same type
        if !enum_variant_eq(&texture.texture_type,&shader_texture_slot.texture_type) {
            return Err(Error::new(EngineError::WrongTextureType(
                get_enum_variant_type_name(&texture.texture_type), 
                texture_parameter_slot_name.to_string(), 
                get_enum_variant_type_name(&shader_texture_slot.texture_type)
            )));
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

    fn is_initialized(&self) -> bool {
        self.handle.is_some()
    }

    fn set_handle(&mut self, handle: Self::Handle) {
        self.handle = Some(handle);
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This resource is using DeferredOperationSystem so keep DeferredOperationManager
        let deferred_operation_component = engine.get_global_component_mut::<DeferredOperationComponent>()?;
        self.deferred_operation_manager = Some(deferred_operation_component.borrow_deferred_operation_manager());

        // Get shader
        let shader = engine.get_resource::<Shader>(&self.shader_handle)?;
        self.shader_name = Some(shader.get_name());
        let shader_renderer_resource_handle = shader.renderer_resource_handle.unwrap();
        
        // Create all missing value parameters slots based on shader definition
        for (value_parameter_name, value_parameter_slot) in shader.value_parameters_slots.iter() {
            match value_parameter_slot.parameter_type {
                ShaderValueParameterType::Float => {
                    if !self.parameters_store.does_float_parameter_exist(value_parameter_name) {
                        self.parameters_store.add_float_parameter(value_parameter_name, 0.0)?;
                    }
                }
                ShaderValueParameterType::Bool => {
                    if !self.parameters_store.does_bool_parameter_exist(value_parameter_name) {
                        self.parameters_store.add_bool_parameter(value_parameter_name, false)?;
                    }
                }
                ShaderValueParameterType::Color => {
                    if !self.parameters_store.does_color_parameter_exist(value_parameter_name) {
                        self.parameters_store.add_color_parameter(value_parameter_name, Color::new(1.0, 1.0, 1.0))?;
                    }
                }
                ShaderValueParameterType::Vector2 => {
                    if !self.parameters_store.does_vector2_parameter_exist(value_parameter_name) {
                        self.parameters_store.add_vector2_parameter(value_parameter_name, Vector2f::new(0.0, 0.0))?;
                    }
                }
            }
        }

        // Validate all value parameters slots are of correct type declared in shader
        for (value_parameter_name, value_parameter) in self.parameters_store.value_parameters_iter() {
            match value_parameter {
                ValueParameter::Float(_) => {
                    let shader_value_parameter_slot = shader.value_parameters_slots.get(value_parameter_name)
                        .context(EngineError::InvalidValueParameterTypeForValueParameterSlot(value_parameter_name.to_string(), "Float".to_string(), get_enum_variant_type_name(shader_value_parameter_slot.parameter_type)))?;
                    if !enum_variant_eq(&ShaderValueParameterType::Float, &shader_value_parameter_slot.parameter_type) {
                        return Err(Error::new(EngineError::WrongMaterialValueParameterType(
                            value_parameter_name.to_string(), 
                            get_enum_variant_type_name(&ShaderValueParameterType::Float), 
                            get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                        )));
                    }
                }
                ValueParameter::Bool(_) => {
                    let shader_value_parameter_slot = shader.value_parameters_slots.get(value_parameter_name)
                        .context(EngineError::MaterialValueSlotNotFound(value_parameter_name.to_string()))?;
                    if !enum_variant_eq(&ShaderValueParameterType::Bool, &shader_value_parameter_slot.parameter_type) {
                        return Err(Error::new(EngineError::WrongMaterialValueParameterType(
                            value_parameter_name.to_string(), 
                            get_enum_variant_type_name(&ShaderValueParameterType::Bool), 
                            get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                        )));
                    }
                }
                ValueParameter::Color(_) => {
                    let shader_value_parameter_slot = shader.value_parameters_slots.get(value_parameter_name)
                        .context(EngineError::MaterialValueSlotNotFound(value_parameter_name.to_string()))?;
                    if !enum_variant_eq(&ShaderValueParameterType::Color, &shader_value_parameter_slot.parameter_type) {
                        return Err(Error::new(EngineError::WrongMaterialValueParameterType(
                            value_parameter_name.to_string(), 
                            get_enum_variant_type_name(&ShaderValueParameterType::Color), 
                            get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                        )));
                    }
                }
                ValueParameter::Vector2(_) => {
                    let shader_value_parameter_slot = shader.value_parameters_slots.get(value_parameter_name)
                        .context(EngineError::MaterialValueSlotNotFound(value_parameter_name.to_string()))?;
                    if !enum_variant_eq(&ShaderValueParameterType::Vector2, &shader_value_parameter_slot.parameter_type) {
                        return Err(Error::new(EngineError::WrongMaterialValueParameterType(
                            value_parameter_name.to_string(), 
                            get_enum_variant_type_name(&ShaderValueParameterType::Vector2), 
                            get_enum_variant_type_name(&shader_value_parameter_slot.parameter_type)
                        )));
                    }
                }
            }
        }   

        // Create all missing texture parameters slots based on shader definition
        for (texture_parameter_name, texture_parameter_slot) in shader.texture_parameters_slots.iter() {
            if !self.parameters_store.does_texture_parameter_exist(texture_parameter_name) {
                self.parameters_store.add_texture_parameter(texture_parameter_name, TextureParameter::new(None))?;
            }
        }

        // Check if assigned textures are of correct type declared in shader
        for (texture_parameter_name, texture_parameter) in self.parameters_store.texture_parameters_iter() {
            self.validate_texture(engine, texture_parameter_name, texture_parameter)?;
        }

        // Assign renderer resource handle to texture slot
        // TODO: Make texture always store pointer to default if no other pointer is provided
        for (texture_parameter_name, texture_parameter) in self.parameters_store.texture_parameters_iter_mut() {
            match texture_parameter.texture_handle {
                Some(texture_handle) => {
                    let texture = engine.get_resource::<Texture>(&texture_handle)
                        .context(EngineError::InvalidTextureHandleForSlot(texture_parameter_name.to_string()))?;
                    texture_parameter.renderer_texture_handle = Some(texture.renderer_resource_handle.unwrap());
                }
                None => {}
            }
        }

        // Create new renderer material resource

        let renderer_resource_handle = engine.renderer.create_material(&self.name, shader_renderer_resource_handle, &self.parameters_store.parameters)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_material(v).unwrap();
        }

        // Find mesh rendering components that use this material and update them

        

        for (scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
            let x = &engine.resource_manager;

            // for (entity_handle, mesh_rendering_component) in engine.iterate_one_component::<MeshRenderingComponent>()? {
            //     if let Some(material_handle) = mesh_rendering_component.material_handle {
            //         // If mesh rendering component has handle to this material 
            //         if material_handle.data() == self_handle.data() {
            //             mesh_rendering_component.set_material_handle(Option::<MaterialHandle>::None);
            //             mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
            //         }
            //     }
            // }
        }

        Ok(())
    }
}