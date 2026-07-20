use pill_core::Result;
use wgpu::util::DeviceExt;

// Layout must match the HLSL `EngineParams` in `include/common.hlsl` (std140):
//   float3 fog_color;      // offset 0,  size 12, but std140 aligns vec3 to 16
//   float  fog_density;    // offset 16, size 4
//   float3 light_position; // offset 32, size 12 → 16 (std140 vec3)
//   float3 light_color;    // offset 48, size 12 → 16
//   float  light_intensity;// offset 64, size 4
//   uint   shadow_cull_mask;// offset 68, size 4
//   // total: 80 bytes (padded to 80, multiple of 16)
#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EngineParametersData {
    pub fog_color: [f32; 3],
    _pad0: f32, // std140: vec3<f32> is 16-byte aligned
    pub fog_density: f32,
    _pad1: [f32; 3], // pad to 32 bytes
    // Light data for RT shadows (offset 32)
    pub light_position: [f32; 3],
    _pad2: f32,
    pub light_color: [f32; 3],
    _pad3: f32,
    pub light_intensity: f32,
    pub shadow_cull_mask: u32,
    _pad4: [f32; 2], // pad to 80 bytes (multiple of 16)
}

impl Default for EngineParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineParametersData {
    pub fn new() -> Self {
        Self {
            fog_color: [0.0; 3],
            _pad0: 0.0,
            fog_density: 0.0,
            _pad1: [0.0; 3],
            light_position: [0.0, 10.0, 15.0], // default: behind objects, shining toward camera
            _pad2: 0.0,
            light_color: [1.0, 1.0, 1.0],
            _pad3: 0.0,
            light_intensity: 5.0,
            shadow_cull_mask: 0xff,
            _pad4: [0.0; 2],
        }
    }

    pub fn update_data(&mut self, fog_density: f32, fog_color: [f32; 3]) {
        self.fog_density = fog_density;
        self.fog_color = fog_color;
    }

    pub fn update_light(
        &mut self,
        position: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        cull_mask: u32,
    ) {
        self.light_position = position;
        self.light_color = color;
        self.light_intensity = intensity;
        self.shadow_cull_mask = cull_mask;
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

    pub fn update(&mut self, queue: &wgpu::Queue, fog_density: f32, fog_color: [f32; 3]) {
        self.parameters_data.update_data(fog_density, fog_color);
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}
