use anyhow::Result;
use wgpu::CommandEncoder;

use crate::graphics::renderer::{
    Pass, PillRenderer as EnginePillRenderer, PipelineV2, RendererTextureHandle, WorldQuery,
};

pub struct PassOverlayDepth {
    label: String,
    pipeline: Option<PipelineV2>,
    bind_group_rect: Option<wgpu::BindGroup>,
    bind_group_material: Option<wgpu::BindGroup>,
    tint_buffer: Option<wgpu::Buffer>,
    rect: [f32; 4],
    tint: [f32; 4],
    target_format: wgpu::TextureFormat,
    depth_texture: RendererTextureHandle,
}

impl PassOverlayDepth {
    pub fn new(
        label: &str,
        rect: [f32; 4],
        tint: [f32; 4],
        target_format: wgpu::TextureFormat,
        depth_texture: RendererTextureHandle,
    ) -> Self {
        Self {
            label: label.to_string(),
            pipeline: None,
            bind_group_rect: None,
            bind_group_material: None,
            tint_buffer: None,
            rect,
            tint,
            target_format,
            depth_texture,
        }
    }
}

impl Pass for PassOverlayDepth {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        let device = renderer.get_device();
        // Create buffer for overlay rect UBO
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_depth_rect_ubo"),
            size: 256,
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

        // Build bind group layouts
        let bgl_material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_depth_material_bgl"),
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
        let bgl_rect = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_depth_rect_bgl"),
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
            label: Some("overlay_depth_pl"),
            bind_group_layouts: &[&bgl_material, &bgl_rect],
            push_constant_ranges: &[],
        });
        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_depth_vs"),
            source: wgpu::ShaderSource::Wgsl(vs.into()),
        });
        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_depth_fs"),
            source: wgpu::ShaderSource::Wgsl(fs.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay_depth_pipeline"),
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

        let bind_group_rect = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_depth_rect_bind_group"),
            layout: &bgl_rect, // Group 1: Rect UBO
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        // Create tint buffer
        let tint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_depth_tint"),
            size: 256,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: true,
        });
        {
            let mut view = tint_buffer.slice(..).get_mapped_range_mut();
            let src = bytemuck::bytes_of(&self.tint);
            view[..src.len()].copy_from_slice(src);
        }
        tint_buffer.unmap();

        // Create material bind group using ResourceManager-provided depth texture view
        let depth_view = resources
            .gpu()
            .textures
            .get(self.depth_texture)
            .expect("depth texture")
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group_material = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_depth_material_bg"),
            layout: &bgl_material, // Group 0: Material bindings
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
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
        self.pipeline = Some(PipelineV2 {
            pipeline,
            bind_group_layouts: vec![bgl_material, bgl_rect],
        });
        self.bind_group_rect = Some(bind_group_rect);
        self.bind_group_material = Some(bind_group_material);
        self.tint_buffer = Some(tint_buffer);

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        _renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        // Create render pass for this pass using the provided frame
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(&self.label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
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
