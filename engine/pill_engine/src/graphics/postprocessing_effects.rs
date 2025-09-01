use crate::{
    config::*, ecs::{ Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, SceneHandle }, engine::Engine, game::{ResourceLoader, Shader, ShaderParameterSlot, ShaderTextureSlot}, graphics::{ compose_render_queue_key, RenderQueueKey }, internal::MaterialParameter, resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
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

// --- Vignette effect ---

pub struct Vignette {
    pub enabled: bool,
    pub opacity: f32,

    pub smoothness: f32,
    pub roundness: f32,
    pub center: Vector2f,
}

impl Vignette {
    pub fn new() -> Self {
        Self {
            enabled: true,
            opacity: 0.0,
            smoothness: 0.0,
            roundness: 0.0,
            center: Vector2f::new(0.0, 0.0),
        }
    }
}

impl PostprocessingEffect for Vignette {
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

impl_trait_accessible!(dyn PostprocessingEffect; Vignette);

pub fn register_vignette_postprocessing_effect(engine: &mut Engine) -> Result<()> {
    let shader_handle = engine.add_default_resource(
        Shader::new(
            VIGNETTE_POSTPROCESSING_SHADER_NAME,
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

// --- Color adjustments effect ---


pub struct ColorAdjustments {
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

impl ColorAdjustments {
    pub fn new() -> Self {
        Self {
            enabled: true,
            opacity: 0.0,
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

impl PostprocessingEffect for ColorAdjustments {
    fn name(&self) -> &'static str { "ColorAdjustments" }
    fn is_enabled(&self) -> bool { self.enabled }
    fn get_opacity(&self) -> f32 { self.opacity }
    fn get_material_handle(&self, engine: &Engine) -> MaterialHandle { engine.get_resource_handle::<Material>(VIGNETTE_POSTPROCESSING_MATERIAL_NAME).unwrap() }
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

impl_trait_accessible!(dyn PostprocessingEffect; ColorAdjustments);


pub fn register_color_adjustments_postprocessing_effect(engine: &mut Engine) -> Result<()> {
    engine.add_default_resource(
        Shader::new(
            COLOR_ADJUSTMENTS_POSTPROCESSING_SHADER_NAME,
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