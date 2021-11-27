
use anyhow::{Result, Context, Error};

use cgmath::Rotation3;


use pill_core::PillSlotMap;
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

// Matrix to scale and translate scene from OpenGL's coordinate sytem to WGPU's
#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);

fn get_master_pipeline_handle() -> RendererPipelineHandle { // [TODO] Very ugly solucion, maybe store there handles in some hashmap with names of pipelines as keys
    RendererPipelineHandle { 0: PillSlotMapKeyData { index: 1, version: std::num::NonZeroU8::new(1).unwrap() } }
}

pub struct Renderer {
    pub state: State,
}

impl PillRenderer for Renderer {
    fn new(window: &Window) -> Self { 
        let mut state: State = pollster::block_on(State::new(&window));
        return Renderer {
            state,
        }; 
    }

    fn initialize(&self) {
        info!("Pill Renderer initialize");
    }

    fn render(
        &mut self,
        render_queue: &Vec<RenderQueueItem>, 
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) -> Result<(), RendererError> {
        self.state.render(render_queue, transform_component_storage)
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!("Renderer resizing!");
        self.state.resize(new_window_size)
    }

    fn create_texture(&mut self, path: &PathBuf, texture_type: TextureType) -> Result<RendererTextureHandle, RendererError> {
        let texture = RendererTexture::new_texture_from_image(&self.state.device, &self.state.queue, path, texture_type).unwrap();
        let handle = self.state.textures.insert(texture);
        Ok(handle)
    }

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle, RendererError> {
        let mesh = RendererMesh::new(&self.state.device, name, mesh_data).unwrap();
        let handle = self.state.meshes.insert(mesh);
        Ok(handle)
    }

 

    fn create_material(&mut self, name: &str, color_texture_renderer_handle: RendererTextureHandle, normal_texture_renderer_handle: RendererTextureHandle) -> Result<RendererMaterialHandle, RendererError> {
        let color_texture = self.state.textures.get(color_texture_renderer_handle).unwrap();
        let normal_texture = self.state.textures.get(normal_texture_renderer_handle).unwrap();

        let pipeline_handle = get_master_pipeline_handle();
        let pipeline = self.state.pipelines.get(pipeline_handle).unwrap();
        let texture_bind_group_layout = pipeline.texture_bind_group_layout;

        let material = RendererMaterial::new(
            &self.state.device,
            name, 
            pipeline_handle,
            color_texture,
            color_texture_renderer_handle,
            normal_texture,
            normal_texture_renderer_handle,
            &texture_bind_group_layout,
        ).unwrap();

        let handle = self.state.materials.insert(material);
        Ok(handle)
    }

    fn update_material_texture(&mut self, material_renderer_handle: RendererMaterialHandle, renderer_texture_handle: RendererTextureHandle, texture_type: TextureType) -> Result<(), RendererError> {
        let material = self.state.materials.get(material_renderer_handle).unwrap();
       
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

    // fn update_material(&mut self, material_renderer_handle: RendererMaterialHandle, updated_material: &Material) -> Result<(), RendererError> {
    //     let material = self.state.materials.get(material_renderer_handle).unwrap();

    //     material.color_texture_handle


    // }

    fn create_camera(&mut self) -> Result<RendererCameraHandle, RendererError> {
        let pipeline_handle = get_master_pipeline_handle();
        let pipeline = self.state.pipelines.get(pipeline_handle).unwrap();
        let camera_bind_group_layout = pipeline.camera_bind_group_layout;
        
        let camera = RendererCamera::new(&self.state.device, &camera_bind_group_layout).unwrap();

        let handle = self.state.cameras.insert(camera);
        Ok(handle)
    }

    

}

pub struct State {
    // Resources
    pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>,
    materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
    active_camera: Option<RendererCameraHandle>,

    // Renderer variables
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    window_size: winit::dpi::PhysicalSize<u32>, 

    instance_buffer: wgpu::Buffer,
    depth_texture: RendererTexture,
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
        let mut pipelines = pill_core::PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(10);
        let textures = pill_core::PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(10);
        let materials = pill_core::PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(10);
        let meshes = pill_core::PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(10);
        let cameras = pill_core::PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(10);

        // Configure instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: (size_of::<Instance>() * 1000) as u64, //  [TODO] move magic value to config
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
   
        // Create depth texture and master pipeline
        let depth_texture = RendererTexture::new_depth_texture(
            &device, 
            &surface_configuration, 
            "depth_texture"
        ).unwrap();

        let color_format = surface_configuration.format;
        let depth_format = Some(wgpu::TextureFormat::Depth32Float);

        let master_pipeline = RendererPipeline::new(
            &device,
            color_format,
            depth_format,
            &[RendererMesh::data_layout_descriptor(), Instance::data_layout_descriptor()],
        ).unwrap();

        pipelines.insert(master_pipeline);

        // Create state
        Self {
            // Resources
            pipelines,
            textures,
            materials,
            meshes,
            cameras,
            active_camera: None,

            // Renderer variables
            surface,
            device,
            queue,
            surface_configuration,
            window_size,
            
            instance_buffer,
            depth_texture,
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
        render_queue: &Vec::<RenderQueueItem>, 
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

        // Prepare instance data




        { // Additional scope to release mutable borrow of encoder done by begin_render_pass
            
            // Start encoding render pass
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { // Use the encoder to create a RenderPass
                label: Some("render_pass"),
                color_attachments: &[
                    wgpu::RenderPassColorAttachment { // This is what [[location(0)]] in the fragment shader targets
                        view: &view, // Specifies what texture to save the colors to (to frame taken from swapchain, so any colors we draw to this attachment will get drawn to the screen)
                        resolve_target: None, // Specifies what texture will receive the resolved output
                        ops: wgpu::Operations { // Tells wgpu what to do with the colors on the screen
                            load: wgpu::LoadOp::Clear(
                                wgpu::Color { // Tells wgpu how to handle colors stored from the previous frame
                                    r: 0.1,
                                    g: 0.2,
                                    b: 0.3,
                                    a: 1.0,
                                }
                            ),
                        store: true,
                        },
                    }
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_texture.texture_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });

            // Set pipeline
            let master_pipeline_handle = get_master_pipeline_handle();
            let master_pipeline = self.pipelines.get(master_pipeline_handle).unwrap();
            render_pass.set_pipeline(&master_pipeline.render_pipeline);


            // Create instance
            let mut instances: Vec<Instance> = Vec::new();

            let position: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(0.0, 0.0, 0.0);
            let rotation: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(0.0, 0.0, 0.0);
            let scale: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(1.0, 1.0, 1.0);

            instances.push( 
                Instance {
                    model: cgmath::Matrix4::model(position, rotation, scale).into(),
                    normal: cgmath::Matrix3::from_euler_angles(rotation).into(),
                }
            );
            
            let position: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(0.0, 2.0, 0.0);
            let rotation: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(15.0, 45.0, 0.0);
            let scale: cgmath::Vector3<f32> = cgmath::Vector3::<f32>::new(1.0, 1.0, 4.0);

            instances.push( 
                Instance {
                    model: cgmath::Matrix4::model(position, rotation, scale).into(),
                    normal: cgmath::Matrix3::from_euler_angles(rotation).into(),
                }
            );

            // Update instance buffer
            self.queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));

            // Set buffer with instance data
            render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); 

            // Set buffer with mesh vertices
            render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..)); 

            // Set buffer with mesh indices
            render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32); 


            render_pass.set_bind_group(0, &material.bind_group, &[]);
            render_pass.set_bind_group(1, camera, &[]);
            render_pass.draw_indexed(0..mesh.num_elements, 0, instances);
        }

        self.queue.submit(iter::once(encoder.finish())); // Finish command buffer and submit it to the GPU's render queue
        frame.present();
        Ok(())
    }
}
