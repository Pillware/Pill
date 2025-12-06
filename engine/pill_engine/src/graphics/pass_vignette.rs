use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct VignetteParams {
    intensity: f32,
    smoothness: f32,
    radius: f32,
    _padding: f32,
}

pub struct PassVignette {
    label: String,
    input_texture: RendererTextureHandle,
    format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    texture_bind_group: Option<wgpu::BindGroup>,
    params_bind_group: Option<wgpu::BindGroup>,
    params_buffer: Option<wgpu::Buffer>,
    sampler: Option<wgpu::Sampler>,

    // Vignette parameters
    pub intensity: f32, // How strong the vignette effect is (0.0 = none, 1.0 = max)
    pub smoothness: f32, // How smooth the falloff is (higher = smoother)
    pub radius: f32,    // Vignette radius (0.0 = very small, 1.0 = full screen)

    // Reference to egui client for parameter updates
    egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,
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
            texture_bind_group: None,
            params_bind_group: None,
            params_buffer: None,
            sampler: None,
            intensity: 0.85,
            smoothness: 0.23,
            radius: 1.15,
            egui_client: None,
        }
    }

    pub fn set_egui_client(&mut self, client: std::sync::Arc<crate::ecs::EguiClient>) {
        self.egui_client = Some(client);
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

        // Texture bind group layout (set 0)
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vignette_texture_bgl"),
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

        // Params bind group layout (set 1)
        let params_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vignette_params_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Create params buffer
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vignette_params_ubo"),
            size: std::mem::size_of::<VignetteParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Get input texture
        let input_tex = resources
            .gpu()
            .textures
            .get(self.input_texture)
            .expect("vignette input texture");

        // Create texture bind group (set 0)
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vignette_texture_bg"),
            layout: &texture_bgl,
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

        // Create params bind group (set 1)
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vignette_params_bg"),
            layout: &params_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
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
            
            struct VignetteParams {
              intensity: f32,
              smoothness: f32,
              radius: f32,
              _padding: f32,
            }
            @group(1) @binding(0) var<uniform> UVignette: VignetteParams;

            @fragment
            fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              let color = textureSample(texInput, smpInput, uv).rgb;
              
              // Calculate distance from center (0.5, 0.5)
              let center = vec2<f32>(0.5, 0.5);
              let dist = distance(uv, center);
              
              // Calculate vignette factor
              // The max distance from center to corner is ~0.707 (sqrt(0.5^2 + 0.5^2))
              let maxDist = 0.707;
              let normalizedDist = dist / maxDist;
              
              // Apply radius parameter to control vignette size
              let adjustedDist = normalizedDist / max(UVignette.radius, 0.1);
              
              // Smooth vignette falloff
              let vignette = smoothstep(UVignette.smoothness, UVignette.smoothness * 2.0, 1.0 - adjustedDist);
              let vignetteAmount = mix(1.0 - UVignette.intensity, 1.0, vignette);
              
              let finalColor = color * vignetteAmount;
              
              return vec4<f32>(finalColor, 1.0);
            }
        "#;

        // Create pipeline
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vignette_pl"),
            bind_group_layouts: &[&texture_bgl, &params_bgl],
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
            bind_group_layouts: vec![texture_bgl, params_bgl],
        });
        self.texture_bind_group = Some(texture_bind_group);
        self.params_bind_group = Some(params_bind_group);
        self.params_buffer = Some(params_buffer);
        self.sampler = Some(sampler);

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        // Update parameters from egui_client if available
        if let Some(ref client) = self.egui_client {
            self.intensity = *client.vignette_intensity.lock().unwrap();
            self.smoothness = *client.vignette_smoothness.lock().unwrap();
            self.radius = *client.vignette_radius.lock().unwrap();
        }

        // Update uniform buffer with current parameters
        let params = VignetteParams {
            intensity: self.intensity,
            smoothness: self.smoothness,
            radius: self.radius,
            _padding: 0.0,
        };
        renderer.get_queue().write_buffer(
            self.params_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&params),
        );

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
        rpass.set_bind_group(0, self.texture_bind_group.as_ref().unwrap(), &[]);
        rpass.set_bind_group(1, self.params_bind_group.as_ref().unwrap(), &[]);
        rpass.draw(0..3, 0..1);
        drop(rpass);

        Ok(())
    }
}
