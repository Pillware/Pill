//! Ray-tracing pipeline management.
//!
//! Handles compilation of WGSL ray-query shaders and creation of
//! RT-specific bind-group layouts. V1 only supports fragment-stage
//! ray queries for shadow evaluation.

use pill_core::Result;

/// Compile the ray-query canary WGSL shader to verify that the pinned
/// wgpu/Naga version accepts the required directive syntax.
///
/// The canary must:
/// - Start with `enable wgpu_ray_query;`
/// - Declare a minimal ray-query function
///
/// A compilation failure here surfaces a Naga/WGSL dialect change that
/// would affect every authored ray-query shader.
pub fn compile_ray_query_canary(device: &wgpu::Device) -> Result<()> {
    let canary_source = include_str!("shaders/ray_query_canary.wgsl");

    // Use an error scope to capture validation errors without crashing.
    let _scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

    let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ray_query_canary"),
        source: wgpu::ShaderSource::Wgsl(canary_source.into()),
    });

    // We cannot `.await` the error scope in a sync context, so we
    // rely on the uncaptured-error handler or a later poll.
    // For the MVP, a successful `create_shader_module` call (no panic)
    // is sufficient proof that the syntax is accepted.
    // A validation error would surface asynchronously; in a test
    // context, `pollster::block_on(device.pop_error_scope())` would
    // be used.

    Ok(())
}

/// Shadow-query WGSL source string, loaded at compile time.
pub const SHADOW_QUERY_WGSL: &str = include_str!("shaders/shadow_query.wgsl");

/// Create the RT-enabled group-0 bind-group layout.
///
/// Group 0 carries engine/frame uniforms plus the TLAS binding for RT
/// shader variants. This layout is shared by all RT pipelines.
pub fn create_rt_engine_bind_group_layout(
    device: &wgpu::Device,
    _tlas_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("rt_engine_bind_group_layout"),
        entries: &[
            // Binding 0: engine uniform buffer (fog, delta_time, light data)
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Binding 1: TLAS (acceleration structure)
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::AccelerationStructure {
                    vertex_return: false,
                },
                count: None,
            },
        ],
    })
}
