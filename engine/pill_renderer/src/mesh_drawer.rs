use std::{num::NonZeroU32, ops::Range};

use crate::{
    resources::RendererCamera, Instance, RendererResourceStorage, INITIAL_INSTANCE_VECTOR_CAPACITY,
};
use pill_core::{RendererError, Timer};
use pill_engine::{
    internal::{
        RenderQueueItem, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle,
        TransformComponent, RENDER_QUEUE_KEY_ORDER,
    },
    ComponentStorage,
};

use anyhow::{Error, Result};

pub struct MeshDrawer {
    max_instance_batch_size: u32,
    instances: Vec<Instance>,
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
        device: &wgpu::Device,
        renderer_resource_storage: &RendererResourceStorage,
        color_attachment: wgpu::RenderPassColorAttachment,
        depth_stencil_attachment: wgpu::RenderPassDepthStencilAttachment,
        // Rendring data
        camera: &RendererCamera,
        render_queue: &Vec<RenderQueueItem>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        timer: &mut Timer,
    ) -> Result<()> {
        timer.record("Prepare render pass");

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("mesh_drawer_encoder"),
        });

        let mut instance_batch_number = 0;
        let mut rendering_context_change_number = 0;

        for (i, instance_batch) in render_queue
            .chunks(self.max_instance_batch_size as usize)
            .enumerate()
        {
            let batch_size = instance_batch.len();
            instance_batch_number += 1;

            timer.begin_context(&format!("Prepare draw instance batch {}", i));

            timer.record(&format!("Write instance buffer"));

            // Prepare instance data and load it to buffer
            self.instances.clear();
            self.instances.reserve(instance_batch.len()); // Pre-allocate exact capacity

            for render_queue_item in instance_batch {
                let transform_slot = transform_component_storage
                    .data
                    .get(render_queue_item.entity_index as usize)
                    .ok_or_else(|| Error::new(RendererError::Other))?;
                let transform_component = transform_slot
                    .as_ref()
                    .ok_or_else(|| Error::new(RendererError::Other))?;
                //println!("Creating new instance with transform component: {:?} {:?}", i, transform_component.position);
                self.instances.push(Instance::new(transform_component));
            }

            queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&self.instances),
            ); // Update instance buffer

            let mut current_rendering_order: u8 = 0;
            let mut current_pipeline_handle: Option<RendererPipelineHandle> = None;
            let mut current_material_handle: Option<RendererMaterialHandle> = None;
            let mut current_mesh_handle: Option<RendererMeshHandle> = None;
            let mut current_mesh_index_count: u32 = 0; // Number of indices in the current mesh

            // Start encoding render pass
            let load_op = if i == 0 {
                wgpu::LoadOp::Clear(wgpu::Color::BLACK)
            } else {
                wgpu::LoadOp::Load
            };

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                    ..color_attachment.clone()
                })],
                depth_stencil_attachment: Some(depth_stencil_attachment.clone()),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer

            timer.record("Record draw instances commands");

            // Reset instance range for each batch
            // Start inclusive, end exclusive (e.g. 0..3 means indices 0, 1, 2.  e.g. 5..7 means indices 5, 6)
            let mut accumulated_instance_range: Range<u32> = 0..0;
            let mut accumulated_instance_count: u32 = 0;
            for (j, render_queue_item) in instance_batch.iter().enumerate() {
                let render_queue_key_fields =
                    pill_engine::internal::decompose_render_queue_key(render_queue_item.key);

                // Recreate resource handles
                let renderer_material_handle = RendererMaterialHandle::new(
                    render_queue_key_fields.material_index.into(),
                    NonZeroU32::new(render_queue_key_fields.material_version.into()).unwrap(),
                );
                let renderer_mesh_handle = RendererMeshHandle::new(
                    render_queue_key_fields.mesh_index.into(),
                    NonZeroU32::new(render_queue_key_fields.mesh_version.into()).unwrap(),
                );

                // Check rendering order
                if current_rendering_order > render_queue_key_fields.order {
                    if accumulated_instance_count > 0 {
                        render_pass.draw_indexed(
                            0..current_mesh_index_count,
                            0,
                            accumulated_instance_range.clone(),
                        );
                        accumulated_instance_range =
                            accumulated_instance_range.end..accumulated_instance_range.end;
                    }

                    // Set new order
                    current_rendering_order = render_queue_key_fields.order;
                    rendering_context_change_number += 1;
                }

                // Check material
                if current_material_handle != Some(renderer_material_handle) {
                    // Render accumulated instances
                    if accumulated_instance_count > 0 {
                        render_pass.draw_indexed(
                            0..current_mesh_index_count,
                            0,
                            accumulated_instance_range.clone(),
                        );
                        accumulated_instance_range =
                            accumulated_instance_range.end..accumulated_instance_range.end;
                    }
                    // Set new material
                    current_material_handle = Some(renderer_material_handle);
                    let material = renderer_resource_storage
                        .materials
                        .get(current_material_handle.unwrap())
                        .unwrap();

                    // Set pipeline if new material is using different one
                    if current_pipeline_handle != Some(material.pipeline_handle) {
                        current_pipeline_handle = Some(material.pipeline_handle);
                        let pipeline = renderer_resource_storage
                            .pipelines
                            .get(current_pipeline_handle.unwrap())
                            .unwrap();
                        render_pass.set_pipeline(&pipeline.render_pipeline);
                    }

                    render_pass.set_bind_group(0, &material.texture_bind_group, &[]);
                    render_pass.set_bind_group(1, &material.parameter_bind_group, &[]);
                    render_pass.set_bind_group(2, &camera.bind_group, &[]);

                    rendering_context_change_number += 1;
                }

                // Check mesh
                if current_mesh_handle != Some(renderer_mesh_handle) {
                    // Render accumulated instances
                    if accumulated_instance_count > 0 {
                        render_pass.draw_indexed(
                            0..current_mesh_index_count,
                            0,
                            accumulated_instance_range.clone(),
                        );
                        accumulated_instance_range =
                            accumulated_instance_range.end..accumulated_instance_range.end;
                    }
                    // Set new mesh
                    current_mesh_handle = Some(renderer_mesh_handle);
                    let mesh = renderer_resource_storage
                        .meshes
                        .get(current_mesh_handle.unwrap())
                        .unwrap();
                    current_mesh_index_count = mesh.index_count;
                    render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    render_pass
                        .set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

                    rendering_context_change_number += 1;
                }

                // Add new instance
                accumulated_instance_range =
                    accumulated_instance_range.start..accumulated_instance_range.end + 1;
                accumulated_instance_count =
                    accumulated_instance_range.end - accumulated_instance_range.start;

                // If last in batch, render accumulated instances
                if j == batch_size - 1 && accumulated_instance_count > 0 {
                    render_pass.draw_indexed(
                        0..current_mesh_index_count,
                        0,
                        accumulated_instance_range.clone(),
                    );
                }
            }

            timer.end_context()?; // End "Draw instance batch" context
        }
        queue.submit(std::iter::once(encoder.finish()));
        Ok(())
    }
}
