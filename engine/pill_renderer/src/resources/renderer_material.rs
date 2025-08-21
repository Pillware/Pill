#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]


use crate::{ RendererResourceStorage };
use crate::config::{
    MATERIAL_PARAMETERS_BINDING_INDEX, 
    PARAMETERS_BIND_GROUP_LAYOUT_INDEX, 
    TEXTURES_BIND_GROUP_LAYOUT_INDEX
};

use pill_core::RendererError;
use pill_engine::internal::{
    get_default_texture_handles, 
    get_renderer_texture_handle_from_material_texture, 
    MaterialParameterMap, 
    MaterialTextureMap, 
    RendererMaterialHandle, 
    RendererShaderHandle, 
    ShaderParameterSlot, 
    ShaderParameterType, 
    ShaderTextureSlot,
};

use wgpu::util::DeviceExt;
use anyhow::{ Result, Error};
use std::collections::HashMap;

// --- Material ---

pub struct RendererMaterial {
    pub name: String,
    pub shader_handle: RendererShaderHandle,
    pub parameters_bind_group: Option<wgpu::BindGroup>,
    pub texture_bind_group: Option<wgpu::BindGroup>,
    pub(crate) material_parameters_uniform_buffer: Option<wgpu::Buffer>,
    pub(crate) material_parameters_uniform_size: usize,
}

impl RendererMaterial {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue, 
        rendering_resource_storage: &RendererResourceStorage,
        name: &str,
        shader_handle: RendererShaderHandle,
        textures: &MaterialTextureMap,
        parameters: &MaterialParameterMap,
    ) -> Result<Self> {
        let shader = rendering_resource_storage.shaders.get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let parameter_slots = &shader.parameter_slots;
        let texture_slots = &shader.texture_slots;

        // Calculate uniform buffer size, create buffer if needed and write data to it
        let material_parameters_uniform_size = if !parameter_slots.is_empty() && !shader.bind_group_layouts.is_empty() {
            Self::calculate_uniform_size(parameter_slots)
        } else {
            0
        };
        let material_parameters_uniform_buffer = if material_parameters_uniform_size > 0 {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("{}_material_buffer", name)),
                size: material_parameters_uniform_size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

            // Write parameter data to buffer
            Self::write_parameters_to_buffer(queue, &buffer, parameter_slots, parameters)?;
            Some(buffer)
        } else {
            None
        };

        // Create bind groups based on shader's bind group layouts
        //let mut bind_groups = Vec::new();
        
// TODO: Do in need to define also bind groups for engine and camera parameters? so it matches the parameter layout? or some entries can be empty?

        // Create parameters uniform buffer bind group if we have uniform buffer
        let parameters_bind_group = if let Some(ref buffer) = material_parameters_uniform_buffer {
            Some(Self::create_parameters_bind_group(
                device,
                &shader.bind_group_layouts[PARAMETERS_BIND_GROUP_LAYOUT_INDEX as usize],
                &format!("{}_parameters", name),
                buffer,
            )?)
        } else {
            None
        };

        // Create texture bind groups
        let texture_bind_group = if !texture_slots.is_empty() && !shader.bind_group_layouts.is_empty() {
            Some(Self::create_texture_bind_group(
                device,
                rendering_resource_storage,
                &shader.bind_group_layouts[TEXTURES_BIND_GROUP_LAYOUT_INDEX as usize],
                &format!("{}_textures", name),
                texture_slots,
                textures,
            )?)
        } else {
            None
        };

        let renderer_material = Self {
            name: name.to_string(),
            shader_handle,
            parameters_bind_group,
            texture_bind_group,
            material_parameters_uniform_buffer,
            material_parameters_uniform_size,
        };

        Ok(renderer_material)
    }

    pub fn update_textures(
        device: &wgpu::Device, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        textures: &MaterialTextureMap
    ) -> Result<()> {
        // let material = rendering_resource_storage.materials.get(material_renderer_handle)
        //     .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        // let shader_handle = material.shader_handle;
        // let shader = rendering_resource_storage.shaders.get(shader_handle)
        //     .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        //let texture_slots = &shader.texture_slots;

        // TODO: Implement
        // Recreate texture bind group
        // if !texture_slots.is_empty() && !shader.bind_group_layouts.is_empty() {
        //     let texture_bind_group = Self::create_texture_bind_group(
        //         device, 
        //         rendering_resource_storage, 
        //         &shader.bind_group_layouts[0], 
        //         &format!("{}_textures", material.name),
        //         texture_slots,
        //         textures
        //     )?;

        //     let material = rendering_resource_storage.materials.get_mut(material_renderer_handle)
        //         .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
            
        //     // if !material.bind_groups.is_empty() {
        //     //     material.bind_groups[0] = texture_bind_group;
        //     // }
        // }

        Ok(())
    }

    pub fn update_parameters(
        device: &wgpu::Device, 
        queue: &wgpu::Queue, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        parameters: &MaterialParameterMap
    ) -> Result<()> {
        let material = rendering_resource_storage.materials.get(material_renderer_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let shader_handle = material.shader_handle;
        let shader = rendering_resource_storage.shaders.get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let parameter_slots = &shader.parameter_slots;

        let material = rendering_resource_storage.materials.get_mut(material_renderer_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        // Update uniform buffer if it exists
        if let Some(ref buffer) = material.material_parameters_uniform_buffer {
            Self::write_parameters_to_buffer(queue, buffer, parameter_slots, parameters)?;
        }

        Ok(())
    }

    fn calculate_uniform_size(parameter_slots: &HashMap<String, ShaderParameterSlot>) -> usize {
        // Calculate total size needed for all parameters
        // Each parameter slot gets 16 bytes (vec4 alignment in WGSL)
        parameter_slots.len() * 16
    }

    fn write_parameters_to_buffer(
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        parameter_slots: &HashMap<String, ShaderParameterSlot>,
        parameters: &MaterialParameterMap
    ) -> Result<()> {
        // Create a temporary buffer to hold all parameter data
        let mut data = Vec::new();
        
        // NOTE: Each parameter is 16 bytes (vec4 alignment in WGSL)
        //       Padding is added to ensure each parameter takes 16 bytes
        //       This is not ideal because we could make it more efficient by packing parameters more tightly
        //       But for simplicity, we will keep it this way for now
        for (slot_name, slot) in parameter_slots {
            match slot.parameter_type {
                ShaderParameterType::Color => {
                    // Color parameter (3 floats + padding)
                    if let Ok(color) = parameters.get_color(slot_name) {
                        data.extend_from_slice(&color.x.to_le_bytes());
                        data.extend_from_slice(&color.y.to_le_bytes());
                        data.extend_from_slice(&color.z.to_le_bytes());
                        data.extend_from_slice(&0.0f32.to_le_bytes()); // Padding
                    } else {
                        data.extend_from_slice(&[0u8; 16]);
                    }
                }
                ShaderParameterType::Scalar => {
                    // Scalar parameter (1 float + padding)
                    if let Ok(scalar) = parameters.get_scalar(slot_name) {
                        data.extend_from_slice(&scalar.to_le_bytes());
                        data.extend_from_slice(&[0u8; 12]); // Padding to 16 bytes
                    } else {
                        data.extend_from_slice(&[0u8; 16]);
                    }
                }
                ShaderParameterType::Bool => {
                    // Bool parameter (1 u32 + padding)
                    if let Ok(boolean) = parameters.get_bool(slot_name) {
                    let value: u32 = if boolean { 1 } else { 0 };
                        data.extend_from_slice(&value.to_le_bytes());
                        data.extend_from_slice(&[0u8; 12]); // Padding to 16 bytes
                    } else {
                        data.extend_from_slice(&[0u8; 16]);
                    }
                }
            }
        }
        
        if !data.is_empty() {
            queue.write_buffer(buffer, 0, &data);
        }
        
        Ok(())
    }

    fn create_texture_bind_group(
        device: &wgpu::Device, 
        rendering_resource_storage: &RendererResourceStorage, 
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        textures: &MaterialTextureMap
    ) -> Result<wgpu::BindGroup> {
        let mut entries = Vec::new();
        
        for (slot_name, slot) in texture_slots {
            // Get texture from material texture map or use default
            if let Some(material_texture) = textures.data.get(slot_name) {
                let renderer_texture_handle = get_renderer_texture_handle_from_material_texture(material_texture)
                    .unwrap_or_else(|| get_default_texture_handles(material_texture.texture_type).1);
                
                let texture = rendering_resource_storage.textures.get(renderer_texture_handle).unwrap();
                
                // Add texture view entry
                entries.push(wgpu::BindGroupEntry {
                    binding: slot.texture_binding,
                    resource: wgpu::BindingResource::TextureView(&texture.texture_view),
                });
                
                // Add sampler entry
                entries.push(wgpu::BindGroupEntry {
                    binding: slot.sampler_binding,
                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                });
            }
        }

        // Set texture resources to the bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
            entries: &entries,
            label: Some(name),
        });

        Ok(bind_group)
    }

    fn create_parameters_bind_group(
        device: &wgpu::Device, 
        parameter_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        buffer: &wgpu::Buffer,
    ) -> Result<wgpu::BindGroup> {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: parameter_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: MATERIAL_PARAMETERS_BINDING_INDEX as u32,
                    resource: buffer.as_entire_binding(),
                },
            ],
            label: Some(name),
        });

        Ok(bind_group)
    }

    // Helper method to get bind group by index
    // pub fn get_bind_group(&self, index: usize) -> Option<&wgpu::BindGroup> {
    //     self.bind_groups.get(index)
    // }
}