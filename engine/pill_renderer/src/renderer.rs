// https://github.com/ejb004/egui-wgpu-demo/blob/master/src/lib.rs
// https://github.com/kaphula/winit-egui-wgpu-template/blob/master/src/main.rs
// https://github.com/emilk/egui/discussions/3067

use crate::{
    resource_manager::ResourceManager,
    resources::{
        RendererCamera, RendererMaterial, RendererMaterialParamsStd140, RendererMaterialTextures,
        RendererMesh, RendererPipeline, RendererTexture, Vertex,
    },
};

use std::sync::Arc;

use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, BufferDesc, CameraComponent,
    ComponentStorage, EntityHandle, MaterialDesc, MeshData, Pass, PillRenderer, PipelineV2,
    PipelineV2Desc, RenderQueueItem, RendererBufferHandle, RendererCameraHandle,
    RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle, RendererPipelineV2Handle,
    RendererTargetDesc, RendererTextureHandle, ShaderDesc, TextureType, TransformComponent,
    WorldQuery, RENDER_QUEUE_KEY_ORDER,
};

use pill_core::{Handle, PillSlotMapKey, PillStyle, RendererError, Timer};

use std::num::NonZeroU32;

use cgmath::{Deg, InnerSpace};
use glam::{Mat4, Vec3, Vec4};
use naga::back::wgsl;
use naga::front::glsl;

use anyhow::{Error, Result};
use log::{error, info};

use crate::egui::EguiRenderer;
use image::GenericImageView;
use wgpu::util::DeviceExt;

pub const MAX_INSTANCE_BATCH_SIZE: usize = 10000; // Maximum number of instances that can be drawn in a single draw call
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 10000;
// M2 inline draw: no MeshDrawer/instance batching

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

pub struct DeviceContext {
    pub(crate) config: config::Config,
    // Window
    pub(crate) window_ref: Arc<winit::window::Window>,
    pub(crate) window_size: winit::dpi::PhysicalSize<u32>,
    // GPU API
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) surface: wgpu::Surface<'static>,
    // Framebuffer variables
    pub(crate) surface_configuration: wgpu::SurfaceConfiguration,
}

pub struct State {
    passes: Vec<Box<dyn Pass>>,
    egui_renderer: crate::egui::EguiRenderer, // TODO: Separate system adding Pass
    // Resources and GPU objects moved from ctor into here explicitly
    pub(crate) resource_manager: ResourceManager,
    pub(crate) color_format: wgpu::TextureFormat,
    pub(crate) depth_format: wgpu::TextureFormat,
    // pub(crate) depth_texture: Arc<RendererTexture>,
    // pub(crate) offscreen_color_texture: Arc<RendererTexture>,
    // Prebuilt PSO handle
    // [SIMILAR] Prebuilt once; no per-draw pipeline churn per TALK
}

pub struct Renderer {
    pub state: State,
    pub ctx: DeviceContext,
}

impl PillRenderer for Renderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self {
        info!("Initializing {}", "Renderer".mobj_style());
        let ctx: DeviceContext = pollster::block_on(async move {
            let window_size = window.inner_size();
            let window_ref = window.clone();

            let backends = wgpu::util::backend_bits_from_env().unwrap_or_default();
            let dx12_shader_compiler =
                wgpu::util::dx12_shader_compiler_from_env().unwrap_or_default();
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

            DeviceContext {
                config,
                window_ref,
                window_size,
                device,
                queue,
                surface,
                surface_configuration,
            }
        });

        let state: State = {
            // Configure collections
            let mut resource_manager = ResourceManager::new();

            // Create depth and color texture
            // let depth_texture = RendererTexture::new_depth_texture(
            //     &ctx.device,
            //     &ctx.surface_configuration,
            //     "depth_texture",
            // )
            // .unwrap();

            // Use Rgba16Float for HDR color buffers; it's the common, well-supported,
            // performant choice. Reserve Rgba32Float for niche cases needing extreme
            // precision and accept the 2× bandwidth/memory hit. If alpha isn't needed,
            // Rg11b10Float/R11G11B10_FLOAT is a fast alternative.
            // Tone-map to the sRGB swapchain in the composite pass.
            let color_format = wgpu::TextureFormat::Rgba16Float;
            let depth_format = wgpu::TextureFormat::Depth32Float;

            // Milestone 6: Create offscreen color target (RENDER_ATTACHMENT | TEXTURE_BINDING)
            // let offscreen_color_texture = Arc::new(create_render_target(
            //     &ctx.device,
            //     color_format,
            //     ctx.surface_configuration.width,
            //     ctx.surface_configuration.height,
            //     "offscreen_color",
            // ));

            let egui_renderer = EguiRenderer::new(
                &ctx.device,
                ctx.surface_configuration.format,
                None,
                1,
                ctx.window_ref.clone(),
            );

            // Create state

            State {
                passes: vec![],
                // Other
                egui_renderer,
                // Resources
                resource_manager,
                // Renderer variables
                color_format,
                depth_format,
                // depth_texture: Arc::new(depth_texture),
                // offscreen_color_texture,
                // Scene pass owns per-draw ring and working buffers now
            }
        };
        Self { state, ctx }
    }

    fn init(&mut self) -> Result<()> {
        // Ensure default textures are created before any pass init that may need them
        self.state
            .resource_manager
            .ensure_default_textures(&self.ctx.device, &self.ctx.queue);
        Ok(())
    }

    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer> {
        let aligned_size = ((desc.byte_size + 64) / 64) * 64; // 64B for Metal UBOs
        let buffer = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: desc.label,
            size: aligned_size,
            usage: desc.usage,
            mapped_at_creation: false,
        });
        // let handle = unsafe {
        //     let storage = &mut *self.state.renderer_resource_storage.as_ptr();
        //     storage.buffers.insert(buffer)
        // };
        Ok(buffer)
    }

    fn create_pipeline_v2(&mut self, desc: PipelineV2Desc) -> Result<PipelineV2> {
        /*
        // Example shader pipeline descriptor for factory method
        n_shader = rm->createShader（｛
            .debugName = "mesh_simple",
            .VS (.byteCode = shaderVS, .entryFunc="main"),
            .PS (.byteCode = shaderPS, . entryFunc="main"),
            .CS (.byteCode = shaderCS, . entryFunc="main"),
            .bindGroups = {
                { m_globalsBindingsLayout }, // Globals bind group (0)
                { materialBindingsLayout }, // Material bind group (1)
            },
            .dynamicBuffers = dynamicBindings-getLayout(),
            .graphicsState = {
                .depthTest = COMPARE::GREATER_OR_EQUAL, // inverse Z
                .vertexBufferBindings = {
                    // Position vertex buffer (8)
                    .byteStride = 12, .attributes = {
                        { .byteOffset = 0, .format = FORMAT::RGB32_FLOAT }
                    }
                },
                {
                    // 2nd vertex buffer: tangent, normal, color, texcoord
                    .byteStride = 24, .attributes = {
                        { .byteOffset = 0, .format = FORMAT::RGBA16_FLOAT },
                        { .byteOffset = 8, .format = FORMAT::RGBA16_FLOAT },
                        { .byteOffset = 16, .format = FORMAT::RGBA8_UNORM },
                        { .byteOffset = 20, .format = FORMAT::RG16_FLOAT }
                    }
                }
            },
            .renderPassLayout = m_renderPassLayout
        });
        */

        let vs_label_owned = format!("{}{}", desc.label.unwrap_or("program"), "_vs");
        let vs_label = vs_label_owned.as_str();
        let vs = self
            .ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(vs_label),
                source: wgpu::ShaderSource::Wgsl(desc.vs.source.into()),
            });

        let ps_label_owned = format!("{}{}", desc.label.unwrap_or("program"), "_ps");
        let ps_label = ps_label_owned.as_str();
        let ps = self
            .ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(ps_label),
                source: wgpu::ShaderSource::Wgsl(desc.ps.source.into()),
            });

        // Create multiple bind group layouts
        let mut bind_group_layouts = Vec::new();
        for (i, bindings) in desc.bind_groups.iter().enumerate() {
            let layout =
                self.ctx
                    .device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(
                            format!("{}{}{}", desc.label.unwrap_or("program"), "_bgl_", i).as_str(),
                        ),
                        entries: bindings,
                    });
            bind_group_layouts.push(layout);
        }

        // Create pipeline layout with all bind groups
        let pipeline_layout =
            self.ctx
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("overlay_pl"),
                    bind_group_layouts: &bind_group_layouts.iter().collect::<Vec<_>>(),
                    push_constant_ranges: &[],
                });

        let pl_label_owned = format!("{}{}", desc.label.unwrap_or("program"), "_pipeline");
        let pl_label = pl_label_owned.as_str();
        let pipeline = self
            .ctx
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(pl_label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vs,
                    entry_point: desc.vs.entry_func,
                    buffers: &[crate::resources::RendererMesh::data_layout_descriptor()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &ps,
                    entry_point: desc.ps.entry_func,
                    targets: &desc.targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: Some(wgpu::Face::Back),
                    ..wgpu::PrimitiveState::default()
                },
                depth_stencil: desc.depth_stencil,
                multisample: desc.multisample,
                multiview: None,
            });

        Ok(PipelineV2 {
            pipeline,
            bind_group_layouts,
        })
    }

    fn create_material(&mut self, desc: MaterialDesc) -> Result<RendererMaterialHandle> {
        // Ensure default textures exist
        let (def_color_h, def_normal_h) = self
            .state
            .resource_manager
            .ensure_default_textures(&self.ctx.device, &self.ctx.queue);
        let def_color = self
            .state
            .resource_manager
            .textures
            .get(def_color_h)
            .expect("default color");
        let def_normal = self
            .state
            .resource_manager
            .textures
            .get(def_normal_h)
            .expect("default normal");

        // Create bind group layouts matching PassScene
        let textures_bgl =
            self.ctx
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("material_textures_bgl"),
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
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                multisampled: false,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 5,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 6,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                multisampled: false,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 7,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let params_bgl =
            self.ctx
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("material_params_bgl"),
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

        // Build params from desc
        let params = RendererMaterialParamsStd140 {
            albedo: desc.albedo,
            _pad0: 0.0,
            metallic: desc.metallic,
            roughness: desc.roughness,
            _pad1: [0.0, 0.0],
            emissive: desc.emissive,
            _pad2: 0.0,
        };

        // Build params buffer + bind group
        let param_buffer = self
            .ctx
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{}_material_params", desc.label)),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let param_bind_group = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &params_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: param_buffer.as_entire_binding(),
                }],
                label: Some(&format!("{}_material_params_bg", desc.label)),
            });
        // Resolve textures (use defaults if None)
        let albedo_tex = desc
            .albedo_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);
        let normal_tex = desc
            .normal_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_normal);
        let mr_tex = desc
            .metallic_roughness_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);
        let emissive_tex = desc
            .emissive_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);

        // Build texture bind group
        let texture_bind_group = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &textures_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&albedo_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&albedo_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&normal_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&normal_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&mr_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&mr_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&emissive_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&emissive_tex.sampler),
                    },
                ],
                label: Some(&format!("{}_material_textures_bg", desc.label)),
            });
        let mat = RendererMaterial {
            name: desc.label.to_string(),
            texture_bind_group,
            param_buffer,
            param_bind_group,
        };
        let handle = self.state.resource_manager.materials.insert(mat);
        Ok(handle)
    }

    fn update_material(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        desc: MaterialDesc,
    ) -> Result<RendererMaterialHandle> {
        // Prepare params struct
        let params = RendererMaterialParamsStd140 {
            albedo: desc.albedo,
            _pad0: 0.0,
            metallic: desc.metallic,
            roughness: desc.roughness,
            _pad1: [0.0, 0.0],
            emissive: desc.emissive,
            _pad2: 0.0,
        };

        // Resolve textures without holding a mutable borrow to materials
        let (def_color_h, def_normal_h) = self
            .state
            .resource_manager
            .ensure_default_textures(&self.ctx.device, &self.ctx.queue);
        let def_color = self
            .state
            .resource_manager
            .textures
            .get(def_color_h)
            .unwrap();
        let def_normal = self
            .state
            .resource_manager
            .textures
            .get(def_normal_h)
            .unwrap();
        let albedo_tex = desc
            .albedo_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);
        let normal_tex = desc
            .normal_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_normal);
        let mr_tex = desc
            .metallic_roughness_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);
        let emissive_tex = desc
            .emissive_tex
            .and_then(|h| self.state.resource_manager.textures.get(h))
            .unwrap_or(def_color);

        // Create a compatible layout and new texture bind group
        let textures_bgl =
            self.ctx
                .device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("material_textures_bgl_update"),
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
                        wgpu::BindGroupLayoutEntry {
                            binding: 4,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                multisampled: false,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 5,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 6,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                multisampled: false,
                                view_dimension: wgpu::TextureViewDimension::D2,
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 7,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
        let new_texture_bg = self
            .ctx
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &textures_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&albedo_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&albedo_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&normal_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&normal_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&mr_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&mr_tex.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&emissive_tex.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::Sampler(&emissive_tex.sampler),
                    },
                ],
                label: Some(&format!("{}_material_textures_bg", desc.label)),
            });

        // Mutate material once
        if let Some(mat) = self
            .state
            .resource_manager
            .materials
            .get_mut(renderer_material_handle)
        {
            self.ctx
                .queue
                .write_buffer(&mat.param_buffer, 0, bytemuck::bytes_of(&params));
            mat.texture_bind_group = new_texture_bg;
        }
        Ok(renderer_material_handle)
    }

    fn create_render_target(&mut self, desc: RendererTargetDesc) -> Result<RendererTextureHandle> {
        let tex = create_render_target(
            &self.ctx.device,
            desc.format,
            desc.width,
            desc.height,
            desc.name.as_str(),
        );
        let handle = self.state.resource_manager.textures.insert(tex);
        Ok(handle)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        let mesh = RendererMesh::new(&self.ctx.device, name, mesh_data)?;
        let handle = self.state.resource_manager.meshes.insert(mesh);

        Ok(handle)
    }

    fn create_texture(
        &mut self,
        name: &str,
        image_data: &image::DynamicImage,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_texture(
            &self.ctx.device,
            &self.ctx.queue,
            Some(name),
            image_data,
            texture_type,
        )?;
        let handle = self.state.resource_manager.textures.insert(texture);

        Ok(handle)
    }

    fn create_depth_texture(&mut self, label: &str) -> Result<RendererTextureHandle> {
        let tex = RendererTexture::new_depth_texture(
            &self.ctx.device,
            &self.ctx.surface_configuration,
            label,
        )?;
        Ok(self.state.resource_manager.textures.insert(tex))
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        let camera_bind_group_layout =
            self.ctx
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
        let camera = RendererCamera::new(&self.ctx.device, &camera_bind_group_layout)?;
        let handle = self.state.resource_manager.cameras.insert(camera);

        Ok(handle)
    }

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        self.state
            .resource_manager
            .meshes
            .remove(renderer_mesh_handle)
            .unwrap();

        Ok(())
    }

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        self.state
            .resource_manager
            .textures
            .remove(renderer_texture_handle)
            .unwrap();

        Ok(())
    }

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        self.state
            .resource_manager
            .cameras
            .remove(renderer_camera_handle)
            .unwrap();

        Ok(())
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Resizing {} resources", "Renderer".mobj_style());
        // self.state.resize(new_window_size)
        if new_window_size.width > 0 && new_window_size.height > 0 {
            self.ctx.window_size = new_window_size;
            self.ctx.surface_configuration.width = new_window_size.width;
            self.ctx.surface_configuration.height = new_window_size.height;
            self.ctx
                .surface
                .configure(&self.ctx.device, &self.ctx.surface_configuration);
            // self.state.depth_texture = Arc::new(
            //     RendererTexture::new_depth_texture(
            //         &self.ctx.device,
            //         &self.ctx.surface_configuration,
            //         "depth_texture",
            //     )
            //     .unwrap(),
            // );

            // ================================
            // Recreate offscreen color target and reinitialize passes
            // self.state.offscreen_color_texture = Arc::new(create_render_target(
            //     &self.ctx.device,
            //     self.state.color_format,
            //     self.ctx.surface_configuration.width,
            //     self.ctx.surface_configuration.height,
            //     "offscreen_color",
            // ));
            // Reinitialize existing passes in place (engine may have set custom passes)
            let mut passes = std::mem::take(&mut self.state.passes);
            for pass in passes.iter_mut() {
                let _ = pass.init(self);
            }
            self.state.passes = passes;
            // Old offscreen texture/view/sampler are dropped when replaced; wgpu defers actual GPU resource
            // destruction until safe. See optional early reclamation via device.poll:
            // https://docs.rs/wgpu/latest/wgpu/struct.Device.html#method.poll
            self.ctx.device.poll(wgpu::Maintain::Wait);
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
        timer.record("Frame: acquire");

        // Get frame or return mapped error if failed
        // ALLOCATION: Surface texture allocation (GPU memory) - ~4MB for 1920x1080 RGBA8
        let frame = self.ctx.surface.get_current_texture().unwrap();

        // ALLOCATION: TextureView creation (GPU memory) - ~64 bytes metadata
        let view: wgpu::TextureView = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        timer.record("Frame: setup view");

        // Scene draw is now a Pass; remaining work is handled in user passes encoder below

        {
            // User passes with separate encoder
            timer.begin_context("User passes");
            // ALLOCATION: CommandEncoder creation (CPU memory) - ~1KB command buffer
            let mut encoder =
                self.ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("user_passes"),
                    });

            // Loop over state.passes, for each
            // Construct world query bundle for passes
            let world_q = WorldQuery {
                active_camera: active_camera_entity_handle,
                render_queue,
                camera_components: camera_component_storage,
                transform_components: transform_component_storage,
            };
            let mut passes = std::mem::take(&mut self.state.passes);
            for pass in passes.iter_mut() {
                let label_owned = pass.get_label().to_string();
                timer.begin_context(&label_owned);
                let _ = pass.draw(&mut encoder, self, &frame, &view, &world_q);
                timer.record(&label_owned);
            }
            self.state.passes = passes;

            self.ctx.queue.submit([encoder.finish()]);
            timer.record("User passes submit");
            timer.end_context()?; // End "User passes"
        }

        // Egui overlay (load over the rendered frame)
        timer.begin_context("UI pass");
        // ALLOCATION: CommandEncoder creation (CPU memory) - ~1KB command buffer
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("egui_encoder"),
            });
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.ctx.window_size.width, self.ctx.window_size.height],
            pixels_per_point: self.state.egui_renderer.window_scale_factor,
        };
        self.state.egui_renderer.draw(
            &self.ctx.device,
            &self.ctx.queue,
            &mut encoder,
            &view,
            screen_descriptor,
            |ctx| egui_ui(ctx),
            timer,
        )?;
        self.ctx.queue.submit([encoder.finish()]);
        timer.record("UI draw & submit");
        timer.end_context()?; // End "UI pass"

        // Present frame
        frame.present();
        timer.record("Present");

        // Allow wgpu to process pending work and retire resources referenced by submitted encoders
        // without blocking the CPU. See Device::poll docs.
        self.ctx.device.poll(wgpu::Maintain::Poll);

        Ok(())
    }

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()> {
        self.state.egui_renderer.handle_input(event);
        Ok(())
    }

    // --- Engine pass management and helpers ---
    fn set_passes(&mut self, mut passes: Vec<Box<dyn Pass>>) -> Result<()> {
        for pass in passes.iter_mut() {
            let _ = pass.init(self);
        }
        self.state.passes = passes;
        Ok(())
    }

    fn get_surface_format(&self) -> wgpu::TextureFormat {
        self.ctx.surface_configuration.format
    }

    fn get_device(&self) -> &wgpu::Device {
        &self.ctx.device
    }

    fn get_texture(&self, h: RendererTextureHandle) -> &wgpu::Texture {
        let tex = self
            .state
            .resource_manager
            .textures
            .get(h)
            .expect("texture");
        &tex.texture
    }

    fn get_mesh_buffers_and_count(
        &self,
        h: RendererMeshHandle,
    ) -> (&wgpu::Buffer, &wgpu::Buffer, u32) {
        let mesh = self.state.resource_manager.meshes.get(h).expect("mesh");
        (&mesh.vertex_buffer, &mesh.index_buffer, mesh.index_count)
    }

    fn get_material_texture_bind_group(&self, h: RendererMaterialHandle) -> &wgpu::BindGroup {
        let mat = self
            .state
            .resource_manager
            .materials
            .get(h)
            .expect("material");
        &mat.texture_bind_group
    }

    fn get_material_params_bind_group(&self, h: RendererMaterialHandle) -> &wgpu::BindGroup {
        let mat = self
            .state
            .resource_manager
            .materials
            .get(h)
            .expect("material");
        &mat.param_bind_group
    }

    fn get_queue(&self) -> &wgpu::Queue {
        &self.ctx.queue
    }
}

impl Renderer {
    pub fn get_surface_format(&self) -> wgpu::TextureFormat {
        self.ctx.surface_configuration.format
    }

    pub fn get_device(&self) -> &wgpu::Device {
        &self.ctx.device
    }

    pub fn get_surface(&self) -> &wgpu::Surface<'_> {
        &self.ctx.surface
    }

    // Lightweight resolve helpers for engine passes
    pub fn get_texture_view(&self, h: RendererTextureHandle) -> &wgpu::TextureView {
        let tex = self
            .state
            .resource_manager
            .textures
            .get(h)
            .expect("texture");
        &tex.texture_view
    }

    pub fn get_texture_sampler(&self, h: RendererTextureHandle) -> &wgpu::Sampler {
        let tex = self
            .state
            .resource_manager
            .textures
            .get(h)
            .expect("texture");
        &tex.sampler
    }
}
