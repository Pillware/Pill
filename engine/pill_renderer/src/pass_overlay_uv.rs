use crate::renderer::{Pass, Renderer, WorldQuery};
use crate::resource_manager::ResourceManager;
use anyhow::Result;
use pill_engine::internal::{BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc};
use wgpu::CommandEncoder;
use wgpu::Queue;

pub struct PassOverlayUV {
    label: String,
    buffer: Option<wgpu::Buffer>,
    pipeline: Option<PipelineV2>,
    bind_group: Option<wgpu::BindGroup>,
    rect: [f32; 4],
    target_format: wgpu::TextureFormat,
}

impl PassOverlayUV {
    pub fn new(label: &str, rect: [f32; 4], target_format: wgpu::TextureFormat) -> Self {
        Self {
            label: label.to_string(),
            buffer: None,
            pipeline: None,
            bind_group: None,
            rect: rect,
            target_format,
        }
    }
}

impl Pass for PassOverlayUV {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, renderer: &mut Renderer) -> Result<()> {
        println!("Initializing pass: {}", self.label);
        let device = &renderer.ctx.device;

        // Create buffer for overlay rect UBO
        let aligned_size = 256u64;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_rect_ubo"),
            size: aligned_size,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let mut view = buffer.slice(..).get_mapped_range_mut();
            let src = bytemuck::bytes_of(&self.rect);
            view[..src.len()].copy_from_slice(src);
        }
        buffer.unmap();

        let vs = r#"
          @group(0) @binding(0) var<uniform> URect: vec4<f32>; // bottom-left, top-right in [0,1]
  
          struct VSOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
          @vertex fn main(@builtin(vertex_index) vi: u32) -> VSOut {
            var unit = array<vec2<f32>, 6>(
              vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0),
              vec2<f32>( 1.0,  1.0), vec2<f32>(-1.0,  1.0), vec2<f32>(-1.0, -1.0)
            );
            let p = unit[vi];  // [-1,1] NDC space
            let s = p*0.5+0.5; // [0,1] screen space
            let r = URect.xy + s * (URect.zw - URect.xy); // move by rect [0,1]
            let ndc = r*2.0-1.0; // [-1,1] NDC space
            var out: VSOut;
            out.pos = vec4<f32>(ndc.x, ndc.y, 0.0, 1.0);
            out.uv = s;
            return out;
          }
          "#;

        let fs = r#"
          @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            return vec4<f32>(uv, 0.0, 0.5);
          }
          "#;

        // Build bind group layout and pipeline
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_uv_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay_uv_pl"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_uv_vs"),
            source: wgpu::ShaderSource::Wgsl(vs.into()),
        });
        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_uv_fs"),
            source: wgpu::ShaderSource::Wgsl(fs.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay_uv_pipeline"),
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
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_rect_bind_group"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        // Store pipeline handle
        self.pipeline = Some(PipelineV2 {
            pipeline,
            bind_group_layouts: vec![bgl],
        });
        self.bind_group = Some(bind_group);

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        _renderer: &mut Renderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        // Create render pass for this pass using the provided frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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

        // Draw small overlay UV quad in normalized rect via UBO
        pass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        pass.set_bind_group(0, self.bind_group.as_ref().unwrap(), &[]);
        pass.draw(0..6, 0..1);

        Ok(())
    }
}
