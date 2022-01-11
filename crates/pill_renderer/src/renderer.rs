use crate::{
    resources::{
        RendererCamera,
        RendererMaterial,
        RendererMesh,
        RendererPipeline,
        RendererTexture,
        Vertex
    }, 
    instance::Instance, 
    renderer_resource_storage::RendererResourceStorage
};

use pill_engine::internal::{
    PillRenderer, 
    EntityHandle, 
    RenderQueueItem, 
    RendererError, 
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
    ResourceManager, 
    RendererTextureHandle, 
    RENDER_QUEUE_KEY_ITEMS_LENGTH, 
    RENDER_QUEUE_KEY_ORDER_IDX, 
    RENDER_QUEUE_KEY_ORDER,
    get_renderer_resource_handle_from_camera_component,
};

use pill_core::{ 
    PillSlotMap, 
    PillSlotMapKey, 
    PillSlotMapKeyData, 
    PillStyle 
};

use std::{
    collections::{ LinkedList, HashMap },
    convert::TryInto,
    env,
    iter,
    num::{ NonZeroU32, NonZeroU8 },
    path::{ Path, PathBuf },
    ops::Range,
    mem::size_of,
};

use anyhow::{ Result, Context, Error };
use cgmath::{ Rotation3, Zero };
use slab::Slab;
use log::{debug, info};

// Default resource handle - Master pipeline
pub const MASTER_PIPELINE_HANDLE: RendererPipelineHandle = RendererPipelineHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};

pub struct Renderer {
    pub state: State,
}

impl PillRenderer for Renderer {
    fn new(window: &winit::window::Window, max_pipelines_count: usize, max_textures_count: usize, max_materials_count: usize, max_meshes_count: usize, max_cameras_count: usize) -> Self { 
        info!("Initializing {}", "Renderer".mobj_style());
        let state: State = pollster::block_on(State::new(&window, max_pipelines_count, max_textures_count, max_materials_count, max_meshes_count, max_cameras_count));

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
        let vertex_shader = wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::util::make_spirv(vertex_shader_bytes),
        };
        let vertex_shader = self.state.device.create_shader_module(&vertex_shader);

        let fragment_shader = wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::util::make_spirv(fragment_shader_bytes),
        };
        let fragment_shader = self.state.device.create_shader_module(&fragment_shader);

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
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) -> Result<(), RendererError> {
        self.state.render(
            active_camera_entity_handle,
            render_queue,
            camera_component_storage,
            transform_component_storage)
    }
}

pub struct State {
    // Resources
    renderer_resource_storage: RendererResourceStorage,

    // Renderer variables
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    window_size: winit::dpi::PhysicalSize<u32>, 

    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,

    depth_texture: RendererTexture,
    mesh_drawer: MeshDrawer,
}


impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: &winit::window::Window, max_pipelines_count: usize, max_textures_count: usize, max_materials_count: usize, max_meshes_count: usize, max_cameras_count: usize) -> Self {
        let window_size = window.inner_size();

        let instance = wgpu::Instance::new(wgpu::Backends::all()); // BackendBit::PRIMARY => Vulkan + Metal + DX12 + Browser WebGPU
        let surface = unsafe { instance.create_surface(window) }; 
        
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


        // [TODO]: Use iteration
        // let adapter = instance // Iterates over all possible adapters for the backend and gets first that support given surface
        //     .enumerate_adapters(wgpu::Backends::PRIMARY)
        //     .filter(|adapter| { adapter.is_surface_supported(&surface) }) // Check if this adapter supports our surface
        //     .next()
        //     .unwrap();
        
        // Create device descriptor
        let device_descriptor = wgpu::DeviceDescriptor {
            label: None,
            features: wgpu::Features::empty(), // Allows to specify what extra features of GPU that needs to be included (e.g. depth clamping, push constants, texture compression, etc)
            limits: wgpu::Limits::default(), // Allows to specify the limit of certain types of resources that will be used (e.g. max samplers, uniform buffers, etc)
        };

        // Create device and queue
        let (device, queue) = adapter.request_device(&device_descriptor,None).await.unwrap();

        // Specify surface configuration
        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, // Defines how the swap_chain's underlying textures will be used
            format: surface.get_preferred_format(&adapter).unwrap(), // Defines how the swap_chain's textures will be stored on the gpu
            width: window_size.width,
            height: window_size.height,
            present_mode: wgpu::PresentMode::Fifo, // Defines how to sync the surface with the display
        };

        // Configure surface
        surface.configure(&device, &surface_configuration);

        // Configure collections
        let renderer_resource_storage = RendererResourceStorage::new(max_pipelines_count, max_textures_count, max_materials_count, max_meshes_count, max_cameras_count);

        // Create depth and color texture
        let depth_texture = RendererTexture::new_depth_texture(
            &device, 
            &surface_configuration, 
            "depth_texture"
        ).unwrap();

        let color_format = surface_configuration.format;
        let depth_format = wgpu::TextureFormat::Depth32Float;

        // Create drawing state
        let mesh_drawer = MeshDrawer::new(&device, 1000); // [TODO] move magic value to config

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
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) -> Result<(), RendererError> { 
    
        // Get frame or return mapped error if failed
        let frame = self.surface.get_current_texture();

        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => match error {
                wgpu::SurfaceError::Lost => return Err(RendererError::SurfaceLost),
                wgpu::SurfaceError::OutOfMemory => return Err(RendererError::SurfaceOutOfMemory),
                _ => return Err(RendererError::SurfaceOther),
            },
        };

        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Get active camera and update it
        let camera_storage = camera_component_storage.data.get(active_camera_entity_handle.data().index as usize).unwrap().borrow();
        let active_camera_component = camera_storage.as_ref().unwrap();
        let renderer_camera = self.renderer_resource_storage.cameras.get_mut(get_renderer_resource_handle_from_camera_component(active_camera_component)).ok_or(RendererError::RendererResourceNotFound)?;
        let camera_transform_storage = transform_component_storage.data.get(active_camera_entity_handle.data().index as usize).unwrap().borrow();
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
                store: true,
                },
            };

            // Create depth attachment
            let depth_stencil_attachment = wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_texture.texture_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: true,
                }),
                stencil_ops: None,
            };

            self.mesh_drawer.draw(
                &self.queue, 
                &mut encoder, 
                &self.renderer_resource_storage, 
                color_attachment, 
                depth_stencil_attachment, 
                &renderer_camera,
                &render_queue, 
                &transform_component_storage
            )
        }

        self.queue.submit(iter::once(encoder.finish())); // Finish command buffer and submit it to the GPU's render queue
        frame.present();

        Ok(())
    }
}

pub struct MeshDrawer {
    current_order: u8,
    current_pipeline_handle: Option<RendererPipelineHandle>,
    current_material_handle: Option<RendererMaterialHandle>,
    current_mesh_handle: Option<RendererMeshHandle>,
    current_mesh_index_count: u32,

    max_instance_count: u32,
    instances: Vec::<Instance>,
    instance_buffer: wgpu::Buffer,
    instance_range: Range<u32>,
}

impl MeshDrawer {
    pub fn new(device: &wgpu::Device, max_instance_count: u32) -> Self {

        // Create instance buffer
        let buffer_size = (size_of::<Instance>() * max_instance_count as usize) as u64;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MeshDrawer {
            current_order: 0,
            //current_camera_handle: None,
            current_pipeline_handle: None,
            current_material_handle: None,
            current_mesh_handle: None,
            current_mesh_index_count: 0,

            max_instance_count,
            instances: Vec::<Instance>::with_capacity(max_instance_count as usize), 
            instance_buffer,
            instance_range: 0..0, // Start inclusive, end exclusive (e.g. 0..3 means indices 0, 1, 2.  e.g. 5..7 means indices 5, 6)
        }
    }

    pub fn draw(
        &mut self, 
        // Resources
        queue: &wgpu::Queue, 
        encoder: &mut wgpu::CommandEncoder, 
        renderer_resource_storage: &RendererResourceStorage, 
        color_attachment: wgpu::RenderPassColorAttachment, 
        depth_stencil_attachment: wgpu::RenderPassDepthStencilAttachment,
        // Rendring data
        camera: &RendererCamera,
        render_queue: &Vec::<RenderQueueItem>, 
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) {
        // Prepare instance data and load it to buffer
        if render_queue.len() as u32 > self.max_instance_count {
            panic!() // [TODO] Throw renderer error
        }

        let render_queue_iter = render_queue.iter();
        for render_queue_item in render_queue_iter {
            let transform_storage =  transform_component_storage.data.get(render_queue_item.entity_index as usize).unwrap().borrow();
            let transform_component = transform_storage.as_ref().unwrap();
            self.instances.push(Instance::new(transform_component));
        }
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances)); // Update instance buffer
        self.instances.clear();

        // Start encoding render pass
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { // Use the encoder to create a RenderPass
            label: Some("render_pass"),
            color_attachments: &[color_attachment],
            depth_stencil_attachment: Some(depth_stencil_attachment),
        });

        render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer

        let render_queue_iter = render_queue.iter();
        for render_queue_item in render_queue_iter {
            
            let render_queue_key_fields = pill_engine::internal::decompose_render_queue_key(render_queue_item.key).unwrap();

            // Recreate resource handles
            let renderer_material_handle = RendererMaterialHandle::new(render_queue_key_fields.material_index.into(), NonZeroU32::new(render_queue_key_fields.material_version.into()).unwrap());
            let renderer_mesh_handle = RendererMeshHandle::new(render_queue_key_fields.mesh_index.into(), NonZeroU32::new(render_queue_key_fields.mesh_version.into()).unwrap());

            // Check order
            if self.current_order > render_queue_key_fields.order {
                if self.get_accumulated_instance_count() > 0 {
                    render_pass.draw_indexed(0..self.current_mesh_index_count, 0, self.instance_range.clone());         
                    self.instance_range = self.instance_range.end..self.instance_range.end;
                }
                // Set new order
                self.current_order = render_queue_key_fields.order;
            }

            // Check material
            if self.current_material_handle != Some(renderer_material_handle) {
                // Render accumulated instances
                if self.get_accumulated_instance_count() > 0 {
                    render_pass.draw_indexed(0..self.current_mesh_index_count, 0, self.instance_range.clone());            
                    self.instance_range = self.instance_range.end..self.instance_range.end;
                }
                // Set new material
                self.current_material_handle = Some(renderer_material_handle);
                let material = renderer_resource_storage.materials.get(self.current_material_handle.unwrap()).unwrap();
               
                // Set pipeline if new material is using different one
                if self.current_pipeline_handle != Some(material.pipeline_handle) {
                    self.current_pipeline_handle = Some(material.pipeline_handle);
                    let pipeline = renderer_resource_storage.pipelines.get( self.current_pipeline_handle.unwrap()).unwrap();
                    render_pass.set_pipeline(&pipeline.render_pipeline);
                }

                render_pass.set_bind_group(0, &material.texture_bind_group, &[]);
                render_pass.set_bind_group(1, &material.parameter_bind_group, &[]);
                render_pass.set_bind_group(2, &camera.bind_group, &[]);
            }

            // Check mesh
            if self.current_mesh_handle != Some(renderer_mesh_handle) {
                // Render accumulated instances
                if self.get_accumulated_instance_count() > 0 {
                    render_pass.draw_indexed(0..self.current_mesh_index_count, 0, self.instance_range.clone());      
                    self.instance_range = self.instance_range.end..self.instance_range.end; 
                }
                // Set new mesh
                self.current_mesh_handle = Some(renderer_mesh_handle);               
                let mesh = renderer_resource_storage.meshes.get(self.current_mesh_handle.unwrap()).unwrap();
                self.current_mesh_index_count = mesh.index_count;
                render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..)); 
                render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32); 
            }

            // Add new instance
            self.instance_range = self.instance_range.start..self.instance_range.end + 1;
        }

        // End of render queue so draw remaining saved objects
        if self.get_accumulated_instance_count() > 0 {
            render_pass.draw_indexed(0..self.current_mesh_index_count, 0, self.instance_range.clone());    
            self.instance_range = self.instance_range.end..self.instance_range.end; 
        }

        // Reset state of mesh drawer
        self.current_order = RENDER_QUEUE_KEY_ORDER.max as u8;
        self.current_pipeline_handle = None;
        self.current_material_handle = None;
        self.current_mesh_handle = None;
        self.current_mesh_index_count = 0;
        self.instance_range = 0..0; 
        
    }

    fn get_accumulated_instance_count(&self) -> u32 {
        self.instance_range.end - self.instance_range.start 
    }
}