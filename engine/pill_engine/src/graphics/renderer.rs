use crate::{
    ecs::{CameraComponent, ComponentStorage, EntityHandle, TransformComponent},
    engine::Engine,
    graphics::{RenderQuery, RenderQueueFactory, RenderQueueItem},
    resources::{MeshData, MeshHandle, TextureHandle, TextureType},
};

use pill_core::PillStyle;
use pill_core::{
    Handle, RendererBufferTag, RendererCameraTag, RendererMaterialTag, RendererMeshTag,
    RendererPipelineTag, RendererPipelineV2Tag, RendererTextureTag, Timer,
};

use anyhow::{Context, Error, Result};
use std::{path::PathBuf, sync::Arc};
use thiserror::Error;

// --- Renderer resource handles (typed generational handles) ---
pub type RendererMeshHandle = Handle<RendererMeshTag>;

pub type RendererPipelineHandle = Handle<RendererPipelineTag>;

pub type RendererMaterialHandle = Handle<RendererMaterialTag>;

pub type RendererCameraHandle = Handle<RendererCameraTag>;

pub type RendererTextureHandle = Handle<RendererTextureTag>;

pub type RendererBufferHandle = Handle<RendererBufferTag>;

pub type RendererPipelineV2Handle = Handle<RendererPipelineV2Tag>;

// --- Descriptors ---

// Minimal buffer creation helper mirroring the planned ResourceManager API
#[derive(Clone, Copy)]
pub struct BufferDesc<'a> {
    pub label: Option<&'a str>,
    pub byte_size: u64,
    pub usage: wgpu::BufferUsages,
}

#[derive(Clone, Copy, Debug)]
pub struct ShaderDesc<'a> {
    pub source: &'a str,
    pub entry_func: &'a str,
}

#[derive(Clone, Debug)]
pub struct PipelineV2Desc<'a> {
    pub label: Option<&'a str>,
    pub vs: ShaderDesc<'a>,
    pub ps: ShaderDesc<'a>,
    pub bind_groups: Vec<Vec<wgpu::BindGroupLayoutEntry>>,
    pub targets: &'a [Option<wgpu::ColorTargetState>],
    pub depth_stencil: Option<wgpu::DepthStencilState>,
    pub multisample: wgpu::MultisampleState,
}

pub struct MaterialDesc<'a> {
    pub label: &'a str,
    // Factors
    pub albedo: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    // Textures (optional; renderer will fallback to defaults if None)
    pub albedo_tex: Option<RendererTextureHandle>,
    pub normal_tex: Option<RendererTextureHandle>,
    pub metallic_roughness_tex: Option<RendererTextureHandle>,
    pub emissive_tex: Option<RendererTextureHandle>,
}

pub struct RendererTargetDesc {
    pub name: String,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

// --- Renderer trait definition ---

// RendererTexture is renderer-internal; engine code uses handles only.

pub struct PipelineV2 {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
}

// --- Shared pass API (engine-visible) -------------------------------------------------
pub trait Pass {
    fn get_label(&self) -> &str;
    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()>;
    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        world: &WorldQuery,
    ) -> Result<()>;
}

// Alias engine render query for passes
pub type WorldQuery<'a> = RenderQuery<'a>;

pub trait PillRenderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self
    where
        Self: Sized;

    fn init(&mut self) -> Result<()>;
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    // Creates a 256B-aligned uniform buffer (COPY_DST) and returns its handle
    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer>;
    fn create_pipeline_v2(&mut self, desc: PipelineV2Desc) -> Result<PipelineV2>;
    fn create_material(&mut self, desc: MaterialDesc) -> Result<RendererMaterialHandle>;
    fn update_material(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        desc: MaterialDesc,
    ) -> Result<RendererMaterialHandle>;
    fn create_render_target(&mut self, desc: RendererTargetDesc) -> Result<RendererTextureHandle>;
    fn create_depth_texture(&mut self, label: &str) -> Result<RendererTextureHandle>;
    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle>;
    fn create_texture(
        &mut self,
        name: &str,
        image_data: &image::DynamicImage,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle>;
    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()>;
    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()>;
    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()>;

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &Vec<RenderQueueItem>,
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn Fn(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<()>;

    // --- Engine pass management and helpers ---
    fn set_passes(&mut self, passes: Vec<Box<dyn Pass>>) -> Result<()>;
    fn get_surface_format(&self) -> wgpu::TextureFormat;
    fn get_device(&self) -> &wgpu::Device;
    fn get_queue(&self) -> &wgpu::Queue;
    fn get_texture(&self, h: RendererTextureHandle) -> &wgpu::Texture;
    fn get_mesh_buffers_and_count(
        &self,
        h: RendererMeshHandle,
    ) -> (&wgpu::Buffer, &wgpu::Buffer, u32);
    fn get_material_texture_bind_group(&self, h: RendererMaterialHandle) -> &wgpu::BindGroup;
    fn get_material_params_bind_group(&self, h: RendererMaterialHandle) -> &wgpu::BindGroup;
}

pub type Renderer = Box<dyn PillRenderer>;

// --- Zero-cost factory helper --------------------------------------------------

/// Renders using a zero-cost factory that provides borrowed references.
/// Keeps the PillRenderer trait object-safe while allowing call sites to be generic and inlined.
#[inline(always)]
pub fn render_with_factory<R, F>(
    renderer: &mut R,
    factory: &F,
    egui_ui: Box<dyn Fn(&egui::Context)>,
    timer: &mut Timer,
) -> Result<()>
where
    R: PillRenderer + ?Sized,
    F: RenderQueueFactory,
{
    let q = factory.get();
    renderer.render(
        q.active_camera,
        q.render_queue,
        q.camera_components,
        q.transform_components,
        egui_ui,
        timer,
    )
}

// WorldView raw-pointer based view to avoid borrow conflicts at call site
pub struct WorldView {
    pub active_camera: crate::ecs::EntityHandle,
    pub render_queue_ptr: *const Vec<RenderQueueItem>,
    pub camera_components_ptr: *const ComponentStorage<CameraComponent>,
    pub transform_components_ptr: *const ComponentStorage<TransformComponent>,
}

pub struct WorldViewFactory {
    pub world: WorldView,
}

impl RenderQueueFactory for WorldViewFactory {
    fn get<'b>(&'b self) -> RenderQuery<'b> {
        unsafe {
            RenderQuery {
                active_camera: self.world.active_camera,
                render_queue: &*self.world.render_queue_ptr,
                camera_components: &*self.world.camera_components_ptr,
                transform_components: &*self.world.transform_components_ptr,
            }
        }
    }
}
