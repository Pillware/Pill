// https://github.com/ejb004/egui-wgpu-demo/blob/master/src/lib.rs
// https://github.com/kaphula/winit-egui-wgpu-template/blob/master/src/main.rs
// https://github.com/emilk/egui/discussions/3067

use crate::{
    pass_compose::PassCompose,
    pass_overlay_depth::PassOverlayDepth,
    pass_overlay_logo::PassOverlayLogo,
    pass_overlay_uv::PassOverlayUV,
    resource_manager::ResourceManager,
    resources::{
        RendererCamera, RendererMaterial, RendererMesh, RendererPipeline, RendererTexture, Vertex,
    },
};

use std::sync::Arc;

use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, BufferDesc, CameraComponent,
    ComponentStorage, EntityHandle, MaterialParameterMap, MaterialTextureMap, MeshData,
    PillRenderer, PipelineV2, PipelineV2Desc, RenderQueueItem, RendererBufferHandle,
    RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle,
    RendererPipelineV2Handle, RendererTextureHandle, ShaderDesc, TextureType, TransformComponent,
    RENDER_QUEUE_KEY_ORDER,
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

// Default resource handle - Master pipeline
pub const MASTER_PIPELINE_HANDLE: RendererPipelineHandle = Handle::INVALID;

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

pub trait Pass {
    fn get_label(&self) -> &str;
    fn init(
        &mut self,
        device: &wgpu::Device,
        res: &mut crate::resource_manager::ResourceManager,
    ) -> Result<()>;
    fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &Renderer,
        frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
    ) -> Result<()>;
}

pub struct DeviceContext {
    config: config::Config,
    // Window
    window_ref: Arc<winit::window::Window>,
    window_size: winit::dpi::PhysicalSize<u32>,
    // GPU API
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    // Framebuffer variables
    surface_configuration: wgpu::SurfaceConfiguration,
}

// Preallocated buffer structs for hot path optimization
struct VisiblePreDraw {
    pipeline_handle: RendererPipelineHandle,
    material_handle: RendererMaterialHandle,
    mesh_handle: RendererMeshHandle,
    entity_index: u32,
    mvp: [[f32; 4]; 4],
}

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

pub struct State {
    passes: Vec<Box<dyn Pass>>,
    egui_renderer: crate::egui::EguiRenderer, // TODO: Separate system adding Pass
    // Resources and GPU objects moved from ctor into here explicitly
    resource_manager: ResourceManager,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    depth_texture: Arc<RendererTexture>,
    offscreen_color_texture: Arc<RendererTexture>,
    // Per-frame dynamic UBO ring (Milestone 5)
    per_draw_stride: u64,
    per_draw_capacity: u64, // in elements
    per_draw_buffer: Option<wgpu::Buffer>,
    per_draw_bind_group_layout: wgpu::BindGroupLayout,
    per_draw_bind_group: Option<wgpu::BindGroup>,
    // Prebuilt PSO handle
    // [SIMILAR] Prebuilt once; no per-draw pipeline churn per TALK
    master_pipeline_handle: RendererPipelineHandle,
    // resources
    tex_logo: RendererTextureHandle,
    // Preallocated buffers for hot path optimization
    visible_pre_draw_buffer: Vec<VisiblePreDraw>,
    groups_buffer: Vec<GroupCmd>,
    staging_buffer: Vec<u8>,
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
            let depth_texture = RendererTexture::new_depth_texture(
                &ctx.device,
                &ctx.surface_configuration,
                "depth_texture",
            )
            .unwrap();

            // Use Rgba16Float for HDR color buffers; it's the common, well-supported,
            // performant choice. Reserve Rgba32Float for niche cases needing extreme
            // precision and accept the 2× bandwidth/memory hit. If alpha isn't needed,
            // Rg11b10Float/R11G11B10_FLOAT is a fast alternative.
            // Tone-map to the sRGB swapchain in the composite pass.
            let color_format = wgpu::TextureFormat::Rgba16Float;
            let depth_format = wgpu::TextureFormat::Depth32Float;

            // Milestone 6: Create offscreen color target (RENDER_ATTACHMENT | TEXTURE_BINDING)
            let offscreen_color_texture = Arc::new(create_render_target(
                &ctx.device,
                color_format,
                ctx.surface_configuration.width,
                ctx.surface_configuration.height,
                "offscreen_color",
            ));

            let egui_renderer = EguiRenderer::new(
                &ctx.device,
                ctx.surface_configuration.format,
                None,
                1,
                ctx.window_ref.clone(),
            );

            // Build static shader modules and pipeline once ([SIMILAR] prebuilt PSO per TALK)
            let vertex_wgsl = r#"
            struct Camera { position: vec4<f32>, view_projection_matrix: mat4x4<f32>, };
            @group(0) @binding(0) var<uniform> GCamera: Camera;

            struct PerDraw {
              mvp: mat4x4<f32>,
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
            let vertex_shader = ctx
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("m2_vertex_shader"),
                    source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
                });
            let fragment_shader = ctx
                .device
                .create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("m2_fragment_shader"),
                    source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
                });
            let default_pipeline = RendererPipeline::new(
                &ctx.device,
                vertex_shader,
                fragment_shader,
                color_format,
                Some(depth_format),
                &[RendererMesh::data_layout_descriptor()],
            )
            .unwrap();
            let master_handle = resource_manager.pipelines.insert(default_pipeline);

            // Create state
            let per_draw_bind_group_layout =
                ctx.device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some("per_draw_bind_group_layout"),
                        entries: &[wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: true,
                                min_binding_size: Some(std::num::NonZeroU64::new(80).unwrap()),
                            },
                            count: None,
                        }],
                    });

            // Ideally Client submitted pass via API
            // Create logo texture via renderer API and pass handle into overlay factory
            let logo_image = image::open(
                            "/Users/mk/dev/demo/Pill-Engine/engine/pill_renderer/res/pill_logo_horizontal_white.png",
                        )
                        .expect("failed to load overlay logo image");

            let tex_logo = {
                let tex = RendererTexture::new_texture(
                    &ctx.device,
                    &ctx.queue,
                    Some("overlay_logo"),
                    &logo_image,
                    TextureType::Color,
                )
                .expect("failed to create overlay logo texture");
                resource_manager.textures.insert(tex)
            };

            State {
                passes: vec![],
                // Other
                egui_renderer,
                // Resources
                resource_manager,
                // Renderer variables
                color_format,
                depth_format,
                depth_texture: Arc::new(depth_texture),
                offscreen_color_texture,
                // Per-frame UBO ring init
                per_draw_stride: 256, // 80 bytes + 256 byte alignment requirement
                per_draw_capacity: 0,
                per_draw_buffer: None,
                // [SIMILAR] Per-draw dynamic UBO layout with has_dynamic_offset=true per TALK (Milestone 5)
                per_draw_bind_group_layout,
                per_draw_bind_group: None,
                // Prebuilt PSO handle
                master_pipeline_handle: master_handle,
                // Overlays
                tex_logo,
                // Preallocated buffers for hot path optimization (60k instances capacity)
                visible_pre_draw_buffer: Vec::with_capacity(100_000),
                groups_buffer: Vec::with_capacity(2000),
                staging_buffer: Vec::with_capacity(100_000 * 144), // 60k instances * 144 bytes per instance
            }
        };
        Self { state, ctx }
    }

    fn init(&mut self) -> Result<()> {
        self.state.passes.clear();

        self.state.passes.push(Box::new(PassCompose::new(
            "compose",
            self.state.offscreen_color_texture.clone(),
            self.ctx.surface_configuration.format,
        )));

        self.state.passes.push(Box::new(PassOverlayUV::new(
            "overlay_uv",
            [0.75, 0.75, 0.95, 0.95],
            self.ctx.surface_configuration.format,
        )));

        let h: f32 = 0.04; // Logo height
        self.state.passes.push(Box::new(PassOverlayLogo::new(
            "overlay_logo",
            [0.98 - 3. * h, 0.02, 0.98, 0.02 + h], // bottom right
            [1.0, 1.0, 1.0, 1.0],
            self.state.tex_logo,
            self.ctx.surface_configuration.format,
        )));

        self.state.passes.push(Box::new(PassOverlayDepth::new(
            "overlay_depth",
            [0.75, 0.50, 0.95, 0.70],
            [1.0, 1.0, 1.0, 1.0],
            self.state.depth_texture.clone(),
            self.ctx.surface_configuration.format,
        )));

        // Initialize passes - call init on each pass
        // Temporarily move passes out to avoid borrowing conflicts
        let mut passes = std::mem::take(&mut self.state.passes);
        for pass in passes.iter_mut() {
            let _ = pass.init(&self.ctx.device, &mut self.state.resource_manager);
        }
        self.state.passes = passes;

        Ok(())
    }

    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer> {
        let aligned_size = ((desc.byte_size + 255) / 256) * 256; // 256B for Metal UBOs
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
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &ps,
                    entry_point: desc.ps.entry_func,
                    targets: &desc.targets,
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
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

    fn create_material(
        &mut self,
        name: &str,
        textures: &MaterialTextureMap,
        parameters: &MaterialParameterMap,
    ) -> Result<RendererMaterialHandle> {
        // Create bind group layouts inline (avoid pipeline storage dependency)
        let material_texture_bind_group_layout =
            self.ctx
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
            self.ctx
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

        // ensure default textures exist for material fallback
        self.state
            .resource_manager
            .ensure_default_textures(&self.ctx.device, &self.ctx.queue);

        let material = RendererMaterial::new(
            &self.ctx.device,
            &self.ctx.queue,
            &self.state.resource_manager,
            name,
            self.state.master_pipeline_handle,
            &material_texture_bind_group_layout,
            textures,
            &material_parameter_bind_group_layout,
            parameters,
        )
        .unwrap();

        let handle = self.state.resource_manager.materials.insert(material);

        Ok(handle)
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

    fn update_material_textures(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        textures: &MaterialTextureMap,
    ) -> Result<()> {
        RendererMaterial::update_textures(
            &self.ctx.device,
            &self.ctx.queue,
            renderer_material_handle,
            &mut self.state.resource_manager,
            textures,
        )
    }

    fn update_material_parameters(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        parameters: &MaterialParameterMap,
    ) -> Result<()> {
        RendererMaterial::update_parameters(
            &self.ctx.device,
            &self.ctx.queue,
            renderer_material_handle,
            &mut self.state.resource_manager,
            parameters,
        )
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

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()> {
        self.state
            .resource_manager
            .materials
            .remove(renderer_material_handle)
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
            self.state.depth_texture = Arc::new(
                RendererTexture::new_depth_texture(
                    &self.ctx.device,
                    &self.ctx.surface_configuration,
                    "depth_texture",
                )
                .unwrap(),
            );

            // ================================
            // Recreate offscreen color target and reinitialize passes
            self.state.offscreen_color_texture = Arc::new(create_render_target(
                &self.ctx.device,
                self.state.color_format,
                self.ctx.surface_configuration.width,
                self.ctx.surface_configuration.height,
                "offscreen_color",
            ));
            // Reinitialize all passes (including compose pass)
            let _ = self.init();
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
        // [SIMILAR] Prebuilt PSO used; avoid hot-path pipeline creation per TALK
        let _per_draw_bind_group_layout = &self
            .state
            .resource_manager
            .pipeline(self.state.master_pipeline_handle)
            .per_draw_bind_group_layout;
        // M3: Hello Mesh + per-draw MVP (dynamic offsets)
        timer.record("Frame: acquire");

        // Get frame or return mapped error if failed
        // ALLOCATION: Surface texture allocation (GPU memory) - ~4MB for 1920x1080 RGBA8
        let frame = self.ctx.surface.get_current_texture().unwrap();

        // ALLOCATION: TextureView creation (GPU memory) - ~64 bytes metadata
        let view: wgpu::TextureView = frame
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
            .state
            .resource_manager
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
            &self.ctx.queue,
            active_camera_component,
            active_camera_transform_component,
        );
        let renderer_camera = self
            .state
            .resource_manager
            .cameras
            .get(get_renderer_resource_handle_from_camera_component(
                active_camera_component,
            ))
            .unwrap();
        let clear_color = active_camera_component.clear_color;

        // Record inline draw pass (M2)
        // ALLOCATION: CommandEncoder creation (CPU memory) - ~1KB command buffer
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("m2_inline_encoder"),
            });

        // [SIMILAR] CPU frustum culling and precomputing per-draw transforms before starting the pass matches TALK's advice
        // [DIFF] Only frustum culling here; TALK discusses 2-pass occlusion and broader CPU-driven pipelines
        // [API->CLIENT] Culling, model matrix building, and visible set construction should live in client/high-level renderer
        // Build view before pass: cull and prepare per-draw MVPs
        // SIMD OPTIMIZATION: Use glam for SIMD-accelerated frustum culling
        let vp_mat: Mat4 =
            Mat4::from_cols_array_2d(&renderer_camera.uniform.view_projection_matrix);

        // Extract frustum planes using SIMD operations
        let row3 = vp_mat.row(3); // w row
        let row0 = vp_mat.row(0); // x row
        let row1 = vp_mat.row(1); // y row
        let row2 = vp_mat.row(2); // z row

        // SIMD plane construction: normalize plane equations
        let make_plane = |plane_vec: Vec4| -> (Vec3, f32) {
            let normal = Vec3::new(plane_vec.x, plane_vec.y, plane_vec.z);
            let len = normal.length();
            if len > 0.0 {
                let normalized = normal / len;
                (normalized, plane_vec.w / len)
            } else {
                (normal, plane_vec.w)
            }
        };

        // Frustum planes using SIMD operations
        let planes = [
            make_plane(row3 + row0), // left
            make_plane(row3 - row0), // right
            make_plane(row3 + row1), // bottom
            make_plane(row3 - row1), // top
            make_plane(row3 + row2), // near
            make_plane(row3 - row2), // far
        ];

        timer.record("Culling & MVP build");
        // REUSE PREALLOCATED BUFFER: Clear and reuse preallocated buffer (no allocation in hot path)
        self.state.visible_pre_draw_buffer.clear();

        for render_queue_item in render_queue.iter() {
            let key = pill_engine::internal::decompose_render_queue_key(render_queue_item.key);
            let mesh_handle =
                RendererMeshHandle::from_parts(key.mesh_index as u32, key.mesh_version as u32);
            let material_handle = RendererMaterialHandle::from_parts(
                key.material_index as u32,
                key.material_version as u32,
            );
            let material_for_pipeline = self
                .state
                .resource_manager
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
            let model: Mat4 = Mat4::from_cols_array_2d(&model_arr);

            // World AABB from mesh local AABB using SIMD operations
            let mesh = self.state.resource_manager.meshes.get(mesh_handle).unwrap();
            let local_min = Vec3::new(mesh.aabb_min[0], mesh.aabb_min[1], mesh.aabb_min[2]);
            let local_max = Vec3::new(mesh.aabb_max[0], mesh.aabb_max[1], mesh.aabb_max[2]);

            // SIMD AABB corner transformation
            let corners = [
                Vec3::new(local_min.x, local_min.y, local_min.z),
                Vec3::new(local_max.x, local_min.y, local_min.z),
                Vec3::new(local_min.x, local_max.y, local_min.z),
                Vec3::new(local_max.x, local_max.y, local_min.z),
                Vec3::new(local_min.x, local_min.y, local_max.z),
                Vec3::new(local_max.x, local_min.y, local_max.z),
                Vec3::new(local_min.x, local_max.y, local_max.z),
                Vec3::new(local_max.x, local_max.y, local_max.z),
            ];

            let mut world_min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
            let mut world_max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

            // SIMD matrix-vector multiplication for corner transformation
            for c in &corners {
                let p4 = model * Vec4::new(c.x, c.y, c.z, 1.0);
                let p = Vec3::new(p4.x, p4.y, p4.z);
                world_min = world_min.min(p);
                world_max = world_max.max(p);
            }

            // SIMD frustum culling test
            let mut outside = false;
            for (normal, d) in &planes {
                // SIMD select operation for AABB corner selection
                let p = Vec3::new(
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
                // SIMD dot product
                let dist = normal.dot(p) + *d;
                if dist < 0.0 {
                    outside = true;
                    break;
                }
            }
            if outside {
                continue;
            }

            let view_proj: Mat4 =
                Mat4::from_cols_array_2d(&renderer_camera.uniform.view_projection_matrix);
            let mvp: [[f32; 4]; 4] = (view_proj * model).to_cols_array_2d();
            // REUSE PREALLOCATED BUFFER: Push to preallocated buffer (no allocation in hot path)
            self.state.visible_pre_draw_buffer.push(VisiblePreDraw {
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
        self.state.visible_pre_draw_buffer.sort_by_key(|v| {
            (
                v.pipeline_handle.index(),
                v.material_handle.index(),
                v.mesh_handle.index(),
            )
        });

        // Build grouped instancing batches by (pipeline, material, mesh)
        timer.record("Sort & group draws");
        // REUSE PREALLOCATED BUFFER: Clear and reuse preallocated groups buffer (no allocation in hot path)
        self.state.groups_buffer.clear();
        for v in &self.state.visible_pre_draw_buffer {
            if self
                .state
                .groups_buffer
                .last()
                .map(|g| {
                    g.pipeline_handle != v.pipeline_handle || g.material_handle != v.material_handle
                })
                .unwrap_or(true)
            {
                self.state.groups_buffer.push(GroupCmd {
                    pipeline_handle: v.pipeline_handle,
                    material_handle: v.material_handle,
                    batches: Vec::new(),
                });
            }
            let g = self.state.groups_buffer.last_mut().unwrap();
            if let Some(batch) = g
                .batches
                .iter_mut()
                .find(|b| b.mesh_handle == v.mesh_handle)
            {
                batch.instances.push(v.mvp);
            } else {
                // ALLOCATION: Vec<[[f32; 4]; 4]> for instances (CPU memory) - optimistic: 1 instance * 64 bytes, pessimistic: 60k instances * 64 bytes = ~3.8MB per batch
                g.batches.push(MeshBatch {
                    mesh_handle: v.mesh_handle,
                    instances: vec![v.mvp],
                    base_offset_u32: 0,
                });
            }
        }
        let draw_call_count: usize = self
            .state
            .groups_buffer
            .iter()
            .map(|g| g.batches.len())
            .sum();
        // Expose draw call count via per-frame timer counters for UI/metrics
        timer.set_counter("draw_calls", draw_call_count as u64);

        // Milestone 5: Per-frame ring buffer
        // [SIMILAR] Batch write dynamic per-draw data once; bind with dynamic offsets
        timer.begin_context("Per-draw UBO setup");
        let needed: u64 = self
            .state
            .groups_buffer
            .iter()
            .map(|g| {
                g.batches
                    .iter()
                    .map(|b| b.instances.len() as u64)
                    .sum::<u64>()
            })
            .sum();
        // total instances; no direct use beyond sanity, avoid unused warning by not binding
        if self.state.per_draw_capacity < needed {
            self.state.per_draw_capacity = needed.next_power_of_two().max(1);
            let size = self.state.per_draw_stride * self.state.per_draw_capacity;
            // ALLOCATION: GPU buffer creation (GPU memory) - optimistic: 1KB, pessimistic: 60k instances * 144 bytes = ~8.6MB
            self.state.per_draw_buffer =
                Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("per_draw_dynamic_ubo_ring"),
                    size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            // ALLOCATION: BindGroup creation (GPU memory) - ~128 bytes metadata
            self.state.per_draw_bind_group = Some(
                self.ctx
                    .device
                    .create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("per_draw_bind_group"),
                        layout: &self
                            .state
                            .resource_manager
                            .pipeline(self.state.master_pipeline_handle)
                            .per_draw_bind_group_layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: self.state.per_draw_buffer.as_ref().unwrap(),
                                offset: 0,
                                size: Some(std::num::NonZeroU64::new(144).unwrap()),
                            }),
                        }],
                    }),
            );
        }
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct PerDrawStd140 {
            mvp: [[f32; 4]; 4], // 4x4 MVP matrix (16 floats)
            tint: [f32; 4],     // RGBA color (4 floats)
        }
        unsafe impl bytemuck::Zeroable for PerDrawStd140 {}
        unsafe impl bytemuck::Pod for PerDrawStd140 {}
        let tint: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        // REUSE PREALLOCATED BUFFER: Clear and reuse preallocated staging buffer (no allocation in hot path)
        self.state.staging_buffer.clear();
        let mut next_offset_u32: u32 = 0;
        for g in self.state.groups_buffer.iter_mut() {
            for b in g.batches.iter_mut() {
                b.base_offset_u32 = next_offset_u32;
                for mvp in &b.instances {
                    let pd = PerDrawStd140 { mvp: *mvp, tint };
                    self.state
                        .staging_buffer
                        .extend_from_slice(bytemuck::bytes_of(&pd));
                    let pad = (self.state.per_draw_stride as usize)
                        - std::mem::size_of::<PerDrawStd140>();
                    self.state
                        .staging_buffer
                        .extend(std::iter::repeat(0u8).take(pad));
                    next_offset_u32 =
                        next_offset_u32.wrapping_add(self.state.per_draw_stride as u32);
                }
            }
        }
        if let Some(buf) = &self.state.per_draw_buffer {
            self.ctx
                .queue
                .write_buffer(buf, 0, &self.state.staging_buffer);
        }
        timer.record("Per-draw UBO write (ring)");
        timer.end_context()?;

        {
            timer.begin_context("Offscreen pass");
            // Render scene into offscreen color target (T0)
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("m6_offscreen_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.state.offscreen_color_texture.texture_view,
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
                    view: &self.state.depth_texture.texture_view,
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
            for group in &self.state.groups_buffer {
                timer.begin_context(&format!(
                    "Pipeline {} / Material {}",
                    group.pipeline_handle.index(),
                    group.material_handle.index()
                ));
                rpass.set_pipeline(
                    &self
                        .state
                        .resource_manager
                        .pipeline(self.state.master_pipeline_handle)
                        .render_pipeline,
                );

                // HOT SoA (per-frame hot path):
                // - RendererMaterial: texture_bind_group, parameter_bind_group
                // - RendererCamera  : bind_group
                // - RendererMesh    : vertex_buffer, index_buffer, index_count
                // COLD metadata (names/descs) lives in *_cold arrays and is not read in the draw loop.

                // Get material and set bind groups
                let material = self
                    .state
                    .resource_manager
                    .materials
                    .get(group.material_handle)
                    .unwrap();
                rpass.set_bind_group(0, &renderer_camera.bind_group, &[]);
                rpass.set_bind_group(1, &material.texture_bind_group, &[]);
                rpass.set_bind_group(2, &material.parameter_bind_group, &[]);

                // Process batches with separate storage borrows to avoid lifetime issues
                for batch in &group.batches {
                    let mesh = self
                        .state
                        .resource_manager
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
                            .wrapping_add((i as u32) * (self.state.per_draw_stride as u32));
                        rpass.set_bind_group(
                            3,
                            self.state.per_draw_bind_group.as_ref().unwrap(),
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

        self.ctx.queue.submit([encoder.finish()]);
        timer.record("Submit scene+compose passes");

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
            for pass in &self.state.passes {
                let label = pass.get_label();
                timer.begin_context(label);
                let _ = pass.draw(&mut encoder, self, &frame, &view);
                timer.record(label);
                timer.end_context()?;
            }

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

    pub fn get_surface_view(&self) -> &wgpu::TextureView {
        &self.state.offscreen_color_texture.texture_view
    }
}
