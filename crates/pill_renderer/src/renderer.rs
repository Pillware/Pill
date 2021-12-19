
use anyhow::{Result, Context, Error};

use cgmath::Rotation3;


use pill_core::PillSlotMap;
use pill_core::PillSlotMapKey;
use pill_core::PillSlotMapKeyData;
use pill_engine::internal::*;


use wgpu::ShaderModule;
use wgpu::ShaderModuleDescriptor;
use wgpu::SurfaceError;
use slab::Slab;
use log::{debug, info};

use crate::camera::RendererCamera;
use crate::instance::Instance;
use crate::instance::MatrixAngleExt;
use crate::instance::MatrixModelExt;
use crate::material::RendererMaterial;

use pill_engine::internal::{
    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererTextureHandle,
    RendererPipelineHandle,
};

use crate::mesh::RendererMesh;
use crate::mesh::Vertex;
use crate::pipeline::RendererPipeline;
use crate::texture::RendererTexture;
use crate::camera;
use crate::texture;
//use crate::texture::RendererTextureHandle;


use std::borrow::Borrow;
use std::borrow::BorrowMut;
use std::mem::size_of;
use std::num::NonZeroU32;
use std::num::NonZeroU8;
use std::path;
use std::path::Path;

use std::iter;
use std::path::PathBuf;
use cgmath::Zero;
use cgmath::prelude::*;
use wgpu::util::DeviceExt;
use winit::{ // Import dependencies
    event::*, // Bring all public items into scope
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use std::collections::LinkedList;
use std::collections::HashMap;

// [TODO] Assure that it cannot be removed!
fn get_master_pipeline_handle() -> RendererPipelineHandle { // [TODO] Very ugly solution, maybe store there handles in some hashmap with names of pipelines as keys
    RendererPipelineHandle::new(1, std::num::NonZeroU32::new(1).unwrap() )
}

pub struct Renderer {
    pub state: State,
}

impl PillRenderer for Renderer {
    fn new(window: &Window) -> Self { 
        let state: State = pollster::block_on(State::new(&window));

        let mut renderer = Renderer {
            state,
        };

        // Load default resource data to/from executable
        let default_color_texture_bytes = include_bytes!("../res/textures/default_color.png");
        renderer.create_texture_from_bytes(default_color_texture_bytes, "DefaultColor", TextureType::Color).unwrap();

        renderer
    }

    fn initialize(&self) {
        info!("Pill Renderer initialize");
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

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Renderer resizing!");
        self.state.resize(new_window_size)
    }

    fn create_texture(&mut self, path: &PathBuf, name: &str, texture_type: TextureType) -> Result<RendererTextureHandle, RendererError> {
        let texture = RendererTexture::new_texture_from_image(&self.state.device, &self.state.queue, path, name, texture_type).unwrap();
        let handle = self.state.rendering_resource_storage.textures.insert(texture);
        Ok(handle)
    }

    fn create_texture_from_bytes(&mut self, bytes: &[u8], name: &str, texture_type: TextureType) -> Result<RendererTextureHandle, RendererError> {
        let texture = RendererTexture::new_texture_from_bytes(&self.state.device, &self.state.queue, bytes, name, texture_type).unwrap();
        let handle = self.state.rendering_resource_storage.textures.insert(texture);
        Ok(handle)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle, RendererError> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data).unwrap();
        let handle = self.state.rendering_resource_storage.meshes.insert(mesh);
        Ok(handle)
    }

    fn create_material(&mut self, name: &str, color_texture_renderer_handle: RendererTextureHandle, normal_texture_renderer_handle: RendererTextureHandle) -> Result<RendererMaterialHandle, RendererError> {
        let color_texture = self.state.rendering_resource_storage.textures.get(color_texture_renderer_handle).unwrap();
        let normal_texture = self.state.rendering_resource_storage.textures.get(normal_texture_renderer_handle).unwrap();

        let pipeline_handle = get_master_pipeline_handle();
        let pipeline = self.state.rendering_resource_storage.pipelines.get(pipeline_handle).unwrap();
        let texture_bind_group_layout = &pipeline.texture_bind_group_layout;

        let material = RendererMaterial::new(
            &self.state.device,
            name, 
            pipeline_handle,
            color_texture,
            color_texture_renderer_handle,
            normal_texture,
            normal_texture_renderer_handle,
            texture_bind_group_layout,
        ).unwrap();

        let handle = self.state.rendering_resource_storage.materials.insert(material);
        Ok(handle)
    }

    fn update_material_texture(&mut self, material_renderer_handle: RendererMaterialHandle, renderer_texture_handle: RendererTextureHandle, texture_type: TextureType) -> Result<(), RendererError> {
        let mut material = self.state.rendering_resource_storage.materials.get_mut(material_renderer_handle).unwrap();
       
        match texture_type {
            TextureType::Color => {
                material.color_texture_handle = renderer_texture_handle;
            },
            TextureType::Normal => {
                material.normal_texture_handle = renderer_texture_handle;
            }
        }

        Ok(())
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle, RendererError> {
        let pipeline_handle = get_master_pipeline_handle();
        let pipeline = self.state.rendering_resource_storage.pipelines.get(pipeline_handle).unwrap();
        let camera_bind_group_layout = &pipeline.camera_bind_group_layout;
        
        let camera = RendererCamera::new(&self.state.device, camera_bind_group_layout).unwrap();

        let handle = self.state.rendering_resource_storage.cameras.insert(camera);
        Ok(handle)
    }
}

pub struct RenderingResourceStorage {
    pub(crate) pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>,
    pub(crate) materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
}

impl RenderingResourceStorage {
    pub fn new() -> Self {
        RenderingResourceStorage {
            pipelines: pill_core::PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(10),
            textures: pill_core::PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(10),
            materials: pill_core::PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(10),
            meshes: pill_core::PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(10),
            cameras: pill_core::PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(10),
        }
    }
}

pub struct State {
    // Resources
    rendering_resource_storage: RenderingResourceStorage,

    // Renderer variables
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    window_size: winit::dpi::PhysicalSize<u32>, 

    depth_texture: RendererTexture,
    mesh_drawer: MeshDrawer,
}


impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: &Window) -> Self {
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
            present_mode: wgpu::PresentMode::Fifo, // Defines how to sync the swap_chain with the display
        };

        // Configure surface
        surface.configure(&device, &surface_configuration);

        // Configure collections
        let mut rendering_resource_storage = RenderingResourceStorage::new();

        // Create depth and color texture
        let depth_texture = RendererTexture::new_depth_texture(
            &device, 
            &surface_configuration, 
            "depth_texture"
        ).unwrap();

        let color_format = surface_configuration.format;
        let depth_format = Some(wgpu::TextureFormat::Depth32Float);

        // Create master pipeline
        let master_pipeline = RendererPipeline::new(
            &device,
            color_format,
            depth_format,
            &[RendererMesh::data_layout_descriptor(), Instance::data_layout_descriptor()],
        ).unwrap();

        rendering_resource_storage.pipelines.insert(master_pipeline);

        // Create drawing state
        let mesh_drawer = MeshDrawer::new(&device, 1000); // [TODO] move magic value to confi

        // Create state
        Self {
            // Resources
            rendering_resource_storage,

            // Renderer variables
            surface,
            device,
            queue,
            surface_configuration,
            window_size,
            
            depth_texture,
            mesh_drawer,
        }
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        if new_window_size.width > 0 && new_window_size.height > 0 {
            //self.projection.resize(new_window_size.width, new_window_size.height); // [TODO]
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

        // Build a command buffer that can be sent to the GPU
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("render_encoder"),
        });

        { // Additional scope to release mutable borrow of encoder done by begin_render_pass
            
            // Create color attachment
            let color_attachment = wgpu::RenderPassColorAttachment { // This is what [[location(0)]] in the fragment shader targets
                view: &view, // Specifies what texture to save the colors to (to frame taken from swapchain, so any colors we draw to this attachment will get drawn to the screen)
                resolve_target: None, // Specifies what texture will receive the resolved output
                ops: wgpu::Operations { // Tells wgpu what to do with the colors on the screen
                    load: wgpu::LoadOp::Clear(
                        wgpu::Color { r: 0.1, g: 0.2, b: 0.3, a: 1.0, } // Tells wgpu how to handle colors stored from the previous frame
                    ),
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

            // Get active camera
            // TODO! - below "let borrows" don't live long enough - deal with it, as it looks bad
            //let active_camera_component = camera_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap();
            {      
                let renderer_camera = self.rendering_resource_storage.cameras.get_mut(camera_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap().get_renderer_resource_handle()).ok_or(RendererError::RendererResourceNotFound)?;
                //let active_camera_transform_component = transform_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap();
                // Update camera uniform buffer
                renderer_camera.update(&self.queue, camera_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap(), transform_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap());
            }
            let renderer_camera = self.rendering_resource_storage.cameras.get(camera_component_storage.data.get(active_camera_entity_handle.index).unwrap().borrow().as_ref().unwrap().get_renderer_resource_handle()).unwrap();

            self.mesh_drawer.draw(
                &self.queue, 
                &mut encoder, 
                &self.rendering_resource_storage, 
                color_attachment, 
                depth_stencil_attachment, 
                &renderer_camera,
                &render_queue, 
                &transform_component_storage
            )
        }

        self.queue.submit(iter::once(encoder.finish())); // Finish command buffer and submit it to the GPU's render queue
        frame.present();
        debug!("Frame rendering completed successfully");
        Ok(())
    }
}

pub struct MeshDrawer {
    current_order: u8,
    //current_camera_handle: Option<RendererCameraHandle>,
    current_pipeline_handle: Option<RendererPipelineHandle>,
    current_material_handle: Option<RendererMaterialHandle>,
    current_mesh_handle: Option<RendererMeshHandle>,
    current_mesh_index_count: u32,

    max_instance_count: u32,
    instance_count: u32,
    instances: Vec::<Instance>,
    instance_buffer: wgpu::Buffer,
}

impl MeshDrawer {
    pub fn new(device: &wgpu::Device, max_instance_count: u32) -> Self {

        // Create instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: (size_of::<Instance>() * max_instance_count as usize) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        MeshDrawer {
            current_order: 31,
            //current_camera_handle: None,
            current_pipeline_handle: None,
            current_material_handle: None,
            current_mesh_handle: None,
            current_mesh_index_count: 0,

            max_instance_count,
            instances: Vec::<Instance>::with_capacity(max_instance_count as usize), 
            instance_buffer,
            instance_count: 0,
        }
    }

    pub fn draw(
        &mut self, 
        // Resources
        queue: &wgpu::Queue, 
        encoder: &mut wgpu::CommandEncoder, 
        rendering_resource_storage: &RenderingResourceStorage, 
        color_attachment: wgpu::RenderPassColorAttachment, 
        depth_stencil_attachment: wgpu::RenderPassDepthStencilAttachment,
        // Rendring data
        camera: &RendererCamera,
        render_queue: &Vec::<RenderQueueItem>, 
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) {

        // Start encoding render pass
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { // Use the encoder to create a RenderPass
            label: Some("render_pass"),
            color_attachments: &[color_attachment],
            depth_stencil_attachment: Some(depth_stencil_attachment),
        });

        let render_queue_iter = render_queue.iter();
        for render_queue_item in render_queue_iter {
            
            let render_queue_key_fields = decompose_render_queue_key(render_queue_item.key).unwrap();

            // Recreate resource handles
            let renderer_material_handle = RendererMaterialHandle::new(render_queue_key_fields.material_index.into(), NonZeroU32::new(render_queue_key_fields.material_version.into()).unwrap());
            let renderer_mesh_handle = RendererMeshHandle::new(render_queue_key_fields.mesh_index.into(), NonZeroU32::new(render_queue_key_fields.mesh_version.into()).unwrap());


            // Check order
            // if self.current_order != render_queue_key_fields.order {
            //     if self.instance_count > 0 {
            //         render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            //         render_pass.draw_indexed(0..self.current_mesh_index_count, 0, 0..self.instance_count);
            //         self.instances.clear();
            //         self.instance_count = 0; 
            //     }
            //     // Set new order
            //     self.current_order = 1;// render_queue_key_fields.order.clone();
            // }

            if self.current_material_handle != Some(renderer_material_handle) {
                // Render accumulated instances
                if self.instance_count > 0 {
                    queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances)); // Update instance buffer
                    render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer
                    render_pass.draw_indexed(0..self.current_mesh_index_count, 0, 0..self.instance_count);            
                    self.instances.clear();
                    self.instance_count = 0; 
                }
                // Set new material
                self.current_material_handle = Some(renderer_material_handle);
                let material = rendering_resource_storage.materials.get(self.current_material_handle.unwrap()).unwrap();
               
                // Set pipeline if new material is using different one
                if self.current_pipeline_handle != Some(material.pipeline_handle) {
                    self.current_pipeline_handle = Some(material.pipeline_handle);
                    let pipeline = rendering_resource_storage.pipelines.get( self.current_pipeline_handle.unwrap()).unwrap();
                    render_pass.set_pipeline(&pipeline.render_pipeline);
                }

                render_pass.set_bind_group(0, &material.bind_group, &[]);
                render_pass.set_bind_group(1, &camera.bind_group, &[]);
            }

            if self.current_mesh_handle != Some(renderer_mesh_handle) {
                // Render accumulated instances
                if self.instance_count > 0 {
                    queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances)); // Update instance buffer
                    render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer
                    render_pass.draw_indexed(0..self.current_mesh_index_count, 0, 0..self.instance_count);    
                    self.instances.clear();
                    self.instance_count = 0;            
                }
                // Set new mesh
                self.current_mesh_handle = Some(renderer_mesh_handle);               
                let mesh = rendering_resource_storage.meshes.get(self.current_mesh_handle.unwrap()).unwrap();
                self.current_mesh_index_count = mesh.index_count;
                render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..)); 
                render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32); 
            }

            // Add new instance
            // TODO! - find way to better express the transform_component - let doesn't work, as it cannot live long enough
            //let transform_component = transform_component_storage.data.get(render_queue_item.entity_index as usize).unwrap().borrow().as_ref().unwrap();
            self.instances.push(Instance::new(transform_component_storage.data.get(render_queue_item.entity_index as usize).unwrap().borrow().as_ref().unwrap()));

            self.instance_count += 1;
        }

        
        // End of render queue so draw remaining saved objects
        if self.instance_count > 0 {
            queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&self.instances)); // Update instance buffer
            render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // Set instance buffer
            render_pass.draw_indexed(0..self.current_mesh_index_count, 0, 0..self.instance_count);    
            self.instances.clear();
            self.instance_count = 0;            
        }
        
        // Reset state of mesh drawer
        self.current_order = 31;
        self.current_pipeline_handle = None;
        self.current_material_handle = None;
        self.current_mesh_handle = None;
        self.current_mesh_index_count = 0;
        self.instances.clear();
        self.instance_count = 0;
    }
}