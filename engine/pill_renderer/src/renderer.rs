#![allow(clippy::too_many_arguments)]
use crate::{
    config::MAX_INSTANCE_PER_DRAWCALL_COUNT,
    drawers::mesh_drawer::MeshDrawer,
    instance::Instance,
    resources::{
        RendererCamera, RendererMaterial, RendererMesh, RendererResourceStorage, RendererShader,
        RendererTexture, Vertex,
    },
};

use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, CameraComponent, ComponentStorage,
    EngineConfig, EntityHandle, MaterialParameter, MaterialTexture,
    MeshData, MeshRenderingComponent, PillRenderer, RayTracingMode, RenderQueueItem,
    RendererCameraHandle, RendererCapabilities, RendererMaterialHandle, RendererMeshHandle,
    RendererShaderHandle, RendererTextureHandle, ShaderParameterSlot, ShaderTextureSlot,
    TextureType, TransformComponent,
};

use pill_core::{debug, info, LogContext, PillSlotMapKey, PillStyle, RendererError, Timer};

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
use pill_core::warn;

use std::{collections::HashMap, sync::Arc};

use pill_core::{ErrorContext, Result};

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
use crate::ray_tracing::{
    blas::{create_blas_size_descriptor, BlasBuildState, RayTracingMesh},
    capability::{
        build_capabilities, check_compile_time_preconditions,
        log_startup_diagnostic, resolve_ray_tracing_mode,
        RayTracingDisabledReason, RayTracingPolicyResult,
    },
    pipeline::compile_ray_query_canary,
    scene::RayTracingScene,
};

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
use pill_engine::internal::HardwareRayQueryCapabilities;

pub struct Renderer {
    pub state: State,
    capabilities: RendererCapabilities,
}

impl Renderer {
    /// Async constructor for WASM - call this instead of PillRenderer::new on web
    pub async fn new_async(
        window: Arc<winit::window::Window>,
        config: EngineConfig,
    ) -> Result<Self> {
        info!(LogContext::Rendering => "Initializing {}", "Renderer".module_object_style());
        let (state, capabilities) = State::new(window, config).await?;
        Ok(Self { state, capabilities })
    }
}

impl PillRenderer for Renderer {
    #[cfg(not(target_arch = "wasm32"))]
    fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self> {
        info!(LogContext::Rendering => "Initializing {}", "Renderer".module_object_style());
        let (state, capabilities) = pollster::block_on(State::new(window, config))?;

        Ok(Self { state, capabilities })
    }

    #[cfg(target_arch = "wasm32")]
    fn new(_window: Arc<winit::window::Window>, _config: EngineConfig) -> Result<Self> {
        panic!("Use Renderer::new_async on WASM")
    }

    // --- Create ---

    fn create_shader(
        &mut self,
        name: &str,
        vertex_wgsl: &str,
        fragment_wgsl: &str,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        parameter_slots: &[(String, ShaderParameterSlot)],
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<RendererShaderHandle> {
        let shader = RendererShader::new(
            name,
            &self.state.device,
            self.state.color_format,
            Some(self.state.depth_format),
            &[
                RendererMesh::data_layout_descriptor(),
                Instance::data_layout_descriptor(),
            ],
            vertex_wgsl,
            fragment_wgsl,
            parameter_slots,
            texture_slots,
            &self
                .state
                .renderer_resource_storage
                .engine_parameters
                .bind_group_layout,
            &self.state.camera_bind_group_layout,
            pass_engine_parameters,
            pass_camera_parameters,
        )?;
        let handle = self.state.renderer_resource_storage.shaders.insert(shader);

        // ── Create RT pipeline variant ───────────────────────────────
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        if pass_engine_parameters {
            if let (Some(rt_bgl), Some(rt_frag_module)) = (
                self.state.rt_bind_group_layout.as_ref(),
                self.state.rt_fragment_module.as_ref(),
            ) {
                let shader_ref = self.state.renderer_resource_storage.shaders.get_mut(handle).unwrap();

                // Compile the vertex shader module for the RT pipeline.
                let rt_vertex_module = self.state.device.create_shader_module(
                    wgpu::ShaderModuleDescriptor {
                        label: Some(&format!("{name}_rt_vertex")),
                        source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
                    },
                );

                // Build pipeline layout for RT variant:
                // group 0: RT bind group (uniform + TLAS)
                // group 1: camera (same as raster)
                // group 2: material params (if any)
                // group 3: material textures (if any)
                let mut rt_bind_group_layouts: Vec<Option<&wgpu::BindGroupLayout>> = Vec::new();
                rt_bind_group_layouts.push(Some(rt_bgl));
                if pass_camera_parameters {
                    rt_bind_group_layouts.push(Some(&self.state.camera_bind_group_layout));
                }
                if let Some(ref mat_bgl) = shader_ref.parameters_bind_group_layout {
                    rt_bind_group_layouts.push(Some(mat_bgl));
                }
                if let Some(ref tex_bgl) = shader_ref.textures_bind_group_layout {
                    rt_bind_group_layouts.push(Some(tex_bgl));
                }

                let rt_pipeline_layout = self.state.device.create_pipeline_layout(
                    &wgpu::PipelineLayoutDescriptor {
                        label: Some(&format!("{name}_rt_pipeline_layout")),
                        bind_group_layouts: &rt_bind_group_layouts,
                        immediate_size: 0,
                    },
                );

                let vertex_buffer_layouts: Vec<Option<wgpu::VertexBufferLayout>> = vec![
                    Some(RendererMesh::data_layout_descriptor()),
                    Some(Instance::data_layout_descriptor()),
                ];

                let rt_pipeline = self.state.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: Some(&format!("{name}_rt_pipeline")),
                        layout: Some(&rt_pipeline_layout),
                        vertex: wgpu::VertexState {
                            module: &rt_vertex_module,
                            entry_point: Some("vs_main"),
                            buffers: &vertex_buffer_layouts,
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: rt_frag_module,
                            entry_point: Some("fs_main"),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: self.state.color_format,
                                blend: Some(wgpu::BlendState {
                                    alpha: wgpu::BlendComponent::REPLACE,
                                    color: wgpu::BlendComponent::REPLACE,
                                }),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                            compilation_options: Default::default(),
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: Some(wgpu::Face::Back),
                            unclipped_depth: false,
                            polygon_mode: wgpu::PolygonMode::Fill,
                            conservative: false,
                        },
                        depth_stencil: Some(wgpu::DepthStencilState {
                            format: self.state.depth_format,
                            depth_write_enabled: Some(true),
                            depth_compare: Some(wgpu::CompareFunction::Less),
                            stencil: wgpu::StencilState::default(),
                            bias: wgpu::DepthBiasState::default(),
                        }),
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    },
                );

                shader_ref.rt_pipeline = Some(rt_pipeline);
                debug!(LogContext::Rendering =>
                    "RT: created RT pipeline variant for shader '{}'", name,
                );
            }
        }

        Ok(handle)
    }

    fn create_material(
        &mut self,
        name: &str,
        renderer_shader_handle: RendererShaderHandle,
        textures: &[(String, MaterialTexture)],
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<RendererMaterialHandle> {
        let material = RendererMaterial::new(
            &self.state.device,
            &self.state.queue,
            &self.state.renderer_resource_storage,
            name,
            renderer_shader_handle,
            textures,
            parameters,
        )?;
        let handle = self
            .state
            .renderer_resource_storage
            .materials
            .insert(material);
        Ok(handle)
    }

    fn create_texture(
        &mut self,
        name: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_texture(
            &self.state.device,
            &self.state.queue,
            Some(name),
            rgba,
            width,
            height,
            texture_type,
        )?;
        let handle = self
            .state
            .renderer_resource_storage
            .textures
            .insert(texture);
        Ok(handle)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data, self.state.ray_tracing_enabled())?;
        let handle = self.state.renderer_resource_storage.meshes.insert(mesh);

        // ── Create BLAS for this mesh when RT is enabled ──────────────
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        if let Some(scene) = self.state.ray_tracing_state.scene_mut() {
            let mesh_ref = self.state.renderer_resource_storage.meshes.get(handle).unwrap();
            let vertex_count = mesh_data.vertices.len() as u32;
            let index_count = mesh_data.indices.len() as u32;

            // Quick validation: skip empty or degenerate meshes.
            if vertex_count > 0 && index_count > 0 && index_count % 3 == 0 {
                let size_desc = create_blas_size_descriptor(
                    vertex_count,
                    index_count,
                    wgpu::IndexFormat::Uint32,
                );

                let blas = self.state.device.create_blas(
                    &wgpu::CreateBlasDescriptor {
                        label: Some(&format!("{name}_blas")),
                        flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
                        update_mode: wgpu::AccelerationStructureUpdateMode::Build,
                    },
                    wgpu::BlasGeometrySizeDescriptors::Triangles {
                        descriptors: vec![size_desc.clone()],
                    },
                );

                let rt_mesh = RayTracingMesh {
                    blas: blas.clone(),
                    size_descriptor: size_desc.clone(),
                    build_state: BlasBuildState::Pending,
                    primitive_count: index_count / 3,
                    vertex_count,
                    index_count,
                };

                scene.blas_cache.insert(handle, rt_mesh);

                // Build the BLAS immediately instead of deferring to the
                // render loop.  A one-shot encoder + submit guarantees the
                // BLAS is ready before the first frame touches the TLAS.
                {
                    let mut blas_encoder = self.state.device.create_command_encoder(
                        &wgpu::CommandEncoderDescriptor {
                            label: Some(&format!("{name}_blas_init")),
                        },
                    );
                    let blas_entry = wgpu::BlasBuildEntry {
                        blas: &blas,
                        geometry: wgpu::BlasGeometries::TriangleGeometries(
                            vec![wgpu::BlasTriangleGeometry {
                                size: &size_desc,
                                vertex_buffer: &mesh_ref.vertex_buffer,
                                first_vertex: 0,
                                vertex_stride: std::mem::size_of::<pill_engine::internal::MeshVertex>() as u64,
                                index_buffer: Some(&mesh_ref.index_buffer),
                                first_index: Some(0),
                                transform_buffer: None,
                                transform_buffer_offset: None,
                            }],
                        ),
                    };
                    blas_encoder.build_acceleration_structures(
                        &[blas_entry],
                        &[],
                    );
                    self.state.queue.submit(std::iter::once(blas_encoder.finish()));

                    // Mark as submitted so the TLAS can reference it.
                    if let Some(mesh) = scene.blas_cache.get_mut(&handle) {
                        mesh.build_state = BlasBuildState::Submitted {
                            submission: scene.submission.next(),
                        };
                    }
                }

                debug!(LogContext::Rendering =>
                    "RT: created BLAS for mesh '{}' ({} verts, {} indices, {} prims)",
                    name, vertex_count, index_count, index_count / 3,
                );
            } else {
                debug!(LogContext::Rendering =>
                    "RT: mesh '{}' skipped for BLAS (degenerate: {} verts, {} indices)",
                    name, vertex_count, index_count,
                );
            }
        }

        Ok(handle)
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        let camera = RendererCamera::new(
            &self.state.device,
            self.state.camera_bind_group_layout.clone(),
        )?;
        let handle = self.state.renderer_resource_storage.cameras.insert(camera);
        Ok(handle)
    }

    // --- Update ---

    fn update_material_textures(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        textures: &[(String, MaterialTexture)],
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
        parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<()> {
        RendererMaterial::update_parameters(
            &self.state.device,
            &self.state.queue,
            renderer_material_handle,
            &mut self.state.renderer_resource_storage,
            parameters,
        )
    }

    // --- Destroy ---

    fn destroy_shader(&mut self, renderer_shader_handle: RendererShaderHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .shaders
            .remove(renderer_shader_handle)
            .unwrap();

        // TODO: Check if there are no materials using this shader (engine should replace them with default shader), if there are prevent shader destruction
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

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .textures
            .remove(renderer_texture_handle)
            .unwrap();
        Ok(())
    }

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        self.state
            .renderer_resource_storage
            .meshes
            .remove(renderer_mesh_handle)
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

    // --- Other ---

    fn capabilities(&self) -> &RendererCapabilities {
        &self.capabilities
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!(LogContext::Rendering => "Resizing {} resources", "Renderer".module_object_style());
        self.state.resize(new_window_size)
    }

    #[cfg(feature = "debug_ui")]
    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()> {
        self.state.egui_drawer.handle_input(event);
        Ok(())
    }

    #[cfg(feature = "debug_ui")]
    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        mesh_rendering_component_storage: &ComponentStorage<MeshRenderingComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        delta_time: f32,
        timer: &mut Timer,
    ) -> Result<()> {
        self.state.pending_egui_ui = Some(egui_ui);
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage,
            mesh_rendering_component_storage,
            delta_time,
            timer,
        )
    }

    #[cfg(not(feature = "debug_ui"))]
    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        mesh_rendering_component_storage: &ComponentStorage<MeshRenderingComponent>,
        delta_time: f32,
        timer: &mut Timer,
    ) -> Result<()> {
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage,
            mesh_rendering_component_storage,
            delta_time,
            timer,
        )
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
    // Drawers
    mesh_drawer: MeshDrawer,
    #[cfg(feature = "debug_ui")]
    egui_drawer: crate::drawers::egui_drawer::EguiDrawer,
    #[allow(clippy::type_complexity)]
    #[cfg(feature = "debug_ui")]
    pending_egui_ui: Option<Box<dyn FnMut(&egui::Context)>>,
    // Other
    camera_bind_group_layout: wgpu::BindGroupLayout,
    // Ray tracing (conditionally compiled)
    #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
    ray_tracing_state: RayTracingStateWrapper,
    // RT resources shared across all RT shader variants.
    #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
    rt_engine_bind_group: Option<wgpu::BindGroup>,
    #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
    rt_bind_group_layout: Option<wgpu::BindGroupLayout>,
    #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
    rt_fragment_module: Option<wgpu::ShaderModule>,
    //profiler: Profiler,
}

/// Wraps the RT state so the field is always present (simpler cfg logic).
#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
#[allow(dead_code)]
enum RayTracingStateWrapper {
    Disabled(RayTracingDisabledReason),
    Enabled(Box<RayTracingScene>),
}

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
impl RayTracingStateWrapper {
    #[allow(dead_code)]
    fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }

    #[allow(dead_code)]
    fn scene(&self) -> Option<&RayTracingScene> {
        match self {
            Self::Enabled(scene) => Some(scene),
            Self::Disabled(_) => None,
        }
    }

    fn scene_mut(&mut self) -> Option<&mut RayTracingScene> {
        match self {
            Self::Enabled(scene) => Some(scene),
            Self::Disabled(_) => None,
        }
    }
}

impl State {
    /// Returns `true` when hardware ray tracing is enabled on the device.
    #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
    fn ray_tracing_enabled(&self) -> bool {
        self.ray_tracing_state.is_enabled()
    }

    /// Always returns `false` when the feature is not compiled in.
    #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
    fn ray_tracing_enabled(&self) -> bool {
        false
    }

    // Creating some of the wgpu types requires async code
    async fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<(Self, RendererCapabilities)> {
        let window_width = config
            .get_int("WINDOW_WIDTH")
            .context("WINDOW_WIDTH is missing from config")? as u32;
        let window_height = config
            .get_int("WINDOW_HEIGHT")
            .context("WINDOW_HEIGHT is missing from config")? as u32;
        let window_size = winit::dpi::PhysicalSize::new(window_width, window_height);
        #[cfg(feature = "debug_ui")]
        let window_ref = window.clone();

        // Resolve RT policy early for diagnostics.
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        let rt_mode = resolve_ray_tracing_mode(&config);
        #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
        let _rt_mode = RayTracingMode::Off;

        // 1. Create instance and surface
        let (instance, surface) = {
            let backends = match std::env::var("WGPU_BACKENDS").as_deref() {
                std::result::Result::Ok("VULKAN") => wgpu::Backends::VULKAN,
                std::result::Result::Ok("DX12") => wgpu::Backends::DX12,
                std::result::Result::Ok("METAL") => wgpu::Backends::METAL,
                std::result::Result::Ok("GL") => wgpu::Backends::GL,
                std::result::Result::Ok("BROWSER_WEBGPU") => wgpu::Backends::BROWSER_WEBGPU,
                _ => wgpu::Backends::all(),
            };

            let instance_descriptor = wgpu::InstanceDescriptor {
                backends,
                flags: wgpu::InstanceFlags::from_build_config().with_env(),
                backend_options: wgpu::BackendOptions::default(),
                memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
                display: None,
            };

            let instance = wgpu::Instance::new(instance_descriptor);
            let surface = instance
                .create_surface(window)
                .context("Failed to create surface")?;
            (instance, surface)
        };

        // 2. Adapter selection with optional RT preference
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        let (adapter, rt_policy_result) = {
            select_adapter_with_rt_policy(&instance, &surface, rt_mode).await?
        };
        #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
        let adapter = {
            let request_adapter_options = wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            };
            instance
                .request_adapter(&request_adapter_options)
                .await
                .context("Failed to request adapter")?
        };

        let info = adapter.get_info();
        info!(LogContext::Rendering => "Using GPU: {} ({:?})", info.name, info.backend);

        // 3. Device and queue
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        let (device, queue) = {
            request_device_with_rt_policy(&adapter, &rt_policy_result).await?
        };
        #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
        let (device, queue) = {
            let wanted = wgpu::Features::DEPTH_CLIP_CONTROL
                | wgpu::Features::TIMESTAMP_QUERY
                | wgpu::Features::PIPELINE_STATISTICS_QUERY;
            let features = wanted & adapter.features();

            let device_descriptor = wgpu::DeviceDescriptor {
                label: None,
                required_features: features,
                required_limits: wgpu::Limits::default(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::default(),
            };

            adapter
                .request_device(&device_descriptor)
                .await
                .context("Failed to request device")?
        };

        // 4. Surface configuration
        let (surface_configuration, color_format, depth_format) = {
            let preferred_format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let surface_capabilities = surface.get_capabilities(&adapter);

            #[cfg(target_arch = "wasm32")]
            let present_mode = wgpu::PresentMode::Fifo;
            #[cfg(not(target_arch = "wasm32"))]
            let present_mode = if surface_capabilities
                .present_modes
                .contains(&wgpu::PresentMode::Mailbox)
            {
                wgpu::PresentMode::Mailbox
            } else if surface_capabilities
                .present_modes
                .contains(&wgpu::PresentMode::Immediate)
            {
                wgpu::PresentMode::Immediate
            } else {
                wgpu::PresentMode::Fifo
            };

            let format = if surface_capabilities.formats.contains(&preferred_format) {
                preferred_format
            } else if surface_capabilities
                .formats
                .contains(&wgpu::TextureFormat::Bgra8UnormSrgb)
            {
                wgpu::TextureFormat::Bgra8UnormSrgb
            } else if surface_capabilities
                .formats
                .contains(&wgpu::TextureFormat::Bgra8Unorm)
            {
                wgpu::TextureFormat::Bgra8Unorm
            } else {
                surface_capabilities.formats[0]
            };

            let surface_configuration = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: window_size.width,
                height: window_size.height,
                desired_maximum_frame_latency: 2,
                present_mode,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![format],
                color_space: wgpu::SurfaceColorSpace::default(),
            };
            surface.configure(&device, &surface_configuration);
            let color_format = surface_configuration.format;
            let depth_format = wgpu::TextureFormat::Depth32Float;
            (surface_configuration, color_format, depth_format)
        };

        // 5. Depth texture
        // 5. Depth texture
        let depth_texture =
            RendererTexture::new_depth_texture(&device, &surface_configuration, "depth_texture")
                .context("Failed to create depth texture")?;

        // 6. Define camera bind group layout
        let camera_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera_parameters_bind_group_layout"),
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

        // 7. Resource storage
        let renderer_resource_storage = RendererResourceStorage::new(&device, &config)?;

        // 8. Drawers
        let mesh_drawer = MeshDrawer::new(&device, MAX_INSTANCE_PER_DRAWCALL_COUNT as u32);
        #[cfg(feature = "debug_ui")]
        let egui_drawer = crate::drawers::egui_drawer::EguiDrawer::new(
            &device,
            surface_configuration.format,
            None,
            1,
            window_ref,
        );

        // 9. Ray tracing scene (conditionally compiled)
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        let ray_tracing_state = {
            if rt_policy_result.is_enabled() {
                // Compile the ray-query canary to verify WGSL dialect.
                if let Err(e) = compile_ray_query_canary(&device) {
                    warn!(LogContext::Rendering =>
                        "Ray query canary compilation failed: {e}. RT disabled."
                    );
                    RayTracingStateWrapper::Disabled(
                        RayTracingDisabledReason::DeviceRequestRejected {
                            reason: format!("canary compilation: {e}"),
                        },
                    )
                } else {
                    let capabilities = rt_policy_result.capabilities()
                        .cloned()
                        .unwrap_or(HardwareRayQueryCapabilities {
                            max_blas_primitive_count: device.limits().max_blas_primitive_count,
                            max_blas_geometry_count: device.limits().max_blas_geometry_count,
                            max_tlas_instance_count: device.limits().max_tlas_instance_count,
                            max_acceleration_structures_per_shader_stage: device.limits().max_acceleration_structures_per_shader_stage,
                            max_buffers_and_acceleration_structures_per_shader_stage: device.limits().max_buffers_and_acceleration_structures_per_shader_stage,
                        });

                    let max_instances = config.get_int("MAX_RT_INSTANCES")
                        .unwrap_or(16384) as u32;

                    let scene = RayTracingScene::new(
                        &device,
                        capabilities,
                        max_instances,
                    );
                    RayTracingStateWrapper::Enabled(Box::new(scene))
                }
            } else {
                let reason = match &rt_policy_result {
                    RayTracingPolicyResult::Disabled { reason } => reason.clone(),
                    _ => RayTracingDisabledReason::PolicyOff,
                };
                RayTracingStateWrapper::Disabled(reason)
            }
        };

        // Log startup diagnostics
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        log_startup_diagnostic(rt_mode, &adapter, &rt_policy_result);

        // Build capabilities report
        let capabilities = {
            #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
            {
                build_capabilities(&adapter, &device, &rt_policy_result)
            }
            #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
            {
                use pill_engine::internal::RendererBackend;
                let info = adapter.get_info();
                RendererCapabilities {
                    backend: match info.backend {
                        wgpu::Backend::Vulkan => RendererBackend::Vulkan,
                        wgpu::Backend::Dx12 => RendererBackend::Dx12,
                        wgpu::Backend::Metal => RendererBackend::Metal,
                        wgpu::Backend::Gl => RendererBackend::Gl,
                        wgpu::Backend::BrowserWebGpu => RendererBackend::BrowserWebGpu,
                        _ => RendererBackend::Unknown,
                    },
                    adapter_name: info.name,
                    hardware_ray_query: None,
                }
            }
        };

        // Create state
        #[allow(unused_mut)]
        let mut renderer = Self {
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
            // Drawers
            mesh_drawer,
            #[cfg(feature = "debug_ui")]
            egui_drawer,
            #[cfg(feature = "debug_ui")]
            pending_egui_ui: None,
            // Other
            camera_bind_group_layout,
            // Ray tracing
            #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
            ray_tracing_state,
            #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
            rt_engine_bind_group: None,
            #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
            rt_bind_group_layout: None,
            #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
            rt_fragment_module: None,
            // profiler
        };

        // ── Post-init: create RT pipeline variant ────────────────────
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        if let Some(scene) = renderer.ray_tracing_state.scene() {
            let rt_bgl = renderer.device.create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("rt_engine_bgl"),
                    entries: &[
                        // Binding 0: engine uniform buffer (fog + light)
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Buffer {
                                ty: wgpu::BufferBindingType::Uniform,
                                has_dynamic_offset: false,
                                min_binding_size: None,
                            },
                            count: None,
                        },
                        // Binding 1: TLAS
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::AccelerationStructure {
                                vertex_return: false,
                            },
                            count: None,
                        },
                    ],
                },
            );

            let rt_bg = renderer.device.create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: Some("rt_engine_bg"),
                    layout: &rt_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: renderer.renderer_resource_storage
                                .engine_parameters
                                .parameters_uniform_buffer
                                .as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::AccelerationStructure(
                                &scene.tlas.tlas,
                            ),
                        },
                    ],
                },
            );

            // Bootstrap the TLAS with a one-shot build so it is
            // valid for binding on the very first frame (even when
            // no BLAS entries are ready yet).
            {
                let mut bootstrap_encoder = renderer.device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("tlas_bootstrap_encoder"),
                    },
                );
                let tlas_slice = std::slice::from_ref(&scene.tlas.tlas);
                bootstrap_encoder.build_acceleration_structures(
                    &[],
                    tlas_slice,
                );
                renderer.queue.submit(std::iter::once(bootstrap_encoder.finish()));
            }

            // RT fragment shader: diffuse + shadow-ray lighting.
            // Binds material tint at group 2 so coloured cubes stay coloured.
            //
            // IMPORTANT: VertexOutput locations MUST match the default
            // vertex shader (pill_engine/res/shaders/default_vertex.wgsl):
            //   @location(5) = world_position_0 : vec3<f32>
            //   @location(4) = TBN_normal_0    : vec3<f32>  (world-space normal)
            //   @location(1) = vertex_texture_coords_0 : vec2<f32>
            const RT_FRAGMENT_SHADER: &str = r#"
enable wgpu_ray_query;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(5) world_position: vec3<f32>,
    @location(4) world_normal: vec3<f32>,
    @location(1) tex_coord: vec2<f32>,
}

struct EngineParams {
    fog_color: vec3<f32>,
    _pad0: f32,
    fog_density: f32,
    _pad1a: f32,
    _pad1b: f32,
    _pad1c: f32,
    light_position: vec3<f32>,
    _pad2: f32,
    light_color: vec3<f32>,
    _pad3: f32,
    light_intensity: f32,
    shadow_cull_mask: u32,
    _pad4a: f32,
    _pad4b: f32,
}

struct MaterialParams {
    tint: vec3<f32>,
    specularity: f32,
}

@group(0) @binding(0)
var<uniform> engine: EngineParams;

@group(0) @binding(1)
var tlas: acceleration_structure;

@group(1) @binding(0)
var<uniform> camera: mat4x4<f32>;

@group(2) @binding(0)
var<uniform> material: MaterialParams;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let N = normalize(in.world_normal);
    let L = normalize(engine.light_position - in.world_position);

    let ambient = vec3<f32>(0.08, 0.08, 0.08);
    let NdotL = max(dot(N, L), 0.0);
    let diffuse = material.tint * engine.light_color * NdotL * engine.light_intensity;

    // Shadow ray
    let bias = 0.05f;
    let origin = in.world_position + N * bias;
    let to_light = engine.light_position - origin;
    let distance = length(to_light);
    let direction = to_light / distance;
    let t_max = distance - 0.01f;

    var shadow: f32 = 1.0;
    if (distance > 0.001 && t_max > 0.001) {
        var rq: ray_query;
        var ray_desc: RayDesc;
        ray_desc.origin = origin;
        ray_desc.dir = direction;
        ray_desc.tmin = 0.001f;
        ray_desc.tmax = t_max;
        ray_desc.flags = 4u;
        ray_desc.cull_mask = engine.shadow_cull_mask;
        rayQueryInitialize(&rq, tlas, ray_desc);

        loop {
            if (rayQueryProceed(&rq)) {
                rayQueryConfirmIntersection(&rq);
                shadow = 0.0;
                break;
            } else {
                break;
            }
        }
    }

    let lit = ambient + diffuse * shadow;
    let fog_factor = 1.0 / exp(engine.fog_density * length(in.world_position - vec3<f32>(0.0, 4.0, -10.0)));
    let color = mix(engine.fog_color, lit, vec3<f32>(fog_factor));

    return vec4<f32>(color, 1.0);
}
"#;

            let rt_sm = renderer.device.create_shader_module(
                wgpu::ShaderModuleDescriptor {
                    label: Some("rt_lit_fragment"),
                    source: wgpu::ShaderSource::Wgsl(RT_FRAGMENT_SHADER.into()),
                },
            );

            renderer.rt_engine_bind_group = Some(rt_bg);
            renderer.rt_bind_group_layout = Some(rt_bgl);
            renderer.rt_fragment_module = Some(rt_sm);
        }

        Ok((renderer, capabilities))
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
        }
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        #[allow(unused_variables)] mesh_rendering_component_storage: &ComponentStorage<MeshRenderingComponent>,
        _delta_time: f32,
        timer: &mut Timer,
    ) -> Result<()> {
        debug!(LogContext::Frame => "Starting frame render");

        timer.record("Get frame");
        // self.profiler.begin_frame();

        // Get frame or return mapped error if failed
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost => return Err(RendererError::SurfaceLost.into()),
            _ => return Err(RendererError::SurfaceOther.into()),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        timer.record("Update camera parameters");

        // Get active camera and update it
        let camera_storage = camera_component_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let active_camera_component = camera_storage.as_ref().unwrap();

        timer.record("Update engine parameters");

        // Engine uniform is updated AFTER the camera lookup so fog (carried on
        // CameraComponent) can be forwarded into the `engine` UBO alongside delta_time.
        self.renderer_resource_storage.engine_parameters.update(
            &self.queue,
            active_camera_component.fog_density,
            [
                active_camera_component.fog_color.x,
                active_camera_component.fog_color.y,
                active_camera_component.fog_color.z,
            ],
        );
        let renderer_camera = self
            .renderer_resource_storage
            .cameras
            .get_mut(get_renderer_resource_handle_from_camera_component(
                active_camera_component,
            ))
            .ok_or_else(|| -> pill_core::PillError {
                RendererError::RendererResourceNotFound.into()
            })?;
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

        // Build a command buffer that can be sent to the GPU
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });
        #[cfg_attr(not(feature = "debug_ui"), allow(unused_mut))]
        let mut additional_command_buffers = Vec::new();
        #[cfg(feature = "debug_ui")]
        let mut egui_textures_to_free = Vec::new();

        // ── Acceleration-structure builds (before the render pass) ──────
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        if self.ray_tracing_state.is_enabled() {
            if let Some(scene) = self.ray_tracing_state.scene_mut() {
                scene.begin_frame();

                // Walk the render queue to populate TLAS instances.
                for item in render_queue {
                    let entity_index = item.entity_index as usize;

                    // Look up the MeshRenderingComponent to get the mesh
                    // handle and ray-visibility policy.
                    let mrc_slot = mesh_rendering_component_storage
                        .data
                        .get(entity_index)
                        .and_then(|s| s.as_ref());
                    let Some(mrc) = mrc_slot else { continue; };
                    if mrc.mesh_handle.is_none() { continue; };

                    // Map engine MeshHandle → renderer RendererMeshHandle.
                    // The render queue key already contains the renderer
                    // handle; we need the full handle to look up the BLAS.
                    // Reconstruct from the render queue key fields.
                    let key_fields = pill_engine::internal::decompose_render_queue_key(item.key);
                    let renderer_mesh_handle = RendererMeshHandle::new(
                        key_fields.mesh_index.into(),
                        std::num::NonZeroU32::new(key_fields.mesh_version.into()).unwrap(),
                    );

                    // Check BLAS cache for this mesh and extract the BLAS handle.
                    // We clone the Blas handle to end the immutable borrow
                    // before the mutable scene.queue_tlas_instance call.
                    let blas_handle = match scene.blas_cache.get(&renderer_mesh_handle) {
                        Some(m) if m.build_state.is_ready() => m.blas.clone(),
                        _ => continue,
                    };

                    // Check ray visibility.
                    if !mrc.ray_visibility.ray_visible || !mrc.ray_visibility.casts_shadow {
                        continue;
                    }
                    if mrc.ray_visibility.mask == 0 {
                        continue;
                    }

                    // Get the transform and extract the model matrix.
                    let transform_slot = transform_component_storage
                        .data
                        .get(entity_index)
                        .and_then(|s| s.as_ref());
                    let Some(transform) = transform_slot else { continue; };

                    // Use the public accessor to get the cached model matrix.
                    let model = pill_engine::internal::get_model_matrix(transform);

                    // Convert to TLAS row-major 3x4 format.
                    let tlas_transform = match crate::ray_tracing::transform::model_to_tlas_transform(&model) {
                        Some(t) => t,
                        None => continue,
                    };

                    // Allocate a ray-instance ID.
                    let instance_id = match scene.instance_table.allocate() {
                        Some(id) => id,
                        None => continue,
                    };

                    // Build the TLAS instance.
                    let tlas_instance = wgpu::TlasInstance::new(
                        &blas_handle,
                        tlas_transform,
                        instance_id.index,
                        mrc.ray_visibility.mask,
                    );
                    scene.pending_tlas_instances.push(tlas_instance);

                    // Write GPU metadata.
                    scene.instance_table.write_instance(
                        &self.queue,
                        instance_id,
                        &crate::ray_tracing::instance_table::GpuRtInstance {
                            mesh_metadata_index: 0,
                            material_metadata_index: 0,
                            entity_debug_id: entity_index as u32,
                            flags: 0x1,
                        },
                    );

                    // Track the entity → instance ID mapping.
                    scene.entity_to_instance_id.insert(entity_index as u32, instance_id);
                }

                scene.build_acceleration_structures(&mut encoder);
            }
        }

        // let _timestamp_query_start = self.profiler.write_timestamp(&mut encoder, "Start Render Pass");

        // Render meshes
        {
            timer.record("Create render pass attachments");

            // Create color attachment
            let color_attachment = wgpu::RenderPassColorAttachment {
                view: &view,          // Specifies what texture to save the colors to
                depth_slice: None,
                resolve_target: None, // Specifies what texture will receive the resolved output
                ops: wgpu::Operations {
                    // Specifies what to do with the colors on the screen
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear_color.x as f64,
                        g: clear_color.y as f64,
                        b: clear_color.z as f64,
                        a: 1.0,
                    }), // Specifies how to handle colors stored from the previous frame
                    store: wgpu::StoreOp::Store,
                },
            };

            // Create depth attachment
            let depth_stencil_attachment = wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_texture.texture_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            };

            debug!(LogContext::Frame => "Start recording mesh draw commands");
            timer.begin_context("Mesh Drawer");

            self.mesh_drawer.record_draw_commands(
                &self.queue,
                &mut encoder,
                &self.renderer_resource_storage,
                color_attachment,
                depth_stencil_attachment,
                renderer_camera,
                render_queue,
                transform_component_storage,
                timer,
                #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
                self.rt_engine_bind_group.as_ref(),
                #[cfg(not(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32"))))]
                None::<&wgpu::BindGroup>,
                //&mut self.profiler
            )?;

            timer.end_context()?;
        }

        // Render egui UI
        #[cfg(feature = "debug_ui")]
        if let Some(egui_ui) = self.pending_egui_ui.take() {
            timer.begin_context("Egui Draw");
            debug!(LogContext::Frame => "Start recording egui draw commands");

            let egui_draw_output = self.egui_drawer.record_draw_commands(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [
                        self.surface_configuration.width,
                        self.surface_configuration.height,
                    ],
                    pixels_per_point: self.egui_drawer.window_scale_factor,
                },
                egui_ui,
                timer,
            )?;
            additional_command_buffers.extend(egui_draw_output.command_buffers);
            egui_textures_to_free = egui_draw_output.textures_to_free;

            timer.end_context()?; // End Egui Draw context
        }
        // let _timestamp_query_end = self.profiler.write_timestamp(&mut encoder, "End Render Pass");

        // Resolve queries recorded this frame
        // self.profiler.resolve_timestamp_queries(&self.device, &mut encoder);
        // self.profiler.resolve_occlusion_queries(&self.device, &mut encoder);
        // self.profiler.resolve_pipeline_statistics_queries(&self.device, &mut encoder);

        timer.record("Submit commands and present frame");

        // Submit the command buffer to the GPU
        self.queue.submit(
            additional_command_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );

        // Advance RT submission tracking.
        #[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
        if let Some(scene) = self.ray_tracing_state.scene_mut() {
            scene.on_submission();
        }

        #[cfg(feature = "debug_ui")]
        self.egui_drawer.free_textures(&egui_textures_to_free);

        timer.record("Read profiling data");

        //  self.profiler.end_frame();

        // Read profiling data
        //self.profiler.summarize_all_blocking(&self.device);

        // Present the frame
        self.queue.present(frame);

        Ok(())
    }
}

// ── Ray-tracing adapter and device negotiation ──────────────────────────

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
async fn select_adapter_with_rt_policy(
    instance: &wgpu::Instance,
    surface: &wgpu::Surface<'static>,
    rt_mode: RayTracingMode,
) -> Result<(wgpu::Adapter, RayTracingPolicyResult)> {
    use crate::ray_tracing::capability::{
        is_certified_rt_backend, validate_as_limits, RayTracingDisabledReason,
        RayTracingPolicyResult,
    };

    // Check compile-time preconditions.
    if let Some(reason) = check_compile_time_preconditions() {
        return match rt_mode {
            RayTracingMode::Require => Err(format!(
                "Ray tracing required but unavailable: {}",
                reason.as_str()
            )
            .into()),
            _ => {
                let adapter = instance
                    .request_adapter(&wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::default(),
                        compatible_surface: Some(surface),
                        force_fallback_adapter: false,
                        apply_limit_buckets: false,
                    })
                    .await
                    .context("Failed to request adapter")?;
                Ok((adapter, RayTracingPolicyResult::Disabled { reason }))
            }
        };
    }

    // Enumerate adapters and rank them for RT.
    let adapters = instance.enumerate_adapters(wgpu::Backends::all()).await;
    if adapters.is_empty() {
        return Err("No GPU adapters found".into());
    }

    // First pass: find an RT-capable adapter.
    let mut best_rt_adapter: Option<wgpu::Adapter> = None;
    let mut best_raster_adapter: Option<wgpu::Adapter> = None;

    for adapter in &adapters {
        let info = adapter.get_info();
        let features = adapter.features();

        // Check presentation support.
        if !surface.get_capabilities(adapter).formats.is_empty() {
            best_raster_adapter = Some(adapter.clone());

            // RT candidacy checks.
            if features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
                && is_certified_rt_backend(info.backend)
            {
                let limits = adapter.limits();
                if validate_as_limits(&limits, 1024).is_none() {
                    best_rt_adapter = Some(adapter.clone());
                    break; // Take the first qualifying adapter.
                }
            }
        }
    }

    match rt_mode {
        RayTracingMode::Off => {
            let adapter = best_raster_adapter
                .or(best_rt_adapter)
                .unwrap_or_else(|| adapters[0].clone());
            Ok((
                adapter,
                RayTracingPolicyResult::Disabled {
                    reason: RayTracingDisabledReason::PolicyOff,
                },
            ))
        }
        RayTracingMode::Prefer => {
            if let Some(rt_adapter) = best_rt_adapter {
                let limits = rt_adapter.limits();
                let capabilities = HardwareRayQueryCapabilities {
                    max_blas_primitive_count: limits.max_blas_primitive_count,
                    max_blas_geometry_count: limits.max_blas_geometry_count,
                    max_tlas_instance_count: limits.max_tlas_instance_count,
                    max_acceleration_structures_per_shader_stage: limits.max_acceleration_structures_per_shader_stage,
                    max_buffers_and_acceleration_structures_per_shader_stage: limits.max_buffers_and_acceleration_structures_per_shader_stage,
                };
                Ok((
                    rt_adapter,
                    RayTracingPolicyResult::Enabled { capabilities },
                ))
            } else {
                let adapter = best_raster_adapter
                    .unwrap_or_else(|| adapters[0].clone());
                warn!(LogContext::Rendering =>
                    "Prefer RT: no certified adapter with EXPERIMENTAL_RAY_QUERY found. Falling back to raster."
                );
                Ok((
                    adapter,
                    RayTracingPolicyResult::Disabled {
                        reason: RayTracingDisabledReason::PreferFallback {
                            reason: "no certified RT adapter".into(),
                        },
                    },
                ))
            }
        }
        RayTracingMode::Require => {
            match best_rt_adapter {
                Some(rt_adapter) => {
                    let limits = rt_adapter.limits();
                    let capabilities = HardwareRayQueryCapabilities {
                        max_blas_primitive_count: limits.max_blas_primitive_count,
                        max_blas_geometry_count: limits.max_blas_geometry_count,
                        max_tlas_instance_count: limits.max_tlas_instance_count,
                        max_acceleration_structures_per_shader_stage: limits.max_acceleration_structures_per_shader_stage,
                        max_buffers_and_acceleration_structures_per_shader_stage: limits.max_buffers_and_acceleration_structures_per_shader_stage,
                    };
                    Ok((
                        rt_adapter,
                        RayTracingPolicyResult::Enabled { capabilities },
                    ))
                }
                None => {
                    // Provide a precise diagnostic.
                    let reason = if let Some(adapter) = best_raster_adapter {
                        let info = adapter.get_info();
                        let features = adapter.features();
                        if !features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
                            RayTracingDisabledReason::FeatureBitAbsent
                        } else if !is_certified_rt_backend(info.backend) {
                            RayTracingDisabledReason::BackendNotSupported {
                                backend: format!("{:?}", info.backend),
                            }
                        } else {
                            RayTracingDisabledReason::RequiredLimitTooSmall {
                                limit_name: "unknown".into(),
                                required: 0,
                                actual: 0,
                            }
                        }
                    } else {
                        RayTracingDisabledReason::NoSurfaceCompatibleAdapter
                    };
                    Err(format!(
                        "Ray tracing required (RAY_TRACING_MODE=require) but cannot be enabled: {}",
                        reason.as_str()
                    )
                    .into())
                }
            }
        }
    }
}

#[cfg(all(feature = "hardware_ray_tracing", not(target_arch = "wasm32")))]
async fn request_device_with_rt_policy(
    adapter: &wgpu::Adapter,
    rt_policy: &RayTracingPolicyResult,
) -> Result<(wgpu::Device, wgpu::Queue)> {
    let baseline = wgpu::Features::DEPTH_CLIP_CONTROL
        | wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::PIPELINE_STATISTICS_QUERY;

    let (required_features, required_limits, experimental_features) = match rt_policy {
        RayTracingPolicyResult::Enabled { .. } => {
            let features = baseline | wgpu::Features::EXPERIMENTAL_RAY_QUERY;
            let limits = wgpu::Limits::default()
                .using_minimum_supported_acceleration_structure_values();
            // SAFETY: isolated opt-in to the pinned experimental ray-query API.
            // All AS descriptors, lifetimes, build ordering, and shader state
            // are validated by RayTracingScene and covered by GPU validation tests.
            let experimental = unsafe { wgpu::ExperimentalFeatures::enabled() };
            (features, limits, experimental)
        }
        RayTracingPolicyResult::Disabled { .. } => {
            let features = baseline & adapter.features();
            let limits = wgpu::Limits::default();
            let experimental = wgpu::ExperimentalFeatures::default();
            (features, limits, experimental)
        }
    };

    let device_descriptor = wgpu::DeviceDescriptor {
        label: None,
        required_features: required_features & adapter.features(),
        required_limits,
        experimental_features,
        memory_hints: wgpu::MemoryHints::default(),
        trace: wgpu::Trace::default(),
    };

    adapter
        .request_device(&device_descriptor)
        .await
        .map_err(|e| -> pill_core::PillError {
            format!("Failed to request device: {e}").into()
        })
}
