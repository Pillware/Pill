//! The engine-wide uniform block a pass may read.
//!
//! # Responsibilities
//!
//! - Define the layout the GPU reads ([`EngineParametersData`]) and hold the
//!   CPU-side copy the renderer updates every frame, so both carry the same
//!   values.
//! - Own the uniform buffer that carries those values, together with its bind
//!   group layout and bind group ([`EngineParameters`]).
//!
//! # Design
//!
//! One instance lives on the renderer and is refilled once for each surface
//! frame: [`EngineParameters::update`] rewrites the whole buffer, so the CPU
//! copy and the GPU buffer always match and no code has to track which fields
//! changed. The fog values come first because the HLSL `EngineParams` in
//! `pill_master_renderer_data/shader_sources/common_shaders/include/common.hlsl`
//! declares them at the same offsets. The frame size and time follow; the
//! engine adds them, and no shipped shader declares them.

// External crates
use wgpu::util::DeviceExt;

/// The engine-wide uniform block a pass may read, in its GPU layout.
///
/// Fog, the frame size and the frame time describe the frame, not one pass.
/// Every pass reads the same buffer, so no two passes can hold copies that
/// disagree.
///
/// The first 16 bytes must match the HLSL `EngineParams` in
/// `pill_master_renderer_data/shader_sources/common_shaders/include/common.hlsl`,
/// which declares the fog pair and nothing after it. The rest is the engine's
/// own addition, and no shipped shader declares it: `frame_size` at offset 16
/// (xy = pixels, zw = their reciprocal) and `time` at offset 32 (x = seconds,
/// y = delta, z = frame). The struct is 48 bytes in total.
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EngineParametersData {
    /// Colour the lit shaders blend toward with distance.
    pub fog_color: [f32; 3],
    /// Fog thickness: how quickly the colour builds up with distance.
    pub fog_density: f32,
    /// The surface size in pixels, followed by the reciprocal of each
    /// dimension.
    ///
    /// It exists for a pass that samples the frame at a texel offset; no
    /// shipped shader reads it. The value comes from the buffer rather than a
    /// pass parameter, so a resize cannot leave it stale.
    pub frame_size: [f32; 4],
    /// x = seconds since the frame loop started, y = this frame's duration,
    /// z = the frame's sequence number. w is unused.
    pub time: [f32; 4],
}

impl Default for EngineParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineParametersData {
    /// Creates an all-zero set of parameters.
    ///
    /// The uniform buffer is created from these values, so every shader reads
    /// zeros until the first frame's `update_data` call replaces them.
    pub fn new() -> Self {
        Self {
            fog_color: [0.0; 3],
            fog_density: 0.0,
            frame_size: [0.0; 4],
            time: [0.0; 4],
        }
    }

    /// Writes one frame's values into the buffer data, converting the surface
    /// size on the way in.
    ///
    /// The size comes in as pixel dimensions and is stored as pixels plus
    /// their reciprocal. That is the form the layout declares and the form a
    /// shader sampling at a texel offset wants. Each dimension is clamped to
    /// at least one pixel before the division, so a minimized or
    /// not-yet-sized surface cannot put an infinity into the buffer.
    pub fn update_data(
        &mut self,
        fog_density: f32,
        fog_color: [f32; 3],
        frame_size: [u32; 2],
        time: [f32; 4],
    ) {
        self.fog_density = fog_density;
        self.fog_color = fog_color;
        self.frame_size = [
            frame_size[0] as f32,
            frame_size[1] as f32,
            1.0 / frame_size[0].max(1) as f32,
            1.0 / frame_size[1].max(1) as f32,
        ];
        self.time = time;
    }
}

// =============================================================================
// Engine Parameters
// =============================================================================

/// The engine parameters as the renderer owns them: the values, the buffer
/// they are uploaded to, and the bindings that expose them.
///
/// Built once when the renderer starts and refilled every frame through
/// [`update`](EngineParameters::update). Every pass that reads engine
/// parameters binds this one group, so all of them see the same fog and frame
/// state.
#[derive(Debug)]
pub struct EngineParameters {
    /// The values currently held in the buffer, kept CPU-side so `update` can
    /// rewrite the buffer.
    pub parameters_data: EngineParametersData,
    /// Uniform buffer holding one [`EngineParametersData`], bound at binding 0
    /// of the group below.
    pub parameters_uniform_buffer: wgpu::Buffer,
    /// Layout a pipeline declares for the engine group, so the one bind group
    /// below works for every pass that reads these values.
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// The group binding the buffer, built from the layout above.
    pub bind_group: wgpu::BindGroup,
}

impl EngineParameters {
    /// Creates the uniform buffer, its layout and its bind group, all holding
    /// the engine parameters at their zero values.
    ///
    /// A pipeline that reads engine parameters must be built against
    /// [`bind_group_layout`](EngineParameters::bind_group_layout), which is
    /// what makes this one group bindable by every pass.
    pub fn new(device: &wgpu::Device) -> Self {
        let parameters_data = EngineParametersData::new();

        let parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("engine_parameters_buffer"),
                contents: bytemuck::cast_slice(&[parameters_data]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("engine_parameters_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, // (set = 0, binding = 0)
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
            label: Some("engine_parameters_bind_group"),
        });

        Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout,
            bind_group,
        }
    }

    /// Refills the buffer with one frame's values.
    ///
    /// The surface size comes in as pixels, and `update_data` stores it in the
    /// form the shaders read. The write is queued before any command buffer
    /// submitted after it, so every pass recorded later in the frame sees the
    /// new values.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        fog_density: f32,
        fog_color: [f32; 3],
        frame_size: [u32; 2],
        time: [f32; 4],
    ) {
        self.parameters_data
            .update_data(fog_density, fog_color, frame_size, time);
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}
