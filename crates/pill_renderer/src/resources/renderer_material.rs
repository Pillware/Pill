use crate::RenderingResourceStorage;
use crate::resources::RendererTexture;

use pill_core::na::SliceRange;
use pill_engine::internal::{
    MaterialParameter,
    MaterialParameterMap,
    ParameterMap,
    RendererError,
    TextureMap,
    RendererMaterialHandle,
    RendererTextureHandle,
    RendererPipelineHandle, 
    TextureHandle,
    MaterialTexture,
};

use wgpu::util::DeviceExt;
use std::path::{ Path, PathBuf };
use anyhow::{Result, Context, Error};

// --- Material Uniform ---

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct MaterialUniform {
    pub(crate) tint: [f32; 3],
}

impl MaterialUniform {
    pub fn new() -> Self {
        Self {
            tint: cgmath::Vector3::<f32>::new(0.0,0.0,0.0).into(),
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
        rendering_resource_storage: &RenderingResourceStorage,
        name: &str,
        pipeline_handle: RendererPipelineHandle,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        textures: &TextureMap,
        parameter_bind_group_layout: &wgpu::BindGroupLayout,
        parameters: &ParameterMap,
    ) -> Result<Self> {

        // Create parameter buffer and write data to it
        let mut uniform = MaterialUniform::new();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("material_buffer"),
            contents: bytemuck::cast_slice(&[uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        uniform.tint = parameters.get_color("Tint").unwrap().into(); // [TODO] Move magic value
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
        rendering_resource_storage: &mut RenderingResourceStorage, 
        textures: &TextureMap
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
        rendering_resource_storage: &RenderingResourceStorage, 
        texture_bind_group_layout: &wgpu::BindGroupLayout,
        name: &str,
        textures: &TextureMap
    ) -> Result<wgpu::BindGroup> {

        // [TODO] Move magic names
        let color_texture_renderer_handle = textures.get("Color").unwrap().get_texture_data().unwrap().1;
        let normal_texture_renderer_handle = textures.get("Color").unwrap().get_texture_data().unwrap().1;
        let color_texture = rendering_resource_storage.textures.get(color_texture_renderer_handle).unwrap();
        let normal_texture = rendering_resource_storage.textures.get(normal_texture_renderer_handle).unwrap(); 

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
        rendering_resource_storage: &mut RenderingResourceStorage, 
        parameters: &ParameterMap
    ) -> Result<()> {
        let material = rendering_resource_storage.materials.get_mut(material_renderer_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let pipeline = rendering_resource_storage.pipelines.get(material.pipeline_handle).ok_or(Error::new(RendererError::RendererResourceNotFound))?;

        material.uniform.tint = parameters.get_color("Tint").unwrap().into(); // [TODO] Move magic value
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
