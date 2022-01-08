use crate::RendererResourceStorage;
use crate::resources::RendererTexture;

use pill_engine::internal::{
    MaterialParameter,
    MaterialParameterMap,
    RendererError,
    MaterialTextureMap,
    RendererMaterialHandle,
    RendererPipelineHandle, 
    TextureHandle,
    MaterialTexture, 
    MASTER_SHADER_COLOR_TEXTURE_SLOT,    
    MASTER_SHADER_NORMAL_TEXTURE_SLOT,
    MASTER_SHADER_TINT_PARAMETER_SLOT, get_default_texture_handles, get_renderer_texture_handle_from_material_texture, MASTER_SHADER_SPECULARITY_PARAMETER_SLOT,
};

use futures::TryFutureExt;
use wgpu::util::DeviceExt;
use std::path::{ Path, PathBuf };
use anyhow::{Result, Context, Error};

// --- Material Uniform ---

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MaterialUniform {
    pub(crate) tint: [f32; 3],
    pub(crate) specularity: f32,
}

impl MaterialUniform {
    pub fn new() -> Self {
        Self {
            tint: cgmath::Vector3::<f32>::new(0.0,0.0,0.0).into(),
            specularity: 0.0,
        }
    }
}

// --- Material ---

pub struct RendererMaterial {
    pub name: String,
    pub pipeline_handle: RendererPipelineHandle,
    pub texture_bind_group: wgpu::BindGroup,
    pub parameter_bind_group: wgpu::BindGroup,
    pub(crate) uniform: MaterialUniform,
    buffer: wgpu::Buffer,
}

impl RendererMaterial {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue, 
        rendering_resource_storage: &RendererResourceStorage,
        name: &str,
        pipeline_handle: RendererPipelineHandle,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        textures: &MaterialTextureMap,
        parameter_bind_group_layout: &wgpu::BindGroupLayout,
        parameters: &MaterialParameterMap,
    ) -> Result<Self> {

        // Create parameter buffer and write data to it
        let mut uniform = MaterialUniform::new();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material_buffer"),
            contents: bytemuck::cast_slice(&[uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        uniform.tint = parameters.get_color(MASTER_SHADER_TINT_PARAMETER_SLOT).unwrap().into();
        uniform.specularity = parameters.get_scalar(MASTER_SHADER_SPECULARITY_PARAMETER_SLOT).unwrap().into();
        queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&[uniform]));

        // Create texture binding group
        let texture_bind_group = Self::create_texture_bind_group(
            device,
            rendering_resource_storage,
            &texture_bind_group_layout,
            &(name.to_owned() + "_textures"),
            textures,
        )?;

        // Create parameter binding group
        let parameter_bind_group = Self::create_parameter_bind_group(
            device,
            &parameter_bind_group_layout,
            &(name.to_owned() + "_parameters"),
            &buffer,
        )?;

        let renderer_material = Self {
            name: name.to_string(),
            pipeline_handle,
            texture_bind_group,
            parameter_bind_group,
            uniform,
            buffer,
        };

        Ok(renderer_material)
    }

    pub fn update_textures(
        device: &wgpu::Device, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage, 
        textures: &MaterialTextureMap
    ) -> Result<()> {
        let material = rendering_resource_storage.materials.get(material_renderer_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let material_name = material.name.clone();
        let pipeline = rendering_resource_storage.pipelines.get(material.pipeline_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        let texture_bind_group = RendererMaterial::create_texture_bind_group(
            &device, 
            &rendering_resource_storage, 
            &pipeline.material_texture_bind_group_layout, 
            &(material_name.to_owned() + "_textures"), 
            textures
        )?;

        let material = rendering_resource_storage.materials.get_mut(material_renderer_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        material.texture_bind_group = texture_bind_group;
        Ok(())
    }

    pub fn create_texture_bind_group(
        device: &wgpu::Device, 
        rendering_resource_storage: &RendererResourceStorage, 
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        textures: &MaterialTextureMap
    ) -> Result<wgpu::BindGroup> {

        // Get texture renderer handles, if is it None use default texture for this type of slot
        let color_texture = textures.data.get(MASTER_SHADER_COLOR_TEXTURE_SLOT).unwrap();
        let color_renderer_texture_handle = 
            get_renderer_texture_handle_from_material_texture(color_texture)
            .unwrap_or_else(|| get_default_texture_handles(color_texture.texture_type).1);

        let normal_texture = textures.data.get(MASTER_SHADER_NORMAL_TEXTURE_SLOT).unwrap();
        let normal_renderer_textur_handle = 
            get_renderer_texture_handle_from_material_texture(normal_texture)
            .unwrap_or_else(|| get_default_texture_handles(normal_texture.texture_type).1);

        let color_texture = rendering_resource_storage.textures.get(color_renderer_texture_handle).unwrap();
        let normal_texture = rendering_resource_storage.textures.get(normal_renderer_textur_handle).unwrap(); 

        // Set texture resources to the bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_texture.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&color_texture.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&normal_texture.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&normal_texture.sampler),
                },
            ],
            label: Some(name),
        });

        Ok(bind_group)
    }

    pub fn update_parameters(
        device: &wgpu::Device, 
        queue: &wgpu::Queue, 
        material_renderer_handle: RendererMaterialHandle,
        rendering_resource_storage: &mut RendererResourceStorage, 
        parameters: &MaterialParameterMap
    ) -> Result<()> {
        let material = rendering_resource_storage.materials.get_mut(material_renderer_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let pipeline = rendering_resource_storage.pipelines.get(material.pipeline_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        material.uniform.tint = parameters.get_color(MASTER_SHADER_TINT_PARAMETER_SLOT).unwrap().into();
        material.uniform.specularity = parameters.get_scalar(MASTER_SHADER_SPECULARITY_PARAMETER_SLOT).unwrap().into();
        queue.write_buffer(&material.buffer, 0, bytemuck::cast_slice(&[material.uniform]));

        material.parameter_bind_group = RendererMaterial::create_parameter_bind_group(
            device, 
            &pipeline.material_parameter_bind_group_layout, 
            &(material.name.to_owned() + "_parameter"), 
            &material.buffer
        )?;

        Ok(())
    }

    fn create_parameter_bind_group(
        device: &wgpu::Device, 
        parameter_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        buffer: &wgpu::Buffer,
    ) -> Result<wgpu::BindGroup> {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &parameter_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
            ],
            label: Some(name),
        });

        Ok(bind_group)
    }
}
