use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DofParams {
    focus_distance: f32,
    focus_range: f32,
    blur_strength: f32,
    samples: i32,
}

pub struct PassDof {
    label: String,
    input_texture: RendererTextureHandle,
    depth_texture: RendererTextureHandle,
    output_texture: RendererTextureHandle,
    format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    texture_bind_group: Option<wgpu::BindGroup>,
    params_bind_group: Option<wgpu::BindGroup>,
    params_buffer: Option<wgpu::Buffer>,
    sampler: Option<wgpu::Sampler>,

    // DOF parameters
    pub focus_distance: f32, // Distance to focus plane (0.0 = near, 1.0 = far)
    pub focus_range: f32,    // Range of sharp focus (smaller = more blur)
    pub blur_strength: f32,  // Maximum blur amount (0.0 = none, 1.0 = max)
    pub samples: i32,        // Number of blur samples (more = better quality, slower)

    // Reference to egui client for parameter updates
    egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,
}

impl PassDof {
    pub fn new(
        label: &str,
        input_texture: RendererTextureHandle,
        depth_texture: RendererTextureHandle,
        output_texture: RendererTextureHandle,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            input_texture,
            depth_texture,
            output_texture,
            format,
            pipeline: None,
            texture_bind_group: None,
            params_bind_group: None,
            params_buffer: None,
            sampler: None,
            focus_distance: 0.5,
            focus_range: 0.2,
            blur_strength: 0.5,
            samples: 32,
            egui_client: None,
        }
    }

    pub fn set_egui_client(&mut self, client: std::sync::Arc<crate::ecs::EguiClient>) {
        self.egui_client = Some(client);
    }
}

impl Pass for PassDof {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        // Vertex shader (fullscreen triangle)
        let vs = r#"
        struct VSOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, };
        @vertex fn main(@builtin(vertex_index) vi: u32) -> VSOut {
          var pos = array<vec2<f32>, 3>(
            vec2<f32>(-1.0, -3.0),
            vec2<f32>( 3.0,  1.0),
            vec2<f32>(-1.0,  1.0)
          );
          var uv = (pos[vi] + 1.0) * 0.5;
          uv.y = 1.0 - uv.y;
          return VSOut(vec4<f32>(pos[vi], 0.0, 1.0), uv);
        }
        "#;

        // Fragment shader with depth-based bokeh blur
        let fs = r#"
        struct DofParams {
            focus_distance: f32,
            focus_range: f32,
            blur_strength: f32,
            samples: i32,
        }

        @group(0) @binding(0) var texColor: texture_2d<f32>;
        @group(0) @binding(1) var smpColor: sampler;
        @group(0) @binding(2) var texDepth: texture_2d<f32>;
        @group(0) @binding(3) var smpDepth: sampler;
        @group(1) @binding(0) var<uniform> params: DofParams;

        @fragment
        fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            // Sample depth
            let depth = textureSample(texDepth, smpDepth, uv).r;
            
            // Calculate blur amount based on distance from focus plane
            let focus_diff = abs(depth - params.focus_distance);
            let blur_amount = smoothstep(0.0, params.focus_range, focus_diff) * params.blur_strength;
            
            // If blur is negligible, return original color
            if (blur_amount < 0.01) {
                return textureSample(texColor, smpColor, uv);
            }
            
            // Bokeh-style circular blur using golden angle spiral
            let texel_size = 1.0 / vec2<f32>(textureDimensions(texColor));
            let blur_radius = blur_amount * 20.0; // Max blur radius in pixels
            
            var color_sum = vec3<f32>(0.0);
            var weight_sum = 0.0;
            
            let golden_angle = 2.399963; // Golden angle in radians
            
            // Use dynamic sample count from uniform
            for (var i: i32 = 0; i < params.samples; i = i + 1) {
                let angle = f32(i) * golden_angle;
                let radius = sqrt(f32(i) / f32(params.samples)) * blur_radius;
                
                let offset = vec2<f32>(cos(angle), sin(angle)) * radius * texel_size;
                let sample_uv = uv + offset;
                
                // Sample color
                let sample_color = textureSample(texColor, smpColor, sample_uv).rgb;
                
                // Weight by distance (softer falloff for bokeh effect)
                let weight = 1.0;
                color_sum = color_sum + sample_color * weight;
                weight_sum = weight_sum + weight;
            }
            
            // Center sample
            let center_color = textureSample(texColor, smpColor, uv).rgb;
            color_sum = color_sum + center_color;
            weight_sum = weight_sum + 1.0;
            
            let final_color = color_sum / weight_sum;
            return vec4<f32>(final_color, 1.0);
        }
        "#;

        // Create pipeline
        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("dof_pipeline"),
            vs: ShaderDesc {
                source: vs,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs,
                entry_func: "main",
            },
            vertex_buffers: &[],
            bind_groups: vec![
                // Textures (set 0)
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                // Parameters (set 1)
                vec![wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            ],
            targets: &[Some(wgpu::ColorTargetState {
                format: self.format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        // Now get device for creating buffers and bind groups
        let device = renderer.get_device();

        // Create sampler for input textures
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create parameters buffer
        let params_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dof_params_buffer"),
            size: std::mem::size_of::<DofParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Create parameters bind group
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dof_params_bg"),
            layout: &pipeline.bind_group_layouts[1],
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params_buffer.as_entire_binding(),
            }],
        });

        // Create texture bind group
        let input_tex = resources
            .gpu()
            .textures
            .get(self.input_texture)
            .expect("input texture");
        let depth_tex = resources
            .gpu()
            .textures
            .get(self.depth_texture)
            .expect("depth texture");

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dof_texture_bg"),
            layout: &pipeline.bind_group_layouts[0],
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&input_tex.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth_tex.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        self.pipeline = Some(pipeline);
        self.params_buffer = Some(params_buffer);
        self.params_bind_group = Some(params_bind_group);
        self.texture_bind_group = Some(texture_bind_group);
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
        // Check if DOF is enabled
        let enabled = if let Some(egui_client) = &self.egui_client {
            *egui_client.dof_enabled.lock().unwrap()
        } else {
            false
        };

        // Update parameters from egui if available
        if let Some(egui_client) = &self.egui_client {
            self.focus_distance = *egui_client.dof_focus_distance.lock().unwrap();
            self.focus_range = *egui_client.dof_focus_range.lock().unwrap();
            self.blur_strength = *egui_client.dof_blur_strength.lock().unwrap();
            self.samples = *egui_client.dof_samples.lock().unwrap();
        }

        // If disabled, set blur strength to 0 (pass-through)
        let effective_blur_strength = if enabled { self.blur_strength } else { 0.0 };

        // Write parameters to buffer
        let params = DofParams {
            focus_distance: self.focus_distance,
            focus_range: self.focus_range,
            blur_strength: effective_blur_strength,
            samples: self.samples,
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
            .expect("output texture")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Render pass
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
