// https://github.com/ejb004/egui-wgpu-demo/blob/master/src/lib.rs
// https://github.com/kaphula/winit-egui-wgpu-template/blob/master/src/main.rs
// https://github.com/emilk/egui/discussions/3067

use crate::{
    renderer_resource_storage::RendererResourceStorage,
    resources::{
        RendererCamera, RendererMaterial, RendererMesh, RendererPipeline, RendererTexture, Vertex,
    },
};

use pill_engine::{
    game::{ResourceLoadType, Texture},
    internal::{
        get_renderer_resource_handle_from_camera_component, CameraComponent, ComponentStorage,
        EntityHandle, MaterialParameterMap, MaterialTextureMap, MeshData, PillRenderer,
        RenderQueueItem, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
        RendererPipelineHandle, RendererTextureHandle, TextureType, TransformComponent,
        RENDER_QUEUE_KEY_ORDER,
    },
};

use pill_core::{PillSlotMapKey, PillSlotMapKeyData, PillStyle, RendererError, Timer};

use std::{num::NonZeroU32, sync::Arc};

use cgmath::{Deg, InnerSpace};
use naga::back::wgsl;
use naga::front::glsl;

use anyhow::{Error, Result};
use log::info;

use crate::egui::EguiRenderer;
use image::GenericImageView;
use wgpu::util::DeviceExt;

pub const MAX_INSTANCE_BATCH_SIZE: usize = 10000; // Maximum number of instances that can be drawn in a single draw call
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 10000;
// M2 inline draw: no MeshDrawer/instance batching

// Default resource handle - Master pipeline
pub const MASTER_PIPELINE_HANDLE: RendererPipelineHandle = RendererPipelineHandle {
    0: PillSlotMapKeyData {
        index: 1,
        version: unsafe { std::num::NonZeroU32::new_unchecked(1) },
    },
};

pub struct Renderer {
    pub state: State,
}

fn compile_glsl_to_wgsl(source: &str, stage: naga::ShaderStage) -> Result<String> {
    let mut frontend = glsl::Frontend::default();
    let options = glsl::Options::from(stage);
    let module = frontend.parse(&options, source).unwrap();

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );

    let info = validator.validate(&module)?;

    let mut output = String::new();
    let mut writer = wgsl::Writer::new(&mut output, wgsl::WriterFlags::empty());
    writer.write(&module, &info)?;

    Ok(output)
}

// KISS helper: create a color render target texture+view+sampler
fn create_render_target(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    label: &str,
) -> RendererTexture {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        lod_min_clamp: 0.0,
        lod_max_clamp: 100.0,
        ..Default::default()
    });
    RendererTexture {
        texture,
        texture_view,
        sampler,
    }
}

// Overlay (UV gradient quad) resources grouped for clarity
struct OverlayResources {
    pipeline: wgpu::RenderPipeline,
    rect_bind_group_layout: wgpu::BindGroupLayout,
    rect_bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
}

// KISS helper: build overlay pipeline, UBO layout/buffer, and bind group
fn create_overlay_uv(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    rect: [f32; 4],
) -> OverlayResources {
    let overlay_vs_wgsl = r#"
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
    let overlay_fs_wgsl = r#"
@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  return vec4<f32>(uv, 0.0, 0.5);
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
        bind_group_layouts: &[&overlay_rect_bind_group_layout],
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

    OverlayResources {
        pipeline: overlay_pipeline,
        rect_bind_group_layout: overlay_rect_bind_group_layout,
        rect_bind_group: overlay_rect_bind_group,
        rect_buffer: overlay_rect_buffer,
    }
}

// Overlay (UV gradient quad) resources grouped for clarity
struct TextureOverlayResources {
    pipeline: wgpu::RenderPipeline,
    rect_bind_group_layout: wgpu::BindGroupLayout,
    rect_bind_group: wgpu::BindGroup,
    rect_buffer: wgpu::Buffer,
    // material for textured overlay
    material_bind_group_layout: wgpu::BindGroupLayout,
    material_bind_group: wgpu::BindGroup,
    material_tint_buffer: wgpu::Buffer,
}
// KISS helper: build overlay pipeline, UBO layout/buffer, and bind group
fn create_overlay_logo(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface_format: wgpu::TextureFormat,
    texture_handle: RendererTextureHandle,
    storage: &RendererResourceStorage,
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
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> UTint: vec4<f32>;
@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let c = textureSample(tex, smp, uv);
    return c * UTint;
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

    // Group 0: material (texture + sampler + tint)
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

    let tex = storage
        .textures
        .get(texture_handle)
        .expect("logo texture handle invalid");
    let logo_texture_view = &tex.texture_view;
    let logo_sampler = &tex.sampler;
    let tint_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay_logo_tint"),
        size: 256,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let tint: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
    queue.write_buffer(&tint_buffer, 0, bytemuck::bytes_of(&tint));

    let overlay_material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("overlay_logo_material_bg"),
        layout: &overlay_material_bind_group_layout,
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

impl PillRenderer for Renderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self {
        info!("Initializing {}", "Renderer".mobj_style());
        let state: State = pollster::block_on(State::new(window, config));

        Self { state }
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Resizing {} resources", "Renderer".mobj_style());
        self.state.resize(new_window_size)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data)?;
        let handle = self.state.renderer_resource_storage.meshes.insert(mesh);

        Ok(handle)
    }

    fn create_texture(
        &mut self,
        name: &str,
        image_data: &image::DynamicImage,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_texture(
            &self.state.device,
            &self.state.queue,
            Some(name),
            image_data,
            texture_type,
        )?;
        let handle = self
            .state
            .renderer_resource_storage
            .textures
            .insert(texture);

        Ok(handle)
    }

    fn create_material(
        &mut self,
        name: &str,
        textures: &MaterialTextureMap,
        parameters: &MaterialParameterMap,
    ) -> Result<RendererMaterialHandle> {
        // Create bind group layouts inline (avoid pipeline storage dependency)
        let material_texture_bind_group_layout =
            self.state
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("material_texture_bind_group_layout"),
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
                            ty: wgpu::BindingType::Texture {
                                multisampled: false,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 3,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let material_parameter_bind_group_layout =
            self.state
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("material_parameter_bind_group_layout"),
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

        let material = RendererMaterial::new(
            &self.state.device,
            &self.state.queue,
            &self.state.renderer_resource_storage,
            name,
            MASTER_PIPELINE_HANDLE,
            &material_texture_bind_group_layout,
            textures,
            &material_parameter_bind_group_layout,
            parameters,
        )
        .unwrap();

        let handle = self
            .state
            .renderer_resource_storage
            .materials
            .insert(material);

        Ok(handle)
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        let camera_bind_group_layout =
            self.state
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("camera_bind_group_layout"),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                });
        let camera = RendererCamera::new(&self.state.device, &camera_bind_group_layout)?;
        let handle = self.state.renderer_resource_storage.cameras.insert(camera);

        Ok(handle)
    }

    fn update_material_textures(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        textures: &MaterialTextureMap,
    ) -> Result<()> {
        RendererMaterial::update_textures(
            &self.state.device,
            renderer_material_handle,
            &mut self.state.renderer_resource_storage,
            textures,
        )
    }

    fn update_material_parameters(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        parameters: &MaterialParameterMap,
    ) -> Result<()> {
        RendererMaterial::update_parameters(
            &self.state.device,
            &self.state.queue,
            renderer_material_handle,
            &mut self.state.renderer_resource_storage,
            parameters,
        )
    }

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .meshes
            .remove(renderer_mesh_handle)
            .unwrap();

        Ok(())
    }

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .textures
            .remove(renderer_texture_handle)
            .unwrap();

        Ok(())
    }

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .materials
            .remove(renderer_material_handle)
            .unwrap();

        Ok(())
    }

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .cameras
            .remove(renderer_camera_handle)
            .unwrap();

        Ok(())
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &Vec<RenderQueueItem>,
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn Fn(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<()> {
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage,
            egui_ui,
            timer,
        )
    }

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()> {
        self.state.egui_renderer.handle_input(event);
        Ok(())
    }
}

// KISS helper: build overlay pipeline, UBO layout/buffer, and bind group
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

pub struct State {
    // Resources
    renderer_resource_storage: RendererResourceStorage,
    // Renderer variables
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    window_size: winit::dpi::PhysicalSize<u32>,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    depth_texture: RendererTexture,
    offscreen_color_texture: RendererTexture,
    // Per-frame dynamic UBO ring (Milestone 5)
    per_draw_stride: u64,
    per_draw_capacity: u64, // in elements
    per_draw_buffer: Option<wgpu::Buffer>,
    per_draw_bind_group_layout: wgpu::BindGroupLayout,
    per_draw_bind_group: Option<wgpu::BindGroup>,
    // Prebuilt PSO (static pipeline)
    // [SIMILAR] Prebuilt once; no per-draw pipeline churn per TALK
    default_pipeline: RendererPipeline,
    // Composition (Milestone 6)
    composite_bind_group_layout: wgpu::BindGroupLayout,
    composite_pipeline: wgpu::RenderPipeline,
    composite_bind_group: wgpu::BindGroup,

    // These look like could be combined into OverlayRenderer, but also good case for user submiter pass API
    overlay_uv: OverlayResources, // Overlay (UV gradient quad in top-right)
    overlay_logo: TextureOverlayResources, // Overlay (Logo in bot-right)
    overlay_depth: TextureOverlayResources, // Overlay (Depth in top-right)
    // Other
    config: config::Config,
    egui_renderer: crate::egui::EguiRenderer, // TODO: Separate system adding Pass
}

impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self {
        let window_size = window.inner_size();

        let window_ref = window.clone();

        let backends = wgpu::util::backend_bits_from_env().unwrap_or_default();
        let dx12_shader_compiler = wgpu::util::dx12_shader_compiler_from_env().unwrap_or_default();
        let gles_minor_version = wgpu::util::gles_minor_version_from_env().unwrap_or_default();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            dx12_shader_compiler,
            gles_minor_version,
        });
        let surface = instance.create_surface(window).unwrap();

        // Specify adapter options (Options passed here are not guaranteed to work for all devices)
        let request_adapter_options = wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        };

        // Create adapter
        let adapter = instance
            .request_adapter(&request_adapter_options)
            .await
            .unwrap();
        let adapter_info = adapter.get_info();
        info!(
            "Using GPU: {} ({:?})",
            adapter_info.name, adapter_info.backend
        );

        let features = wgpu::Features::DEPTH_CLIP_CONTROL;

        // Create device descriptor
        let device_descriptor = wgpu::DeviceDescriptor {
            label: None,
            required_features: features, // Allows to specify what extra features of GPU that needs to be included (e.g. depth clamping, push constants, texture compression, etc)
            required_limits: wgpu::Limits::default(), // Allows to specify the limit of certain types of resources that will be used (e.g. max samplers, uniform buffers, etc)
                                                      //memory_hints: wgpu::MemoryHints::MemoryUsage,
        };

        // Create device and queue
        let (device, queue) = adapter
            .request_device(&device_descriptor, None)
            .await
            .unwrap();

        // Specify surface configuration
        let preferred_format = wgpu::TextureFormat::Rgba8UnormSrgb;

        // Get supported present modes and choose the best one
        let surface_caps = surface.get_capabilities(&adapter);
        let present_mode = if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            wgpu::PresentMode::Mailbox
        } else if surface_caps
            .present_modes
            .contains(&wgpu::PresentMode::Immediate)
        {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };

        // Choose the best supported format
        let format = if surface_caps.formats.contains(&preferred_format) {
            preferred_format
        } else if surface_caps
            .formats
            .contains(&wgpu::TextureFormat::Bgra8UnormSrgb)
        {
            wgpu::TextureFormat::Bgra8UnormSrgb
        } else if surface_caps
            .formats
            .contains(&wgpu::TextureFormat::Bgra8Unorm)
        {
            wgpu::TextureFormat::Bgra8Unorm
        } else {
            surface_caps.formats[0] // Use first available format
        };

        // macOS (Retina) note: use physical size (inner_size) and prefer premultiplied alpha if supported
        let alpha_mode = if surface_caps
            .alpha_modes
            .contains(&wgpu::CompositeAlphaMode::PreMultiplied)
        {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            wgpu::CompositeAlphaMode::Auto
        };

        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, // Defines how the swap_chain's underlying textures will be used
            format: format, // Defines how the swap_chain's textures will be stored on the gpu
            width: window_size.width,
            height: window_size.height,
            desired_maximum_frame_latency: 2,
            present_mode: present_mode, // Defines how to sync the surface with the display
            alpha_mode,
            view_formats: vec![format],
        };

        // Configure surface
        surface.configure(&device, &surface_configuration);

        // Configure collections
        let mut renderer_resource_storage = RendererResourceStorage::new(&config);

        // Create depth and color texture
        let depth_texture =
            RendererTexture::new_depth_texture(&device, &surface_configuration, "depth_texture")
                .unwrap();

        // Use Rgba16Float for HDR color buffers; it's the common, well-supported,
        // performant choice. Reserve Rgba32Float for niche cases needing extreme
        // precision and accept the 2× bandwidth/memory hit. If alpha isn't needed,
        // Rg11b10Float/R11G11B10_FLOAT is a fast alternative.
        // Tone-map to the sRGB swapchain in the composite pass.
        let color_format = wgpu::TextureFormat::Rgba16Float;
        let depth_format = wgpu::TextureFormat::Depth32Float;

        // Milestone 6: Create offscreen color target (RENDER_ATTACHMENT | TEXTURE_BINDING)
        let offscreen_color_texture = create_render_target(
            &device,
            color_format,
            surface_configuration.width,
            surface_configuration.height,
            "offscreen_color",
        );

        let egui_renderer =
            EguiRenderer::new(&device, surface_configuration.format, None, 1, window_ref);

        // Build static shader modules and pipeline once ([SIMILAR] prebuilt PSO per TALK)
        let vertex_wgsl = r#"
struct Camera { position: vec4<f32>, view_projection_matrix: mat4x4<f32>, };
@group(0) @binding(0) var<uniform> GCamera: Camera;

struct PerDraw {
  mvp: mat4x4<f32>,
  model: mat4x4<f32>,
  tint: vec4<f32>,
};
@group(3) @binding(0) var<uniform> UPerDraw: PerDraw;

struct VSIn { @location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>, };
struct VSOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, };

@vertex fn main(input: VSIn) -> VSOut {
  var out: VSOut;
  out.pos = UPerDraw.mvp * vec4<f32>(input.pos, 1.0);
  out.uv = input.uv;
  return out;
}
"#;
        let fragment_wgsl = r#"
// Material set(1):
@group(1) @binding(0) var tex_diffuse: texture_2d<f32>;
@group(1) @binding(1) var smp_diffuse: sampler;
@group(1) @binding(2) var tex_normal: texture_2d<f32>;
@group(1) @binding(3) var smp_normal: sampler;

// Material parameters set(2) - pack tint.rgb + specularity in a single vec4
struct MaterialParams { tint_spec: vec4<f32>, }
@group(2) @binding(0) var<uniform> UMaterial: MaterialParams;

// Per-draw (set 3): supports per-entity tint for M4
struct PerDraw {
  mvp: mat4x4<f32>,
  model: mat4x4<f32>,
  tint: vec4<f32>,
};
@group(3) @binding(0) var<uniform> UPerDraw: PerDraw;

@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let albedo = textureSample(tex_diffuse, smp_diffuse, uv);
  let tinted = vec4<f32>(UMaterial.tint_spec.rgb, 1.0) * UPerDraw.tint;
  let spec_boost = 0.5 + 0.5 * UMaterial.tint_spec.a;
  let color = albedo * tinted * spec_boost;
  return vec4<f32>(color.rgb, 1.0);
}
"#;
        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("m2_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("m2_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });
        let default_pipeline = RendererPipeline::new(
            &device,
            vertex_shader,
            fragment_shader,
            color_format,
            Some(depth_format),
            &[RendererMesh::data_layout_descriptor()],
        )
        .unwrap();

        // Milestone 6: Composition pipeline (fullscreen triangle sampling offscreen texture)
        let comp_vs_wgsl = r#"
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
        let comp_fs_wgsl = r#"
@group(0) @binding(0) var t_src: texture_2d<f32>;
@group(0) @binding(1) var s_src: sampler;
@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
  let hdr = textureSample(t_src, s_src, uv).rgb;
  // Reinhard tone mapping; output remains in linear space. The sRGB swapchain will encode.
  let ldr = hdr / (1.0 + hdr);
  return vec4<f32>(ldr, 1.0);
}
"#;
        let comp_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose_vs"),
            source: wgpu::ShaderSource::Wgsl(comp_vs_wgsl.into()),
        });
        let comp_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compose_fs"),
            source: wgpu::ShaderSource::Wgsl(comp_fs_wgsl.into()),
        });
        let composite_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("compose_bgl"),
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
        let comp_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("compose_pl"),
            bind_group_layouts: &[&composite_bind_group_layout],
            push_constant_ranges: &[],
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compose_pipeline"),
            layout: Some(&comp_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &comp_vs,
                entry_point: "main",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &comp_fs,
                entry_point: "main",
                targets: &[Some(wgpu::ColorTargetState {
                    // Composition writes to the swapchain view; target must match surface format
                    format: surface_configuration.format,
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
        let composite_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("compose_bg"),
            layout: &composite_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        &offscreen_color_texture.texture_view,
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&offscreen_color_texture.sampler),
                },
            ],
        });

        // Overlay UV gradient pipeline (small quad in top-right corner)
        let overlay_uv = create_overlay_uv(
            &device,
            &queue,
            surface_configuration.format,
            [0.75, 0.75, 0.95, 0.95],
        );

        // Ideally Client submitted pass via API
        // Create logo texture via renderer API and pass handle into overlay factory
        let logo_image = image::open(
            "/Users/mk/dev/demo/Pill-Engine/engine/pill_renderer/res/pill_logo_horizontal_white.png",
        )
        .expect("failed to load overlay logo image");

        let logo_handle = {
            let tex = RendererTexture::new_texture(
                &device,
                &queue,
                Some("overlay_logo"),
                &logo_image,
                TextureType::Color,
            )
            .expect("failed to create overlay logo texture");
            renderer_resource_storage.textures.insert(tex)
        };
        // Overlay logo pipeline (small quad)
        // res: 1024 × 320, 1.0 x 0.3
        let h: f32 = 0.04;
        let overlay_logo = create_overlay_logo(
            &device,
            &queue,
            surface_configuration.format,
            logo_handle,
            &renderer_resource_storage,
            [0.98 - 3. * h, 0.02, 0.98, 0.02 + h], // bottom right
        );

        let overlay_depth = create_overlay_depth(
            &device,
            &queue,
            surface_configuration.format,
            &depth_texture,
            [0.75, 0.55, 0.95, 0.72], // top right, 2 row
        );

        // Create state
        let per_draw_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("per_draw_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: Some(std::num::NonZeroU64::new(144).unwrap()),
                    },
                    count: None,
                }],
            });
        Self {
            // Resources
            renderer_resource_storage,
            // Renderer variables
            surface,
            device,
            queue,
            surface_configuration,
            window_size,
            color_format,
            depth_format,
            depth_texture,
            offscreen_color_texture,
            // Per-frame UBO ring init
            per_draw_stride: 256,
            per_draw_capacity: 0,
            per_draw_buffer: None,
            // [SIMILAR] Per-draw dynamic UBO layout with has_dynamic_offset=true per TALK (Milestone 5)
            per_draw_bind_group_layout,
            per_draw_bind_group: None,
            // Prebuilt PSO
            default_pipeline,
            // Composition
            composite_bind_group_layout,
            composite_pipeline,
            composite_bind_group,
            // Overlays
            overlay_uv,
            overlay_logo,
            overlay_depth,
            // Other
            config,
            egui_renderer,
        }
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        if new_window_size.width > 0 && new_window_size.height > 0 {
            self.window_size = new_window_size;
            self.surface_configuration.width = new_window_size.width;
            self.surface_configuration.height = new_window_size.height;
            self.surface
                .configure(&self.device, &self.surface_configuration);
            self.depth_texture = RendererTexture::new_depth_texture(
                &self.device,
                &self.surface_configuration,
                "depth_texture",
            )
            .unwrap();
            // Recreate offscreen color target and its bind group
            self.offscreen_color_texture = create_render_target(
                &self.device,
                self.color_format,
                self.surface_configuration.width,
                self.surface_configuration.height,
                "offscreen_color",
            );
            self.composite_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("compose_bg"),
                layout: &self.composite_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &self.offscreen_color_texture.texture_view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(
                            &self.offscreen_color_texture.sampler,
                        ),
                    },
                ],
            });
            // Rebind overlay depth material to the new depth texture view
            self.overlay_depth.material_bind_group =
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay_depth_material_bg"),
                    layout: &self.overlay_depth.material_bind_group_layout,
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
                                buffer: &self.overlay_depth.material_tint_buffer,
                                offset: 0,
                                size: Some(std::num::NonZeroU64::new(16).unwrap()),
                            }),
                        },
                    ],
                });
            // Old offscreen texture/view/sampler are dropped when replaced; wgpu defers actual GPU resource
            // destruction until safe. See optional early reclamation via device.poll:
            // https://docs.rs/wgpu/latest/wgpu/struct.Device.html#method.poll
            self.device.poll(wgpu::Maintain::Wait);
        }
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &Vec<RenderQueueItem>,
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn Fn(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<()> {
        // [SIMILAR] Prebuilt PSO used; avoid hot-path pipeline creation per TALK
        // Use prebuilt default_pipeline for bind group layouts and pipeline
        let _per_draw_bind_group_layout = &self.default_pipeline.per_draw_bind_group_layout;
        // M3: Hello Mesh + per-draw MVP (dynamic offsets)
        timer.record("Frame: acquire");

        // Get frame or return mapped error if failed
        let frame = self.surface.get_current_texture();

        let frame = match frame {
            std::result::Result::Ok(frame) => frame,
            std::result::Result::Err(error) => match error {
                wgpu::SurfaceError::Lost => return Err(RendererError::SurfaceLost.into()),
                wgpu::SurfaceError::OutOfMemory => {
                    return Err(RendererError::SurfaceOutOfMemory.into())
                }
                _ => return Err(RendererError::SurfaceOther.into()),
            },
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        timer.record("Frame: setup view & camera");

        // [SIMILAR] Modify resources (camera UBO) before draw; separates data mods from drawing as per TALK
        // [SIMILAR] Bind group slot indices same as in TALK's convention (0=globals,1=material,2=shader,3=dynamic);
        // Get active camera and update it
        let camera_storage = camera_component_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let active_camera_component = camera_storage.as_ref().unwrap();
        let renderer_camera = self
            .renderer_resource_storage
            .cameras
            .get_mut(get_renderer_resource_handle_from_camera_component(
                active_camera_component,
            ))
            .ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let camera_transform_storage = transform_component_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let active_camera_transform_component = camera_transform_storage.as_ref().unwrap();
        renderer_camera.update(
            &self.queue,
            active_camera_component,
            active_camera_transform_component,
        );
        let renderer_camera = self
            .renderer_resource_storage
            .cameras
            .get(get_renderer_resource_handle_from_camera_component(
                active_camera_component,
            ))
            .unwrap();
        let clear_color = active_camera_component.clear_color;

        // Record inline draw pass (M2)
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("m2_inline_encoder"),
            });

        // [SIMILAR] CPU frustum culling and precomputing per-draw transforms before starting the pass matches TALK's advice
        // [DIFF] Only frustum culling here; TALK discusses 2-pass occlusion and broader CPU-driven pipelines
        // [API->CLIENT] Culling, model matrix building, and visible set construction should live in client/high-level renderer
        // Build view before pass: cull and prepare per-draw MVPs
        let vp_mat: cgmath::Matrix4<f32> = renderer_camera.uniform.view_projection_matrix.into();
        let m = vp_mat;
        let row = |i: usize| -> cgmath::Vector4<f32> {
            // cgmath is column-major; construct row i
            cgmath::Vector4::new(m.x[i], m.y[i], m.z[i], m.w[i])
        };
        let make_plane = |v: cgmath::Vector4<f32>| -> (cgmath::Vector3<f32>, f32) {
            let n = cgmath::Vector3::new(v.x, v.y, v.z);
            let len = n.magnitude();
            let normal = if len > 0.0 { n / len } else { n };
            let d = v.w / if len > 0.0 { len } else { 1.0 };
            (normal, d)
        };
        let planes = [
            make_plane(row(3) + row(0)), // left
            make_plane(row(3) - row(0)), // right
            make_plane(row(3) + row(1)), // bottom
            make_plane(row(3) - row(1)), // top
            make_plane(row(3) + row(2)), // near
            make_plane(row(3) - row(2)), // far
        ];

        struct VisiblePreDraw {
            pipeline_handle: RendererPipelineHandle,
            material_handle: RendererMaterialHandle,
            mesh_handle: RendererMeshHandle,
            entity_index: u32,
            mvp: [[f32; 4]; 4],
        }
        timer.record("Culling & MVP build");
        let mut visible: Vec<VisiblePreDraw> = Vec::with_capacity(render_queue.len());

        for render_queue_item in render_queue.iter() {
            let key = pill_engine::internal::decompose_render_queue_key(render_queue_item.key);
            let mesh_handle = RendererMeshHandle::new(
                key.mesh_index.into(),
                NonZeroU32::new(key.mesh_version.into()).unwrap(),
            );
            let material_handle = RendererMaterialHandle::new(
                key.material_index.into(),
                NonZeroU32::new(key.material_version.into()).unwrap(),
            );
            let material_for_pipeline = self
                .renderer_resource_storage
                .materials
                .get(material_handle)
                .unwrap();
            let pipeline_handle = material_for_pipeline.pipeline_handle;

            // Transform and model
            let entity_index = render_queue_item.entity_index as usize;
            let transform = transform_component_storage
                .data
                .get(entity_index)
                .unwrap()
                .as_ref()
                .unwrap();
            let model_arr = pill_engine::internal::get_model_matrix(transform);
            let model: cgmath::Matrix4<f32> = model_arr.into();

            // World AABB from mesh local AABB
            let mesh = self
                .renderer_resource_storage
                .meshes
                .get(mesh_handle)
                .unwrap();
            let local_min =
                cgmath::Vector3::new(mesh.aabb_min[0], mesh.aabb_min[1], mesh.aabb_min[2]);
            let local_max =
                cgmath::Vector3::new(mesh.aabb_max[0], mesh.aabb_max[1], mesh.aabb_max[2]);
            let corners = [
                cgmath::Vector3::new(local_min.x, local_min.y, local_min.z),
                cgmath::Vector3::new(local_max.x, local_min.y, local_min.z),
                cgmath::Vector3::new(local_min.x, local_max.y, local_min.z),
                cgmath::Vector3::new(local_max.x, local_max.y, local_min.z),
                cgmath::Vector3::new(local_min.x, local_min.y, local_max.z),
                cgmath::Vector3::new(local_max.x, local_min.y, local_max.z),
                cgmath::Vector3::new(local_min.x, local_max.y, local_max.z),
                cgmath::Vector3::new(local_max.x, local_max.y, local_max.z),
            ];
            let mut world_min = cgmath::Vector3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
            let mut world_max =
                cgmath::Vector3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for c in &corners {
                let p4 = model * cgmath::Vector4::new(c.x, c.y, c.z, 1.0);
                let p = cgmath::Vector3::new(p4.x, p4.y, p4.z);
                world_min.x = world_min.x.min(p.x);
                world_min.y = world_min.y.min(p.y);
                world_min.z = world_min.z.min(p.z);
                world_max.x = world_max.x.max(p.x);
                world_max.y = world_max.y.max(p.y);
                world_max.z = world_max.z.max(p.z);
            }
            let mut outside = false;
            for (normal, d) in &planes {
                let p = cgmath::Vector3::new(
                    if normal.x >= 0.0 {
                        world_max.x
                    } else {
                        world_min.x
                    },
                    if normal.y >= 0.0 {
                        world_max.y
                    } else {
                        world_min.y
                    },
                    if normal.z >= 0.0 {
                        world_max.z
                    } else {
                        world_min.z
                    },
                );
                let dist = normal.dot(p) + *d;
                if dist < 0.0 {
                    outside = true;
                    break;
                }
            }
            if outside {
                continue;
            }

            let view_proj: cgmath::Matrix4<f32> =
                renderer_camera.uniform.view_projection_matrix.into();
            let mvp: [[f32; 4]; 4] = (view_proj * model).into();
            visible.push(VisiblePreDraw {
                pipeline_handle,
                material_handle,
                mesh_handle,
                entity_index: render_queue_item.entity_index,
                mvp,
            });
        }

        // [SIMILAR] Sort by pipeline/material/mesh to minimize state changes as in TALK
        // [API->CLIENT] Sorting/grouping is client responsibility; low-level should accept an already ordered draw stream
        // Sort by pipeline -> material -> mesh to minimize state changes
        visible.sort_by_key(|v| {
            (
                v.pipeline_handle.data().index,
                v.material_handle.data().index,
                v.mesh_handle.data().index,
            )
        });

        // Build grouped instancing batches by (pipeline, material, mesh)
        timer.record("Sort & group draws");
        struct MeshBatch {
            mesh_handle: RendererMeshHandle,
            instances: Vec<[[f32; 4]; 4]>,
            base_offset_u32: u32, // offset into per-draw ring for first instance
        }
        struct GroupCmd {
            pipeline_handle: RendererPipelineHandle,
            material_handle: RendererMaterialHandle,
            batches: Vec<MeshBatch>,
        }
        let mut groups: Vec<GroupCmd> = Vec::new();
        for v in &visible {
            if groups
                .last()
                .map(|g| {
                    g.pipeline_handle != v.pipeline_handle || g.material_handle != v.material_handle
                })
                .unwrap_or(true)
            {
                groups.push(GroupCmd {
                    pipeline_handle: v.pipeline_handle,
                    material_handle: v.material_handle,
                    batches: Vec::new(),
                });
            }
            let g = groups.last_mut().unwrap();
            if let Some(batch) = g
                .batches
                .iter_mut()
                .find(|b| b.mesh_handle == v.mesh_handle)
            {
                batch.instances.push(v.mvp);
            } else {
                g.batches.push(MeshBatch {
                    mesh_handle: v.mesh_handle,
                    instances: vec![v.mvp],
                    base_offset_u32: 0,
                });
            }
        }
        let draw_call_count: usize = groups.iter().map(|g| g.batches.len()).sum();
        // Expose draw call count via per-frame timer counters for UI/metrics
        timer.set_counter("draw_calls", draw_call_count as u64);

        // Milestone 5: Per-frame ring buffer
        // [SIMILAR] Batch write dynamic per-draw data once; bind with dynamic offsets
        timer.begin_context("Per-draw UBO setup");
        let needed: u64 = groups
            .iter()
            .map(|g| {
                g.batches
                    .iter()
                    .map(|b| b.instances.len() as u64)
                    .sum::<u64>()
            })
            .sum();
        // total instances; no direct use beyond sanity, avoid unused warning by not binding
        if self.per_draw_capacity < needed {
            self.per_draw_capacity = needed.next_power_of_two().max(1);
            let size = self.per_draw_stride * self.per_draw_capacity;
            self.per_draw_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("per_draw_dynamic_ubo_ring"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
            self.per_draw_bind_group =
                Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("per_draw_bind_group"),
                    layout: &self.default_pipeline.per_draw_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: self.per_draw_buffer.as_ref().unwrap(),
                            offset: 0,
                            size: Some(std::num::NonZeroU64::new(144).unwrap()),
                        }),
                    }],
                }));
        }
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct PerDrawStd140 {
            mvp: [[f32; 4]; 4],
            model: [[f32; 4]; 4],
            tint: [f32; 4],
        }
        unsafe impl bytemuck::Zeroable for PerDrawStd140 {}
        unsafe impl bytemuck::Pod for PerDrawStd140 {}
        let model: [[f32; 4]; 4] = cgmath::Matrix4::<f32>::from_scale(1.0).into();
        let tint: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        let mut staging: Vec<u8> = Vec::with_capacity((self.per_draw_stride * needed) as usize);
        let mut next_offset_u32: u32 = 0;
        for g in groups.iter_mut() {
            for b in g.batches.iter_mut() {
                b.base_offset_u32 = next_offset_u32;
                for mvp in &b.instances {
                    let pd = PerDrawStd140 {
                        mvp: *mvp,
                        model,
                        tint,
                    };
                    staging.extend_from_slice(bytemuck::bytes_of(&pd));
                    let pad =
                        (self.per_draw_stride as usize) - std::mem::size_of::<PerDrawStd140>();
                    staging.extend(std::iter::repeat(0u8).take(pad));
                    next_offset_u32 = next_offset_u32.wrapping_add(self.per_draw_stride as u32);
                }
            }
        }
        if let Some(buf) = &self.per_draw_buffer {
            self.queue.write_buffer(buf, 0, &staging);
        }
        timer.record("Per-draw UBO write (ring)");
        timer.end_context()?;

        {
            timer.begin_context("Offscreen pass");
            // Render scene into offscreen color target (T0)
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("m6_offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.offscreen_color_texture.texture_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color.x as f64,
                            g: clear_color.y as f64,
                            b: clear_color.z as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.texture_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // [SIMILAR] Bind pipeline/material once per group; change only minimal per-draw state (offsets)
            // Instancing: one draw per (pipeline, material, mesh) batch with instance_count
            for group in &groups {
                timer.begin_context(&format!(
                    "Pipeline {} / Material {}",
                    group.pipeline_handle.data().index,
                    group.material_handle.data().index
                ));
                rpass.set_pipeline(&self.default_pipeline.render_pipeline);

                // HOT SoA (per-frame hot path):
                // - RendererMaterial: texture_bind_group, parameter_bind_group
                // - RendererCamera  : bind_group
                // - RendererMesh    : vertex_buffer, index_buffer, index_count
                // COLD metadata (names/descs) lives in *_cold arrays and is not read in the draw loop.

                let material = self
                    .renderer_resource_storage
                    .materials
                    .get(group.material_handle)
                    .unwrap();
                rpass.set_bind_group(0, &renderer_camera.bind_group, &[]);
                rpass.set_bind_group(1, &material.texture_bind_group, &[]);
                rpass.set_bind_group(2, &material.parameter_bind_group, &[]);

                for batch in &group.batches {
                    let mesh = self
                        .renderer_resource_storage
                        .meshes
                        .get(batch.mesh_handle)
                        .unwrap();
                    // [API->CLIENT] Provide packed mesh atlas + per-draw base offsets so low-level can draw without re-binding buffers
                    // [DIFF] VB/IB still rebound per batch; packing meshes could avoid rebinding
                    rpass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    rpass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    // Per-instance draws with dynamic uniform offsets (no shader change required)
                    for i in 0..batch.instances.len() {
                        let offset = batch
                            .base_offset_u32
                            .wrapping_add((i as u32) * (self.per_draw_stride as u32));
                        rpass.set_bind_group(
                            3,
                            self.per_draw_bind_group.as_ref().unwrap(),
                            &[offset],
                        );
                        rpass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                }
                timer.end_context()?; // End pipeline/material group
            }
            timer.record("Encode offscreen pass");
        }
        timer.end_context()?; // End "Offscreen pass"

        // Composition pass: sample T0 and draw to swapchain view, then overlay UV quad
        {
            timer.begin_context("Fullscreen resolve pass");
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("m6_compose_pass"),
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
            rpass.set_pipeline(&self.composite_pipeline);
            rpass.set_bind_group(0, &self.composite_bind_group, &[]);
            rpass.draw(0..3, 0..1);

            timer.record("Encode compose pass");
        }
        timer.end_context()?; // End "Compose pass"

        // Overlay passes
        {
            timer.begin_context("Overlay passes");
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("m6_overlay_passes"),
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
            rpass.set_pipeline(&self.overlay_uv.pipeline);
            rpass.set_bind_group(0, &self.overlay_uv.rect_bind_group, &[]);
            rpass.draw(0..6, 0..1);

            // Draw depth overlay (bind material at group 0 and rect at group 1)
            // Rebind overlay depth material to the new depth texture view
            self.overlay_depth.material_bind_group =
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay_depth_material_bg"),
                    layout: &self.overlay_depth.material_bind_group_layout,
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
                                buffer: &self.overlay_depth.material_tint_buffer,
                                offset: 0,
                                size: Some(std::num::NonZeroU64::new(16).unwrap()),
                            }),
                        },
                    ],
                });
            rpass.set_pipeline(&self.overlay_depth.pipeline);
            rpass.set_bind_group(0, &self.overlay_depth.material_bind_group, &[]);
            rpass.set_bind_group(1, &self.overlay_depth.rect_bind_group, &[]);
            rpass.draw(0..6, 0..1);

            // Draw logo overlay (bind material at group 0 and rect at group 1)
            rpass.set_pipeline(&self.overlay_logo.pipeline);
            rpass.set_bind_group(0, &self.overlay_logo.material_bind_group, &[]);
            rpass.set_bind_group(1, &self.overlay_logo.rect_bind_group, &[]);
            rpass.draw(0..6, 0..1);

            timer.record("Encode overlay passes");
        }
        timer.end_context()?; // End "Compose pass"

        self.queue.submit([encoder.finish()]);
        timer.record("Submit scene+compose passes");

        // Egui overlay (load over the rendered frame)
        timer.begin_context("UI pass");
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui_encoder"),
            });
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.window_size.width, self.window_size.height],
            pixels_per_point: self.egui_renderer.window_scale_factor,
        };
        self.egui_renderer.draw(
            &self.device,
            &self.queue,
            &mut encoder,
            &view,
            screen_descriptor,
            |ctx| egui_ui(ctx),
            timer,
        )?;
        self.queue.submit([encoder.finish()]);
        timer.record("UI draw & submit");
        timer.end_context()?; // End "UI pass"

        // Present frame
        frame.present();
        timer.record("Present");

        // Allow wgpu to process pending work and retire resources referenced by submitted encoders
        // without blocking the CPU. See Device::poll docs.
        self.device.poll(wgpu::Maintain::Poll);

        Ok(())
    }

    // old render path removed (M2 uses inline draws)
}
