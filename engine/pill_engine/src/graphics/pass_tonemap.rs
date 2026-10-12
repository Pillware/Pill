use crate::config::{TONEMAP_FRAGMENT_SHADER, TONEMAP_VERTEX_SHADER};
use crate::ecs::{RenderStateComponent, Tonemap};
use crate::graphics::{
    BufferDesc, FrameTarget, Pass, PassTextureHandle, PillRenderer, Pipeline, PipelineDesc,
    ShaderDesc, WorldQuery,
};

use pill_core::Result;

pub struct PassTonemap {
    hdr_source: PassTextureHandle,
    state: Option<TonemapState>,
}

struct TonemapState {
    pipeline: Pipeline,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
}

/// Mirrors `TonemapParams` in tonemap_fragment.hlsl.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TonemapParams {
    reinhard: f32,              // 1 = Reinhard, 0 = pass-through
    inverse_white_squared: f32, // 1/W², 0 for an infinite white point
    _padding: [f32; 2],
}

impl TonemapParams {
    fn from(tonemap: Tonemap) -> Self {
        let (reinhard, inverse_white_squared) = match tonemap {
            Tonemap::None => (0.0, 0.0),
            Tonemap::Reinhard { white_point } => (1.0, 1.0 / (white_point * white_point)),
        };
        Self {
            reinhard,
            inverse_white_squared,
            _padding: [0.0; 2],
        }
    }
}

impl PassTonemap {
    pub fn new(hdr_source: PassTextureHandle) -> Self {
        Self {
            hdr_source,
            state: None,
        }
    }
}

impl Pass for PassTonemap {
    fn get_label(&self) -> &str {
        "pass_tonemap"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        // Renders to FrameTarget::srgb_view — GPU applies the OETF on write even when the surface
        // format is linear (e.g. WebGPU/wasm32).
        let swapchain_view_format = renderer.get_surface_format().add_srgb_suffix();

        let bind_groups = vec![vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ]];

        let pipeline = renderer.create_pipeline(PipelineDesc {
            label: Some("pass_tonemap"),
            vs: ShaderDesc {
                source: TONEMAP_VERTEX_SHADER,
                entry_func: "vs_main",
            },
            ps: ShaderDesc {
                source: TONEMAP_FRAGMENT_SHADER,
                entry_func: "fs_main",
            },
            vertex_buffers: &[],
            bind_groups,
            targets: &[Some(wgpu::ColorTargetState {
                format: swapchain_view_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
                unclipped_depth: false,
            },
        })?;

        let sampler = renderer
            .get_device()
            .create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });

        let params_buffer = renderer.create_buffer(BufferDesc {
            label: Some("pass_tonemap_params"),
            byte_size: std::mem::size_of::<TonemapParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })?;

        self.state = Some(TonemapState {
            pipeline,
            sampler,
            params_buffer,
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        frame_target: &FrameTarget,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_ref()
            .expect("PassTonemap: state not initialized — call init() before draw()");
        let tonemap = world.get_global::<RenderStateComponent>()?.tonemap;
        renderer.get_queue().write_buffer(
            &state.params_buffer,
            0,
            bytemuck::bytes_of(&TonemapParams::from(tonemap)),
        );
        let hdr_view = renderer
            .get_render_target_view(self.hdr_source)
            .expect("PassTonemap: HDR render target missing");

        let bind_group = {
            let layout = &state.pipeline.bind_group_layouts[0];
            renderer
                .get_device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pass_tonemap_bind_group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(hdr_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&state.sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: state.params_buffer.as_entire_binding(),
                        },
                    ],
                })
        };

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass_tonemap_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &frame_target.srgb_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&state.pipeline.pipeline);
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..3, 0..1);
        Ok(())
    }
}
