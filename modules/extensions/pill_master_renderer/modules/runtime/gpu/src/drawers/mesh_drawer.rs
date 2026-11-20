//! Batching and draw recording for the master renderer's mesh path.
//!
//! # Responsibilities
//!
//! - Track the shader, material, and mesh that the bound GPU resources belong
//!   to, and draw the accumulated instances when any of them changes
//!   ([`DrawingContext`]).
//! - Own the instance buffer, keep it large enough for a frame's instances,
//!   and upload them once per frame ([`MeshDrawer::upload_instances`]).
//! - Open a render pass and record the draws of the ranges it is given
//!   ([`MeshDrawer::record_draw_commands`]).
//!
//! # Design
//!
//! The queue arrives sorted by its packed key: rendering order first, then
//! shader, material, and mesh. Each run of equal keys becomes one instanced
//! draw, and a new draw starts only where the key changes, where the pass's
//! pipeline changes between ranges, or where a range ends.
//!
//! The frame's instances are uploaded once, in queue order, into one growable
//! buffer, so a queue position is also an instance index. Every pass draws
//! straight from that upload, and nothing is copied or written per pass.

// Standard library
use std::ops::Range;

// External crates
use pill_core::debug;
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::{
    components::RenderViewport,
    config::{
        CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
        MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX,
    },
    profiler::Profiler,
    render_queue::RenderQueueItem,
    resources::{
        RendererCamera, RendererMaterialHandle, RendererMeshHandle, RendererResourceStorage,
        RendererShader, RendererShaderHandle,
    },
    Instance,
};

// =============================================================================
// Drawing Context
// =============================================================================

/// The bound state and instance range of the batch being accumulated.
///
/// It holds the shader, material, and mesh the bound GPU resources belong to,
/// plus the range of instances accumulated since the last recorded draw.
/// `Default` leaves it empty, so the first item always counts as a change and
/// its resources are bound before anything is drawn.
#[derive(Debug, Clone, Default)]
pub(crate) struct DrawingContext {
    shader_handle: Option<RendererShaderHandle>,
    shader_name: String,
    material_handle: Option<RendererMaterialHandle>,
    material_name: String,
    mesh_handle: Option<RendererMeshHandle>,
    mesh_name: String,
    /// How many indices the currently bound mesh has.
    mesh_index_count: u32,

    accumulated_instance_range: Range<u32>,
    accumulated_instance_count: u32,
}

impl DrawingContext {
    /// Writes the telemetry line that describes the draw just recorded.
    ///
    /// The line carries the instance range and the shader, material, and mesh
    /// names, so a frame that batches the wrong draws can be traced from the
    /// log alone.
    pub fn log(&self) {
        debug!(
            target: telemetry_target::RENDERING,
            instances = self.accumulated_instance_count,
            first_instance = self.accumulated_instance_range.start,
            last_instance = self.accumulated_instance_range.end - 1,
            shader = %self.shader_name,
            material = %self.material_name,
            mesh = %self.mesh_name,
            "draw command recorded"
        );
    }

    /// Records the instances accumulated since the last draw as one indexed
    /// draw, when there are any, and then empties the range.
    ///
    /// Call it wherever the accumulated instances stop sharing the bound
    /// state: before a shader, material, or mesh switch, when the pass's
    /// pipeline changes between ranges, and at the end of the last range.
    pub fn record_draw_accumulated_instances(&mut self, render_pass: &mut wgpu::RenderPass) {
        if self.accumulated_instance_count > 0 {
            render_pass.draw_indexed(
                0..self.mesh_index_count,
                0,
                self.accumulated_instance_range.clone(),
            );
            self.log();
            self.accumulated_instance_range =
                self.accumulated_instance_range.end..self.accumulated_instance_range.end;
            self.accumulated_instance_count = 0;
        }
    }

    /// Extends the range the next draw covers by one instance.
    ///
    /// The caller makes one call per queue item, so the range grows instance by
    /// instance until a state change or the end of the range records it.
    pub fn accumulate_instance(&mut self) {
        self.accumulated_instance_range =
            self.accumulated_instance_range.start..self.accumulated_instance_range.end + 1;
        self.accumulated_instance_count =
            self.accumulated_instance_range.end - self.accumulated_instance_range.start;
    }

    /// Draws what is accumulated, then starts a new run at `queue_position`.
    ///
    /// A range can begin anywhere in the instance buffer, so its first instance
    /// does not have to follow the one the previous range ended with.
    pub fn start_run(&mut self, render_pass: &mut wgpu::RenderPass, queue_position: u32) {
        self.record_draw_accumulated_instances(render_pass);
        self.accumulated_instance_range = queue_position..queue_position;
    }

    /// Forget which shader is bound, so the next item binds its pipeline again
    /// even when the shader handle has not changed.
    ///
    /// A queue key names the shader, material, and mesh a draw binds, but never
    /// the pipeline it draws through. A run drawn through another pipeline
    /// therefore has to bind that pipeline again.
    pub fn forget_bound_shader(&mut self) {
        self.shader_handle = None;
    }

    /// Binds the pipeline and bind groups the given shader draws through.
    ///
    /// The shader's own pipeline is used unless the pass supplies an override,
    /// and the engine and camera bind groups are set only when the shader
    /// declares that it reads them.
    pub fn change_shader(
        &mut self,
        renderer_resource_storage: &RendererResourceStorage,
        shader_handle: RendererShaderHandle,
        pipeline_override: Option<&wgpu::RenderPipeline>,
        render_pass: &mut wgpu::RenderPass,
        camera: &RendererCamera,
    ) {
        self.shader_handle = Some(shader_handle);
        let shader: &RendererShader = renderer_resource_storage
            .shaders
            .get(shader_handle)
            .unwrap();
        self.shader_name = shader.name.clone();

        debug!(target: telemetry_target::RENDERING, shader = %self.shader_name, "changing shader");

        // A pass that has its own pipeline draws through it. That pipeline was
        // built for the pass's target format, depth, and culling, which the
        // pipeline the shader was built with knows nothing about.
        render_pass.set_pipeline(pipeline_override.unwrap_or(&shader.render_pipeline));

        if shader.reads_engine_parameters {
            render_pass.set_bind_group(
                ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &renderer_resource_storage.engine_parameters.bind_group,
                &[],
            );
            debug!(target: telemetry_target::RENDERING, "engine parameters bound");
        }

        if shader.reads_camera_parameters {
            render_pass.set_bind_group(
                CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &camera.bind_group,
                &[],
            );
            debug!(target: telemetry_target::RENDERING, "camera parameters bound");
        }

        debug!(target: telemetry_target::RENDERING, shader = %self.shader_name, "renderer pipeline shader changed");
    }

    /// Binds the parameter and texture bind groups the given material draws
    /// through.
    ///
    /// Either group may be absent, because a material whose shader declares no
    /// parameters, or no textures, stores no group for it. A missing group
    /// leaves whatever was bound before it in place, so a material does not
    /// clear bindings it does not use.
    pub fn change_material(
        &mut self,
        renderer_resource_storage: &RendererResourceStorage,
        material_handle: RendererMaterialHandle,
        render_pass: &mut wgpu::RenderPass,
    ) {
        self.material_handle = Some(material_handle);
        let material = renderer_resource_storage
            .materials
            .get(material_handle)
            .unwrap();
        self.material_name = material.name.clone();

        debug!(target: telemetry_target::RENDERING, material = %self.material_name, "changing material");

        if let Some(ref parameters_bind_group) = material.parameters_bind_group {
            render_pass.set_bind_group(
                MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                parameters_bind_group,
                &[],
            );
            debug!(target: telemetry_target::RENDERING, "material parameters bound");
        }

        if let Some(ref texture_bind_group) = material.textures_bind_group {
            render_pass.set_bind_group(
                MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX,
                texture_bind_group,
                &[],
            );
            debug!(target: telemetry_target::RENDERING, "material textures bound");
        }

        debug!(target: telemetry_target::RENDERING, material = %self.material_name, "renderer pipeline material changed");
    }

    /// Binds the vertex and index buffers of the given mesh.
    ///
    /// The mesh's index count is stored alongside the binding, because the
    /// draw calls read it from the context, not from the mesh.
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

        debug!(target: telemetry_target::RENDERING, mesh = %self.mesh_name, "renderer pipeline mesh changed");
    }
}

// =============================================================================
// Mesh Drawer
// =============================================================================

/// Owns the instance buffer and records a frame's mesh draws through it.
///
/// One drawer lives for the renderer's lifetime. The buffer is reused every
/// frame and grows only when a frame brings more instances than it holds.
pub(crate) struct MeshDrawer {
    /// How many instances the buffer grows by at a time.
    instance_capacity_step: usize,
    instance_buffer: wgpu::Buffer,
    /// How many instances the buffer has room for.
    instance_capacity: usize,
}

impl MeshDrawer {
    /// Creates a drawer whose instance buffer has room for
    /// `instance_capacity_step` instances, and grows in steps of that many.
    pub fn new(device: &wgpu::Device, instance_capacity_step: usize) -> Self {
        MeshDrawer {
            instance_capacity_step,
            instance_buffer: Self::allocate(device, instance_capacity_step),
            instance_capacity: instance_capacity_step,
        }
    }

    /// The buffer the frame's instances were uploaded into, in queue order.
    pub fn instance_buffer(&self) -> &wgpu::Buffer {
        &self.instance_buffer
    }

    /// Creates the instance buffer with room for `instance_capacity` instances.
    fn allocate(device: &wgpu::Device, instance_capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: (size_of::<Instance>() * instance_capacity) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Uploads the frame's instances, in queue order, in one write.
    ///
    /// Call this once per frame, before any pass records: every geometry pass
    /// draws from this one upload, addressing instances by queue position. The
    /// data is copied to the GPU before the command buffers that draw from it
    /// are submitted, so one write here serves every pass. A buffer written
    /// between passes would instead hold only the last write when the draws
    /// run, and every pass would read that.
    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[Instance],
    ) {
        if instances.len() > self.instance_capacity {
            // Grown in whole steps, so a frame one instance over the limit
            // reallocates once rather than on every frame after it.
            let instance_capacity =
                instances.len().div_ceil(self.instance_capacity_step) * self.instance_capacity_step;
            self.instance_buffer = Self::allocate(device, instance_capacity);
            self.instance_capacity = instance_capacity;
        }
        if !instances.is_empty() {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        }
    }

    /// Opens a render pass on `encoder`, records the draws for the queue
    /// positions in `ranges`, and ends the pass before returning.
    ///
    /// The instances were uploaded by [`MeshDrawer::upload_instances`] in queue
    /// order, so a queue position is also the instance's index in the bound
    /// buffer and nothing is copied here. Consecutive items with equal keys
    /// become one instanced draw.
    ///
    /// `pass_pipeline` is the pass's own pipeline. A run the pass owns draws
    /// through it; a run it does not own draws through the pipeline of the
    /// shader its material names.
    #[allow(clippy::too_many_arguments)]
    pub fn record_draw_commands(
        &mut self,
        // Resources
        encoder: &mut wgpu::CommandEncoder,
        renderer_resource_storage: &RendererResourceStorage,
        label: &str,
        pass_pipeline: Option<&wgpu::RenderPipeline>,
        color_attachments: &[Option<wgpu::RenderPassColorAttachment>],
        depth_stencil_attachment: wgpu::RenderPassDepthStencilAttachment,
        // Rendering data
        camera: &RendererCamera,
        render_queue: &[RenderQueueItem],
        ranges: &[crate::chain::DrawRange],
        viewport: RenderViewport,
        // Counts the pass's shader invocations and the samples it made visible,
        // when GPU profiling is on.
        profiler: Option<&Profiler>,
    ) {
        // Step 1: Open the pass and set the state every draw in it shares.
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments,
            depth_stencil_attachment: Some(depth_stencil_attachment),
            timestamp_writes: None,
            occlusion_query_set: profiler.and_then(|profiler| profiler.get_occlusion_query_set()),
            multiview_mask: None,
        });

        render_pass.set_viewport(
            viewport.x as f32,
            viewport.y as f32,
            viewport.width as f32,
            viewport.height as f32,
            0.0,
            1.0,
        );
        render_pass.set_scissor_rect(viewport.x, viewport.y, viewport.width, viewport.height);
        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        let statistics_query_begun = profiler
            .and_then(|profiler| profiler.begin_pipeline_statistics_query(&mut render_pass))
            .is_some();
        let occlusion_query_begun = profiler
            .and_then(|profiler| profiler.begin_occlusion_query(&mut render_pass))
            .is_some();

        // Step 2: Record the draws of every range.
        let mut current_drawing_context = DrawingContext::default();
        // Key of the last item whose state was checked. It keeps its value
        // across ranges, because the bindings it describes stay bound too.
        let mut previous_key: Option<u64> = None;
        // Whether the last range drew through the pass's own pipeline. A change
        // here draws what accumulated and forgets the bound shader, so the next
        // item binds the pipeline again.
        let mut own_pipeline: Option<bool> = None;

        for draw_range in ranges {
            // A range starts a new run of instance indices, so the instances the
            // previous range accumulated are drawn first.
            current_drawing_context.start_run(&mut render_pass, draw_range.range.start);
            // The pass's own pipeline only draws instances its own shader
            // shades. When a range switches pipeline, record what was
            // collected and make the next item bind its pipeline again.
            if own_pipeline != Some(draw_range.own_pipeline) {
                own_pipeline = Some(draw_range.own_pipeline);
                current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                current_drawing_context.forget_bound_shader();
            }
            let pipeline_override = if draw_range.own_pipeline {
                pass_pipeline
            } else {
                None
            };
            let render_queue_items =
                &render_queue[draw_range.range.start as usize..draw_range.range.end as usize];

            for render_queue_item in render_queue_items {
                // The key only says which draws belong together; the handles the
                // draw binds travel beside it. So an item with the same key as
                // the previous one joins the batch being accumulated.
                if previous_key == Some(render_queue_item.key) {
                    current_drawing_context.accumulate_instance();
                    continue;
                }
                previous_key = Some(render_queue_item.key);

                // A different shader means a different pipeline and layout, so
                // draw what has accumulated before binding it.
                if current_drawing_context.shader_handle != Some(render_queue_item.shader) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_shader(
                        renderer_resource_storage,
                        render_queue_item.shader,
                        pipeline_override,
                        &mut render_pass,
                        camera,
                    );
                }

                // A different material replaces the parameter and texture bind
                // groups, so draw what has accumulated before binding them.
                if current_drawing_context.material_handle != Some(render_queue_item.material) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_material(
                        renderer_resource_storage,
                        render_queue_item.material,
                        &mut render_pass,
                    );
                }

                // A different mesh replaces the vertex and index buffers, so
                // draw what has accumulated before binding them.
                if current_drawing_context.mesh_handle != Some(render_queue_item.mesh) {
                    current_drawing_context.record_draw_accumulated_instances(&mut render_pass);
                    current_drawing_context.change_mesh(
                        renderer_resource_storage,
                        render_queue_item.mesh,
                        &mut render_pass,
                    );
                }

                current_drawing_context.accumulate_instance();
            }
        }
        // Draw whatever the last range accumulated.
        current_drawing_context.record_draw_accumulated_instances(&mut render_pass);

        // Step 3: End the profiling queries and close the pass.
        if let (true, Some(profiler)) = (statistics_query_begun, profiler) {
            profiler.end_pipeline_statistics_query(&mut render_pass);
        }
        if let (true, Some(profiler)) = (occlusion_query_begun, profiler) {
            profiler.end_occlusion_query(&mut render_pass);
        }

        // End the pass here: the borrow of the encoder must finish before the
        // caller can finish the encoder.
        drop(render_pass);
    }
}
