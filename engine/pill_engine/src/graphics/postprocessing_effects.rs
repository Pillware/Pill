use crate::{
    config::*, ecs::{ Component, ComponentStorage, DeferredOperationComponent, DeferredOperationManagerPointer, EntityHandle, SceneHandle }, engine::Engine, game::{ResourceLoader, Shader}, graphics::{ compose_render_queue_key, RenderQueueKey, RendererMaterialHandle }, internal::MaterialParametersStore, resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager, ShaderType }
};
use pill_core::{
    get_type_name, impl_trait_accessible, BoundingBox, Color, EngineError, OptionStorage, PillTraitTypeMap, PillTypeMap, PillTypeMapKey, SingleStorage, TraitAccessible, TraitAccessor, Vector2f, Vector3f
};
use anyhow::{Context, Result, Error};
use std::{ 
    any::Any, 
    cell::RefCell, 
    collections::{ HashMap, VecDeque }, 
    marker::PhantomData, 
    ops::IndexMut
};





pub trait PostprocessingEffect: Any + Send {
    fn name(&self) -> &'static str;
    fn is_enabled(&self) -> bool;
    fn get_opacity(&self) -> f32;
    fn get_material_handle(&self, engine: &Engine) -> MaterialHandle;
    fn get_parameters(&self) -> HashMap<String, MaterialParameter>;
}

pub struct PostprocessingVolumeRendererData {
    pub effect_data: Vec<PostprocessingEffectRendererData>,
}

pub struct PostprocessingEffectRendererData {
    pub material_handle: RendererMaterialHandle,
    pub material_parameters: HashMap<String, MaterialParameter>,
    pub influence: f32,
}



pub trait PostprocessingEffectX: Any + Send {
    fn name(&self) -> &'static str;
    fn is_enabled(&self) -> bool;
    fn get_opacity(&self) -> f32;
    fn get_material_handle(&self, engine: &Engine) -> MaterialHandle;
    fn get_parameters_store_mut(&self) -> &mut MaterialParametersStore;
}


pub struct TestPostprocessingEffect {
    pub enabled: bool,
    pub opacity: f32,
    pub(crate) parameters_store: MaterialParametersStore,
}

impl TestPostprocessingEffect {
    pub fn new(shader_handle: ShaderHandle) -> Self {
        Self {
            enabled: true,
            opacity: 1.0,
            material: Material::new(shader_handle),
            //parameters_store: MaterialParametersStore::default(),
        }
    }
}


// When we delete shader we want to visit all effects and and update them. Set shader to none and disable effect. But keep the parameter values.
// This can't be component since components my be unloaded.
// So the only way is to store all effects as resources.
// We should have postprocessing effect template resource. But then for each postprocessing volume will actually store instance (set of parameters).
// But then we still have a problem. Since someone can delete texture resource and reference to this resource is in set of parameters, not in effect template.
// So on delete of this texture we cant iterate over all components with parameters since they can be unloaded.



// 
// 




// --- Color adjustments effect ---

pub struct ColorAdjustmentsPostprocessingEffect {
    pub enabled: bool,
    pub opacity: f32,

    pub exposure: f32,
    pub tint: Color,
    pub white_balance: f32,
    pub hue: f32,
    pub saturation: f32,
    pub contrast: f32,
    pub brightness: f32,
    pub invert: bool,
    pub gamma: f32,
}

impl ColorAdjustmentsPostprocessingEffect {
    pub fn new() -> Self {
        Self {
            enabled: true,
            opacity: 1.0,
            exposure: 0.0,
            tint: Color::new(1.0, 1.0, 1.0),
            white_balance: 0.0,
            hue: 0.0,
            saturation: 1.0,
            contrast: 1.0,
            brightness: 0.0,
            invert: false,
            gamma: 1.0,
        }
    }
}

impl PostprocessingEffect for ColorAdjustmentsPostprocessingEffect {
    fn name(&self) -> &'static str { "ColorAdjustments" }
    fn is_enabled(&self) -> bool { self.enabled }
    fn get_opacity(&self) -> f32 { self.opacity }
    fn get_material_handle(&self, engine: &Engine) -> MaterialHandle { engine.get_resource_handle::<Material>(COLOR_ADJUSTMENTS_POSTPROCESSING_MATERIAL_NAME).unwrap() }
    fn get_parameters(&self) -> HashMap<String, MaterialParameter> {
        let mut parameters = HashMap::new();
        parameters.insert("opacity".to_string(), MaterialParameter::Scalar(self.opacity));
        parameters.insert("exposure".to_string(), MaterialParameter::Scalar(self.exposure));
        parameters.insert("tint".to_string(), MaterialParameter::Color(self.tint));
        parameters.insert("white_balance".to_string(), MaterialParameter::Scalar(self.white_balance));
        parameters.insert("hue".to_string(), MaterialParameter::Scalar(self.hue));
        parameters.insert("saturation".to_string(), MaterialParameter::Scalar(self.saturation));
        parameters.insert("contrast".to_string(), MaterialParameter::Scalar(self.contrast));
        parameters.insert("brightness".to_string(), MaterialParameter::Scalar(self.brightness));
        parameters.insert("invert".to_string(), MaterialParameter::Bool(self.invert));
        parameters.insert("gamma".to_string(), MaterialParameter::Scalar(self.gamma));
        parameters
    }
}

impl_trait_accessible!(dyn PostprocessingEffect; ColorAdjustmentsPostprocessingEffect);


pub fn register_color_adjustments_postprocessing_effect(engine: &mut Engine) -> Result<()> {
    engine.add_default_resource(
        Shader::new(
            COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_NAME,
            ShaderType::Fullscreen,
            ResourceLoader::Bytes(Box::new(*include_bytes!("../../res/shaders/postprocessing/postprocessing_vertex.glsl"))),
            ResourceLoader::Bytes(Box::new(*include_bytes!("../../res/shaders/postprocessing/postprocessing_color_adjustments_fragment.glsl"))),
            vec![
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_EXPOSURE_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_EXPOSURE_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_TINT_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_TINT_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_WHITE_BALANCE_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_WHITE_BALANCE_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_HUE_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_HUE_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SATURATION_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SATURATION_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_CONTRAST_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_CONTRAST_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_BRIGHTNESS_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_BRIGHTNESS_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_INVERT_FLAG_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_INVERT_FLAG_PARAMETER_SLOT.1)),
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_GAMMA_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_GAMMA_PARAMETER_SLOT.1)),
            ].into_iter().collect(),
            vec![
                (COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.0.to_string(), ShaderTextureSlot::new(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.1, COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.2)),
            ].into_iter().collect(),
            true,
            true
        )
    )?;

    engine.add_default_resource(
        Material::builder(COLOR_ADJUSTMENTS_POSTPROCESSING_MATERIAL_NAME)
            .shader(engine.get_resource_handle::<Shader>(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_NAME).unwrap())?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.0, 0.0)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_EXPOSURE_PARAMETER_SLOT.0, 0.0)?
            .color_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_TINT_PARAMETER_SLOT.0, Color::new(1.0, 1.0, 1.0))?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_WHITE_BALANCE_PARAMETER_SLOT.0, 0.0)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_HUE_PARAMETER_SLOT.0, 0.0)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_SATURATION_PARAMETER_SLOT.0, 1.0)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_CONTRAST_PARAMETER_SLOT.0, 1.0)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_BRIGHTNESS_PARAMETER_SLOT.0, 0.0)?
            .bool_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_INVERT_FLAG_PARAMETER_SLOT.0, false)?
            .scalar_parameter(COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_GAMMA_PARAMETER_SLOT.0, 1.0)?
            .build()
    )?;

    Ok(())
}

// --- Vignette effect ---

pub struct VignettePostProcessingEffect {
    pub enabled: bool,
    pub opacity: f32,

    pub smoothness: f32,
    pub roundness: f32,
    pub center: Vector2f,
}

impl VignettePostProcessingEffect {
    pub fn new() -> Self {
        Self {
            enabled: true,
            opacity: 1.0,
            smoothness: 0.0,
            roundness: 0.0,
            center: Vector2f::new(0.0, 0.0),
        }
    }
}

impl PostprocessingEffect for VignettePostProcessingEffect {
    fn name(&self) -> &'static str { "Vignette" }
    fn is_enabled(&self) -> bool { self.enabled }
    fn get_opacity(&self) -> f32 { self.opacity }
    fn get_material_handle(&self, engine: &Engine) -> MaterialHandle { engine.get_resource_handle::<Material>(VIGNETTE_POSTPROCESSING_MATERIAL_NAME).unwrap() }
    fn get_parameters(&self) -> HashMap<String, MaterialParameter> {
        let mut parameters = HashMap::new();
        parameters.insert("opacity".to_string(), MaterialParameter::Scalar(self.opacity));
        parameters.insert("smoothness".to_string(), MaterialParameter::Scalar(self.smoothness));
        parameters.insert("roundness".to_string(), MaterialParameter::Scalar(self.roundness));
        parameters.insert("center".to_string(), MaterialParameter::Vector2(self.center));
        parameters
    }
}

impl_trait_accessible!(dyn PostprocessingEffect; VignettePostProcessingEffect);

pub fn register_vignette_postprocessing_effect(engine: &mut Engine) -> Result<()> {
    let shader_handle = engine.add_default_resource(
        Shader::new(
            VIGNETTE_POSTPROCESSING_SHADER_NAME,
            ShaderType::Fullscreen,
            ResourceLoader::Bytes(Box::new(*include_bytes!("../../res/shaders/postprocessing/postprocessing_vertex.glsl"))),
            ResourceLoader::Bytes(Box::new(*include_bytes!("../../res/shaders/postprocessing/postprocessing_vignette_fragment.glsl"))),
            vec![
                (VIGNETTE_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(VIGNETTE_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.1)),
                (VIGNETTE_POSTPROCESSING_SHADER_SMOOTHNESS_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(VIGNETTE_POSTPROCESSING_SHADER_SMOOTHNESS_PARAMETER_SLOT.1)),
                (VIGNETTE_POSTPROCESSING_SHADER_ROUNDNESS_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(VIGNETTE_POSTPROCESSING_SHADER_ROUNDNESS_PARAMETER_SLOT.1)),
                (VIGNETTE_POSTPROCESSING_SHADER_CENTER_PARAMETER_SLOT.0.to_string(), ShaderParameterSlot::new(VIGNETTE_POSTPROCESSING_SHADER_CENTER_PARAMETER_SLOT.1)),
            ].into_iter().collect(),
            vec![
                (VIGNETTE_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.0.to_string(), ShaderTextureSlot::new(VIGNETTE_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.1, VIGNETTE_POSTPROCESSING_SHADER_SCENE_TEXTURE_SLOT.2)),
            ].into_iter().collect(),
            true,
            true
        )
    )?;

    engine.add_default_resource(
        Material::builder(VIGNETTE_POSTPROCESSING_MATERIAL_NAME)
            .shader(shader_handle)?
            .scalar_parameter(VIGNETTE_POSTPROCESSING_SHADER_OPACITY_PARAMETER_SLOT.0, 0.0)?
            .scalar_parameter(VIGNETTE_POSTPROCESSING_SHADER_SMOOTHNESS_PARAMETER_SLOT.0, 0.0)?
            .scalar_parameter(VIGNETTE_POSTPROCESSING_SHADER_ROUNDNESS_PARAMETER_SLOT.0, 0.0)?
            .vector2_parameter(VIGNETTE_POSTPROCESSING_SHADER_CENTER_PARAMETER_SLOT.0, Vector2f::new(0.0, 0.0))?
            .build()
    )?;

    Ok(())
}
