use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc, WorldQuery,
};
use crate::graphics::RendererTextureHandle;
use anyhow::Result;
use glam::{Mat4, Quat, Vec3};
use pill_core::PillSlotMapKey;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    position: [f32; 4],
    view_projection_matrix: [[f32; 4]; 4],
    inverse_projection_matrix: [[f32; 4]; 4],
    inverse_view_matrix: [[f32; 4]; 4],
}

impl CameraUniform {
    fn new() -> Self {
        Self {
            position: [0.0; 4],
            view_projection_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            inverse_projection_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            inverse_view_matrix: Mat4::IDENTITY.to_cols_array_2d(),
        }
    }
}

pub struct PassSkyboxEquirect {
    label: String,
    offscreen_color_texture: RendererTextureHandle,
    color_format: wgpu::TextureFormat,
    env_texture: RendererTextureHandle,

    // GPU
    camera_buffer: Option<wgpu::Buffer>,
    camera_bind_group: Option<wgpu::BindGroup>,
    env_bind_group: Option<wgpu::BindGroup>,
    pipeline: Option<PipelineV2>,
    sampler_clamp: Option<wgpu::Sampler>,
}

impl PassSkyboxEquirect {
    pub fn new(
        label: &str,
        offscreen_color_texture: RendererTextureHandle,
        color_format: wgpu::TextureFormat,
        env_texture: RendererTextureHandle,
    ) -> Self {
        Self {
            label: label.to_string(),
            offscreen_color_texture,
            color_format,
            env_texture,
            camera_buffer: None,
            camera_bind_group: None,
            env_bind_group: None,
            pipeline: None,
            sampler_clamp: None,
        }
    }
}

impl Pass for PassSkyboxEquirect {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        // Log the formats used by the skybox pass. The pipeline target format must match the
        // offscreen color texture's format. This offscreen render target typically matches the
        // swapchain/surface format for later composition.
        log::info!(
            "skybox:init offscreen_color_format={:?} surface_format={:?}",
            self.color_format,
            renderer.get_surface_format()
        );
        // Camera buffer + bind group
        let camera_bgl =
            renderer
                .get_device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("skybox_camera_bgl"),
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
        let camera_buffer = renderer
            .get_device()
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("skybox_camera_ubo"),
                size: std::mem::size_of::<CameraUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        let camera_bind_group =
            renderer
                .get_device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("skybox_camera_bg"),
                    layout: &camera_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: camera_buffer.as_entire_binding(),
                    }],
                });

        // Env texture + sampler (clamp)
        let env_bgl =
            renderer
                .get_device()
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("skybox_env_bgl"),
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
        let sampler_clamp = renderer
            .get_device()
            .create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat, // wrap horizontally
                address_mode_v: wgpu::AddressMode::ClampToEdge, // clamp vertically at poles
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });
        let env_tex = resources
            .gpu()
            .textures
            .get(self.env_texture)
            .expect("env texture");
        let env_bind_group = renderer
            .get_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("skybox_env_bg"),
                layout: &env_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&env_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler_clamp),
                    },
                ],
            });

        // Pipeline (fullscreen triangle)
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
        let ps_src = r#"
            const PI: f32 = 3.141592653589793;
            struct Camera {
              position: vec4<f32>,
              viewProjection: mat4x4<f32>,
              invProjection: mat4x4<f32>,
              invView: mat4x4<f32>,
            };
            @group(0) @binding(0) var<uniform> UCamera: Camera;
            @group(1) @binding(0) var TEnv: texture_2d<f32>;
            @group(1) @binding(1) var SEnv: sampler;

            fn dir_to_equirect_uv(dir: vec3<f32>) -> vec2<f32> {
              let d = normalize(dir);
              // Match common equirectangular convention: forward (-Z) maps to u=0.5
              let u = 0.5 + atan2(d.x, -d.z) / (2.0 * PI);
              let v = 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI;
              return vec2<f32>(fract(u), clamp(v, 0.0, 1.0));
            }

            fn setCamera(camDir: vec3<f32>) -> mat3x3<f32> {
              let cw = normalize(camDir);
              let cp = vec3<f32>(0.0, 1.0, 0.0);
              let cu = normalize(cross(cw, cp));
              let cv = cross(cu, cw);
              return mat3x3<f32>(cu, cv, cw);
            }

            @fragment
            fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              // Raylib-style ray reconstruction using camera basis and aspect-scaled screen coords
              let camDir = normalize((UCamera.invView * vec4<f32>(0.0, 0.0, -1.0, 0.0)).xyz);
              let ca = setCamera(camDir);
              var nCoord = vec2<f32>(-1.0 + 2.0 * uv.x, 1.0 - 2.0 * uv.y);
              let aspect = UCamera.invProjection[0][0] / UCamera.invProjection[1][1];
              nCoord.x *= aspect;
              let fl = length(camDir);
              let rd = ca * normalize(vec3<f32>(nCoord, fl));
              let st = dir_to_equirect_uv(rd);
              let col = textureSample(TEnv, SEnv, st);
              return vec4<f32>(col.rgb, 1.0);
            }
        "#;
        // Build pipeline directly (no vertex buffers)
        let device = renderer.get_device();
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skybox_equirect_pl"),
            bind_group_layouts: &[&camera_bgl, &env_bgl],
            push_constant_ranges: &[],
        });
        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skybox_equirect_v2_vs"),
            source: wgpu::ShaderSource::Wgsl(vs_src.into()),
        });
        let ps_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("skybox_equirect_v2_ps"),
            source: wgpu::ShaderSource::Wgsl(ps_src.into()),
        });
        let rp = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skybox_equirect_v2_pipeline"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &vs_mod,
                entry_point: "main",
                buffers: &[], // no vertex inputs
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ps_mod,
                entry_point: "main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.color_format,
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

        self.camera_buffer = Some(camera_buffer);
        self.camera_bind_group = Some(camera_bind_group);
        self.env_bind_group = Some(env_bind_group);
        self.pipeline = Some(PipelineV2 {
            pipeline: rp,
            bind_group_layouts: vec![camera_bgl, env_bgl],
        });
        self.sampler_clamp = Some(sampler_clamp);
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
        // Update camera UBO
        let active_camera_entity_handle = world.active_camera;
        let camera_storage = world
            .camera_components
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let camera = camera_storage.as_ref().unwrap();
        let transform_storage = world
            .transform_components
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let transform = transform_storage.as_ref().unwrap();
        // Compute view-projection from camera + transform
        let eye = Vec3::new(
            transform.position.x,
            transform.position.y,
            transform.position.z,
        );
        let rot_rad = Vec3::new(
            transform.rotation.y.to_radians(),
            transform.rotation.x.to_radians(),
            transform.rotation.z.to_radians(),
        );
        let q = Quat::from_euler(glam::EulerRot::YXZ, rot_rad.x, rot_rad.y, rot_rad.z);
        // Engine forward is -Z; use -Z so that camera forward maps to center of the equirect
        let dir = q * -Vec3::Z;
        let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
        const OPENGL_TO_WGPU_MATRIX: Mat4 = Mat4::from_cols_array(&[
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 0.5, 0.5, //
            0.0, 0.0, 0.0, 1.0, //
        ]);
        let fov_y = camera.fov.to_radians();
        let aspect = camera.aspect.get_value();
        let z_near = camera.range.start;
        let z_far = camera.range.end;
        let proj = OPENGL_TO_WGPU_MATRIX * Mat4::perspective_rh(fov_y, aspect, z_near, z_far);
        let view_proj = proj * view;
        let inv_proj = proj.inverse();
        let inv_view = view.inverse();
        let mut cam = CameraUniform::new();
        cam.position = [eye.x, eye.y, eye.z, 1.0];
        cam.view_projection_matrix = view_proj.to_cols_array_2d();
        cam.inverse_projection_matrix = inv_proj.to_cols_array_2d();
        cam.inverse_view_matrix = inv_view.to_cols_array_2d();
        renderer.get_queue().write_buffer(
            self.camera_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&cam),
        );

        // Begin render pass targeting offscreen color
        let color_view = resources
            .gpu()
            .textures
            .get(self.offscreen_color_texture)
            .expect("offscreen color")
            .texture
            .create_view(&wgpu::TextureViewDescriptor {
                label: Some("skybox_offscreen_color_view"),
                ..Default::default()
            });
        log::info!(
            "skybox:draw begin offscreen_view target_format={:?}",
            self.color_format
        );
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        // Bind and draw fullscreen triangle
        rpass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        rpass.set_bind_group(0, self.camera_bind_group.as_ref().unwrap(), &[]);
        rpass.set_bind_group(1, self.env_bind_group.as_ref().unwrap(), &[]);
        rpass.draw(0..3, 0..1);
        // Explicitly end the pass so pass boundaries are clear in logs and ordering.
        drop(rpass);
        log::info!("skybox:draw completed");
        Ok(())
    }
}
