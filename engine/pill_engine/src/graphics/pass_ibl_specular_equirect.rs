use crate::graphics::renderer::{Pass, PillRenderer as EnginePillRenderer, WorldQuery};
use crate::graphics::{PipelineV2, PipelineV2Desc, ShaderDesc};
use anyhow::Result;
use pill_core::PillSlotMapKey;
use wgpu::CommandEncoder;

pub struct PassIblSpecularEquirect {
    label: String,
    env_texture: crate::graphics::RendererTextureHandle,
    // Outputs
    prefilter_handle: crate::graphics::RendererTextureHandle,
    brdf_lut_handle: crate::graphics::RendererTextureHandle,
    // GPU
    env_bind_group: Option<wgpu::BindGroup>,
    params_buffer: Option<wgpu::Buffer>,
    params_bind_group: Option<wgpu::BindGroup>,
    prefilter_pipeline: Option<PipelineV2>,
    brdf_pipeline: Option<wgpu::RenderPipeline>,
    // Tracking last-known formats to support adaptive re-init after resize/changes
    last_prefilter_format: Option<wgpu::TextureFormat>,
    last_brdf_format: Option<wgpu::TextureFormat>,
    // Config
    mip_count: u32,
    done: bool,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct PrefilterParams {
    roughness: f32,
    _pad: [f32; 3],
}

impl PassIblSpecularEquirect {
    pub fn new(
        label: &str,
        env_texture: crate::graphics::RendererTextureHandle,
        prefilter_handle: crate::graphics::RendererTextureHandle,
        brdf_lut_handle: crate::graphics::RendererTextureHandle,
        mip_count: u32,
    ) -> Self {
        Self {
            label: label.to_string(),
            env_texture,
            prefilter_handle,
            brdf_lut_handle,
            env_bind_group: None,
            params_buffer: None,
            params_bind_group: None,
            prefilter_pipeline: None,
            brdf_pipeline: None,
            last_prefilter_format: None,
            last_brdf_format: None,
            mip_count,
            done: false,
        }
    }

    pub fn prefilter_texture_handle(&self) -> crate::graphics::RendererTextureHandle {
        self.prefilter_handle
    }
    pub fn brdf_lut_handle(&self) -> crate::graphics::RendererTextureHandle {
        self.brdf_lut_handle
    }
}

impl Pass for PassIblSpecularEquirect {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        // All outputs provided by caller; build/refresh pipelines based on current target formats.

        // Build env bind group layout via pipeline desc for prefilter (set 0 -> env tex+sampler, set 1 -> params)
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
        let fs_prefilter = r#"
        const PI: f32 = 3.14159265359;
        @group(0) @binding(0) var TEnv: texture_2d<f32>;
        @group(0) @binding(1) var SEnv: sampler;
        struct Params { roughness: f32, _pad0: vec3<f32>, }
        @group(1) @binding(0) var<uniform> UParams: Params;

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

        // GGX VNDF importance sampling (approx)
        fn hammersley(i: u32, n: u32) -> vec2<f32> {
          // radical inverse Van der Corput in base 2 (underscores removed for wider WGSL parser support)
          var bits = i;
          bits = (bits << 16u) | (bits >> 16u);
          bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
          bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
          bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
          bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
          let rdi = f32(bits) * 2.3283064365386963e-10;
          return vec2<f32>(f32(i)/f32(n), rdi);
        }
        fn importance_sample_ggx(xi: vec2<f32>, roughness: f32, N: vec3<f32>) -> vec3<f32> {
          let a = roughness * roughness;
          let phi = 2.0 * PI * xi.x;
          let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a*a - 1.0) * xi.y));
          let sin_theta = sqrt(1.0 - cos_theta * cos_theta);
          // tangent space
          let Ht = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
          // build basis
          let up = vec3<f32>(0.0, 1.0, 0.0);
          let T = normalize(cross(up, N));
          let B = cross(N, T);
          // to world
          return normalize(T * Ht.x + B * Ht.y + N * Ht.z);
        }

        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
          let N = uv_to_dir_equirect(uv);
          let V = N;
          let R = N;
          let rough = clamp(UParams.roughness, 0.001, 1.0);
          var prefiltered = vec3<f32>(0.0, 0.0, 0.0);
          var total = 0.0;
          let SAMPLE_COUNT: u32 = 1024u;
          for (var i: u32 = 0u; i < SAMPLE_COUNT; i = i + 1u) {
            let xi = hammersley(i, SAMPLE_COUNT);
            let H = importance_sample_ggx(xi, rough, N);
            let L = normalize(reflect(-V, H));
            let NdotL = max(dot(N, L), 0.0);
            if (NdotL > 0.0) {
              let st = dir_to_equirect_uv(L);
              let c = textureSample(TEnv, SEnv, st).rgb;
              prefiltered = prefiltered + c * NdotL;
              total = total + NdotL;
            }
          }
          prefiltered = prefiltered / max(total, 1e-4);
          return vec4<f32>(prefiltered, 1.0);
        }
        "#;

        // Current target formats (may change after resize or handle reuse).
        // IMPORTANT: In wgpu, RenderPipeline.fragment.targets[n].format MUST equal the
        // RenderPass color_attachments[n] texture view format or validation will fail at set_pipeline().
        // See ColorTargetState docs:
        // https://docs.rs/wgpu/0.20.1/wgpu/struct.ColorTargetState.html
        let prefilter_format = _resources
            .gpu()
            .textures
            .get(self.prefilter_handle)
            .expect("prefilter texture handle")
            .format;
        let env_h = self.env_texture;
        let pre_h = self.prefilter_handle;
        let brdf_h = self.brdf_lut_handle;
        log::info!(
            "ibl_spec:init env=({},{}) prefilter=({},{}) brdf=({},{})",
            env_h.index(),
            env_h.generation(),
            pre_h.index(),
            pre_h.generation(),
            brdf_h.index(),
            brdf_h.generation()
        );
        log::info!(
            "ibl_spec:init prefilter_format={:?} surface_format={:?}",
            prefilter_format,
            renderer.get_surface_format()
        );
        // Rebuild prefilter pipeline if first time or target format changed.
        if self
            .last_prefilter_format
            .map(|f| f != prefilter_format)
            .unwrap_or(false)
            || self.prefilter_pipeline.is_none()
        {
            let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
                label: Some("ibl_specular_prefilter_equirect"),
                vs: ShaderDesc {
                    source: vs,
                    entry_func: "main",
                },
                ps: ShaderDesc {
                    source: fs_prefilter,
                    entry_func: "main",
                },
                vertex_buffers: &[],
                bind_groups: vec![
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
                    format: prefilter_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
            })?;
            self.prefilter_pipeline = Some(pipeline);
            self.last_prefilter_format = Some(prefilter_format);
            log::info!(
                "ibl_spec:init (re)built prefilter pipeline for {:?}",
                prefilter_format
            );
        }

        // Params buffer/bind group
        let params_buf = renderer
            .get_device()
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("ibl_spec_params"),
                // WGSL/std140: struct { f32 + vec3<f32> } occupies 32 bytes (vec3 rounds to 16 and struct size rounds up).
                // Allocate 32 bytes to satisfy shader expectations even if our Rust struct packs to 16.
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        let params_bg = renderer
            .get_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ibl_spec_params_bg"),
                layout: &self.prefilter_pipeline.as_ref().unwrap().bind_group_layouts[1],
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buf.as_entire_binding(),
                }],
            });
        self.params_buffer = Some(params_buf);
        self.params_bind_group = Some(params_bg);

        // Env bind group (lazy, created in draw when env is guaranteed accessible)
        self.env_bind_group = None;

        // BRDF LUT pipeline (separate simple pipeline directly with device API)
        // Fragment target format must match the BRDF LUT render target texture format.
        let pl = renderer
            .get_device()
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("ibl_brdf_pl"),
                bind_group_layouts: &[],
                push_constant_ranges: &[],
            });
        let vs_mod = renderer
            .get_device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ibl_brdf_vs"),
                source: wgpu::ShaderSource::Wgsl(vs.into()),
            });
        let fs_brdf = r#"
        const PI: f32 = 3.14159265359;
        fn radical_inverse_vdc(bits_in: u32) -> f32 {
          var bits = bits_in;
          bits = (bits << 16u) | (bits >> 16u);
          bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
          bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
          bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
          bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
          return f32(bits) * 2.3283064365386963e-10;
        }
        fn hammersley(i: u32, n: u32) -> vec2<f32> {
          return vec2<f32>(f32(i) / f32(n), radical_inverse_vdc(i));
        }
        fn geometry_schlick_ggx(n_dot_v: f32, roughness: f32) -> f32 {
          let a = roughness * roughness;
          let k = (a * a) / 2.0;
          let denom = n_dot_v * (1.0 - k) + k;
          return n_dot_v / denom;
        }
        fn geometry_smith(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, roughness: f32) -> f32 {
          let n_dot_v = max(dot(n, v), 0.0);
          let n_dot_l = max(dot(n, l), 0.0);
          let ggx2 = geometry_schlick_ggx(n_dot_v, roughness);
          let ggx1 = geometry_schlick_ggx(n_dot_l, roughness);
          return ggx1 * ggx2;
        }
        fn importance_sample_ggx(xi: vec2<f32>, roughness: f32, n: vec3<f32>) -> vec3<f32> {
          let a = roughness * roughness;
          let phi = 2.0 * PI * xi.x;
          let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a*a - 1.0) * xi.y));
          let sin_theta = sqrt(1.0 - cos_theta*cos_theta);
          let h_t = vec3<f32>(cos(phi) * sin_theta, sin(phi) * sin_theta, cos_theta);
          let up = vec3<f32>(0.0, 1.0, 0.0);
          let t = normalize(cross(up, n));
          let b = cross(n, t);
          return normalize(t*h_t.x + b*h_t.y + n*h_t.z);
        }
        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
          let n = vec3<f32>(0.0, 0.0, 1.0);
          let v = vec3<f32>(sqrt(1.0 - uv.x*uv.x), 0.0, uv.x);
          var a = 0.0;
          var b = 0.0;
          let roughness = uv.y;
          let samples: u32 = 512u;
          for (var i: u32 = 0u; i < samples; i = i + 1u) {
            let xi = hammersley(i, samples);
            let h = importance_sample_ggx(xi, roughness, n);
            let l = normalize(2.0 * dot(v, h) * h - v);
            let n_dot_l = max(l.z, 0.0);
            let n_dot_h = max(h.z, 0.0);
            let v_dot_h = max(dot(v, h), 0.0);
            if (n_dot_l > 0.0) {
              let g = geometry_smith(n, v, l, roughness);
              let g_vis = (g * v_dot_h) / max(n_dot_h * max(n.z, 0.0), 1e-4);
              let fc = pow(1.0 - v_dot_h, 5.0);
              a = a + (1.0 - fc) * g_vis;
              b = b + fc * g_vis;
            }
          }
          a = a / f32(samples);
          b = b / f32(samples);
          return vec4<f32>(a, b, 0.0, 1.0);
        }
        "#;
        let fs_mod = renderer
            .get_device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("ibl_brdf_fs"),
                source: wgpu::ShaderSource::Wgsl(fs_brdf.into()),
            });
        // Match BRDF pipeline color target to BRDF LUT texture format
        let brdf_format = _resources
            .gpu()
            .textures
            .get(self.brdf_lut_handle)
            .map(|t| t.format)
            .unwrap_or(wgpu::TextureFormat::Rgba16Float);
        log::info!(
            "ibl_spec:init brdf_lut_format={:?} surface_format={:?}",
            brdf_format,
            renderer.get_surface_format()
        );
        if self
            .last_brdf_format
            .map(|f| f != brdf_format)
            .unwrap_or(false)
            || self.brdf_pipeline.is_none()
        {
            let rp =
                renderer
                    .get_device()
                    .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("ibl_brdf_pipeline"),
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
                                format: brdf_format,
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
            self.brdf_pipeline = Some(rp);
            self.last_brdf_format = Some(brdf_format);
            log::info!(
                "ibl_spec:init (re)built brdf pipeline for {:?}",
                brdf_format
            );
        }

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
        // Log formats used by this pass to quickly diagnose target/pass mismatches.
        log::info!(
            "ibl_spec:draw handles env=({},{}) pre=({},{}) brdf=({},{})",
            self.env_texture.index(),
            self.env_texture.generation(),
            self.prefilter_handle.index(),
            self.prefilter_handle.generation(),
            self.brdf_lut_handle.index(),
            self.brdf_lut_handle.generation()
        );
        let surface_fmt = renderer.get_surface_format();
        let prefilter_fmt = resources
            .gpu()
            .textures
            .get(self.prefilter_handle)
            .expect("prefilter")
            .format;
        log::info!(
            "ibl_spec:draw start surface_format={:?} prefilter_target_format={:?}",
            surface_fmt,
            prefilter_fmt
        );
        // Optional guardrail: warn if someone accidentally attempts to render IBL into the swapchain.
        if surface_fmt == prefilter_fmt {
            log::warn!(
                "ibl_spec: surface format equals prefilter format; ensure this pass renders to the HDR prefilter target, not the swapchain"
            );
        }
        // Defensive guard: if pipeline was built for a different format, skip to avoid panic.
        if let Some(built_for) = self.last_prefilter_format {
            if built_for != prefilter_fmt {
                log::error!(
                    "ibl_spec: prefilter format changed (built_for={:?}, current={:?}); skipping pass to avoid wgpu validation error",
                    built_for,
                    prefilter_fmt
                );
                return Ok(());
            }
        }
        // Lazy env bind group
        if self.env_bind_group.is_none() {
            if let Some(env) = resources.gpu().textures.get(self.env_texture) {
                let device = renderer.get_device();
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("ibl_spec_env_bg"),
                    layout: &self.prefilter_pipeline.as_ref().unwrap().bind_group_layouts[0],
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&env.texture_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&env.sampler),
                        },
                    ],
                });
                self.env_bind_group = Some(bg);
            } else {
                return Ok(());
            }
        }

        // Prefilter: render each mip level with roughness
        {
            let queue = renderer.get_queue();
            let params_buf = self.params_buffer.as_ref().unwrap();
            let prefilter_tex = &resources
                .gpu()
                .textures
                .get(self.prefilter_handle)
                .expect("prefilter")
                .texture;
            // Validate target format again before encoding multiple passes
            if let (Some(built_for), Some(current)) =
                (self.last_prefilter_format, Some(prefilter_fmt))
            {
                if built_for != current {
                    log::error!(
                        "ibl_spec:refusing to render prefilter; pipeline format={:?} current target={:?}",
                        built_for,
                        current
                    );
                    self.done = true;
                    return Ok(());
                }
            }
            // If the current prefilter texture doesn't have a mipchain (common for swapchain-like targets),
            // avoid rendering higher mips to prevent TextureView creation errors.
            let effective_mip_count = if self.last_prefilter_format
                == Some(wgpu::TextureFormat::Rgba16Float)
            {
                self.mip_count
            } else {
                log::warn!(
                    "ibl_spec: prefilter target format {:?} likely has no mipchain; limiting to mip 0",
                    prefilter_fmt
                );
                1
            };
            for mip in 0..effective_mip_count {
                let roughness = (mip as f32) / ((self.mip_count - 1) as f32).max(1.0);
                let params = PrefilterParams {
                    roughness,
                    _pad: [0.0, 0.0, 0.0],
                };
                log::info!(
                    "ibl_spec:prefilter pass mip={} roughness={:.4} target_format={:?}",
                    mip,
                    roughness,
                    prefilter_fmt
                );
                queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
                let view = prefilter_tex.create_view(&wgpu::TextureViewDescriptor {
                    label: Some(&format!("prefilter_mip{}", mip)),
                    format: None,
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    aspect: wgpu::TextureAspect::All,
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    base_array_layer: 0,
                    array_layer_count: Some(1),
                });
                // Begin a render pass whose color attachment view inherits the HDR texture format.
                // This MUST match the pipeline fragment target format; otherwise wgpu validation will error at set_pipeline.
                // See RenderPassDescriptor docs:
                // https://docs.rs/wgpu/0.20.1/wgpu/struct.RenderPassDescriptor.html
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&format!("{}_mip{}", self.label, mip)),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
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
                rp.set_pipeline(&self.prefilter_pipeline.as_ref().unwrap().pipeline);
                rp.set_bind_group(0, self.env_bind_group.as_ref().unwrap(), &[]);
                rp.set_bind_group(1, self.params_bind_group.as_ref().unwrap(), &[]);
                rp.draw(0..3, 0..1);
                drop(rp);
            }
        }

        // BRDF LUT
        {
            let brdf_fmt = resources
                .gpu()
                .textures
                .get(self.brdf_lut_handle)
                .expect("brdf")
                .format;
            log::info!("ibl_spec:brdf_lut pass target_format={:?}", brdf_fmt);
            if let Some(built_for) = self.last_brdf_format {
                if built_for != brdf_fmt {
                    log::error!(
                        "ibl_spec: brdf lut format changed (built_for={:?}, current={:?}); skipping BRDF pass",
                        built_for,
                        brdf_fmt
                    );
                    self.done = true;
                    return Ok(());
                }
            }
            let brdf_view = resources
                .gpu()
                .textures
                .get(self.brdf_lut_handle)
                .expect("brdf")
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ibl_brdf_lut_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &brdf_view,
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
            rp.set_pipeline(self.brdf_pipeline.as_ref().unwrap());
            rp.draw(0..3, 0..1);
            drop(rp);
        }

        self.done = true;
        Ok(())
    }
}
