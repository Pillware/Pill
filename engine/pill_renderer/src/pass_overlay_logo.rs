use crate::renderer::{Pass, Renderer, WorldQuery};
use crate::resource_manager::ResourceManager;
use anyhow::Result;
use pill_engine::internal::{
    BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc, RendererTextureHandle, ShaderDesc,
};
use wgpu::CommandEncoder;
use wgpu::Queue;

pub struct PassOverlayLogo {
    label: String,
    tex_logo: RendererTextureHandle,
    buffer: Option<wgpu::Buffer>,
    pipeline: Option<PipelineV2>,
    bind_group_rect: Option<wgpu::BindGroup>,
    bind_group_material: Option<wgpu::BindGroup>,
    rect: [f32; 4],
    tint: [f32; 4],
    target_format: wgpu::TextureFormat,
}

impl PassOverlayLogo {
    pub fn new(
        label: &str,
        rect: [f32; 4],
        tint: [f32; 4],
        tex_logo: RendererTextureHandle,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            label: label.to_string(),
            tex_logo,
            buffer: None,
            pipeline: None,
            bind_group_rect: None,
            bind_group_material: None,
            rect: rect,
            tint: tint,
            target_format,
        }
    }
}

impl Pass for PassOverlayLogo {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, device: &wgpu::Device, _res: &mut ResourceManager) -> Result<()> {
        println!("Initializing pass: {}", self.label);

        // Create buffer for overlay rect UBO
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_logo_ubo"),
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
        @group(0) @binding(0) var tex: texture_2d<f32>;
        @group(0) @binding(1) var smp: sampler;
        @group(0) @binding(2) var<uniform> UTint: vec4<f32>;
        @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
            let c = textureSample(tex, smp, uv);
            return c * UTint;
        }
        "#;

        // Build bind group layouts
        let bgl_material = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay_logo_material_bgl"),
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
            label: Some("overlay_logo_rect_bgl"),
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
            label: Some("overlay_logo_pl"),
            bind_group_layouts: &[&bgl_material, &bgl_rect],
            push_constant_ranges: &[],
        });
        let vs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_logo_vs"),
            source: wgpu::ShaderSource::Wgsl(vs.into()),
        });
        let fs_mod = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay_logo_fs"),
            source: wgpu::ShaderSource::Wgsl(fs.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay_logo_pipeline"),
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
            label: Some("overlay_rect_bind_group"),
            layout: &bgl_rect, // Group 1: Rect UBO
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        let tex = _res
            .textures
            .get(self.tex_logo)
            .expect("logo texture handle invalid");
        let logo_texture_view = &tex.texture_view;
        let logo_sampler = &tex.sampler;
        let tint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay_logo_tint"),
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

        let bind_group_material = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay_logo_material_bg"),
            layout: &bgl_material, // Group 0: Material bindings
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(logo_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(logo_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
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

        pass.set_pipeline(&self.pipeline.as_ref().unwrap().pipeline);
        pass.set_bind_group(0, &self.bind_group_material.as_ref().unwrap(), &[]);
        pass.set_bind_group(1, &self.bind_group_rect.as_ref().unwrap(), &[]);
        pass.draw(0..6, 0..1);

        Ok(())
    }
}
