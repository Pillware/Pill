use crate::renderer::{Pass, Renderer};
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

    fn init(&mut self, _queue: &wgpu::Queue, pr: &mut Renderer) {
        println!("Building pass: {}", self.label);

        let _rect = [0.75, 0.75, 0.95, 0.95];
        let _h_buffer = pr.create_buffer(BufferDesc {
            label: Some("overlay_rect_ubo"),
            byte_size: 256,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        // TODO: Get actual buffer from handle
        // queue.write_buffer(&h_buffer.unwrap(), 0, bytemuck::bytes_of(&rect));

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

        let _pipeline_handle = pr.create_pipeline_v2(PipelineV2Desc {
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
                format: wgpu::TextureFormat::Bgra8UnormSrgb, // TODO: Get actual surface format
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        });
    }

    fn render(&self, _encoder: &mut CommandEncoder) -> Result<(), Box<dyn std::error::Error>> {
        println!("Rendering dummy pass: {}", self.label);
        Ok(())
    }
}
