// https://github.com/ejb004/egui-wgpu-demo/blob/master/src/lib.rs
// https://github.com/kaphula/winit-egui-wgpu-template/blob/master/src/main.rs
// https://github.com/emilk/egui/discussions/3067

use crate::{
    instance::Instance, mesh_drawer::MeshDrawer, renderer_resource_storage::RendererResourceStorage, resources::{
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

use crate::egui::EguiRenderer;

pub const MAX_INSTANCE_BATCH_SIZE: usize = 10000; // Maximum number of instances that can be drawn in a single draw call
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 10000;

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
        egui_ui: Box<dyn Fn(&egui::Context)>,
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
        self.state.egui_renderer.handle_input(event);
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
    mesh_drawer: MeshDrawer,
    // Other
    config: config::Config,
    egui_renderer: crate::egui::EguiRenderer,
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
        let adapter = instance.request_adapter(&request_adapter_options).await.unwrap();
        let adapter_info = adapter.get_info();
        info!("Using GPU: {} ({:?})", adapter_info.name, adapter_info.backend);
        
        let features = wgpu::Features::DEPTH_CLIP_CONTROL;

        // Create device descriptor
        let device_descriptor = wgpu::DeviceDescriptor {
            label: None,
            required_features: features, // Allows to specify what extra features of GPU that needs to be included (e.g. depth clamping, push constants, texture compression, etc)
            required_limits: wgpu::Limits::default(), // Allows to specify the limit of certain types of resources that will be used (e.g. max samplers, uniform buffers, etc)
            //memory_hints: wgpu::MemoryHints::MemoryUsage, 
        };

        // Create device and queue
        let (device, queue) = adapter.request_device(&device_descriptor,None).await.unwrap();

        // Specify surface configuration
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, // Defines how the swap_chain's underlying textures will be used
            format: format, // Defines how the swap_chain's textures will be stored on the gpu
            width: window_size.width,
            height: window_size.height,
            desired_maximum_frame_latency: 2,
            present_mode: wgpu::PresentMode::Mailbox, // Defines how to sync the surface with the display
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![format],
        };

        // Configure surface
        surface.configure(&device, &surface_configuration);

        // Configure collections
        let renderer_resource_storage = RendererResourceStorage::new(&config);

        // Create depth and color texture
        let depth_texture = RendererTexture::new_depth_texture(
            &device, 
            &surface_configuration, 
            "depth_texture"
        ).unwrap();

        let color_format = surface_configuration.format;
        let depth_format = wgpu::TextureFormat::Depth32Float;

        // Create drawing state
        let mesh_drawer = MeshDrawer::new(&device, MAX_INSTANCE_BATCH_SIZE as u32);

        let egui_renderer = EguiRenderer::new(
            &device,
            surface_configuration.format, 
            None, 
            1,            
            window_ref,
        );
        
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
            mesh_drawer,
            // Other
            config,
            egui_renderer
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
        egui_ui: Box<dyn Fn(&egui::Context)>,
        timer: &mut Timer
    ) -> Result<()> { 
        timer.record("Get frame");
    
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

        { // Additional scope to release mutable borrow of encoder done by begin_render_pass
            
            // Create color attachment
            let color_attachment = wgpu::RenderPassColorAttachment {
                view: &view, // Specifies what texture to save the colors to
                resolve_target: None, // Specifies what texture will receive the resolved output
                ops: wgpu::Operations { // Specifies what to do with the colors on the screen
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: clear_color.x as f64, g: clear_color.y as f64, b: clear_color.z as f64, a: 1.0, } ), // Specifies how to handle colors stored from the previous frame
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

            timer.begin_context("Mesh Drawer");

            self.mesh_drawer.record_draw_commands(
                &self.queue, 
                &self.device,
                &self.renderer_resource_storage, 
                color_attachment, 
                depth_stencil_attachment, 
                &renderer_camera,
                &render_queue, 
                &transform_component_storage,
                timer
            )?;

            timer.end_context()?;
        }  

        timer.begin_context("Egui Draw");

        timer.end_context()?; // End Egui Draw context

        timer.record("Submit commands and present frame");

        frame.present();

        Ok(())
    }
}
