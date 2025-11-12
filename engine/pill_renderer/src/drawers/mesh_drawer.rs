use core::time;
use std::{num::NonZeroU32, ops::Range};

use pill_core::{debug, LogContext, PillStyle, RendererError, Timer};
use pill_engine::{ internal::{ RenderQueueItem, RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle, TransformComponent, RENDER_QUEUE_KEY_ORDER}, ComponentStorage};
use crate::{ 
    config::{
        CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, INITIAL_INSTANCE_VECTOR_CAPACITY, MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX
    }, 
    resources::{ RendererCamera, RendererResourceStorage, RendererShader }, 
    Instance, 
};

use anyhow::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct DrawingContext {
    rendering_order: u8,
    shader_handle: Option<RendererShaderHandle>,
    shader_name: String,
    material_handle: Option<RendererMaterialHandle>,
    material_name: String,
    mesh_handle: Option<RendererMeshHandle>,
    mesh_name: String,
    mesh_index_count: u32, // Number of indices in the current mesh   

    accumulated_instance_range: Range<u32>,
    accumulated_instance_count: u32,

    rendering_context_change_number: u32,

    instance_batch_number: u32,
    instance_batch_size: u32,
}

impl DrawingContext {
    pub fn log(&self) {
        debug!(
            LogContext::Frame =>
            "Draw {} instance(s) {}->{}/{} command recorded [Batch: {}, Rendering order: {}, Shader: {}, Material: {}, Mesh: {}]",
            self.accumulated_instance_count, 
            self.accumulated_instance_range.start, 
            self.accumulated_instance_range.end - 1, 
            self.instance_batch_size, 
            self.instance_batch_number, 
            self.rendering_order, 
            self.shader_name.name_style(), 
            self.material_name.name_style(), 
            self.mesh_name.name_style()
        );
    }

    pub fn record_draw_accumulated_instances(&mut self, render_pass: &mut wgpu::RenderPass) {
        if self.accumulated_instance_count > 0 {
            render_pass.draw_indexed(0..self.mesh_index_count, 0, self.accumulated_instance_range.clone());
            self.log();
            self.accumulated_instance_range = self.accumulated_instance_range.end..self.accumulated_instance_range.end;
            self.accumulated_instance_count = 0;
        }
    }

    pub fn accumulate_instance(&mut self) {
        self.accumulated_instance_range = self.accumulated_instance_range.start..self.accumulated_instance_range.end + 1;
        self.accumulated_instance_count = self.accumulated_instance_range.end - self.accumulated_instance_range.start;
    }

    pub fn change_rendering_order(&mut self, new_order: u8) {
        self.rendering_order = new_order;
        
        self.rendering_context_change_number += 1;
        debug!(LogContext::Frame => "Rendering order changed to: {}", self.rendering_order);
    }

    pub fn change_shader(
        &mut self, 
        renderer_resource_storage: &RendererResourceStorage, 
        shader_handle: RendererShaderHandle,
        render_pass: &mut wgpu::RenderPass,
        camera: &RendererCamera,
    ) {

        self.shader_handle = Some(shader_handle);
        let shader: &RendererShader = renderer_resource_storage.shaders.get(shader_handle).unwrap();
        self.shader_name = shader.name.clone();

        debug!(LogContext::Frame => "Changing shader to: {}", self.shader_name.name_style());

        render_pass.set_pipeline(&shader.render_pipeline);

        if shader.pass_engine_parameters {
            render_pass.set_bind_group(ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, &renderer_resource_storage.engine_parameters.bind_group, &[]);
            debug!(LogContext::Frame => "Engine parameters bound");
        }   

        if shader.pass_camera_parameters {
            render_pass.set_bind_group(CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, &camera.bind_group, &[]);
            debug!(LogContext::Frame => "Camera parameters bound");
        }

        self.rendering_context_change_number += 1;
        debug!(LogContext::Frame => "Renderer pipeline shader changed to: {}", self.shader_name.name_style());
    }

    pub fn change_material(
        &mut self, 
        renderer_resource_storage: &RendererResourceStorage, 
        material_handle: RendererMaterialHandle,
        render_pass: &mut wgpu::RenderPass,
    ) {
        self.material_handle = Some(material_handle);
        let material = renderer_resource_storage.materials.get(material_handle).unwrap();
        self.material_name = material.name.clone();
    
        debug!(LogContext::Frame => "Changing material to: {}", self.material_name.name_style());

        if let Some(ref parameters_bind_group) = material.parameters_bind_group {
            render_pass.set_bind_group(MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, parameters_bind_group, &[]);
            debug!(LogContext::Frame => "Material parameters bound"); 
        }

        if let Some(ref texture_bind_group) = material.textures_bind_group {
            render_pass.set_bind_group(MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX, texture_bind_group, &[]);
            debug!(LogContext::Frame => "Material textures bound");
        }
        
        self.rendering_context_change_number += 1;
        debug!(LogContext::Frame => "Renderer pipeline material changed to: {}", self.material_name.name_style());
    }

    pub fn change_mesh(
        &mut self, 
        renderer_resource_storage: &RendererResourceStorage, 
        mesh_handle: RendererMeshHandle,
        render_pass: &mut wgpu::RenderPass,
    ) {
        self.mesh_handle = Some(mesh_handle);               
        let mesh = renderer_resource_storage.meshes.get(mesh_handle).unwrap();
        self.mesh_name = mesh.name.clone();

        self.mesh_index_count = mesh.index_count;
        render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..)); 
        render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32); 

        self.rendering_context_change_number += 1;
        debug!(LogContext::Frame => "Renderer pipeline mesh changed to: {}", self.mesh_name.name_style());
    }
}

pub struct MeshDrawer {
    max_instance_batch_size: u32,
    instances: Vec::<Instance>,
    instance_buffer: wgpu::Buffer,
}

impl MeshDrawer {
    pub fn new(device: &wgpu::Device, max_instance_batch_size: u32) -> Self {
        // Create instance buffer
        let buffer_size = (size_of::<Instance>() * max_instance_batch_size as usize) as u64;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MeshDrawer {
            max_instance_batch_size,
            instances: Vec::<Instance>::with_capacity(INITIAL_INSTANCE_VECTOR_CAPACITY), 
            instance_buffer,
        }
    }

    pub fn record_draw_commands(
        &mut self, 
        // Resources
        queue: &wgpu::Queue, 
        encoder: &mut wgpu::CommandEncoder,
        renderer_resource_storage: &RendererResourceStorage, 
        color_attachment: wgpu::RenderPassColorAttachment, 
        depth_stencil_attachment: wgpu::RenderPassDepthStencilAttachment,
        // Rendring data
        camera: &RendererCamera,
        render_queue: &Vec::<RenderQueueItem>, 
        transform_component_storage: &ComponentStorage<TransformComponent>,
        timer: &mut Timer,
       // profiler: &mut Profiler,
    ) -> Result<()> { 
        timer.record("Prepare render pass");
        debug!(LogContext::Frame => "Recording mesh draw commands");

        //let _timestamp_query_start = profiler.write_timestamp(encoder, "xx");

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("main pass"),
            color_attachments: &[Some(color_attachment.clone())],
            depth_stencil_attachment: Some(depth_stencil_attachment.clone()),
            timestamp_writes: None,
            occlusion_query_set: None, // profiler.get_occlusion_query_set(), // immut borrow ends after this stmt
            //timestamp_writes: None,
            // occlusion_query_set: profiler.get_occlusion_query_set(), // immut borrow ends after this stmt
            // timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
            //     query_set: profiler.get_timestamp_query_set().unwrap(),
            //     beginning_of_pass_write_index: Some(0),
            //     end_of_pass_write_index: Some(1),
            // }),
        });

       // let _pipeline_statistics_query_start = profiler.begin_pipeline_statistics_query(&mut render_pass);
        //let _occlusion_query_start = profiler.begin_occlusion_query(&mut render_pass);

       
 // let mut current_rendering_order: u8 = 0;

        // let mut current_shader_handle: Option<RendererShaderHandle> = None;
        // let mut current_shader_name = "";
        // let mut current_material_handle: Option<RendererMaterialHandle> = None;
        // let mut current_material_name = "";
        // let mut current_mesh_handle: Option<RendererMeshHandle> = None;
        // let mut current_mesh_name = "";
        // let mut current_mesh_index_count: u32 = 0; // Number of indices in the current mesh
    
        let mut current_drawing_context = DrawingContext::default();

        for (i, instance_batch) in render_queue.chunks(self.max_instance_batch_size as usize).enumerate() {

            let batch_size = instance_batch.len();
            current_drawing_context.instance_batch_number = i as u32;
            current_drawing_context.instance_batch_size = batch_size as u32;

            timer.begin_context(&format!("Prepare draw instance batch {}", i));
            
            timer.record(&format!("Write instance buffer"));

            // Prepare instance data and load it to buffer
            self.instances.clear();
            self.instances.reserve(instance_batch.len()); // Pre-allocate exact capacity

            for render_queue_item in instance_batch {
                 let transform_slot =  transform_component_storage.data
                    .get(render_queue_item.entity_index as usize)
                    .ok_or_else(|| Error::new(RendererError::Other))?;
                let transform_component = transform_slot
                    .as_ref()
                    .ok_or_else(|| Error::new(RendererError::Other))?;
                //println!("Creating new instance with transform component: {:?} {:?}", i, transform_component.position);
                self.instances.push(Instance::new(transform_component));
            }

            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances)); // Update instance buffer

            render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer
            
            timer.record("Record draw instances commands");

            // Reset instance range for each batch
            current_drawing_context.accumulated_instance_range = 0..0;
            current_drawing_context.accumulated_instance_count = 0; 

            for (j, render_queue_item) in instance_batch.iter().enumerate() {
                let render_queue_key_fields = pill_engine::internal::decompose_render_queue_key(render_queue_item.key);

                // Recreate resource handles
                let renderer_shader_handle = RendererShaderHandle::new(render_queue_key_fields.shader_index.into(), NonZeroU32::new(render_queue_key_fields.shader_version.into()).unwrap());
                let renderer_material_handle = RendererMaterialHandle::new(render_queue_key_fields.material_index.into(), NonZeroU32::new(render_queue_key_fields.material_version.into()).unwrap());
                let renderer_mesh_handle = RendererMeshHandle::new(render_queue_key_fields.mesh_index.into(), NonZeroU32::new(render_queue_key_fields.mesh_version.into()).unwrap());

                // Check for rendering order change
                if current_drawing_context.rendering_order > render_queue_key_fields.order {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_rendering_order(render_queue_key_fields.order);
                }

                // Check for shader change
                if current_drawing_context.shader_handle != Some(renderer_shader_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_shader(renderer_resource_storage, renderer_shader_handle, &mut render_pass, camera);
                }

                // Check for material change
                if current_drawing_context.material_handle != Some(renderer_material_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_material(renderer_resource_storage, renderer_material_handle, &mut render_pass);
                }

                // Check for mesh change
                if current_drawing_context.mesh_handle != Some(renderer_mesh_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_mesh(renderer_resource_storage, renderer_mesh_handle, &mut render_pass);
                }

                // Add new instance
                current_drawing_context.accumulate_instance();

                // If last in batch, draw accumulated instances
                if j == batch_size - 1 {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                }
            }

            timer.end_context()?; // End "Draw instance batch" context
        }
        
        // Drop render_pass before finishing encoder
        //let _occlusion_query_end = profiler.end_occlusion_query(&mut render_pass);
        //let _pipeline_statistics_query_end = profiler.end_pipeline_statistics_query(&mut render_pass);
        
        drop(render_pass);

       // let _timestamp_query_end = profiler.write_timestamp(encoder, "xx12");

        //queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }
}