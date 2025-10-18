use crate::renderer::{Pass, Renderer};
use anyhow::Result;
use pill_engine::internal::{BufferDesc, PillRenderer, PipelineV2Desc, ShaderDesc};
use wgpu::CommandEncoder;
use wgpu::Queue;

/// A dummy pass implementation for testing and development
pub struct PassOverlayUV {
    label: String,
}

impl PassOverlayUV {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
        }
    }
}

impl Pass for PassOverlayUV {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, queue: &wgpu::Queue, renderer: &Renderer) -> Result<()> {
        println!("Initializing pass: {}", self.label);

        // TODO: create whole overlay_uv pass using factory methods from PillRenderer
        let rect = [0.75, 0.75, 0.95, 0.95];
        let buffer_handle = renderer.create_buffer(BufferDesc {
            label: Some("overlay_rect_ubo"),
            byte_size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })?;

        // Get buffer from RefCell storage
        let storage = renderer.get_resource_storage().borrow();
        let buffer = storage.buffers.get(buffer_handle).unwrap();

        let vs_wgsl = r#"
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

        let fs_wgsl = r#"
          @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            return vec4<f32>(uv, 0.0, 0.5);
          }
          "#;

        queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&rect));

        let _pipeline_handle = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("overlay_uv"),
            vs: ShaderDesc {
                source: vs_wgsl,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs_wgsl,
                entry_func: "main",
            },
            bindings: vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                },
                count: None,
            }],
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.get_surface_format(),
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        Ok(())
    }

    fn draw(&self, encoder: &mut CommandEncoder, renderer: &Renderer) -> Result<()> {
        // println!("Rendering dummy pass: {}", self.label);

        Ok(())
    }
}
