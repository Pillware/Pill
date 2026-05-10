use crate::graphics::{
    Pass, PillRenderer, PipelineV2, PipelineV2Desc, RendererTextureHandle, ShaderDesc, WorldQuery,
};

use pill_core::Result;

static VS: &str = include_str!("../../res/shaders/tonemap_vertex.wgsl");
static FS: &str = include_str!("../../res/shaders/tonemap_fragment.wgsl");

pub struct PassTonemap {
    hdr_source: RendererTextureHandle,
    state: Option<TonemapState>,
}

struct TonemapState {
    pipeline: PipelineV2,
    sampler: wgpu::Sampler,
    // sRGB view of the swapchain — GPU applies OETF on write even when surface format is linear (e.g. WebGPU/wasm32).
    swapchain_view_format: wgpu::TextureFormat,
}

impl PassTonemap {
    pub fn new(hdr_source: RendererTextureHandle) -> Self {
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
        let surface_format = renderer.get_surface_format();
        let swapchain_view_format = if surface_format.is_srgb() {
            surface_format
        } else {
            surface_format.add_srgb_suffix()
        };

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
        ]];

        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("pass_tonemap"),
            vs: ShaderDesc {
                source: VS,
                entry_func: "vs_main",
            },
            ps: ShaderDesc {
                source: FS,
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

        self.state = Some(TonemapState {
            pipeline,
            sampler,
            swapchain_view_format,
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        frame: &wgpu::SurfaceTexture,
        _view: &wgpu::TextureView,
        _world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_ref()
            .expect("PassTonemap: state not initialized — call init() before draw()");
        let hdr_view = renderer
            .get_render_target_view(self.hdr_source)
            .expect("PassTonemap: HDR render target missing");
        let swapchain_view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(state.swapchain_view_format),
            ..Default::default()
        });

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
                    ],
                })
        };

        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass_tonemap_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &swapchain_view,
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
