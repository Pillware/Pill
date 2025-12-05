use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MotionBlurParams {
    intensity: f32,
    samples: u32,
    _padding: [f32; 2],
    current_view_proj: [[f32; 4]; 4],
    previous_view_proj: [[f32; 4]; 4],
    inv_current_view_proj: [[f32; 4]; 4],
}

pub struct PassMotionBlur {
    label: String,
    input_texture: RendererTextureHandle,
    depth_texture: RendererTextureHandle,
    output_texture: RendererTextureHandle, // Output render target
    format: wgpu::TextureFormat,
    pipeline: Option<PipelineV2>,
    texture_bind_group: Option<wgpu::BindGroup>,
    params_bind_group: Option<wgpu::BindGroup>,
    params_buffer: Option<wgpu::Buffer>,
    sampler: Option<wgpu::Sampler>,

    // Motion blur parameters
    pub intensity: f32, // Blur strength (0.0 = none, 1.0 = max)
    pub samples: u32,   // Number of samples (more = smoother but slower)

    // Reference to egui client for parameter updates
    egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,

    // Previous frame view-projection matrix for motion vector calculation
    prev_view_proj: [[f32; 4]; 4],
}

impl PassMotionBlur {
    pub fn new(
        label: &str,
        input_texture: RendererTextureHandle,
        depth_texture: RendererTextureHandle,
        output_texture: RendererTextureHandle, // Add output parameter
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            input_texture,
            depth_texture,
            output_texture, // Store output
            format,
            pipeline: None,
            texture_bind_group: None,
            params_bind_group: None,
            params_buffer: None,
            sampler: None,
            intensity: 5.0, // Much higher default for visible blur
            samples: 16,    // More samples for smoother blur
            egui_client: None,
            prev_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
        }
    }

    pub fn set_egui_client(&mut self, client: std::sync::Arc<crate::ecs::EguiClient>) {
        self.egui_client = Some(client);
    }
}

impl Pass for PassMotionBlur {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        let device = renderer.get_device();

        // Create sampler for textures
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Texture bind group layout (set 0) - input color and depth
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion_blur_texture_bgl"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Depth,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        // Params bind group layout (set 1)
        let params_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion_blur_params_bgl"),
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
            label: Some("motion_blur_params_ubo"),
            size: std::mem::size_of::<MotionBlurParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Get input textures
        let input_tex = resources
            .gpu()
            .textures
            .get(self.input_texture)
            .expect("motion blur input texture");

        let depth_tex = resources
            .gpu()
            .textures
            .get(self.depth_texture)
            .expect("motion blur depth texture");

        // Create depth sampler
        let depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: None,
            ..Default::default()
        });

        // Create texture bind group (set 0)
        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion_blur_texture_bg"),
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth_tex.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&depth_sampler),
                },
            ],
        });

        // Create params bind group (set 1)
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion_blur_params_bg"),
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
            @group(0) @binding(2) var texDepth: texture_depth_2d;
            @group(0) @binding(3) var smpDepth: sampler;
            
            struct MotionBlurParams {
              intensity: f32,
              samples: u32,
              _padding: vec2<f32>,
              current_view_proj: mat4x4<f32>,
              previous_view_proj: mat4x4<f32>,
              inv_current_view_proj: mat4x4<f32>,
            }
            @group(1) @binding(0) var<uniform> UMotionBlur: MotionBlurParams;

            @fragment
            fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              // Sample depth
              let depth = textureSample(texDepth, smpDepth, uv);
              
              // Reconstruct world position from depth
              // NDC space: x,y in [-1,1], z is depth value
              let ndc = vec4<f32>(uv.x * 2.0 - 1.0, (1.0 - uv.y) * 2.0 - 1.0, depth, 1.0);
              
              // Transform to world space
              let world_pos = UMotionBlur.inv_current_view_proj * ndc;
              let world_pos_div = world_pos.xyz / world_pos.w;
              
              // Project to previous frame screen space
              let prev_clip = UMotionBlur.previous_view_proj * vec4<f32>(world_pos_div, 1.0);
              let prev_ndc = prev_clip.xyz / prev_clip.w;
              
              // Convert from NDC [-1,1] to UV [0,1]
              let prev_uv = vec2<f32>(prev_ndc.x * 0.5 + 0.5, 1.0 - (prev_ndc.y * 0.5 + 0.5));
              
              // Calculate velocity (motion vector) in screen space
              // The velocity from reprojection is in pixels/frame, but we need to amplify it
              // to create visible blur trails across texture features
              let velocity = (uv - prev_uv) * UMotionBlur.intensity * 10.0;  // 10x base multiplier
              
              var color = vec3<f32>(0.0, 0.0, 0.0);
              let samples = f32(UMotionBlur.samples);
              
              // Sample along the velocity vector (trailing blur)
              for (var i = 0u; i < UMotionBlur.samples; i = i + 1u) {
                let t = f32(i) / (samples - 1.0);  // 0.0 to 1.0
                let sample_uv = uv - velocity * t;  // Sample backward along velocity (trailing)
                
                // Check bounds
                if (sample_uv.x >= 0.0 && sample_uv.x <= 1.0 && 
                    sample_uv.y >= 0.0 && sample_uv.y <= 1.0) {
                  color = color + textureSample(texInput, smpInput, sample_uv).rgb;
                } else {
                  color = color + textureSample(texInput, smpInput, uv).rgb;
                }
              }
              
              color = color / samples;
              
              return vec4<f32>(color, 1.0);
            }
        "#;

        // Create pipeline
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("motion_blur_pl"),
            bind_group_layouts: &[&texture_bgl, &params_bgl],
            push_constant_ranges: &[],
        });

        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion_blur_vs"),
            source: wgpu::ShaderSource::Wgsl(vs_src.into()),
        });

        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion_blur_fs"),
            source: wgpu::ShaderSource::Wgsl(fs_src.into()),
        });

        let rp = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("motion_blur_pipeline"),
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
        world: &WorldQuery,
    ) -> Result<()> {
        use crate::ecs::CameraComponent;
        use crate::ecs::TransformComponent;
        use pill_core::PillSlotMapKey;

        // Update parameters from egui_client if available
        if let Some(ref client) = self.egui_client {
            self.intensity = *client.motion_blur_intensity.lock().unwrap();
            self.samples = *client.motion_blur_samples.lock().unwrap();
        }

        // Get camera matrices
        let active_camera_entity_handle = world.active_camera;
        let camera_storage = world.camera_components;
        let transform_storage = world.transform_components;

        let camera_opt = camera_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .and_then(|c| c.as_ref());
        let transform_opt = transform_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .and_then(|t| t.as_ref());

        let (current_view_proj, inv_current_view_proj) = if let (Some(camera), Some(transform)) =
            (camera_opt, transform_opt)
        {
            use glam::{EulerRot, Mat4, Quat, Vec3};

            const OPENGL_TO_WGPU_MATRIX: Mat4 = Mat4::from_cols_array(&[
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.0, 1.0,
            ]);

            let yaw = transform.rotation.y.to_radians();
            let pitch = transform.rotation.x.to_radians();
            let roll = transform.rotation.z.to_radians();
            let q = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);
            let eye = Vec3::new(
                transform.position.x,
                transform.position.y,
                transform.position.z,
            );
            let dir = q * Vec3::Z;
            let view = Mat4::look_to_rh(eye, dir, Vec3::Y);

            let fov_y = camera.fov.to_radians();
            let aspect = camera.aspect.get_value();
            let z_near = camera.range.start;
            let z_far = camera.range.end;
            let proj = OPENGL_TO_WGPU_MATRIX * Mat4::perspective_rh(fov_y, aspect, z_near, z_far);

            let vp = proj * view;
            (vp, vp.inverse())
        } else {
            (glam::Mat4::IDENTITY, glam::Mat4::IDENTITY)
        };

        // Update uniform buffer with current parameters
        let params = MotionBlurParams {
            intensity: self.intensity,
            samples: self.samples,
            _padding: [0.0, 0.0],
            current_view_proj: current_view_proj.to_cols_array_2d(),
            previous_view_proj: self.prev_view_proj,
            inv_current_view_proj: inv_current_view_proj.to_cols_array_2d(),
        };
        renderer.get_queue().write_buffer(
            self.params_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&params),
        );

        // Store current as previous for next frame
        self.prev_view_proj = current_view_proj.to_cols_array_2d();

        // Get output texture view
        let output_view = resources
            .gpu()
            .textures
            .get(self.output_texture)
            .expect("motion blur output texture")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &output_view, // Write to output texture instead of final view
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
