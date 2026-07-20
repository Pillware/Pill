#![allow(clippy::too_many_arguments)]
use crate::{
    app_config::EngineConfig,
    ecs::{
        CameraComponent, ComponentStorage, EntityHandle, MeshRenderingComponent, TransformComponent,
    },
    graphics::RenderQueueItem,
    internal::{MaterialParameter, MaterialTexture, MeshData},
    resources::{ShaderParameterSlot, ShaderTextureSlot, TextureType},
};

use pill_core::{Matrix3fA, Matrix4f, Timer, Vector3f};

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

// --- Renderer backend ---

/// Identifies the GPU backend in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RendererBackend {
    Vulkan,
    Dx12,
    Metal,
    Gl,
    BrowserWebGpu,
    Headless,
    Unknown,
}

impl RendererBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Vulkan => "Vulkan",
            Self::Dx12 => "Dx12",
            Self::Metal => "Metal",
            Self::Gl => "OpenGL",
            Self::BrowserWebGpu => "WebGPU",
            Self::Headless => "Headless",
            Self::Unknown => "Unknown",
        }
    }
}

// --- Renderer capabilities ---

/// Backend-neutral capability report produced after device creation.
/// Reports what was actually enabled, not merely what the adapter advertised.
#[derive(Debug, Clone)]
pub struct RendererCapabilities {
    pub backend: RendererBackend,
    pub adapter_name: String,
    pub hardware_ray_query: Option<HardwareRayQueryCapabilities>,
}

/// Limits and enabled-feature report for hardware inline ray queries.
/// All fields reflect the created device, not the adapter maximums.
#[derive(Debug, Clone, Copy)]
pub struct HardwareRayQueryCapabilities {
    pub max_blas_primitive_count: u32,
    pub max_blas_geometry_count: u32,
    pub max_tlas_instance_count: u32,
    pub max_acceleration_structures_per_shader_stage: u32,
    pub max_buffers_and_acceleration_structures_per_shader_stage: u32,
}

// --- Ray visibility (per-object policy) ---

/// Backend-neutral per-instance ray-tracing participation description.
/// Attached to `MeshRenderingComponent`; does not import wgpu types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayVisibility {
    /// Whether the instance may participate in renderer ray queries at all.
    pub ray_visible: bool,
    /// Whether this instance casts shadows (V1: controls TLAS inclusion).
    pub casts_shadow: bool,
    /// 8-bit instance visibility mask for ray culling.
    pub mask: u8,
    /// How the renderer should resolve geometry opacity for ray traversal.
    pub opacity: RayOpacityMode,
}

impl Default for RayVisibility {
    fn default() -> Self {
        Self {
            ray_visible: true,
            casts_shadow: true,
            mask: 0xff,
            opacity: RayOpacityMode::Auto,
        }
    }
}

/// Determines whether geometry is treated as opaque for ray traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RayOpacityMode {
    /// Opaque only when the built-in shader/resource class is registered as
    /// opaque; custom/transparent materials are excluded.
    Auto,
    /// Author guarantee: alpha/discard behaviour may be ignored; geometry is
    /// marked OPAQUE for acceleration-structure purposes.
    ForceOpaque,
    /// Never participates in the V1 TLAS.
    Exclude,
}

// --- Frame boundary types ---

/// Self-sufficient frame description sent from the engine to the renderer.
/// The renderer must not reach back into ECS storage.
#[derive(Debug, Clone)]
pub struct RenderFrame<'a> {
    pub camera: RenderCamera,
    pub instances: &'a [RenderInstance],
    pub lights: &'a [RenderLight],
    pub delta_time: f32,
}

/// Camera data extracted once per frame for the renderer.
#[derive(Debug, Clone, Copy)]
pub struct RenderCamera {
    pub entity: EntityHandle,
    pub renderer_handle: RendererCameraHandle,
    pub world_position: Vector3f,
    pub view: Matrix4f,
    pub projection: Matrix4f,
    pub view_projection: Matrix4f,
    pub inverse_view: Matrix4f,
    pub inverse_projection: Matrix4f,
    pub clear_color: Vector3f,
    pub fog_density: f32,
    pub fog_color: Vector3f,
}

/// Per-instance data extracted once per frame.
#[derive(Debug, Clone, Copy)]
pub struct RenderInstance {
    pub entity: EntityHandle,
    pub mesh: RendererMeshHandle,
    pub material: RendererMaterialHandle,
    pub shader: RendererShaderHandle,
    /// Retained for raster draw-ordering; TLAS metadata must not be packed
    /// into this key.
    pub raster_sort_key: u64,
    pub model: Matrix4f,
    pub normal: Matrix3fA,
    pub ray_visibility: RayVisibility,
}

/// Light data extracted once per frame.
#[derive(Debug, Clone, Copy)]
pub struct RenderLight {
    pub position: Vector3f,
    pub color: Vector3f,
    pub intensity: f32,
    /// 8-bit cull mask ANDed with `TlasInstance::mask` during shadow queries.
    pub shadow_cull_mask: u8,
}

impl Default for RenderLight {
    fn default() -> Self {
        Self {
            position: Vector3f::new(0.0, 5.0, 0.0),
            color: Vector3f::new(1.0, 1.0, 1.0),
            intensity: 1.0,
            shadow_cull_mask: 0xff,
        }
    }
}

// --- Renderer trait definition ---

pub trait PillRenderer {
    fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self>
    where
        Self: Sized;

    // --- Create ---

    fn create_shader(
        &mut self,
        name: &str,
        vertex_wgsl: &str,
        fragment_wgsl: &str,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        parameter_slots: &[(String, ShaderParameterSlot)],
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<RendererShaderHandle>;

    fn create_material(
        &mut self,
        name: &str,
        renderer_shader_handle: RendererShaderHandle,
        textures: &[(String, MaterialTexture)],
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<RendererMaterialHandle>;

    fn create_texture(
        &mut self,
        name: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle>;

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle>;

    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    // --- Update ---

    fn update_material_textures(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        textures: &[(String, MaterialTexture)],
    ) -> Result<()>;

    fn update_material_parameters(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<()>;

    // --- Destroy ---

    fn destroy_shader(&mut self, renderer_shader_handle: RendererShaderHandle) -> Result<()>;

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()>;

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()>;

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()>;

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;

    // --- Other ---

    /// Returns backend-neutral capabilities of the created device.
    /// `DummyRenderer` reports headless and no ray-query capability.
    fn capabilities(&self) -> &RendererCapabilities;

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    #[cfg(feature = "debug_ui")]
    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()>;

    #[cfg(feature = "debug_ui")]
    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        mesh_rendering_component_storage: &ComponentStorage<MeshRenderingComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        delta_time: f32,
        timer: &mut Timer,
    ) -> Result<()>;

    #[cfg(not(feature = "debug_ui"))]
    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        mesh_rendering_component_storage: &ComponentStorage<MeshRenderingComponent>,
        delta_time: f32,
        timer: &mut Timer,
    ) -> Result<()>;
}

pub type Renderer = Box<dyn PillRenderer>;
