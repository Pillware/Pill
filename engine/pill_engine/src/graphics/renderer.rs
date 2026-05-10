#![allow(clippy::too_many_arguments)]
#[cfg(feature = "ui")]
use crate::ecs::EguiClient;
use crate::{
    app_config::EngineConfig,
    ecs::{
        Component, ComponentStorage, EntityHandle, GlobalComponent, GlobalComponentStorage, Scene,
    },
    resources::{ResourceManager, ShaderParameterSlot, ShaderTextureSlot, TextureType},
};

use pill_core::{PillTypeMap, Timer};

use pill_core::Result;
use std::{collections::HashMap, sync::Arc};

// --- Renderer resource handles ---

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMaterialHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMeshHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererCameraHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererTextureHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererShaderHandle;
}

// --- Pass API types ---

/// Read-only view of the world handed to each pass. A pass calls `query::<T>()` for whatever
/// component types it needs and builds its OWN draw list (opaque, translucent, shadow, …).
/// The renderer stays domain-agnostic — it never names concrete component types.
pub struct WorldQuery<'a> {
    pub active_camera: EntityHandle,
    pub delta_time: f32,
    pub resources: &'a ResourceManager,
    scene: &'a Scene,
    globals: &'a PillTypeMap,
}

impl<'a> WorldQuery<'a> {
    pub fn new(
        active_camera: EntityHandle,
        scene: &'a Scene,
        globals: &'a PillTypeMap,
        delta_time: f32,
        resources: &'a ResourceManager,
    ) -> Self {
        Self {
            active_camera,
            delta_time,
            resources,
            scene,
            globals,
        }
    }

    /// Borrow the storage for per-entity component type `T` from the active scene.
    pub fn query<T>(&self) -> Result<&'a ComponentStorage<T>>
    where
        T: Component<Storage = ComponentStorage<T>>,
    {
        self.scene.get_component_storage::<T>()
    }

    /// Borrow a global component `T` (e.g. RenderStateComponent) from the world.
    pub fn get_global<T>(&self) -> Result<&'a T>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        crate::ecs::get_global_component_from::<T>(self.globals)
    }
}

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
    pub vertex_buffers: &'a [wgpu::VertexBufferLayout<'a>],
    pub bind_groups: Vec<Vec<wgpu::BindGroupLayoutEntry>>,
    pub targets: &'a [Option<wgpu::ColorTargetState>],
    pub depth_stencil: Option<wgpu::DepthStencilState>,
    pub multisample: wgpu::MultisampleState,
    pub primitive: wgpu::PrimitiveState,
}

pub struct RendererTargetDesc {
    pub name: String,
    pub format: wgpu::TextureFormat,
    pub width: u32,
    pub height: u32,
}

pub struct PipelineV2 {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
}

// --- Pass trait ---

pub trait Pass {
    /// Returns a short human-readable identifier used in profiling labels.
    fn get_label(&self) -> &str;
    /// Allocates all GPU resources the pass needs; called once before the first frame.
    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()>;
    /// Records this pass's GPU commands into `encoder` for the current frame.
    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        world: &WorldQuery<'_>,
    ) -> Result<()>;
}

// --- Renderer trait definition ---

pub trait PillRenderer {
    /// Creates the renderer synchronously; panics on WASM where async init is required.
    fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self>
    where
        Self: Sized;

    // --- Create ---

    /// Compiles vertex and fragment WGSL into a `RendererShader` with slot and bind-group metadata.
    fn create_shader_struct(
        &mut self,
        name: &str,
        vertex_wgsl: &str,
        fragment_wgsl: &str,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        parameter_slots: &[(String, ShaderParameterSlot)],
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<crate::renderer::resources::RendererShader>;

    /// Allocates a GPU camera uniform buffer and returns a handle to it.
    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    // --- Destroy ---

    /// Releases the GPU resources associated with the given camera handle.
    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;

    // --- Other ---

    /// Reconfigures the swap chain and depth texture after a window resize.
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    /// Returns the OS window the renderer is bound to.
    fn get_window(&self) -> std::sync::Arc<winit::window::Window>;

    /// Drives the full frame: acquires the surface, runs all passes, and presents.
    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        scene: &Scene,
        globals: &PillTypeMap,
        delta_time: f32,
        timer: &mut Timer,
        resource_manager: &ResourceManager,
    ) -> Result<()>;

    // --- Pass API ---

    /// Replaces the current pass chain; calls `Pass::init` on each new pass before storing it.
    fn set_passes(&mut self, passes: Vec<Box<dyn Pass>>) -> Result<()>;

    /// Returns the current surface dimensions `(width, height)` in pixels.
    fn get_surface_size(&self) -> (u32, u32);

    /// Returns the wgpu `Device`; required by passes that allocate their own GPU resources.
    fn get_device(&self) -> &wgpu::Device;

    /// Returns the wgpu `Queue`; required by passes that upload data each frame.
    fn get_queue(&self) -> &wgpu::Queue;

    /// Returns the surface texture format; used when creating pass render pipelines.
    fn get_surface_format(&self) -> wgpu::TextureFormat;

    /// Returns the shared engine-parameters UBO; required by passes that use legacy RendererShader pipelines.
    fn get_engine_parameters(&self) -> &crate::renderer::resources::EngineParameters;

    /// Returns a clone of the camera bind-group layout; required to create a RendererCamera in pass init.
    fn get_camera_bind_group_layout(&self) -> wgpu::BindGroupLayout;

    /// Allocates a GPU buffer with the given descriptor.
    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer>;

    /// Compiles a render pipeline from WGSL sources and bind group layout descriptors.
    fn create_pipeline_v2(&mut self, desc: PipelineV2Desc) -> Result<PipelineV2>;

    /// Creates an off-screen render target texture and returns its handle.
    fn create_render_target(&mut self, desc: RendererTargetDesc) -> Result<RendererTextureHandle>;

    /// Creates a depth texture sized to the current surface and returns its handle.
    fn create_depth_texture(&mut self, label: &str) -> Result<RendererTextureHandle>;

    /// Returns the texture view for a previously created render target, or `None` if not found.
    fn get_render_target_view(&self, handle: RendererTextureHandle) -> Option<&wgpu::TextureView>;

    /// Uploads pixel data as a GPU texture and returns a handle stored in the renderer.
    /// `mip_pixels[i]` is the raw byte slice for mip level `i`.
    fn create_texture_from_pixels(
        &mut self,
        name: &str,
        mip_pixels: &[&[u8]],
        base_width: u32,
        base_height: u32,
        format: wgpu::TextureFormat,
    ) -> RendererTextureHandle;

    /// Creates a new `TextureView` for a previously registered texture handle.
    /// Returns `None` if the handle is not found.
    fn get_texture_view(&self, handle: RendererTextureHandle) -> Option<wgpu::TextureView>;
}

pub type Renderer = Box<dyn PillRenderer>;
