use anyhow::Result;
use wgpu::CommandEncoder;

use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, RendererTextureHandle, WorldQuery,
};
use pill_core::PillSlotMapKey;

pub struct PassLinearizeDepth {
    label: String,
    pipeline: Option<PipelineV2>,
    bind_group_material: Option<wgpu::BindGroup>,
    near_far_buffer: Option<wgpu::Buffer>,
    target_format: wgpu::TextureFormat,
    // Inputs/outputs
    depth_texture: RendererTextureHandle,
    linear_depth_target: RendererTextureHandle,
}

impl PassLinearizeDepth {
    pub fn new(
        label: &str,
        depth_texture: RendererTextureHandle,
        linear_depth_target: RendererTextureHandle,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            pipeline: None,
            bind_group_material: None,
            near_far_buffer: None,
            target_format,
            depth_texture,
            linear_depth_target,
        }
    }
}

impl Pass for PassLinearizeDepth {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        let device = renderer.get_device();

        // Fullscreen triangle (no vertex buffers)
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
        struct NearFar {
          near: f32,
          far:  f32,
          _pad: vec2<f32>,
        };
        @group(0) @binding(0) var texDepth: texture_depth_2d;
        @group(0) @binding(1) var<uniform> UNearFar: NearFar;

        fn linearizeDepth(depth: f32, near: f32, far: f32) -> f32 {
          // WebGPU 0..1 depth convention
          return (near * far) / (far - depth * (far - near));
        }

        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
          let dims = textureDimensions(texDepth, 0u);
          let coord = vec2<i32>(uv * vec2<f32>(dims));
          let d = textureLoad(texDepth, coord, 0);
          let lin = linearizeDepth(d, UNearFar.near, UNearFar.far);
          // Pack: X = non-linear depth, Y = linear depth, Z = normalized linear depth
          return vec4<f32>(d, lin, lin / UNearFar.far, 1.0);
        }
        "#;

        // Bind group layout: depth texture + near/far UBO
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("linearize_depth_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Depth,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        // 16B to satisfy alignment (near, far, pad)
                        min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("linearize_depth_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("linearize_depth_vs"),
            source: wgpu::ShaderSource::Wgsl(vs.into()),
        });
        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("linearize_depth_fs"),
            source: wgpu::ShaderSource::Wgsl(fs.into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("linearize_depth_pipeline"),
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

        // Material bind group: depth view + UBO (initialized with defaults; updated per-frame)
        let depth_view = resources
            .gpu()
            .textures
            .get(self.depth_texture)
            .expect("scene depth")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let near_far_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("linearize_depth_near_far"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group_material = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("linearize_depth_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: near_far_buffer.as_entire_binding(),
                },
            ],
        });

        self.pipeline = Some(PipelineV2 {
            pipeline,
            bind_group_layouts: vec![bgl],
        });
        self.bind_group_material = Some(bind_group_material);
        self.near_far_buffer = Some(near_far_buffer);

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
        // Update near/far from active camera
        let active_camera_entity_handle = world.active_camera;
        let camera_storage = world
            .camera_components
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let cam = camera_storage.as_ref().unwrap();
        let near_far = [cam.range.start, cam.range.end, 0.0_f32, 0.0_f32];
        renderer.get_queue().write_buffer(
            self.near_far_buffer.as_ref().unwrap(),
            0,
            bytemuck::bytes_of(&near_far),
        );

        // Target view: write to the linear-depth render target
        let out_view = resources
            .gpu()
            .textures
            .get(self.linear_depth_target)
            .expect("linear depth target")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

        pass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        pass.set_bind_group(0, &self.bind_group_material.as_ref().unwrap(), &[]);
        pass.draw(0..3, 0..1);

        Ok(())
    }
}
