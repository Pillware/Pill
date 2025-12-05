use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use wgpu::CommandEncoder;

pub struct PassVignette {
    label: String,
    input_texture: RendererTextureHandle,
    format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    bind_group: Option<wgpu::BindGroup>,
    sampler: Option<wgpu::Sampler>,

    // Vignette parameters
    pub intensity: f32, // How strong the vignette effect is (0.0 = none, 1.0 = max)
    pub smoothness: f32, // How smooth the falloff is (higher = smoother)
}

impl PassVignette {
    pub fn new(
        label: &str,
        input_texture: RendererTextureHandle,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            input_texture,
            format,
            pipeline: None,
            bind_group: None,
            sampler: None,
            intensity: 0.5,
            smoothness: 0.5,
        }
    }
}

impl Pass for PassVignette {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        let device = renderer.get_device();

        // Create sampler for input texture
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Bind group layout
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vignette_bgl"),
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

        // Get input texture
        let input_tex = resources
            .gpu()
            .textures
            .get(self.input_texture)
            .expect("vignette input texture");

        // Create bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vignette_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_tex.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // Shaders
        let vs_src = r#"
            struct VSOut {
              @builtin(position) pos: vec4<f32>,
              @location(0) uv: vec2<f32>,
            };
            @vertex
            fn main(@builtin(vertex_index) vid: u32) -> VSOut {
              var out: VSOut;
              var p: vec2<f32>;
              switch (vid) {
                case 0u: { p = vec2<f32>(-1.0, -1.0); }
                case 1u: { p = vec2<f32>(-1.0,  3.0); }
                default: { p = vec2<f32>( 3.0, -1.0); }
              }
              let uv = p * 0.5 + vec2<f32>(0.5, 0.5);
              out.pos = vec4<f32>(p, 0.0, 1.0);
              out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
              return out;
            }
        "#;

        let fs_src = r#"
            @group(0) @binding(0) var texInput: texture_2d<f32>;
            @group(0) @binding(1) var smpInput: sampler;

            @fragment
            fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              let color = textureSample(texInput, smpInput, uv).rgb;
              
              // Calculate distance from center (0.5, 0.5)
              let center = vec2<f32>(0.5, 0.5);
              let dist = distance(uv, center);
              
              // Vignette parameters
              let intensity = 0.7;   // How dark the edges get
              let smoothness = 0.5;  // Falloff smoothness
              
              // Calculate vignette factor
              // The max distance from center to corner is ~0.707 (sqrt(0.5^2 + 0.5^2))
              let maxDist = 0.707;
              let normalizedDist = dist / maxDist;
              
              // Smooth vignette falloff
              let vignette = smoothstep(smoothness, smoothness * 2.0, 1.0 - normalizedDist);
              let vignetteAmount = mix(1.0 - intensity, 1.0, vignette);
              
              let finalColor = color * vignetteAmount;
              
              return vec4<f32>(finalColor, 1.0);
            }
        "#;

        // Create pipeline
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vignette_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vignette_vs"),
            source: wgpu::ShaderSource::Wgsl(vs_src.into()),
        });

        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vignette_fs"),
            source: wgpu::ShaderSource::Wgsl(fs_src.into()),
        });

        let rp = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vignette_pipeline"),
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
                    format: self.format,
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

        self.pipeline = Some(PipelineV2 {
            pipeline: rp,
            bind_group_layouts: vec![bgl],
        });
        self.bind_group = Some(bind_group);
        self.sampler = Some(sampler);

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        _renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

        rpass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        rpass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        rpass.draw(0..3, 0..1);
        drop(rpass);

        Ok(())
    }
}
