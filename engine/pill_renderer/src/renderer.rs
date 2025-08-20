// https://github.com/ejb004/egui-wgpu-demo/blob/master/src/lib.rs
// https://github.com/kaphula/winit-egui-wgpu-template/blob/master/src/main.rs
// https://github.com/emilk/egui/discussions/3067

use crate::{
    instance::Instance, mesh_renderer::MeshDrawer,  renderer_resource_storage::RendererResourceStorage, resources::{
        RendererCamera,
        RendererMaterial,
        RendererMesh,
        RendererPipeline,
        RendererTexture,
        Vertex
    }
};

use pill_engine::internal::{
    PillRenderer, 
    EntityHandle, 
    RenderQueueItem, 
    TextureType,
    MeshData, 
    MaterialTextureMap,
    TransformComponent,
    ComponentStorage, 
    CameraComponent,
    MaterialParameterMap,
    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererPipelineHandle,
    RendererTextureHandle, 
    RENDER_QUEUE_KEY_ORDER,
    get_renderer_resource_handle_from_camera_component,
};

use pill_core::{ 
    PillSlotMapKey, PillSlotMapKeyData, PillStyle, RendererError, Timer 
};

use std::{
    iter, mem::size_of, num::NonZeroU32, ops::Range, sync::Arc
};

use naga::front::glsl;
use naga::back::wgsl;

use anyhow::{Error, Result};
use log::{ info };

use crate::egui_drawer::EguiDrawer;

pub const MAX_INSTANCE_BATCH_SIZE: usize = 120000; // Maximum number of instances that can be drawn in a single draw call
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 120000;

// Default resource handle - Master pipeline
pub const MASTER_PIPELINE_HANDLE: RendererPipelineHandle = RendererPipelineHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
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

impl PillRenderer for Renderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self { 
        info!("Initializing {}", "Renderer".mobj_style());
        let state: State = pollster::block_on(State::new(window, config));

        Self {
            state,
        }
    }   

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Resizing {} resources", "Renderer".mobj_style());
        self.state.resize(new_window_size)
    }


    fn set_master_pipeline(&mut self, vertex_shader_bytes: &[u8], fragment_shader_bytes: &[u8]) -> Result<()> {
        
        // Create shaders
        // Convert bytes to string
        let vertex_shader_source = std::str::from_utf8(vertex_shader_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in vertex shader: {}", e))?;
        let fragment_shader_source = std::str::from_utf8(fragment_shader_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid UTF-8 in fragment shader: {}", e))?;


        // Convert GLSL to WGSL
        let vertex_wgsl = compile_glsl_to_wgsl(vertex_shader_source, naga::ShaderStage::Vertex).unwrap();
        let fragment_wgsl = compile_glsl_to_wgsl(fragment_shader_source, naga::ShaderStage::Fragment).unwrap();

        // Create shader modules with WGSL
        let vertex_shader = self.state.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });

        let fragment_shader = self.state.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });


        // Create master pipeline
        let master_pipeline = RendererPipeline::new(
            &self.state.device,
            vertex_shader,
            fragment_shader,
            self.state.color_format,
            Some(self.state.depth_format),
            &[RendererMesh::data_layout_descriptor(), Instance::data_layout_descriptor()],
        ).unwrap();

        self.state.renderer_resource_storage.pipelines.insert(master_pipeline);

        Ok(())
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data)?;
        let handle = self.state.renderer_resource_storage.meshes.insert(mesh);

        Ok(handle)
    }

    fn create_texture(&mut self, name: &str, image_data: &image::DynamicImage, texture_type: TextureType) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_texture(&self.state.device, &self.state.queue, Some(name), image_data, texture_type)?;
        let handle = self.state.renderer_resource_storage.textures.insert(texture);

        Ok(handle)
    }

    fn create_material(&mut self, name: &str, textures: &MaterialTextureMap, parameters: &MaterialParameterMap) -> Result<RendererMaterialHandle> {
        let pipeline_handle = MASTER_PIPELINE_HANDLE;
        let pipeline = self.state.renderer_resource_storage.pipelines.get(pipeline_handle).unwrap();

        let material = RendererMaterial::new(
            &self.state.device,
            &self.state.queue,
            &self.state.renderer_resource_storage,
            name,
            pipeline_handle,
            &pipeline.material_texture_bind_group_layout,
            textures,
            &pipeline.material_parameter_bind_group_layout,
            parameters,
        ).unwrap();

        let handle = self.state.renderer_resource_storage.materials.insert(material);

        Ok(handle)
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        let pipeline_handle = MASTER_PIPELINE_HANDLE;
        let pipeline = self.state.renderer_resource_storage.pipelines.get(pipeline_handle).unwrap();
        let camera_bind_group_layout = &pipeline.camera_bind_group_layout;
        let camera = RendererCamera::new(&self.state.device, camera_bind_group_layout)?;
        let handle = self.state.renderer_resource_storage.cameras.insert(camera);

        Ok(handle)
    }

    fn update_material_textures(&mut self, renderer_material_handle: RendererMaterialHandle, textures: &MaterialTextureMap) -> Result<()> {
        RendererMaterial::update_textures(&self.state.device, renderer_material_handle, &mut self.state.renderer_resource_storage, textures)
    }

    fn update_material_parameters(&mut self, renderer_material_handle: RendererMaterialHandle, parameters: &MaterialParameterMap) -> Result<()> {
        RendererMaterial::update_parameters(&self.state.device, &self.state.queue, renderer_material_handle, &mut self.state.renderer_resource_storage, parameters)
    }

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        self.state.renderer_resource_storage.meshes.remove(renderer_mesh_handle).unwrap();

        Ok(())
    }

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        self.state.renderer_resource_storage.textures.remove(renderer_texture_handle).unwrap();

        Ok(())
    }

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()> {
        self.state.renderer_resource_storage.materials.remove(renderer_material_handle).unwrap();

        Ok(())
    }

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        self.state.renderer_resource_storage.cameras.remove(renderer_camera_handle).unwrap();
        
        Ok(())
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &Vec<RenderQueueItem>, 
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        timer: &mut Timer
    ) -> Result<()> {
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage,
            egui_ui,
            timer
        )
    }
    
    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()> {
        self.state.egui_drawer.handle_input(event);
        Ok(())
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
    config: config::Config,
    //profiler: Profiler,
}

impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self {
        let window_size = window.inner_size();
        let window_ref = window.clone();

        // 1. Create instance and surface
        let (instance, surface) = {
            let backends = match std::env::var("WGPU_BACKENDS").as_deref() {
            Ok("VULKAN") => wgpu::Backends::VULKAN,
            Ok("DX12") => wgpu::Backends::DX12,
            Ok("METAL") => wgpu::Backends::METAL,
            Ok("GL") => wgpu::Backends::GL,
            Ok("BROWSER_WEBGPU") => wgpu::Backends::BROWSER_WEBGPU,
            _ => wgpu::Backends::all(),
        };

        let instance_descriptor = wgpu::InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::from_build_config().with_env(),
            backend_options: wgpu::BackendOptions::default(),
        };

let instance = wgpu::Instance::new(&instance_descriptor);
            let surface = instance.create_surface(window).expect("create surface");
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
                .expect("request adapter")
        };

        let info = adapter.get_info();
        info!("Using GPU: {} ({:?})", info.name, info.backend);

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
                .expect("request device")
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

        // 5. Renderer resources
        let (renderer_resource_storage, depth_texture) = {
            let renderer_resource_storage = RendererResourceStorage::new(&config);
            let depth_texture = RendererTexture::new_depth_texture(
                &device,
                &surface_configuration,
                "depth_texture",
            )
            .expect("create depth texture");
            (renderer_resource_storage, depth_texture)
        };

        // 6. Drawers
        let (mesh_drawer, egui_drawer) = {
            let mesh_drawer = MeshDrawer::new(&device, MAX_INSTANCE_BATCH_SIZE as u32);
            let egui_drawer = EguiDrawer::new(
                &device,
                surface_configuration.format,
                None,
                1,
                window_ref,
            );
            (mesh_drawer, egui_drawer)
        };

        // 7. Profiler
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
            // Drawers
            mesh_drawer,
            egui_drawer,
            // Other
            config,
           // profiler
        }
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
        render_queue: &Vec<RenderQueueItem>, 
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn FnMut(&egui::Context)>,
        timer: &mut Timer
    ) -> Result<()> { 
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

        timer.record("Get clear color and create render pass attachments");

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

            timer.begin_context("Mesh Drawer");

            self.mesh_drawer.record_draw_commands(
                &self.queue, 
                &mut encoder,
                &self.renderer_resource_storage, 
                color_attachment, 
                depth_stencil_attachment, 
                &renderer_camera,
                &render_queue, 
                &transform_component_storage,
                timer,
                //&mut self.profiler
            )?;

            timer.end_context()?;
        }  

        // Render egui UI
        {
            timer.begin_context("Egui Draw");

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
