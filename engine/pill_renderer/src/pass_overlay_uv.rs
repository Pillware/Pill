use crate::renderer::{Pass, Renderer};
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
}

impl PassOverlayUV {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            buffer: None,
            pipeline: None,
            bind_group: None,
            rect: [0.75, 0.75, 0.95, 0.95],
        }
    }
}

impl Pass for PassOverlayUV {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, queue: &wgpu::Queue, renderer: &Renderer) -> Result<()> {
        println!("Initializing pass: {}", self.label);

        // Create buffer for overlay rect UBO
        let buffer = renderer.create_buffer(BufferDesc {
            label: Some("overlay_rect_ubo"),
            byte_size: 4 * 32, // 4 floats * 32 bytes per float = 128 bytes, will be aligned to 256 bytes for Metal UBOs
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })?;
        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&self.rect));

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

        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("overlay_uv"),
            vs: ShaderDesc {
                source: vs,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs,
                entry_func: "main",
            },
            bind_groups: vec![vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                },
                count: None,
            }]],
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.get_surface_format(),
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        let bind_group = renderer
            .get_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay_rect_bind_group"),
                layout: &pipeline.bind_group_layouts[0],
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });

        // Store pipeline handle
        self.pipeline = Some(pipeline);
        self.bind_group = Some(bind_group);

        Ok(())
    }

    fn draw(
        &self,
        encoder: &mut CommandEncoder,
        _renderer: &Renderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
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
