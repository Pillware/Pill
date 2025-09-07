#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

use indexmap::IndexMap;
use pill_core::{ debug, Color, LogContext, PillStyle, RendererError };
use pill_engine::internal::{
    get_default_texture_handles, get_renderer_texture_handle_from_material_texture, MaterialParameter, MaterialTexture, RendererMaterialHandle, RendererShaderHandle, ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot
};

use wgpu::util::DeviceExt;
use anyhow::{ Result, Error};
use std::collections::HashMap;
use crate::resources::RendererResourceStorage;

// --- Material ---



pub struct RendererMaterial {
    pub name: String,
    pub shader_handle: RendererShaderHandle,
    pub value_parameters_bind_group: Option<wgpu::BindGroup>,
    pub texture_parameters_bind_group: Option<wgpu::BindGroup>,
    pub(crate) value_parameters_uniform_buffer: Option<wgpu::Buffer>,

    // RendererMaterial is "an instance" of a material, 
    // this means that in its buffer it stores set of concrete parameters values for a specific shader

    // These is a special situation with postprocessing effects. They are added in postprocessing volumes scattered around the scene.
    // Each effect has its own shader and set of parameters.
    // When the rendering camera is inside the volume, all its effects are applied in order in postprocessing render pass.
    // Camera can be in multiple volumes at the same time.
    // Moreover, volumes support falloff, what means that the effect can be stronger or weaker depending on how deep the camera is inside the volume.
    // This allows for more dynamic and interesting visual effects.

    // This all means that buffers storing postprocessing effects parameters have to be dynamically updated each frame.
    // This also means that we would need to create material for each of the postprocessing effect existing in the scene, what is really inefficient.
    // To avoid this, we can create a single material for each postprocessing effect type (shader) which can store more that one set of parameters in its buffer,
    // and each frame we gather effects that affect the camera, then we update their parameter buffers with offsets.
    // So for each postprocessing effect type (shader) we have a single material with a buffer that can store multiple sets of parameters,

    // So for example. We have 2 volumes affecting the camera during the frame, each with color grading effect. 
    // They both use the same material. Before rendering the postprocessing pass, we update this material's buffer with 2 sets of parameters, one for each effect.
    // Then during the postprocessing pass, we draw the fullscreen quad twice, each time with different dynamic offset to access different set of parameters in the buffer.
    // This way we avoid creating multiple materials for each effect instance, and we can still have different parameters for each effect.
    // This is a trade-off between memory usage and performance, as we avoid creating multiple materials.

// Store postprocessing effect previous state: volume handle, set of affecting effects, opacity (from falloff)
// If any change to postprocessing effect parameters, we mark whole effect as dirty (wont work if user defines its own effect as struct in game!!!)
// In rendering system we iterate through volumes affecting the camera. If there is any change to the previous state we mark for update buffer in gpu.
// Thanks to this we avoid updating buffers every frame, only when there is a change.







    pub(crate) value_parameters_stride: Option<u32>, // bytes, aligned for dynamic offsets
    pub(crate) value_parameters_sets: u32,           // how many sets are stored 
}

impl RendererMaterial {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue, 
        rendering_resource_storage: &RendererResourceStorage,
        name: &str,
        shader_handle: RendererShaderHandle,
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<Self> {
        debug!(LogContext::Rendering => "Creating material {}", name.name_style());
        
        let shader = rendering_resource_storage.shaders.get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let value_parameters_slots = &shader.value_parameters_slots;
        let texture_parameters_slots = &shader.texture_parameters_slots;


        // Create parameters uniform buffer and bind group if there are parameter slots
        let (value_parameters_bind_group, parameters_uniform_buffer) = { 
            if !value_parameters_slots.is_empty() {
                // Calculate uniform buffer size, create buffer if needed and write data to it
                let value_parameters_uniform_buffer_size = Self::calculate_uniform_size(value_parameters_slots);

                let value_parameters_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{}_material_value_parameters_buffer", name)),
                    size: value_parameters_uniform_buffer_size as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });

                // Write parameter data to buffer
                Self::write_parameters_to_buffer(queue, &value_parameters_uniform_buffer, value_parameters_slots, parameters)?;

                debug!(LogContext::Rendering => "Uniform buffer of size {} bytes created", value_parameters_uniform_buffer_size);

                // Create parameters uniform buffer bind group
                let value_parameters_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("{}_material_value_parameters_bind_group", name)),
                    layout: shader.value_parameters_bind_group_layout.as_ref().unwrap(),
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0, // (set = 2, binding = 0)
                            resource: value_parameters_uniform_buffer.as_entire_binding(),
                        },
                    ],
                });

                debug!(LogContext::Rendering => "Parameters bind group created");

                (Some(value_parameters_bind_group), Some(value_parameters_uniform_buffer))
            } else {
                debug!(LogContext::Rendering => "No parameter slots found, skipping uniform buffer and bind group creation");
                (None, None)
            }
        };

        // Create texture bind group
        let texture_parameters_bind_group = if !texture_slots.is_empty() {
            Some(Self::create_textures_bind_group(
                device,
                rendering_resource_storage,
                &shader.texture_parameters_bind_group_layout.as_ref().unwrap(),
                &format!("{}_textures", name),
                texture_parameters_slots,
                textures,
            )?)
        } else {
            None
        };

        debug!(LogContext::Rendering => "Textures bind group created");

        let renderer_material = Self {
            name: name.to_string(),
            shader_handle,
            value_parameters_bind_group,
            texture_parameters_bind_group,
            value_parameters_uniform_buffer,

            value_parameters_stride: None,
            value_parameters_sets: 1,
        };

        debug!(LogContext::Rendering => "Material creation successful");

        Ok(renderer_material)
    }

    pub fn update_texture_parameters(
        device: &wgpu::Device, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        parameters: &IndexMap<String, MaterialTexture>
    ) -> Result<()> {
        let material = rendering_resource_storage.materials.get(material_renderer_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let shader_handle = material.shader_handle;
        let shader = rendering_resource_storage.shaders.get(shader_handle)
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let texture_slots = &shader.texture_slots;

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


    pub fn update_value_parameters(
        device: &wgpu::Device, 
        queue: &wgpu::Queue, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        parameters: &IndexMap<String, MaterialParameter>
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
        if let Some(ref buffer) = material.parameters_uniform_buffer {
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
        parameters: &HashMap<String, MaterialParameter>
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
                ShaderParameterType::Vector2 => {
                    // Vector2 parameter (2 floats + padding)
                    if let Some(MaterialParameter::Vector2(value)) = parameters.get(slot_name) {
                        data.extend_from_slice(&value.x.to_le_bytes());
                        data.extend_from_slice(&value.y.to_le_bytes());
                        data.extend_from_slice(&0.0f32.to_le_bytes()); // Padding
                        data.extend_from_slice(&0.0f32.to_le_bytes()); // Padding
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

    fn create_texture_parameters_bind_group(
        device: &wgpu::Device, 
        rendering_resource_storage: &RendererResourceStorage, 
        texture_parameters_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        texture_parameters_slots: &HashMap<String, ShaderTextureSlot>,
        textures: &IndexMap<String, MaterialTexture>
    ) -> Result<wgpu::BindGroup> {
        let mut entries = Vec::new();

        for (slot_name, slot) in texture_parameters_slots {
            // Get texture from material texture map or use default
            let renderer_texture_handle = match textures.get(slot_name) {
                Some(material_texture) => {
                    debug!(LogContext::Rendering => "Material texture slot {} found in material textures", slot_name.name_style());
                    get_renderer_texture_handle_from_material_texture(material_texture).unwrap()
                },
                None => {
                    debug!(LogContext::Rendering => "Material texture slot {} not found in material textures, using default texture", slot_name.name_style());
                    get_default_texture_handles(slot.texture_type).1
                }
            };

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

        // Set texture resources to the bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
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