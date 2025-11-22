use crate::{
    config::MAX_INSTANCE_PER_DRAWCALL_COUNT,
    instance::Instance,
    drawers::mesh_drawer::MeshDrawer,
    drawers::egui_drawer::EguiDrawer,
    resources::{
        RendererResourceStorage,
        RendererCamera,
        RendererMaterial,
        RendererMesh,
        RendererShader,
        RendererTexture,
        Vertex
    }
};

use indexmap::IndexMap;

use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, CameraComponent, ComponentStorage, EntityHandle, MaterialParameter, MaterialTexture, MeshData, PillRenderer, RenderQueueItem, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle, RendererTextureHandle, ShaderParameterSlot, ShaderTextureSlot, TextureType, TransformComponent, RENDER_QUEUE_KEY_ORDER
};

use pill_core::{
    debug,
    info,
    LogContext,
    PillSlotMapKey,
    PillSlotMapKeyData,
    PillStyle,
    RendererError,
    Timer
};

use std::{
    collections::HashMap,
    iter,
    mem::size_of,
    num::NonZeroU32,
    ops::Range,
    sync::Arc
};

use naga::front::glsl;
use naga::back::wgsl;

use anyhow::{Context, Error, Ok, Result};

pub struct Renderer {
    pub state: State,
}

impl PillRenderer for Renderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Result<Self> {
        info!(LogContext::Rendering => "Initializing {}", "Renderer".module_object_style());
        let state: State = pollster::block_on(State::new(window, config))?;

        Ok(Self {
            state,
        })
    }

    // --- Create ---

    fn create_shader(
        &mut self,
        name: &str,
        vertex_shader_bytes: &[u8],
        fragment_shader_bytes: &[u8],
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        parameter_slots: &HashMap<String, ShaderParameterSlot>,
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<RendererShaderHandle> {
        let shader = RendererShader::new(
            name,
            &self.state.device,
            self.state.color_format,
            Some(self.state.depth_format),
            &[RendererMesh::data_layout_descriptor(), Instance::data_layout_descriptor()],
            vertex_shader_bytes,
            fragment_shader_bytes,
            parameter_slots,
            texture_slots,
            &self.state.renderer_resource_storage.engine_parameters.bind_group_layout,
            &self.state.camera_bind_group_layout,
            pass_engine_parameters,
            pass_camera_parameters
        )?;
        let handle = self.state.renderer_resource_storage.shaders.insert(shader);

        Ok(handle)
    }

    fn create_material(
        &mut self,
        name: &str,
        renderer_shader_handle: RendererShaderHandle,
        textures: &IndexMap<String, MaterialTexture>,
        parameters: &HashMap<String, MaterialParameter>
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
        let handle = self.state.renderer_resource_storage.materials.insert(material);
        Ok(handle)
    }

    fn create_texture(&mut self, name: &str, image_data: &image::DynamicImage, texture_type: TextureType) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_texture(&self.state.device, &self.state.queue, Some(name), image_data, texture_type)?;
        let handle = self.state.renderer_resource_storage.textures.insert(texture);
        Ok(handle)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data)?;
        let handle = self.state.renderer_resource_storage.meshes.insert(mesh);
        Ok(handle)
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        let camera = RendererCamera::new(&self.state.device, self.state.camera_bind_group_layout.clone())?;
        let handle = self.state.renderer_resource_storage.cameras.insert(camera);
        Ok(handle)
    }

    // --- Update ---

    fn update_material_textures(&mut self, renderer_material_handle: RendererMaterialHandle, textures: &IndexMap<String, MaterialTexture>) -> Result<()> {
        RendererMaterial::update_textures(
            &self.state.device,
            renderer_material_handle,
            &mut self.state.renderer_resource_storage,
            textures
        )
    }

    fn update_material_parameters(&mut self, renderer_material_handle: RendererMaterialHandle, parameters: &HashMap<String, MaterialParameter>) -> Result<()> {
        RendererMaterial::update_parameters(
            &self.state.device,
            &self.state.queue,
            renderer_material_handle,
            &mut self.state.renderer_resource_storage,
            parameters
        )
    }

    // --- Destroy ---

    fn destroy_shader(&mut self, renderer_shader_handle: RendererShaderHandle) -> Result<()> {
        self.state.renderer_resource_storage.shaders.remove(renderer_shader_handle).unwrap();

        // TODO: Check if there are no materials using this shader (engine should replace them with default shader), if there are prevent shader destruction
        Ok(())
    }

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()> {
        self.state.renderer_resource_storage.materials.remove(renderer_material_handle).unwrap();
        Ok(())
    }

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        self.state.renderer_resource_storage.textures.remove(renderer_texture_handle).unwrap();
        Ok(())
    }

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        self.state.renderer_resource_storage.meshes.remove(renderer_mesh_handle).unwrap();
        Ok(())
    }

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        self.state.renderer_resource_storage.cameras.remove(renderer_camera_handle).unwrap();
        Ok(())
    }

    // --- Other ---

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!(LogContext::Rendering => "Resizing {} resources", "Renderer".module_object_style());
        self.state.resize(new_window_size)
    }

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()> {
        self.state.egui_drawer.handle_input(event);
        Ok(())
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        delta_time: f32,
        timer: &mut Timer
    ) -> Result<()> {
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage,
            egui_ui,
            delta_time,
            timer
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
    egui_drawer: EguiDrawer,
    // Other
    camera_bind_group_layout: wgpu::BindGroupLayout,
    config: config::Config,
    //profiler: Profiler,
}

impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: Arc<winit::window::Window>, config: config::Config) -> Result<Self> {
        let window_size = window.inner_size();
        let window_ref = window.clone();

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
        };

        let instance = wgpu::Instance::new(&instance_descriptor);
            let surface = instance.create_surface(window).context("Failed to create surface")?;
            (instance, surface)
        };

        // 2. Adapter
        let adapter = {
            let request_adapter_options = wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            };
            instance
                .request_adapter(&request_adapter_options)
                .await
                .context("Failed to request adapter")?
        };

        let info = adapter.get_info();
        info!(LogContext::Rendering => "Using GPU: {} ({:?})", info.name, info.backend);

        // 3. Device and queue
        let (device, queue) = {
            let features = wgpu::Features::DEPTH_CLIP_CONTROL
                | wgpu::Features::TIMESTAMP_QUERY
                | wgpu::Features::PIPELINE_STATISTICS_QUERY;

            let device_descriptor = wgpu::DeviceDescriptor {
                label: None,
                required_features: features,
                required_limits: wgpu::Limits::default(),
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
            let format = wgpu::TextureFormat::Rgba8UnormSrgb;
            let surface_configuration = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                width: window_size.width,
                height: window_size.height,
                desired_maximum_frame_latency: 2,
                present_mode: wgpu::PresentMode::Mailbox,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: vec![format],
            };
            surface.configure(&device, &surface_configuration);
            let color_format = surface_configuration.format;
            let depth_format = wgpu::TextureFormat::Depth32Float;
            (surface_configuration, color_format, depth_format)
        };

        // 5. Depth texture
        let depth_texture = RendererTexture::new_depth_texture(
                &device,
                &surface_configuration,
                "depth_texture",
            )
            .context("Failed to create depth texture")?;

        // 6. Define camera bind group layout
        // Each camera instance has the same bind group layout is we define it here once
        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera_parameters_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, // (set = X, binding = 0)
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            }]
        });

        // 7. Resource storage
        let renderer_resource_storage = RendererResourceStorage::new(&device, &config)?;

        // 8. Drawers
        let (mesh_drawer, egui_drawer) = {
            let mesh_drawer = MeshDrawer::new(&device, MAX_INSTANCE_PER_DRAWCALL_COUNT as u32);
            let egui_drawer = EguiDrawer::new(
                &device,
                surface_configuration.format,
                None,
                1,
                window_ref,
            );
            (mesh_drawer, egui_drawer)
        };

        // 9. Profiler
        // let profiler = {
        //     Profiler::new(
        //         &device,
        //         &queue,
        //         &adapter,
        //         16, // up to 16 timestamp marks per frame
        //         64, // up to 64 occlusion queries per frame
        //         64, // up to 64 pipeline statistics queries per frame
        //         wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
        //             | wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,
        //     )
        // };

        // Create state
        let renderer = Self {
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
            egui_drawer,
            // Other
            camera_bind_group_layout,
            config,
           // profiler
        };

        Ok(renderer)
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        if new_window_size.width > 0 && new_window_size.height > 0 {
            self.window_size = new_window_size;
            self.surface_configuration.width = new_window_size.width;
            self.surface_configuration.height = new_window_size.height;
            self.surface.configure(&self.device, &self.surface_configuration);
            self.depth_texture = RendererTexture::new_depth_texture(
                &self.device,
                &self.surface_configuration,
                "depth_texture",
            ).unwrap();
        }
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        delta_time: f32,
        timer: &mut Timer
    ) -> Result<()> {

        debug!(LogContext::Frame => "Starting frame render");

        timer.record("Get frame");
        // self.profiler.begin_frame();

        // Get frame or return mapped error if failed
        let frame = self.surface.get_current_texture();
        let frame = match frame {
            std::result::Result::Ok(frame) => frame,
            std::result::Result::Err(error) => match error {
                wgpu::SurfaceError::Lost => return Err(RendererError::SurfaceLost.into()),
                wgpu::SurfaceError::OutOfMemory => return Err(RendererError::SurfaceOutOfMemory.into()),
                _ => return Err(RendererError::SurfaceOther.into()),
            },
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        timer.record("Update engine parameters");

        self.renderer_resource_storage.engine_parameters.update(&self.queue, delta_time);

        timer.record("Update camera parameters");

        // Get active camera and update it
        let camera_storage = camera_component_storage.data.get(active_camera_entity_handle.data().index as usize).unwrap();
        let active_camera_component = camera_storage.as_ref().unwrap();
        let renderer_camera = self.renderer_resource_storage.cameras.get_mut(get_renderer_resource_handle_from_camera_component(active_camera_component)).ok_or(Error::new(RendererError::RendererResourceNotFound))?;
        let camera_transform_storage = transform_component_storage.data.get(active_camera_entity_handle.data().index as usize).unwrap();
        let active_camera_transform_component = camera_transform_storage.as_ref().unwrap();
        renderer_camera.update(&self.queue, active_camera_component, active_camera_transform_component);
        let renderer_camera = self.renderer_resource_storage.cameras.get(get_renderer_resource_handle_from_camera_component(active_camera_component)).unwrap();
        let clear_color = active_camera_component.clear_color;

        // Build a command buffer that can be sent to the GPU
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render_encoder"),
        });


 // let _timestamp_query_start = self.profiler.write_timestamp(&mut encoder, "Start Render Pass");

        // Render meshes
        {
            timer.record("Create render pass attachments");

            // Create color attachment
            let color_attachment = wgpu::RenderPassColorAttachment {
                view: &view, // Specifies what texture to save the colors to
                resolve_target: None, // Specifies what texture will receive the resolved output
                ops: wgpu::Operations { // Specifies what to do with the colors on the screen
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear_color.x as f64, g: clear_color.y as f64, b: clear_color.z as f64, a: 1.0, } ), // Specifies how to handle colors stored from the previous frame
                    store: wgpu::StoreOp::Store,
                },
                //depth_slice: None,
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
                //&mut self.profiler
            )?;

            timer.end_context()?;
        }

        // Render egui UI
        {
            timer.begin_context("Egui Draw");
            debug!(LogContext::Frame => "Start recording egui draw commands");

            // Render egui UI
            self.egui_drawer.record_draw_commands(
                &self.device,
                &self.queue,
                &mut encoder,
                &view,
                egui_wgpu::ScreenDescriptor {
                    size_in_pixels: [self.surface_configuration.width, self.surface_configuration.height],
                    pixels_per_point: self.egui_drawer.window_scale_factor,
                },
                egui_ui,
                timer
            )?;

            timer.end_context()?; // End Egui Draw context
        }
 // let _timestamp_query_end = self.profiler.write_timestamp(&mut encoder, "End Render Pass");

        // Resolve queries recorded this frame
        // self.profiler.resolve_timestamp_queries(&self.device, &mut encoder);
        // self.profiler.resolve_occlusion_queries(&self.device, &mut encoder);
        // self.profiler.resolve_pipeline_statistics_queries(&self.device, &mut encoder);

        timer.record("Submit commands and present frame");

        // Submit the command buffer to the GPU
        self.queue.submit(std::iter::once(encoder.finish()));

        timer.record("Read profiling data");

      //  self.profiler.end_frame();

        // Read profiling data
        //self.profiler.summarize_all_blocking(&self.device);

        // Present the frame
        frame.present();

        Ok(())
    }
}
