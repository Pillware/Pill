use anyhow::Result;
use wgpu::util::DeviceExt;

// Layout must match the GLSL `engine` uniform block (std140):
//   float delta_time;      // offset 0
//   float fog_density;     // offset 4
//   // vec3 must align to 16 → 8 bytes of padding here
//   vec3  fog_color;       // offset 16
//   // struct size rounded up to 16 → 4 bytes tail padding → 32 bytes total
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EngineParametersData {
    pub delta_time: f32,
    pub fog_density: f32,
    pub _pad0: [f32; 2],
    pub fog_color: [f32; 3],
    pub _pad1: f32,
}

impl Default for EngineParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineParametersData {
    pub fn new() -> Self {
        Self {
            delta_time: 0.0,
            fog_density: 0.0,
            _pad0: [0.0; 2],
            fog_color: [0.0; 3],
            _pad1: 0.0,
        }
    }

    pub fn update_data(&mut self, delta_time: f32, fog_density: f32, fog_color: [f32; 3]) {
        self.delta_time = delta_time;
        self.fog_density = fog_density;
        self.fog_color = fog_color;
    }
}

// --- Camera ---

#[derive(Debug)]
pub struct EngineParameters {
    pub parameters_data: EngineParametersData,
    pub parameters_uniform_buffer: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl EngineParameters {
    pub fn new(device: &wgpu::Device) -> Result<Self> {
        let parameters_data = EngineParametersData::new();

        let parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("engine_parameters_buffer"),
                contents: bytemuck::cast_slice(&[parameters_data]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        // Define engine bind group layout
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("engine_parameters_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, // (set = X, binding = 0)
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0, // (set = X, binding = 0)
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
            label: Some("engine_parameters_bind_group"),
        });

        let camera = Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout,
            bind_group,
        };

        Ok(camera)
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        delta_time: f32,
        fog_density: f32,
        fog_color: [f32; 3],
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
