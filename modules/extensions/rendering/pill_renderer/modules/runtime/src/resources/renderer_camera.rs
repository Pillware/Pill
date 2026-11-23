//! The GPU resources that carry the camera values the shaders read.
//!
//! # Responsibilities
//!
//! - Own the uniform buffer and the bind group that every shader reading the
//!   camera binds ([`RendererCamera`]).
//! - Refill that buffer from the camera and transform components once per
//!   frame, and report an unusable projection input once per change.
//!
//! # Design
//!
//! The value and the GPU objects are separate: [`CameraParameters`] is the
//! `#[repr(C)]` layout the shaders read, built by `pill_rendering_core` so
//! every renderer fills the same one, and [`RendererCamera`] is the buffer and
//! the bind group that hold it. The buffer is updated in place, so moving the
//! camera costs one buffer write and no bind group rebuild.
//!
//! A replacement of an unusable projection input is reported once per change
//! rather than once per frame, so a value the game never fixes does not fill
//! the log.

// External crates
use pill_core::telemetry::telemetry_target;
use wgpu::util::DeviceExt;

// Current crate
use crate::components::{CameraComponent, TransformComponent};

// The camera values themselves, and the maths that fills them, are shared by
// every renderer; only the GPU objects below belong to this one.
pub use pill_rendering_core::camera::CameraParameters;

// =============================================================================
// RendererCameraHandle
// =============================================================================

pill_core::define_slot_key!(RendererCameraHandle);

// =============================================================================
// ShadowParametersData
// =============================================================================

/// The shadow values the lit shaders read, laid out as
/// `pbr_surface.hlsl`'s `ShadowParams` declares them: four 32-bit words, which
/// is also a uniform buffer's minimum size.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowParametersData {
    /// Whether the ray tracing pass ran for this surface.
    enabled: u32,
    /// How many pixels wide the mask is.
    mask_width: u32,
    /// How many pixels tall the mask is.
    mask_height: u32,
    /// Unused, so the structure fills a whole uniform slot.
    reserved: u32,
}

impl ShadowParametersData {
    /// The values of a surface whose shadows are not traced.
    fn off() -> Self {
        Self {
            enabled: 0,
            mask_width: 1,
            mask_height: 1,
            reserved: 0,
        }
    }
}

// =============================================================================
// RendererCamera
// =============================================================================

/// The GPU-side resources of one camera: its uniform values, the buffer those
/// values are written to, and the bind group that exposes the buffer to
/// shaders.
///
/// It is built once and then only updated in place. [`RendererCamera::update`]
/// rewrites the buffer's contents on every frame, so moving the camera costs
/// one buffer write and no bind group rebuild.
#[derive(Debug)]
pub struct RendererCamera {
    /// The values for the current frame. [`RendererCamera::update`] rebuilds
    /// them before it writes them to the buffer.
    pub parameters_data: CameraParameters,
    /// The uniform buffer that holds `parameters_data`.
    pub parameters_uniform_buffer: wgpu::Buffer,
    /// The layout the bind group was built from, kept beside it.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// The bind group every shader that reads the camera binds: at group 1 in
    /// the chain's pipelines and the mesh drawer, at group 0 in the overlay's.
    pub bind_group: wgpu::BindGroup,
    /// Whether the ray tracing shadow pass ran for this surface, and the mask's
    /// size, as the shaders read them.
    shadow_parameters_uniform_buffer: wgpu::Buffer,
    /// The target the mask binding points at: the mask size while the pass
    /// runs, and `None` while it does not.
    shadow_binding: Option<(u32, u32)>,
    /// One readable value, bound in the mask's place while the pass does not
    /// run, so a shader never reads a buffer that does not exist.
    shadow_stand_in: wgpu::Buffer,
    /// Whether the last update already found unusable projection values and
    /// warned about them. The flag keeps that warning to once per change
    /// instead of once per frame, and clears when the values become usable.
    projection_reported: bool,
}

impl RendererCamera {
    /// Creates a camera on `device`, with its bind group built from
    /// `camera_bind_group_layout`.
    ///
    /// `camera_bind_group_layout` is the layout the renderer creates at
    /// startup and hands to the camera and to every shader that reads the
    /// camera. Sharing it is what makes the bind group compatible with those
    /// pipelines.
    pub fn new(device: &wgpu::Device, camera_bind_group_layout: wgpu::BindGroupLayout) -> Self {
        let parameters_data = CameraParameters::new();

        let parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("camera_parameters_buffer"),
                contents: bytemuck::cast_slice(&[parameters_data]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        // The shadow parameters start with the pass off, so the lit shaders
        // shade exactly what they shaded before the path existed.
        let shadow_parameters = ShadowParametersData::off();
        let shadow_parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow_parameters_buffer"),
                contents: bytemuck::bytes_of(&shadow_parameters),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        // A stand-in the shader can always read: one value, fully lit.
        let shadow_stand_in = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("shadow_mask_stand_in"),
            contents: bytemuck::cast_slice(&[1.0_f32]),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let bind_group = Self::create_bind_group(
            device,
            &camera_bind_group_layout,
            &parameters_uniform_buffer,
            &shadow_parameters_uniform_buffer,
            &shadow_stand_in,
        );

        Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout: camera_bind_group_layout,
            bind_group,
            shadow_parameters_uniform_buffer,
            shadow_binding: None,
            shadow_stand_in,
            projection_reported: false,
        }
    }

    /// Points the shaders at `mask` for `size`, or back at the stand-in.
    ///
    /// `None` turns the pass off: the shaders read the stand-in and the
    /// `enabled` flag says not to. The bind group is rebuilt only when what it
    /// should point at changed, so a frame that draws the same surface again
    /// costs nothing.
    pub fn bind_shadow_mask(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mask: Option<&wgpu::Buffer>,
        size: (u32, u32),
    ) {
        let wanted = mask.map(|_| size);
        if wanted == self.shadow_binding {
            return;
        }
        let parameters = match mask {
            Some(_) => ShadowParametersData {
                enabled: 1,
                mask_width: size.0.max(1),
                mask_height: size.1.max(1),
                reserved: 0,
            },
            None => ShadowParametersData::off(),
        };
        queue.write_buffer(
            &self.shadow_parameters_uniform_buffer,
            0,
            bytemuck::bytes_of(&parameters),
        );
        let target = mask.unwrap_or(&self.shadow_stand_in);
        self.bind_group = Self::create_bind_group(
            device,
            &self.bind_group_layout,
            &self.parameters_uniform_buffer,
            &self.shadow_parameters_uniform_buffer,
            target,
        );
        self.shadow_binding = wanted;
    }

    /// Builds the bind group the lit shaders bind for their camera, their
    /// shadow parameters and their shadow mask.
    fn create_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        parameters: &wgpu::Buffer,
        shadow_parameters: &wgpu::Buffer,
        shadow_mask: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0, // (set = 1, binding = 0)
                    resource: parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1, // (set = 1, binding = 1)
                    resource: shadow_parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2, // (set = 1, binding = 2)
                    resource: shadow_mask.as_entire_binding(),
                },
            ],
            label: Some("camera_parameters_bind_group"),
        })
    }

    /// Uploads the camera's values for this frame to the uniform buffer.
    ///
    /// Called once for each frame the surface draws, with the drawing
    /// viewport's aspect ratio. An unusable projection value is reported when
    /// it first appears, not on every frame it persists, so a broken camera
    /// produces one warning instead of a stream of them.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
        aspect: f32,
    ) {
        // Step 1: Rebuild the values for this frame.
        let substitution_report =
            self.parameters_data
                .update(camera_component, transform_component, aspect);
        // Step 2: Warn once for each change, not once per frame.
        match substitution_report {
            Some(reason) if !self.projection_reported => {
                pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "camera projection has unusable values ({reason}); the renderer substitutes defaults"
                );
                self.projection_reported = true;
            }
            None => self.projection_reported = false,
            Some(_) => {}
        }
        // Step 3: Upload the values. This write is ordered before every
        // command buffer submitted after it, so this frame's passes read them.
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}
