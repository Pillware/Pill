use anyhow::Result;
use wgpu::CommandEncoder;

use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, RendererTextureHandle, WorldQuery,
};

pub struct PassCompose {
    label: String,
    offscreen_color_texture: RendererTextureHandle,
    target_format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    bind_group: Option<wgpu::BindGroup>,
}

impl PassCompose {
    pub fn new(
        label: &str,
        offscreen_color_texture: RendererTextureHandle,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            offscreen_color_texture,
            target_format,
            pipeline: None,
            bind_group: None,
        }
    }
}

impl Pass for PassCompose {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, renderer: &mut dyn EnginePillRenderer) -> Result<()> {
        let device = renderer.get_device();

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

        // Build bind group layout
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("compose_bgl"),
            entries: &[
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
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compose_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose_vs"),
            source: wgpu::ShaderSource::Wgsl(vs.into()),
        });
        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose_fs"),
            source: wgpu::ShaderSource::Wgsl(fs.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compose_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &vs_mod,
                entry_point: "main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fs_mod,
                entry_point: "main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        // Bind offscreen color texture from renderer (offscreen target)
        // let (view, sampler) = renderer.get_offscreen_color_view_and_sampler();
        let view = renderer
            .get_texture(self.offscreen_color_texture)
            .create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = renderer
            .get_device()
            .create_sampler(&wgpu::SamplerDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compose_bind_group"),
            layout: &bgl, // Group 0: Texture and sampler bindings
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // Store pipeline handle
        self.pipeline = Some(PipelineV2 {
            pipeline,
            bind_group_layouts: vec![bgl],
        });
        self.bind_group = Some(bind_group);

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        _renderer: &mut dyn EnginePillRenderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        // Create render pass for this pass using the provided frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
