use crate::renderer::{Pass, Renderer};
use crate::resources::RendererTexture;
use anyhow::Result;
use pill_engine::internal::{BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc};
use std::sync::Arc;
use wgpu::CommandEncoder;
use wgpu::Queue;

pub struct PassOverlayDepth {
    label: String,
    depth_texture: Arc<RendererTexture>,
    pipeline: Option<PipelineV2>,
    bind_group_rect: Option<wgpu::BindGroup>,
    bind_group_material: Option<wgpu::BindGroup>,
    tint_buffer: Option<wgpu::Buffer>,
    rect: [f32; 4],
    tint: [f32; 4],
}

impl PassOverlayDepth {
    pub fn new(
        label: &str,
        rect: [f32; 4],
        tint: [f32; 4],
        depth_texture: Arc<RendererTexture>,
    ) -> Self {
        Self {
            label: label.to_string(),
            depth_texture,
            pipeline: None,
            bind_group_rect: None,
            bind_group_material: None,
            tint_buffer: None,
            rect: rect,
            tint: tint,
        }
    }
}

impl Pass for PassOverlayDepth {
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
          @group(1) @binding(0) var<uniform> URect: vec4<f32>; // bottom-left, top-right in [0,1]
  
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
            out.uv = vec2<f32>(s.x, 1.0 - s.y);
            return out;
          }
          "#;

        let fs = r#"
          @group(0) @binding(0) var tex: texture_depth_2d;
          @group(0) @binding(1) var<uniform> UTint: vec4<f32>;
          @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            let dims = textureDimensions(tex, 0u);
            let coord = vec2<i32>(uv * vec2<f32>(dims));
            let d = textureLoad(tex, coord, 0);
            let vis = fract(1000.0*d);
            return vec4<f32>(vis, vis, vis, 1.0) * UTint;
          }
          "#;

        let pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
            label: Some("overlay_depth"),
            vs: ShaderDesc {
                source: vs,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fs,
                entry_func: "main",
            },
            bind_groups: vec![
                // Group 0: Material bindings (texture, tint buffer)
                vec![
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
                            min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                        },
                        count: None,
                    },
                ],
                // Group 1: Rect UBO
                vec![wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                    },
                    count: None,
                }],
            ],
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.get_surface_format(),
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
        })?;

        let bind_group_rect = renderer
            .get_device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("overlay_rect_bind_group"),
                layout: &pipeline.bind_group_layouts[1], // Group 1: Rect UBO
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });

        // Create tint buffer
        let tint_buffer = renderer
            .get_device()
            .create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay_depth_tint"),
                size: 256,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        queue.write_buffer(&tint_buffer, 0, bytemuck::bytes_of(&self.tint));

        // Create material bind group
        let bind_group_material =
            renderer
                .get_device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay_depth_material_bg"),
                    layout: &pipeline.bind_group_layouts[0], // Group 0: Material bindings
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &self.depth_texture.texture_view,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &tint_buffer,
                                offset: 0,
                                size: Some(std::num::NonZeroU64::new(16).unwrap()),
                            }),
                        },
                    ],
                });

        // Store pipeline handle
        self.pipeline = Some(pipeline);
        self.bind_group_rect = Some(bind_group_rect);
        self.bind_group_material = Some(bind_group_material);
        self.tint_buffer = Some(tint_buffer);

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

        // Draw depth overlay (bind material at group 0 and rect at group 1)
        pass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        pass.set_bind_group(0, &self.bind_group_material.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, &self.bind_group_rect.as_ref().unwrap(), &[]);
        pass.draw(0..6, 0..1);

        Ok(())
    }
}

/*
fn create_overlay_depth(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    depth_rt: &RendererTexture,
    rect: [f32; 4],
) -> TextureOverlayResources {
    let overlay_vs_wgsl = r#"
@group(1) @binding(0) var<uniform> URect: vec4<f32>; // bottom-left, top-right in [0,1]

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
        out.uv = vec2<f32>(s.x, 1.0 - s.y);
        return out;
        }
        "#;

    let overlay_fs_wgsl = r#"
@group(0) @binding(0) var tex: texture_depth_2d;
@group(0) @binding(1) var<uniform> UTint: vec4<f32>;
@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let dims = textureDimensions(tex, 0u);
  let coord = vec2<i32>(uv * vec2<f32>(dims));
  let d = textureLoad(tex, coord, 0);
  let vis = fract(100.0*d);
  return vec4<f32>(vis, vis, vis, 1.0) * UTint;
}
"#;

    let overlay_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("overlay_vs"),
        source: wgpu::ShaderSource::Wgsl(overlay_vs_wgsl.into()),
    });
    let overlay_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("overlay_fs"),
        source: wgpu::ShaderSource::Wgsl(overlay_fs_wgsl.into()),
    });

    // Group 0: material (texture + tint)
    let overlay_material_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_logo_material_bgl"),
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
                        min_binding_size: Some(std::num::NonZeroU64::new(16).unwrap()),
                    },
                    count: None,
                },
            ],
        });
    // Group 1: rect UBO
    let overlay_rect_bind_group_layout =
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_rect_bgl"),
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
    let overlay_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("overlay_pl"),
        bind_group_layouts: &[
            &overlay_material_bind_group_layout,
            &overlay_rect_bind_group_layout,
        ],
        push_constant_ranges: &[],
    });

    // No vertex buffer layout needed; vertices are generated procedurally in the vertex shader
    let overlay_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("overlay_pipeline"),
        layout: Some(&overlay_pipeline_layout),
        vertex: wgpu::VertexState {
            module: &overlay_vs,
            entry_point: "main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &overlay_fs,
            entry_point: "main",
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
    });

    // Apple Metal constant buffer alignment (256 bytes)
    let overlay_rect_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay_rect_ubo"),
        size: 256,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&overlay_rect_buffer, 0, bytemuck::bytes_of(&rect));

    let overlay_rect_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("overlay_rect_bg"),
        layout: &overlay_rect_bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &overlay_rect_buffer,
                offset: 0,
                size: Some(std::num::NonZeroU64::new(16).unwrap()),
            }),
        }],
    });

    let tint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay_tint"),
        size: 256,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let tint: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    queue.write_buffer(&tint_buffer, 0, bytemuck::bytes_of(&tint));

    let overlay_material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("overlay_material_bg"),
        layout: &overlay_material_bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&depth_rt.texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &tint_buffer,
                    offset: 0,
                    size: Some(std::num::NonZeroU64::new(16).unwrap()),
                }),
            },
        ],
    });

    TextureOverlayResources {
        pipeline: overlay_pipeline,
        rect_bind_group_layout: overlay_rect_bind_group_layout,
        rect_bind_group: overlay_rect_bind_group,
        rect_buffer: overlay_rect_buffer,
        material_bind_group_layout: overlay_material_bind_group_layout,
        material_bind_group: overlay_material_bind_group,
        material_tint_buffer: tint_buffer,
    }
}

*/
