use std::{num::NonZeroU32, ops::Range};

use crate::ecs::{CameraComponent, PbrRenderableComponent, TransformComponent};
use crate::graphics::{RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle};
use crate::internal::decompose_render_queue_key;
use crate::{
    graphics::{Pass, PillRenderer, RendererTextureHandle, WorldQuery},
    renderer::{
        config::{
            CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
            INITIAL_INSTANCE_VECTOR_CAPACITY, MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
            MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX, MAX_INSTANCE_PER_DRAWCALL_COUNT,
        },
        instance::Instance,
        resources::{
            EngineParameters, RendererCamera, RendererMaterial, RendererMesh, RendererShader,
        },
    },
    resources::ResourceManager,
};
use pill_core::{debug, LogContext, PillSlotMapKey, PillStyle, RendererError, Result};

// --- DrawingContext ---

#[derive(Debug, Clone, Default)]
struct DrawingContext {
    rendering_order: u8,
    shader_handle: Option<RendererShaderHandle>,
    shader_name: String,
    material_handle: Option<RendererMaterialHandle>,
    material_name: String,
    mesh_handle: Option<RendererMeshHandle>,
    mesh_name: String,
    mesh_index_count: u32,

    accumulated_instance_range: Range<u32>,

    instance_batch_number: u32,
    instance_batch_size: u32,
}

impl DrawingContext {
    fn log(&self) {
        debug!(
            LogContext::Frame =>
            "Draw {} instance(s) {}->{}/{} command recorded [Batch: {}, Rendering order: {}, Shader: {}, Material: {}, Mesh: {}]",
            self.accumulated_instance_range.end - self.accumulated_instance_range.start,
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

    fn record_draw_accumulated_instances(&mut self, render_pass: &mut wgpu::RenderPass) {
        if self.accumulated_instance_range.end > self.accumulated_instance_range.start {
            render_pass.draw_indexed(
                0..self.mesh_index_count,
                0,
                self.accumulated_instance_range.clone(),
            );
            self.log();
            self.accumulated_instance_range =
                self.accumulated_instance_range.end..self.accumulated_instance_range.end;
        }
    }

    fn accumulate_instance(&mut self) {
        self.accumulated_instance_range.end += 1;
    }

    fn change_rendering_order(&mut self, new_order: u8) {
        self.rendering_order = new_order;
        debug!(LogContext::Frame => "Rendering order changed to: {}", self.rendering_order);
    }

    fn change_shader(
        &mut self,
        resource_manager: &ResourceManager,
        engine_parameters: &EngineParameters,
        shader_handle: RendererShaderHandle,
        render_pass: &mut wgpu::RenderPass,
        camera: &RendererCamera,
    ) {
        self.shader_handle = Some(shader_handle);
        let shader: &RendererShader = resource_manager
            .get_resource::<RendererShader>(&shader_handle)
            .expect("RendererShader not found");
        self.shader_name = shader.name.clone();

        debug!(LogContext::Frame => "Changing shader to: {}", self.shader_name.name_style());

        render_pass.set_pipeline(&shader.render_pipeline);

        if shader.pass_engine_parameters {
            render_pass.set_bind_group(
                ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &engine_parameters.bind_group,
                &[],
            );
        }

        if shader.pass_camera_parameters {
            render_pass.set_bind_group(
                CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &camera.bind_group,
                &[],
            );
        }
    }

    fn change_material(
        &mut self,
        resource_manager: &ResourceManager,
        material_handle: RendererMaterialHandle,
        render_pass: &mut wgpu::RenderPass,
    ) {
        self.material_handle = Some(material_handle);
        let material = resource_manager
            .get_resource::<RendererMaterial>(&material_handle)
            .expect("RendererMaterial not found");
        self.material_name = material.name.clone();

        if let Some(ref parameters_bind_group) = material.parameters_bind_group {
            render_pass.set_bind_group(
                MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                parameters_bind_group,
                &[],
            );
        }

        if let Some(ref texture_bind_group) = material.textures_bind_group {
            render_pass.set_bind_group(
                MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX,
                texture_bind_group,
                &[],
            );
        }
    }

    fn change_mesh(
        &mut self,
        resource_manager: &ResourceManager,
        mesh_handle: RendererMeshHandle,
        render_pass: &mut wgpu::RenderPass,
    ) {
        self.mesh_handle = Some(mesh_handle);
        let mesh = resource_manager
            .get_resource::<RendererMesh>(&mesh_handle)
            .expect("RendererMesh not found");
        self.mesh_name = mesh.name.clone();

        self.mesh_index_count = mesh.index_count;
        render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    }
}

// --- PassMesh ---

/// Instance-batched rendering pass using `RendererShader` pipelines.
/// Install via `renderer.set_passes(vec![Box::new(PassMesh::new())])`.
pub struct PassMesh {
    depth_handle: Option<RendererTextureHandle>,
    camera: Option<RendererCamera>,
    instance_buffer: Option<wgpu::Buffer>,
    instances: Vec<Instance>,
}

impl PassMesh {
    /// Creates a `PassMesh`; GPU resources are allocated lazily in `Pass::init`.
    pub fn new() -> Self {
        Self {
            depth_handle: None,
            camera: None,
            instance_buffer: None,
            instances: Vec::with_capacity(INITIAL_INSTANCE_VECTOR_CAPACITY),
        }
    }
}

impl Default for PassMesh {
    fn default() -> Self {
        Self::new()
    }
}

impl Pass for PassMesh {
    fn get_label(&self) -> &str {
        "pass_mesh"
    }

    /// Allocates the depth texture, camera UBO, and instance buffer.
    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        self.depth_handle = Some(renderer.create_depth_texture("pass_mesh_depth")?);

        let layout = renderer.get_camera_bind_group_layout();
        self.camera = Some(RendererCamera::new(renderer.get_device(), layout)?);

        let buffer_size =
            (std::mem::size_of::<Instance>() * MAX_INSTANCE_PER_DRAWCALL_COUNT) as u64;
        self.instance_buffer = Some(
            renderer
                .get_device()
                .create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pass_mesh_instance_buffer"),
                    size: buffer_size,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
        );

        Ok(())
    }

    /// Records instance-batched draw commands for the current frame.
    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        // Query the component storages this pass needs.
        let camera_components = world.query::<CameraComponent>()?;
        let transform_components = world.query::<TransformComponent>()?;
        let pbr_renderable_components = world.query::<PbrRenderableComponent>()?;

        // Resolve active camera.
        let active_camera_index = world.active_camera.data().index as usize;
        let active_camera_component = camera_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();
        let active_camera_transform = transform_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();

        // Update camera UBO.
        self.camera.as_mut().unwrap().update(
            renderer.get_queue(),
            active_camera_component,
            active_camera_transform,
        );

        let clear_color = active_camera_component.clear_color;

        let depth_view = renderer
            .get_render_target_view(self.depth_handle.unwrap())
            .ok_or_else(|| -> pill_core::PillError { RendererError::Other.into() })?;

        debug!(LogContext::Frame => "Recording mesh draw commands");

        let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass_mesh"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear_color.x as f64,
                        g: clear_color.y as f64,
                        b: clear_color.z as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let mut current_drawing_context = DrawingContext::default();

        let mut render_pass = render_pass.forget_lifetime();

        // Build this pass's draw list by querying the world (data-driven; no engine-global queue):
        // every entity with a PbrRenderableComponent is an instance.
        let renderable_indices: Vec<u32> = pbr_renderable_components
            .data
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.as_ref().map(|_| index as u32))
            .collect();
        for (i, instance_batch) in renderable_indices
            .chunks(MAX_INSTANCE_PER_DRAWCALL_COUNT)
            .enumerate()
        {
            let batch_size = instance_batch.len();
            current_drawing_context.instance_batch_number = i as u32;
            current_drawing_context.instance_batch_size = batch_size as u32;

            self.instances.clear();

            for &entity_index in instance_batch {
                let transform_slot = transform_components
                    .data
                    .get(entity_index as usize)
                    .ok_or_else(|| -> pill_core::PillError { RendererError::Other.into() })?;
                let transform_component = transform_slot
                    .as_ref()
                    .ok_or_else(|| -> pill_core::PillError { RendererError::Other.into() })?;
                self.instances.push(Instance::new(transform_component));
            }

            let instance_buffer = self.instance_buffer.as_ref().unwrap();
            renderer.get_queue().write_buffer(
                instance_buffer,
                0,
                bytemuck::cast_slice(&self.instances),
            );
            render_pass.set_vertex_buffer(1, instance_buffer.slice(..));

            current_drawing_context.accumulated_instance_range = 0..0;

            for (j, &entity_index) in instance_batch.iter().enumerate() {
                let key = pbr_renderable_components
                    .data
                    .get(entity_index as usize)
                    .and_then(|slot| slot.as_ref())
                    .and_then(|pbr| pbr.render_queue_key)
                    .unwrap_or(0);
                let key_fields = decompose_render_queue_key(key);

                let renderer_shader_handle = RendererShaderHandle::new(
                    key_fields.shader_index.into(),
                    NonZeroU32::new(key_fields.shader_version.into()).unwrap(),
                );
                let renderer_material_handle = RendererMaterialHandle::new(
                    key_fields.material_index.into(),
                    NonZeroU32::new(key_fields.material_version.into()).unwrap(),
                );
                let renderer_mesh_handle = RendererMeshHandle::new(
                    key_fields.mesh_index.into(),
                    NonZeroU32::new(key_fields.mesh_version.into()).unwrap(),
                );

                if current_drawing_context.rendering_order > key_fields.order {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_rendering_order(key_fields.order);
                }

                if current_drawing_context.shader_handle != Some(renderer_shader_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_shader(
                        world.resources,
                        renderer.get_engine_parameters(),
                        renderer_shader_handle,
                        &mut render_pass,
                        self.camera.as_ref().unwrap(),
                    );
                }

                if current_drawing_context.material_handle != Some(renderer_material_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_material(
                        world.resources,
                        renderer_material_handle,
                        &mut render_pass,
                    );
                }

                if current_drawing_context.mesh_handle != Some(renderer_mesh_handle) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_mesh(
                        world.resources,
                        renderer_mesh_handle,
                        &mut render_pass,
                    );
                }

                current_drawing_context.accumulate_instance();

                if j == batch_size - 1 {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                }
            }
        }

        Ok(())
    }
}
