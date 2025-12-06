use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ChromaticAberrationParams {
    intensity: f32,
    radial_falloff: f32,
    _padding: [f32; 2],
}

pub struct PassChromaticAberration {
    label: String,
    input_texture: RendererTextureHandle,
    output_texture: RendererTextureHandle,
    format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    texture_bind_group: Option<wgpu::BindGroup>,
    params_bind_group: Option<wgpu::BindGroup>,
    params_buffer: Option<wgpu::Buffer>,
    sampler: Option<wgpu::Sampler>,

    // Chromatic aberration parameters
    pub intensity: f32, // Strength of the aberration effect (0.0 = none, larger = more offset)
    pub radial_falloff: f32, // How much the effect increases toward screen edges (0.0 = uniform, 1.0 = strong edge effect)

    // Reference to egui client for parameter updates
    egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,
}

impl PassChromaticAberration {
    pub fn new(
        label: &str,
        input_texture: RendererTextureHandle,
        output_texture: RendererTextureHandle,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            input_texture,
            output_texture,
            format,
            pipeline: None,
            texture_bind_group: None,
            params_bind_group: None,
            params_buffer: None,
            sampler: None,
            intensity: 0.005,
            radial_falloff: 2.0,
            egui_client: None,
        }
    }

    pub fn set_egui_client(&mut self, client: std::sync::Arc<crate::ecs::EguiClient>) {
        self.egui_client = Some(client);
    }
}

impl Pass for PassChromaticAberration {
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
            label: Some("chromatic_aberration_texture_bgl"),
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
            label: Some("chromatic_aberration_params_bgl"),
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
            label: Some("chromatic_aberration_params_ubo"),
            size: std::mem::size_of::<ChromaticAberrationParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Get input texture
        let input_tex = resources
            .gpu()
            .textures
            .get(self.input_texture)
            .expect("chromatic aberration input texture");

        // Create texture bind group (set 0)
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("chromatic_aberration_texture_bg"),
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
            label: Some("chromatic_aberration_params_bg"),
            layout: &params_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        // Vertex shader (fullscreen triangle)
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

        // Fragment shader with chromatic aberration
        let fs_src = r#"
            @group(0) @binding(0) var texInput: texture_2d<f32>;
            @group(0) @binding(1) var smpInput: sampler;
            
            struct ChromaticAberrationParams {
              intensity: f32,
              radial_falloff: f32,
            }
            @group(1) @binding(0) var<uniform> UChroma: ChromaticAberrationParams;

            @fragment
            fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              // Calculate distance from center (0.0 at center, ~0.707 at corners)
              let center = vec2<f32>(0.5, 0.5);
              let offset_vec = uv - center;
              let distance_from_center = length(offset_vec);
              
              // Normalize distance (max distance to corner is ~0.707)
              let max_dist = 0.707;
              let normalized_dist = distance_from_center / max_dist;
              
              // Apply radial falloff to create edge-only effect
              // This makes the effect stronger at edges/corners and zero at center
              let edge_factor = pow(normalized_dist, UChroma.radial_falloff);
              
              // Calculate aberration offset based on edge factor
              let aberration_amount = UChroma.intensity * edge_factor;
              
              // Only apply aberration if we're away from center
              if (distance_from_center < 0.01) {
                // Near center: no aberration, just sample normally
                return textureSample(texInput, smpInput, uv);
              }
              
              // Normalize the offset direction
              let direction = normalize(offset_vec);
              
              // Sample each color channel with different offsets
              // Red channel: shift outward from center (toward edges)
              let r_uv = uv + direction * aberration_amount;
              let r = textureSample(texInput, smpInput, r_uv).r;
              
              // Green channel: no shift (reference)
              let g = textureSample(texInput, smpInput, uv).g;
              
              // Blue channel: shift inward (toward center)
              let b_uv = uv - direction * aberration_amount;
              let b = textureSample(texInput, smpInput, b_uv).b;
              
              return vec4<f32>(r, g, b, 1.0);
            }
        "#;

        // Create pipeline
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chromatic_aberration_pl"),
            bind_group_layouts: &[&texture_bgl, &params_bgl],
            push_constant_ranges: &[],
        });

        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chromatic_aberration_vs"),
            source: wgpu::ShaderSource::Wgsl(vs_src.into()),
        });

        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chromatic_aberration_fs"),
            source: wgpu::ShaderSource::Wgsl(fs_src.into()),
        });

        let rp = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chromatic_aberration_pipeline"),
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
        resources: &mut crate::resources::ResourceManager,
        _frame: &wgpu::SurfaceTexture,
        _view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        // Update parameters from egui_client if available
        if let Some(ref client) = self.egui_client {
            self.intensity = *client.chromatic_aberration_intensity.lock().unwrap();
            self.radial_falloff = *client.chromatic_aberration_radial_falloff.lock().unwrap();
        }

        // Update uniform buffer with current parameters
        let params = ChromaticAberrationParams {
            intensity: self.intensity,
            radial_falloff: self.radial_falloff,
            _padding: [0.0; 2],
        };
        renderer.get_queue().write_buffer(
            self.params_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&params),
        );

        // Get output texture view
        let output_view = resources
            .gpu()
            .textures
            .get(self.output_texture)
            .expect("chromatic aberration output texture")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
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

        Ok(())
    }
}
