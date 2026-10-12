#![allow(clippy::too_many_arguments)]
#[cfg(feature = "ui")]
use crate::ecs::EguiClient;
use crate::{
    app_config::EngineConfig,
    ecs::{
        CameraComponent, Component, ComponentStorage, EntityHandle, GlobalComponent,
        GlobalComponentStorage, Scene, TransformComponent,
    },
    renderer::resources::{RendererMesh, RendererTexture},
    resources::{Mesh, MeshHandle, ResourceManager, Texture, TextureHandle, TextureType},
};

use pill_core::{EngineError, Matrix4f, PillSlotMapKey, PillTypeMap, Timer, Vector3f};

use pill_core::Result;
use std::sync::Arc;

// --- Renderer resource handles ---

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMaterialHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMeshHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererTextureHandle;
}

// Renderer-owned textures (targets, environment, IBL); a separate key type from
// RendererTextureHandle (material textures) so the two cannot be mixed up.
pill_core::define_new_pill_slotmap_key! {
    pub struct PassTextureHandle;
}

// --- Pass API types ---

/// Read-only world view handed to each pass; passes query the components they need and build
/// their own draw lists.
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

    /// Camera and transform components of the active camera entity.
    pub fn active_camera(&self) -> Result<(&'a CameraComponent, &'a TransformComponent)> {
        let index = self.active_camera.data().index as usize;
        let camera = self.query::<CameraComponent>()?.data.get(index);
        let transform = self.query::<TransformComponent>()?.data.get(index);
        match (camera, transform) {
            (Some(Some(camera)), Some(Some(transform))) => Ok((camera, transform)),
            _ => Err(EngineError::NoActiveCamera.into()),
        }
    }

    /// GPU buffers of a mesh resource.
    pub fn renderer_mesh(&self, mesh_handle: &MeshHandle) -> Result<&'a RendererMesh> {
        let mesh_name = &self.resources.get_resource::<Mesh>(mesh_handle)?.name;
        self.resources
            .get_resource_by_name::<RendererMesh>(mesh_name)
    }

    /// GPU texture (view + sampler) of a texture resource.
    pub fn renderer_texture(&self, texture_handle: &TextureHandle) -> Result<&'a RendererTexture> {
        let texture_name = &self.resources.get_resource::<Texture>(texture_handle)?.name;
        self.resources
            .get_resource_by_name::<RendererTexture>(texture_name)
    }

    /// Borrow a global component `T` (e.g. RenderStateComponent) from the world.
    pub fn get_global<T>(&self) -> Result<&'a T>
    where
        T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
    {
        crate::ecs::get_global_component_from::<T>(self.globals)
    }
}

/// Direction the camera looks at: the transform's forward (−Z), same as get_forward_direction.
pub fn camera_forward(transform: &TransformComponent) -> Vector3f {
    transform.get_forward_direction()
}

/// Right-handed perspective view-projection of a camera entity.
pub fn camera_view_projection(
    camera: &CameraComponent,
    transform: &TransformComponent,
) -> Matrix4f {
    let view = Matrix4f::look_to_rh(transform.position, camera_forward(transform), Vector3f::Y);
    let projection = Matrix4f::perspective_rh(
        camera.fov.to_radians(),
        camera.aspect.get_value(),
        camera.range.start,
        camera.range.end,
    );
    projection * view
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
pub struct PipelineDesc<'a> {
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

/// Pixel data of a texture to upload; `mips[i]` holds the raw bytes of mip level `i`
/// (`mips[0]` is `width` × `height`, each next level halves both, min 1).
#[derive(Clone, Copy)]
pub struct TextureData<'a> {
    pub width: u32,
    pub height: u32,
    pub format: wgpu::TextureFormat,
    pub mips: &'a [&'a [u8]],
}

impl TextureData<'_> {
    /// Checks dimensions, mip count and each mip's byte length before anything reaches wgpu.
    pub fn validate(&self) -> Result<()> {
        if self.width == 0 || self.height == 0 {
            return Err("Texture data has zero width or height".into());
        }
        let max_mip_count = 32 - self.width.max(self.height).leading_zeros();
        let mip_count = self.mips.len() as u32;
        if mip_count == 0 || mip_count > max_mip_count {
            return Err(format!(
                "Texture data has {mip_count} mips; {}x{} allows 1..={max_mip_count}",
                self.width, self.height
            )
            .into());
        }
        let bytes_per_texel = self
            .format
            .block_copy_size(None)
            .ok_or("Texture data format has no texel size")? as usize;
        for (mip, data) in self.mips.iter().enumerate() {
            let expected = (self.width >> mip).max(1) as usize
                * (self.height >> mip).max(1) as usize
                * bytes_per_texel;
            if data.len() != expected {
                return Err(format!(
                    "Texture data mip {mip} is {} bytes, expected {expected}",
                    data.len()
                )
                .into());
            }
        }
        Ok(())
    }
}

pub struct RendererTargetDesc {
    pub name: String,
    pub format: wgpu::TextureFormat,
    pub size: TargetSize,
}

pub enum TargetSize {
    /// Matches the surface and is rebuilt (same handle) when the window is resized.
    Surface,
    /// Fixed width and height.
    Fixed(u32, u32),
}

pub struct Pipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
}

/// Swapchain image of the current frame, as two views of the same texture.
pub struct FrameTarget {
    /// View in the surface format (what egui and other UI passes expect).
    pub view: wgpu::TextureView,
    /// sRGB view; the hardware encodes linear shader output (used by the tonemap pass).
    pub srgb_view: wgpu::TextureView,
}

// --- Pass trait ---

/// One step of the frame. Scene passes render into the HDR and depth targets held in
/// `RenderStateComponent`; the last scene pass (tonemap) resolves into the `FrameTarget`.
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
        frame_target: &FrameTarget,
        world: &WorldQuery<'_>,
    ) -> Result<()>;
}

// --- Renderer trait definition ---

pub trait PillRenderer {
    /// Creates the renderer synchronously; panics on WASM where async init is required.
    fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self>
    where
        Self: Sized;

    // --- Other ---

    /// Reconfigures the swap chain and rebuilds surface-sized render targets (handles stay valid).
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    /// Returns the OS window the renderer is bound to.
    fn get_window(&self) -> Arc<winit::window::Window>;

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

    /// Initializes `pass` and inserts it into the running chain before the tonemap pass.
    fn insert_pass(&mut self, pass: Box<dyn Pass>) -> Result<()>;

    /// Returns the current surface dimensions `(width, height)` in pixels.
    fn get_surface_size(&self) -> (u32, u32);

    /// Returns the wgpu `Device`; required by passes that allocate their own GPU resources.
    fn get_device(&self) -> &wgpu::Device;

    /// Returns the wgpu `Queue`; required by passes that upload data each frame.
    fn get_queue(&self) -> &wgpu::Queue;

    /// Returns the surface texture format; used when creating pass render pipelines.
    fn get_surface_format(&self) -> wgpu::TextureFormat;

    /// Allocates a GPU buffer with the given descriptor.
    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer>;

    /// Compiles a render pipeline from WGSL sources and bind group layout descriptors.
    fn create_pipeline(&mut self, desc: PipelineDesc) -> Result<Pipeline>;

    /// Creates an off-screen render target texture and returns its handle.
    fn create_render_target(&mut self, desc: RendererTargetDesc) -> Result<PassTextureHandle>;

    /// Creates a depth texture sized to the current surface and returns its handle.
    fn create_depth_texture(&mut self, label: &str) -> Result<PassTextureHandle>;

    /// Returns the texture view for a previously created render target, or `None` if not found.
    fn get_render_target_view(&self, handle: PassTextureHandle) -> Option<&wgpu::TextureView>;

    /// Uploads pixel data as a GPU texture and returns a handle stored in the renderer.
    fn create_texture_from_pixels(
        &mut self,
        name: &str,
        data: TextureData,
    ) -> Result<PassTextureHandle>;

    /// Creates a new `TextureView` for a previously registered texture handle.
    /// Returns `None` if the handle is not found.
    fn get_texture_view(&self, handle: PassTextureHandle) -> Option<wgpu::TextureView>;
}

pub type Renderer = Box<dyn PillRenderer>;

#[cfg(test)]
mod tests {
    use super::TextureData;

    fn data<'a>(width: u32, height: u32, mips: &'a [&'a [u8]]) -> TextureData<'a> {
        TextureData {
            width,
            height,
            format: wgpu::TextureFormat::Rgba8Unorm,
            mips,
        }
    }

    #[test]
    fn texture_data_2x2_with_2_mips_is_valid() {
        assert!(data(2, 2, &[&[0; 16], &[0; 4]]).validate().is_ok());
    }

    #[test]
    fn texture_data_without_mips_is_rejected() {
        assert!(data(2, 2, &[]).validate().is_err());
    }

    #[test]
    fn texture_data_with_too_many_mips_is_rejected() {
        assert!(data(1, 1, &[&[0; 4], &[0; 4]]).validate().is_err());
    }

    #[test]
    fn texture_data_with_short_mip_is_rejected() {
        assert!(data(2, 2, &[&[0; 15]]).validate().is_err());
    }

    #[test]
    fn texture_data_with_zero_width_is_rejected() {
        assert!(data(0, 2, &[&[]]).validate().is_err());
    }
}
