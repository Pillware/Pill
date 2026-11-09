//! Shared plumbing of the full-screen post-processing passes: one bind group (group 0), a
//! full-screen triangle and one color target.

use crate::config::FULLSCREEN_VERTEX_SHADER;
use crate::graphics::{PassTextureHandle, PillRenderer, Pipeline, PipelineDesc, ShaderDesc};

use pill_core::Result;

/// Sampled 2D texture; `filterable: false` for depth and 32-bit float targets read with Load.
pub(crate) fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2,
            sample_type: wgpu::TextureSampleType::Float { filterable },
        },
        count: None,
    }
}

pub(crate) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

pub(crate) fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// Full-screen pipeline: `fragment_shader` (`fs_main`) with bind group 0 = `entries`, writing
/// one `format` target with `blend` (None = replace).
pub(crate) fn create_fullscreen_pipeline(
    renderer: &mut dyn PillRenderer,
    label: &str,
    fragment_shader: &str,
    entries: Vec<wgpu::BindGroupLayoutEntry>,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> Result<Pipeline> {
    renderer.create_pipeline(PipelineDesc {
        label: Some(label),
        vs: ShaderDesc {
            source: FULLSCREEN_VERTEX_SHADER,
            entry_func: "vs_main",
        },
        ps: ShaderDesc {
            source: fragment_shader,
            entry_func: "fs_main",
        },
        vertex_buffers: &[],
        bind_groups: vec![entries],
        targets: &[Some(wgpu::ColorTargetState {
            format,
            blend,
            write_mask: wgpu::ColorWrites::ALL,
        })],
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        primitive: wgpu::PrimitiveState::default(),
    })
}

/// Bilinear, clamp-to-edge.
pub(crate) fn linear_clamp_sampler(renderer: &dyn PillRenderer) -> wgpu::Sampler {
    renderer
        .get_device()
        .create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        })
}

pub(crate) fn uniform_buffer<T>(renderer: &dyn PillRenderer, label: &str) -> wgpu::Buffer {
    renderer
        .get_device()
        .create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: std::mem::size_of::<T>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
}

/// One bind group resource, in binding order.
pub(crate) enum Binding<'a> {
    Target(PassTextureHandle),
    Sampler(&'a wgpu::Sampler),
    Buffer(&'a wgpu::Buffer),
}

/// Group 0 of one draw site, kept until a resize replaces the target views it holds. Each call
/// site owns one and always passes the same bindings.
#[derive(Default)]
pub(crate) struct CachedBindGroup {
    bind_group: Option<(u64, wgpu::BindGroup)>,
}

impl CachedBindGroup {
    /// The cached group, rebuilt from `bindings` (binding i = bindings[i]) after a resize.
    fn get(
        &mut self,
        renderer: &dyn PillRenderer,
        label: &str,
        pipeline: &Pipeline,
        bindings: &[Binding],
    ) -> Result<&wgpu::BindGroup> {
        let generation = renderer.target_generation();
        if !matches!(&self.bind_group, Some((built, _)) if *built == generation) {
            self.bind_group = Some((
                generation,
                create_bind_group(renderer, label, pipeline, bindings)?,
            ));
        }
        Ok(&self.bind_group.as_ref().expect("bind group just built").1)
    }
}

/// Draws the full-screen triangle into `output`, cleared first, with group 0 from `cache`.
pub(crate) fn draw_fullscreen(
    encoder: &mut wgpu::CommandEncoder,
    renderer: &dyn PillRenderer,
    label: &str,
    pipeline: &Pipeline,
    cache: &mut CachedBindGroup,
    bindings: &[Binding],
    output: &wgpu::TextureView,
) -> Result<()> {
    draw_fullscreen_with_load(
        encoder,
        renderer,
        label,
        pipeline,
        cache,
        bindings,
        output,
        wgpu::LoadOp::Clear(wgpu::Color::BLACK),
    )
}

/// `draw_fullscreen` with an explicit load op (`Load` to blend onto existing content).
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_fullscreen_with_load(
    encoder: &mut wgpu::CommandEncoder,
    renderer: &dyn PillRenderer,
    label: &str,
    pipeline: &Pipeline,
    cache: &mut CachedBindGroup,
    bindings: &[Binding],
    output: &wgpu::TextureView,
    load: wgpu::LoadOp<wgpu::Color>,
) -> Result<()> {
    let bind_group = cache.get(renderer, label, pipeline, bindings)?;

    let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: output,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    render_pass.set_pipeline(&pipeline.pipeline);
    render_pass.set_bind_group(0, bind_group, &[]);
    render_pass.draw(0..3, 0..1);
    Ok(())
}

/// Group 0 from `bindings` (binding i = bindings[i]).
fn create_bind_group(
    renderer: &dyn PillRenderer,
    label: &str,
    pipeline: &Pipeline,
    bindings: &[Binding],
) -> Result<wgpu::BindGroup> {
    let mut entries = Vec::with_capacity(bindings.len());
    for (index, binding) in bindings.iter().enumerate() {
        let resource = match binding {
            Binding::Target(handle) => wgpu::BindingResource::TextureView(
                renderer
                    .get_render_target_view(*handle)
                    .ok_or_else(|| format!("{label}: input target missing"))?,
            ),
            Binding::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
            Binding::Buffer(buffer) => buffer.as_entire_binding(),
        };
        entries.push(wgpu::BindGroupEntry {
            binding: index as u32,
            resource,
        });
    }
    Ok(renderer
        .get_device()
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &pipeline.bind_group_layouts[0],
            entries: &entries,
        }))
}

/// View of a pass output target.
pub(crate) fn target_view<'a>(
    renderer: &'a dyn PillRenderer,
    handle: PassTextureHandle,
    label: &str,
) -> Result<&'a wgpu::TextureView> {
    renderer
        .get_render_target_view(handle)
        .ok_or_else(|| format!("{label}: output target missing").into())
}
