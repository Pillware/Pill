use crate::renderer::{Pass, Renderer};
use crate::resources::RendererTexture;
use anyhow::Result;
use pill_engine::internal::{PillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc};
use std::sync::Arc;
use wgpu::CommandEncoder;
use wgpu::Queue;

pub struct PassCompose {
    label: String,
    offscreen_texture: Arc<RendererTexture>,
    pipeline: Option<PipelineV2>,
    bind_group: Option<wgpu::BindGroup>,
}

impl PassCompose {
    pub fn new(label: &str, offscreen_texture: Arc<RendererTexture>) -> Self {
        Self {
            label: label.to_string(),
            offscreen_texture,
            pipeline: None,
            bind_group: None,
        }
    }
}

impl Pass for PassCompose {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, _queue: &wgpu::Queue, renderer: &Renderer) -> Result<()> {
        println!("Initializing pass: {}", self.label);

        let vs = r#"
        struct VSOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, };
        @vertex fn main(@builtin(vertex_index) vi: u32) -> VSOut {
          var pos = array<vec2<f32>, 3>(
            vec2<f32>(-1.0, -3.0),
            vec2<f32>( 3.0,  1.0),
            vec2<f32>(-1.0,  1.0)
          );
          var tuv = array<vec2<f32>, 3>(
            vec2<f32>(0.0, 2.0),
            vec2<f32>(2.0, 0.0),
            vec2<f32>(0.0, 0.0)
          );
          var o: VSOut;
          o.pos = vec4<f32>(pos[vi], 0.0, 1.0);
          o.uv = tuv[vi];
          return o;
        }
        "#;

        let fs = r#"
        @group(0) @binding(0) var t_src: texture_2d<f32>;
        @group(0) @binding(1) var s_src: sampler;
        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
          let hdr = textureSample(t_src, s_src, uv).rgb;
          // Reinhard tone mapping; output remains in linear space. The sRGB swapchain will encode.
          let ldr = hdr / (1.0 + hdr);
          return vec4<f32>(ldr, 1.0);
        }
        "#;

        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("compose"),
            vs: ShaderDesc {
                source: vs,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs,
                entry_func: "main",
            },
            bind_groups: vec![
                // Group 0: Texture and sampler bindings
                vec![
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
                ],
            ],
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.get_surface_format(),
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        let bind_group = renderer
            .get_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compose_bind_group"),
                layout: &pipeline.bind_group_layouts[0], // Group 0: Texture and sampler bindings
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &self.offscreen_texture.texture_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.offscreen_texture.sampler),
                    },
                ],
            });

        // Store pipeline handle
        self.pipeline = Some(pipeline);
        self.bind_group = Some(bind_group);

        Ok(())
    }

    fn draw(
        &self,
        encoder: &mut CommandEncoder,
        _renderer: &Renderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
    ) -> Result<()> {
        // Create render pass for this pass using the provided frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // Draw fullscreen triangle for composition
        pass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        pass.set_bind_group(0, &self.bind_group.as_ref().unwrap(), &[]);
        pass.draw(0..3, 0..1);

        Ok(())
    }
}
