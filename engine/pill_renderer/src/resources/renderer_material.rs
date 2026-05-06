#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

use indexmap::IndexMap;
use pill_core::{debug, LogContext, PillStyle, RendererError};
use pill_engine::internal::{
    get_default_texture_handles, get_renderer_texture_handle_from_material_texture,
    MaterialParameter, MaterialTexture, RendererMaterialHandle, RendererShaderHandle,
    ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot,
};

use crate::resources::RendererResourceStorage;
use anyhow::{Error, Result};
use std::collections::HashMap;

// --- Material ---

pub struct RendererMaterial {
    pub name: String,
    pub shader_handle: RendererShaderHandle,
    pub parameters_bind_group: Option<wgpu::BindGroup>,
    pub textures_bind_group: Option<wgpu::BindGroup>,
    pub(crate) parameters_uniform_buffer: Option<wgpu::Buffer>,
}

impl RendererMaterial {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        rendering_resource_storage: &RendererResourceStorage,
        name: &str,
        shader_handle: RendererShaderHandle,
        textures: &IndexMap<String, MaterialTexture>,
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<Self> {
        debug!(LogContext::Rendering => "Creating material {}", name.name_style());

        let shader = rendering_resource_storage
            .shaders
            .get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let parameter_slots = &shader.parameter_slots;
        let texture_slots = &shader.texture_slots;

        // Create parameters uniform buffer and bind group if there are parameter slots
        let (parameters_bind_group, parameters_uniform_buffer) = {
            if !parameter_slots.is_empty() {
                // Calculate uniform buffer size, create buffer if needed and write data to it
                let parameters_uniform_buffer_size = Self::calculate_uniform_size(parameter_slots);

                let parameters_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{}_material_parameters_buffer", name)),
                    size: parameters_uniform_buffer_size as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                // Write parameter data to buffer
                Self::write_parameters_to_buffer(
                    queue,
                    &parameters_uniform_buffer,
                    parameter_slots,
                    parameters,
                )?;

                debug!(LogContext::Rendering => "Uniform buffer of size {} bytes created", parameters_uniform_buffer_size);

                // Create parameters uniform buffer bind group
                let parameters_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("{}_material_parameters_bind_group", name)),
                    layout: shader.parameters_bind_group_layout.as_ref().unwrap(),
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0, // (set = 2, binding = 0)
                        resource: parameters_uniform_buffer.as_entire_binding(),
                    }],
                });

                debug!(LogContext::Rendering => "Parameters bind group created");

                (Some(parameters_bind_group), Some(parameters_uniform_buffer))
            } else {
                debug!(LogContext::Rendering => "No parameter slots found, skipping uniform buffer and bind group creation");
                (None, None)
            }
        };

        // Create texture bind group
        let textures_bind_group = if !texture_slots.is_empty() {
            Some(Self::create_textures_bind_group(
                device,
                rendering_resource_storage,
                shader.textures_bind_group_layout.as_ref().unwrap(),
                &format!("{}_textures", name),
                texture_slots,
                textures,
            )?)
        } else {
            None
        };

        debug!(LogContext::Rendering => "Textures bind group created");

        let renderer_material = Self {
            name: name.to_string(),
            shader_handle,
            parameters_bind_group,
            textures_bind_group,
            parameters_uniform_buffer,
        };

        debug!(LogContext::Rendering => "Material creation successful");

        Ok(renderer_material)
    }

    pub fn update_textures(
        _device: &wgpu::Device,
        _material_renderer_handle: RendererMaterialHandle,
        _rendering_resource_storage: &mut RendererResourceStorage,
        _textures: &IndexMap<String, MaterialTexture>,
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
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<()> {
        let material = rendering_resource_storage
            .materials
            .get(material_renderer_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let shader_handle = material.shader_handle;
        let shader = rendering_resource_storage
            .shaders
            .get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let parameter_slots = &shader.parameter_slots;

        let material = rendering_resource_storage
            .materials
            .get_mut(material_renderer_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        // Update uniform buffer if it exists
        if let Some(ref buffer) = material.parameters_uniform_buffer {
            Self::write_parameters_to_buffer(queue, buffer, parameter_slots, parameters)?;
        }

        Ok(())
    }

    fn calculate_uniform_size(parameter_slots: &IndexMap<String, ShaderParameterSlot>) -> usize {
        // Calculate total size needed for all parameters
        // Each parameter slot gets 16 bytes (vec4 alignment in WGSL)
        parameter_slots.len() * 16
    }

    fn write_parameters_to_buffer(
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        parameter_slots: &IndexMap<String, ShaderParameterSlot>,
        parameters: &HashMap<String, MaterialParameter>,
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
                    if let Some(MaterialParameter::Color(value)) = parameters.get(slot_name) {
                        data.extend_from_slice(&value.x.to_le_bytes());
                        data.extend_from_slice(&value.y.to_le_bytes());
                        data.extend_from_slice(&value.z.to_le_bytes());
                        data.extend_from_slice(&0.0f32.to_le_bytes()); // Padding
                    } else {
                        data.extend_from_slice(&[0u8; 16]);
                    }
                }
                ShaderParameterType::Scalar => {
                    // Scalar parameter (1 float + padding)
                    if let Some(MaterialParameter::Scalar(value)) = parameters.get(slot_name) {
                        data.extend_from_slice(&value.to_le_bytes());
                        data.extend_from_slice(&[0u8; 12]); // Padding to 16 bytes
                    } else {
                        data.extend_from_slice(&[0u8; 16]);
                    }
                }
                ShaderParameterType::Bool => {
                    // Bool parameter (1 u32 + padding)
                    if let Some(MaterialParameter::Bool(value)) = parameters.get(slot_name) {
                        let value: u32 = if *value { 1 } else { 0 };
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

    fn create_textures_bind_group(
        device: &wgpu::Device,
        rendering_resource_storage: &RendererResourceStorage,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        textures: &IndexMap<String, MaterialTexture>,
    ) -> Result<wgpu::BindGroup> {
        let mut entries = Vec::new();

        for (slot_name, slot) in texture_slots {
            // Get texture from material texture map or use default
            let renderer_texture_handle = match textures.get(slot_name) {
                Some(material_texture) => {
                    debug!(LogContext::Rendering => "Material texture slot {} found in material textures", slot_name.name_style());
                    get_renderer_texture_handle_from_material_texture(material_texture).unwrap()
                }
                None => {
                    debug!(LogContext::Rendering => "Material texture slot {} not found in material textures, using default texture", slot_name.name_style());
                    get_default_texture_handles(slot.texture_type).1
                }
            };

            let texture = rendering_resource_storage
                .textures
                .get(renderer_texture_handle)
                .unwrap();

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

        // Set texture resources to the bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: texture_bind_group_layout,
            entries: &entries,
            label: Some(name),
        });

        Ok(bind_group)
    }

    // fn create_parameters_bind_group(
    //     device: &wgpu::Device,
    //     parameter_bind_group_layout: &wgpu::BindGroupLayout,
    //     name: &str,
    //     buffer: &wgpu::Buffer,
    // ) -> Result<wgpu::BindGroup> {
    //     let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
    //         layout: parameter_bind_group_layout,
    //         entries: &[
    //             wgpu::BindGroupEntry {
    //                 binding: MATERIAL_PARAMETERS_BINDING_INDEX as u32,
    //                 resource: buffer.as_entire_binding(),
    //             },
    //         ],
    //         label: Some(name),
    //     });

    //     Ok(bind_group)
    // }

    // Helper method to get bind group by index
    // pub fn get_bind_group(&self, index: usize) -> Option<&wgpu::BindGroup> {
    //     self.bind_groups.get(index)
    // }
}
