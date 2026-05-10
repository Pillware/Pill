use crate::{
    ecs::{CameraComponent, ComponentStorage, TransformComponent},
    graphics::{
        Pass, PillRenderer, PipelineV2, PipelineV2Desc, RendererTextureHandle, ShaderDesc,
        WorldQuery,
    },
};
use glam::{Mat3, Vec3};
use pill_core::{PillSlotMapKey, Result};

use crate::config::DEFAULT_EQUIRECT_FALLBACK_PIXEL;

static VERTEX_SHADER: &str = include_str!("../../res/shaders/background_vertex.wgsl");
static FRAGMENT_SHADER: &str = include_str!("../../res/shaders/background_fragment.wgsl");

pub struct PassBackground {
    hdr_target: RendererTextureHandle,
    equirect: Option<RendererTextureHandle>,
    background_color: [f32; 3],
    state: Option<BackgroundState>,
}

struct BackgroundState {
    pipeline: PipelineV2,
    bind_group: wgpu::BindGroup,
    camera_buffer: wgpu::Buffer,
    _sampler: wgpu::Sampler,
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BackgroundCameraUniform {
    right: [f32; 3],
    tangent_half_fov: f32,
    up: [f32; 3],
    aspect: f32,
    forward: [f32; 3],
    _padding: f32,
    background_color: [f32; 3],
    _padding_2: f32,
}

impl PassBackground {
    pub fn new(hdr_target: RendererTextureHandle) -> Self {
        Self {
            hdr_target,
            equirect: None,
            background_color: [1.0, 1.0, 1.0],
            state: None,
        }
    }

    pub fn with_equirect(mut self, handle: RendererTextureHandle) -> Self {
        self.equirect = Some(handle);
        self
    }

    pub fn with_background_color(mut self, color: [f32; 3]) -> Self {
        self.background_color = color;
        self
    }
}

impl Pass for PassBackground {
    fn get_label(&self) -> &str {
        "pass_background"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let bind_groups = vec![vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ]];

        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("pass_background"),
            vs: ShaderDesc {
                source: VERTEX_SHADER,
                entry_func: "vs_main",
            },
            ps: ShaderDesc {
                source: FRAGMENT_SHADER,
                entry_func: "fs_main",
            },
            vertex_buffers: &[],
            bind_groups,
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
                unclipped_depth: false,
            },
        })?;

        // Create equirect view; register a 1×1 Rgba32Float black fallback when no handle is set.
        let equirect_handle = match self.equirect {
            Some(handle) => handle,
            None => renderer.create_texture_from_pixels(
                "equirect_fallback",
                &[DEFAULT_EQUIRECT_FALLBACK_PIXEL],
                1,
                1,
                wgpu::TextureFormat::Rgba32Float,
            ),
        };
        let view = renderer
            .get_texture_view(equirect_handle)
            .expect("equirect handle invalid");

        let sampler = renderer
            .get_device()
            .create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat, // equirect wraps in U
                address_mode_v: wgpu::AddressMode::ClampToEdge, // poles clamp in V
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            });

        let camera_buffer = renderer
            .get_device()
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("pass_background_camera"),
                size: std::mem::size_of::<BackgroundCameraUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

        let bind_group = {
            let layout = &pipeline.bind_group_layouts[0];
            renderer
                .get_device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("pass_background_bind_group"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: camera_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                })
        };

        self.state = Some(BackgroundState {
            pipeline,
            bind_group,
            camera_buffer,
            _sampler: sampler,
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame: &wgpu::SurfaceTexture,
        _view: &wgpu::TextureView,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self.state.as_ref().unwrap();

        // Compute inv_view_proj from active camera — same matrices as PBR pass.
        let camera_components = world.query::<CameraComponent>()?;
        let transform_components = world.query::<TransformComponent>()?;
        let active_camera_index = world.active_camera.data().index as usize;
        let camera = camera_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();
        let transform = transform_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();

        let eye = Vec3::new(
            transform.position.x,
            transform.position.y,
            transform.position.z,
        );
        let forward = if let Some(target) = camera.look_at {
            (Vec3::new(target.x, target.y, target.z) - eye).normalize()
        } else {
            let roll = Mat3::from_rotation_z(transform.rotation.z.to_radians());
            let yaw = Mat3::from_rotation_y(transform.rotation.y.to_radians());
            let pitch = Mat3::from_rotation_x(transform.rotation.x.to_radians());
            (yaw * pitch * roll) * Vec3::Z
        };
        let right = forward.cross(Vec3::Y).normalize();
        let up = right.cross(forward);

        let uniform = BackgroundCameraUniform {
            right: right.to_array(),
            tangent_half_fov: (camera.fov.to_radians() / 2.0).tan(),
            up: up.to_array(),
            aspect: camera.aspect.get_value(),
            forward: forward.to_array(),
            _padding: 0.0,
            background_color: self.background_color,
            _padding_2: 0.0,
        };
        renderer
            .get_queue()
            .write_buffer(&state.camera_buffer, 0, bytemuck::bytes_of(&uniform));

        let hdr_target_view = renderer.get_render_target_view(self.hdr_target).unwrap();
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass_background_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: hdr_target_view,
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
        render_pass.set_pipeline(&state.pipeline.pipeline);
        render_pass.set_bind_group(0, &state.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        Ok(())
    }
}
