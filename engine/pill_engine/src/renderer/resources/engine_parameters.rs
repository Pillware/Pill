use pill_core::{Result, Vector3f};
use wgpu::util::DeviceExt;

// Layout must match the WGSL `EngineParams_std140_0` compiled from `include/common.hlsl`:
//   float delta_time;      // offset 0  (4 bytes, @align(16) so 12 bytes padding follow)
//   float fog_density;     // offset 4  (4 bytes, @align(4), 8 bytes padding to reach @align(16) vec3)
//   [padding]              // offset 8  (8 bytes)
//   vec3  fog_color;       // offset 16 (12 bytes, @align(16))
//   [padding]              // offset 28 (4 bytes, struct size rounded to 16)
//   // total: 32 bytes
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EngineParametersData {
    pub delta_time: f32,
    pub fog_density: f32,
    _pad0: [f32; 2],
    pub fog_color: Vector3f,
    _pad1: f32,
}

impl Default for EngineParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineParametersData {
    /// Returns a zeroed instance; all fields are written before the first GPU upload.
    pub fn new() -> Self {
        Self {
            delta_time: 0.0,
            fog_density: 0.0,
            _pad0: [0.0; 2],
            fog_color: Vector3f::ZERO,
            _pad1: 0.0,
        }
    }

    /// Writes the latest per-frame values into the CPU-side struct before the GPU upload.
    pub fn update_data(&mut self, delta_time: f32, fog_density: f32, fog_color: Vector3f) {
        self.delta_time = delta_time;
        self.fog_density = fog_density;
        self.fog_color = fog_color;
    }
}

#[derive(Debug)]
pub struct EngineParameters {
    pub parameters_data: EngineParametersData,
    pub parameters_uniform_buffer: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl EngineParameters {
    /// Allocates the GPU uniform buffer and bind group; called once during renderer init.
    pub fn new(device: &wgpu::Device) -> Result<Self> {
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
                binding: 0,
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

        Ok(Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout,
            bind_group,
        })
    }

    /// Updates the CPU-side data and writes it to the GPU buffer; called once per frame before draw.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        delta_time: f32,
        fog_density: f32,
        fog_color: Vector3f,
    ) {
        self.parameters_data
            .update_data(delta_time, fog_density, fog_color);
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}
