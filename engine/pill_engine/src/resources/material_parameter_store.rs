use indexmap::IndexMap;
use pill_core::{Color, EngineError, Vector2f};
use crate::{game::TextureHandle, graphics::RendererTextureHandle};
use anyhow::{ Result, Context, Error };

// --- Parameters ---

#[derive(Debug, Clone)]
pub enum ValueParameter {
    Float(f32),
    Bool(bool),
    Color(Color),
    Vector2(Vector2f),
}

#[derive(Debug, Clone)]
pub struct TextureParameter {
    pub texture_handle: Option<TextureHandle>,
    pub(crate) renderer_texture_handle: Option<RendererTextureHandle>,
}

impl TextureParameter {
    pub fn new(texture_handle: TextureHandle) -> Self {
        Self {
            texture_handle: None,
            renderer_texture_handle: None,
        }
    }
}

#[derive(Debug , Clone)]
pub enum MaterialParameter {
    Value(ValueParameter),
    Texture(TextureParameter),
}

// This needed so that renderer can get renderer texture handle from material texture while it is still hidden in game API
pub fn get_renderer_texture_handle_from_texture_parameter(texture_parameter: &TextureParameter) -> &Option<RendererTextureHandle> {
    &texture_parameter.renderer_texture_handle
}

// --- Store ---

#[derive(Debug, Default)]
pub struct MaterialParametersStore {
    pub parameters: IndexMap<String, MaterialParameter>,
    pub is_dirty: bool,
}

// Getters and setters
impl MaterialParametersStore {

    // Float
    pub fn does_float_parameter_exist(&self, parameter_name: &str) -> bool {
        if let Some(MaterialParameter::Value(ValueParameter::Float(_))) = self.parameters.get(parameter_name) {
            true
        } else {
            false
        }
    }

    pub fn add_float_parameter(&mut self, parameter_name: &str, new_value: f32) -> Result<()> {
        if self.parameters.contains_key(parameter_name) {
            return Err(Error::new(EngineError::MaterialParameterSlotAlreadyExists(parameter_name.to_string(), "Float".to_string())));
        }
        self.parameters.insert(
            parameter_name.to_string(), 
            MaterialParameter::Value(ValueParameter::Float(new_value))
        );
        self.is_dirty = true;
        Ok(())
    }

    pub fn get_float_parameter(&self, parameter_name: &str) -> Result<f32> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Float".to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Float(value)) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_float_parameter(&mut self, parameter_name: &str, new_value: f32) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Float".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Float(v)) => {
                *v = new_value;
                self.is_dirty = true;
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    // Bool
    pub fn does_bool_parameter_exist(&self, parameter_name: &str) -> bool {
        if let Some(MaterialParameter::Value(ValueParameter::Bool(_))) = self.parameters.get(parameter_name) {
            true
        } else {
            false
        }
    }

    pub fn add_bool_parameter(&mut self, parameter_name: &str, new_value: bool) -> Result<()> {
        if self.parameters.contains_key(parameter_name) {
            return Err(Error::new(EngineError::MaterialParameterSlotAlreadyExists(parameter_name.to_string(), "Bool".to_string())));
        }
        self.parameters.insert(
            parameter_name.to_string(),
            MaterialParameter::Value(ValueParameter::Bool(new_value))
        );
        self.is_dirty = true;
        Ok(())
    }

    pub fn get_bool_parameter(&self, parameter_name: &str) -> Result<bool> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Bool".to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Bool(value)) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_bool_parameter(&mut self, parameter_name: &str, new_value: bool) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Bool".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Bool(v)) => {
                *v = new_value; 
                self.is_dirty = true;
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    // Color
    pub fn does_color_parameter_exist(&self, parameter_name: &str) -> bool {
        if let Some(MaterialParameter::Value(ValueParameter::Color(_))) = self.parameters.get(parameter_name) {
            true
        } else {
            false
        }
    }

    pub fn add_color_parameter(&mut self, parameter_name: &str, new_value: Color) -> Result<()> {
        if self.parameters.contains_key(parameter_name) {
            return Err(Error::new(EngineError::MaterialParameterSlotAlreadyExists(parameter_name.to_string(), "Color".to_string())));
        }
        self.parameters.insert(
            parameter_name.to_string(),
            MaterialParameter::Value(ValueParameter::Color(Color::new(new_value.x.clamp(0.0, 1.0), new_value.y.clamp(0.0, 1.0), new_value.z.clamp(0.0, 1.0))))
        );
        self.is_dirty = true;
        Ok(())
    }

    pub fn get_color_parameter(&self, parameter_name: &str) -> Result<Color> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Color".to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Color(value)) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_color_parameter(&mut self, parameter_name: &str, new_value: Color) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Color".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Color(v)) => {
                *v = Color::new(new_value.x.clamp(0.0, 1.0), new_value.y.clamp(0.0, 1.0), new_value.z.clamp(0.0, 1.0));
                self.is_dirty = true;
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    // Vector2
    pub fn does_vector2_parameter_exist(&self, parameter_name: &str) -> bool {
        if let Some(MaterialParameter::Value(ValueParameter::Vector2(_))) = self.parameters.get(parameter_name) {
            true
        } else {
            false
        }
    }

    pub fn add_vector2_parameter(&mut self, parameter_name: &str, new_value: Vector2f) -> Result<()> {
        if self.parameters.contains_key(parameter_name) {
            return Err(Error::new(EngineError::MaterialParameterSlotAlreadyExists(parameter_name.to_string(), "Vector2".to_string())));
        }
        self.parameters.insert(
            parameter_name.to_string(),
            MaterialParameter::Value(ValueParameter::Vector2(new_value))
        );
        self.is_dirty = true;
        Ok(())
    }

    pub fn get_vector2_parameter(&self, parameter_name: &str) -> Result<Vector2f> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Vector2".to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Vector2(value)) => Ok(*value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_vector2_parameter(&mut self, parameter_name: &str, new_value: Vector2f) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Vector2".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Value(ValueParameter::Vector2(v)) => {
                *v = new_value; 
                self.is_dirty = true;
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    // Texture
    pub fn does_texture_parameter_exist(&self, parameter_name: &str) -> bool {
        if let Some(MaterialParameter::Texture(_)) = self.parameters.get(parameter_name) {
            true
        } else {
            false
        }
    }

    pub fn add_texture_parameter(&mut self, parameter_name: &str, new_value: TextureParameter) -> Result<()> {
        if self.parameters.contains_key(parameter_name) {
            return Err(Error::new(EngineError::MaterialParameterSlotAlreadyExists(parameter_name.to_string(), "Texture".to_string())));
        }
        self.parameters.insert(
            parameter_name.to_string(),
            MaterialParameter::Texture(new_value)
        );
        self.is_dirty = true;
        Ok(())
    }

    pub fn get_texture_parameter(&self, parameter_name: &str) -> Result<&TextureParameter> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Texture".to_string());
        let parameter = self.parameters.get(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Texture(value) => Ok(value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn get_texture_parameter_mut(&mut self, parameter_name: &str) -> Result<&mut TextureParameter> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Texture".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Texture(value) => Ok(value),
            _ => Err(Error::new(error)),
        }
    }

    pub fn set_texture_parameter(&mut self, parameter_name: &str, new_value: TextureParameter) -> Result<()> {
        let error = EngineError::MaterialParameterSlotNotFound(parameter_name.to_string(), "Texture".to_string());
        let parameter = self.parameters.get_mut(parameter_name).context(error.clone())?;
        match parameter {
            MaterialParameter::Texture(v) => {
                *v = new_value; 
                self.is_dirty = true;
                Ok(())
            },
            _ => Err(Error::new(error)),
        }
    }

    // Iterators

    pub fn value_parameters_iter(&self) -> impl Iterator<Item = (&String, &ValueParameter)> {
        self.parameters.iter().filter_map(|(key, value)| {
            if let MaterialParameter::Value(value_parameter) = value {
                Some((key, value_parameter))
            } else {
                None
            }
        })
    }

    pub fn value_parameters_iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut ValueParameter)> {
        self.parameters.iter_mut().filter_map(|(key, value)| {
            if let MaterialParameter::Value(value_parameter) = value {
                Some((key, value_parameter))
            } else {
                None
            }
        })
    }

    pub fn texture_parameters_iter(&self) -> impl Iterator<Item = (&String, &TextureParameter)> {
        self.parameters.iter().filter_map(|(key, value)| {
            if let MaterialParameter::Texture(texture_parameter) = value {
                Some((key, texture_parameter))
            } else {
                None
            }
        })
    }

    pub fn texture_parameters_iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut TextureParameter)> {
        self.parameters.iter_mut().filter_map(|(key, value)| {
            if let MaterialParameter::Texture(texture_parameter) = value {
                Some((key, texture_parameter))
            } else {
                None
            }
        })
    }
}