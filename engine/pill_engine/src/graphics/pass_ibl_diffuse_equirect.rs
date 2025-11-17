use crate::graphics::renderer::{Pass, PillRenderer as EnginePillRenderer, WorldQuery};
use crate::graphics::{PipelineV2, PipelineV2Desc, RendererTextureHandle, ShaderDesc};
use anyhow::Result;
use wgpu::CommandEncoder;

pub struct PassIblDiffuseEquirect {
    label: String,
    env_texture: RendererTextureHandle,
    output_handle: RendererTextureHandle,
    output_format: wgpu::TextureFormat,
    // GPU
    bind_group_env: Option<wgpu::BindGroup>,
    pipeline: Option<PipelineV2>,
    done: bool,
}

impl PassIblDiffuseEquirect {
    pub fn new(
        label: &str,
        env_texture: RendererTextureHandle,
        output_handle: RendererTextureHandle,
        output_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            env_texture,
            output_handle,
            output_format,
            bind_group_env: None,
            pipeline: None,
            done: false,
        }
    }
}

impl Pass for PassIblDiffuseEquirect {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        if self.pipeline.is_some() {
            return Ok(());
        }
        // BGLs are declared via PipelineV2Desc (no manual BGL creation needed here)

        // Build pipeline (fullscreen triangle; writes to color via standard render pass)
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
        const PI: f32 = 3.14159265359;
        @group(0) @binding(0) var TEnv: texture_2d<f32>;
        @group(0) @binding(1) var SEnv: sampler;

        fn dir_to_equirect_uv(dir: vec3<f32>) -> vec2<f32> {
          let d = normalize(dir);
          let u = 0.5 + atan2(d.x, -d.z) / (2.0 * PI);
          let v = 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI;
          return vec2<f32>(fract(u), clamp(v, 0.0, 1.0));
        }

        fn uv_to_dir_equirect(uv: vec2<f32>) -> vec3<f32> {
          let phi = (uv.x * 2.0 * PI) - PI;
          let theta = uv.y * PI;
          let x =  sin(phi) * sin(theta);
          let y =  cos(theta);
          let z = -cos(phi) * sin(theta);
          return normalize(vec3<f32>(x, y, z));
        }

        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
          // Output direction (normal) from output pixel uv
          let N = uv_to_dir_equirect(uv);
          // Hemisphere integration around N (discrete)
          let sample_delta: f32 = 0.025;
          var irradiance = vec3<f32>(0.0, 0.0, 0.0);
          var weight = 0.0;
          var phi: f32 = 0.0;
          loop {
            if (phi >= 2.0 * PI) { break; }
            var theta: f32 = 0.0;
            loop {
              if (theta >= 0.5 * PI) { break; }
              let x = cos(phi) * sin(theta);
              let y = sin(theta);
              let z = sin(phi) * sin(theta);
              // tangent-to-world around N
              let up = vec3<f32>(0.0, 1.0, 0.0);
              let right = normalize(cross(up, N));
              let upn = cross(N, right);
              let sample_vec = normalize(right * x + upn * y + N * cos(theta));
              let st = dir_to_equirect_uv(sample_vec);
              let radiance = textureSample(TEnv, SEnv, st).rgb;
              irradiance = irradiance + radiance * cos(theta) * sin(theta);
              weight = weight + cos(theta) * sin(theta);
              theta = theta + sample_delta;
            }
            phi = phi + sample_delta;
          }
          irradiance = irradiance / max(weight, 1e-4);
          return vec4<f32>(irradiance, 1.0);
        }
        "#;

        // Pipeline fragment target format must match the output render target format used in draw().
        // See wgpu ColorTargetState docs:
        // https://docs.rs/wgpu/0.20.1/wgpu/struct.ColorTargetState.html
        log::info!(
            "ibl_diff:init output_format={:?} surface_format={:?}",
            self.output_format,
            renderer.get_surface_format()
        );
        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("ibl_diffuse_equirect"),
            vs: ShaderDesc {
                source: vs,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs,
                entry_func: "main",
            },
            vertex_buffers: &[], // fullscreen triangle via vertex_index; no vertex buffers
            bind_groups: vec![
                // env
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
                format: self.output_format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        self.pipeline = Some(pipeline);
        // Bind groups built lazily in draw once resources are guaranteed ready
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
        if self.done {
            return Ok(());
        }
        // Log formats to verify pipeline target equals the render pass attachment format.
        let surface_fmt = renderer.get_surface_format();
        log::info!(
            "ibl_diff:draw start surface_format={:?} irradiance_target_format={:?}",
            surface_fmt,
            self.output_format
        );
        if surface_fmt == self.output_format {
            log::warn!(
                "ibl_diff: target format equals surface; ensure this pass renders to irradiance RT, not the swapchain"
            );
        }
        // Lazy build bind groups
        if self.bind_group_env.is_none() {
            if let Some(pipeline) = &self.pipeline {
                let device = renderer.get_device();
                // env
                if let Some(env) = resources.gpu().textures.get(self.env_texture) {
                    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                        address_mode_u: wgpu::AddressMode::Repeat,
                        address_mode_v: wgpu::AddressMode::ClampToEdge,
                        address_mode_w: wgpu::AddressMode::ClampToEdge,
                        mag_filter: wgpu::FilterMode::Linear,
                        min_filter: wgpu::FilterMode::Linear,
                        mipmap_filter: wgpu::FilterMode::Nearest,
                        ..Default::default()
                    });
                    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("ibl_diff_env_bg"),
                        layout: &pipeline.bind_group_layouts[0],
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&env.texture_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(&sampler),
                            },
                        ],
                    });
                    self.bind_group_env = Some(bg);
                }
            }
        }
        // If still not ready (e.g., env missing), skip this frame
        if self.bind_group_env.is_none() {
            return Ok(());
        }
        // Render once into output
        let out_view = resources
            .gpu()
            .textures
            .get(self.output_handle)
            .expect("out")
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("ibl_irradiance_rt_view"),
                ..Default::default()
            });
        log::info!(
            "ibl_diff:begin pass target=irradiance_2d format={:?}",
            self.output_format
        );
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &out_view,
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
        rp.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        rp.set_bind_group(0, self.bind_group_env.as_ref().unwrap(), &[]);
        rp.draw(0..3, 0..1);
        drop(rp);
        log::info!("ibl_diff:draw completed");
        self.done = true;
        Ok(())
    }
}
