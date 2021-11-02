
use cgmath::Rotation3;
use pill_core::*;

use pill_engine::RendererError;
use pill_engine::Scene;
use pill_engine::Pill_Renderer;
use wgpu::ShaderModule;
use wgpu::ShaderModuleDescriptor;
use wgpu::SurfaceError;



use crate::model;
use model::{DrawModel, Vertex};
use crate::camera;
use crate::texture;

use std::path;
use std::path::Path;

use std::iter;
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

pub struct Renderer {
    pub state: State,
}

impl Pill_Renderer for Renderer {
    fn new(window: &Window) -> Self { 
        let mut state: State = pollster::block_on(State::new(&window));
        return Renderer {
            state,
        }; 
    }

    //pub fn update(&mut self, _dt: std::time::Duration) {}

    fn initialize(&self) {
        println!("[Renderer] Init");
    }

    fn render(&mut self, scene: &Scene, dt: std::time::Duration) -> Result<(), pill_engine::RendererError> {
        self.state.update(dt);
        self.state.render(scene)
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        println!("Renderer resizing!");
        self.state.resize(new_window_size)
    }

    fn create_model(&mut self, path: Box<&Path>) -> usize {
        let res_dir = std::path::Path::new(env!("OUT_DIR")).join("res");
        let obj_model = model::Model::load( // Load model and create resources for it
            &self.state.device,
            &self.state.queue,
            &self.state.texture_bind_group_layout,
            path,
        )
        .unwrap();
        let id: usize = self.state.obj_models.len();
        self.state.obj_models.insert(id, Box::new(obj_model));
        //self.state.obj_models.push_back(Box::new(obj_model));
        id // Return id
    }

}







const NUM_INSTANCES_PER_ROW: u32 = 10;
const INSTANCE_DISPLACEMENT: cgmath::Vector3<f32> = cgmath::Vector3::new(
    NUM_INSTANCES_PER_ROW as f32 * 0.5,
    0.0,
    NUM_INSTANCES_PER_ROW as f32 * 0.5,
);

// Matrix to scale and translate our scene from OpenGL's coordinate sytem to WGPU's
#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);



struct Instance {
    position: cgmath::Vector3<f32>,
    rotation: cgmath::Quaternion<f32>,
}

impl Instance {
    fn to_raw(&self) -> InstanceRaw {
        InstanceRaw {
            model: (cgmath::Matrix4::from_translation(self.position) * cgmath::Matrix4::from(self.rotation)).into(), // Create model matrix
            normal: cgmath::Matrix3::from(self.rotation).into(),
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
#[allow(dead_code)]
struct InstanceRaw {
    model: [[f32; 4]; 4], // It is not possible to use cgmath with bytemuck directly. Conversion from Quaternion into a 4x4 f32 array (matrix) needed
    normal: [[f32; 3]; 3], // It is matrix3 because we only need the rotation component
}

impl model::Vertex for InstanceRaw {
    fn descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<InstanceRaw>() as wgpu::BufferAddress,
            // We need to switch from using a step mode of Vertex to Instance
            // This means that shaders will only change to use the next instance when the shader starts processing a new instance
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                // A mat4 takes up 4 vertex slots as it is technically 4 vec4s. We need to define a slot for each vec4. We don't have to do this in code though.
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32x4,
                },

                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 19]>() as wgpu::BufferAddress,
                    shader_location: 10,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 22]>() as wgpu::BufferAddress,
                    shader_location: 11,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}


// Uniform buffer, blob of data that is available to every invocation of a set of shaders
#[repr(C)] // We need this for Rust to store our data correctly for the shaders
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)] // This is so we can store this in a buffer
struct CameraUniform {
    view_position: [f32; 4],
    view_projection: [[f32; 4]; 4], // It is not possible to use cgmath with bytemuck directly. Conversion from Matrix4 into a 4x4 f32 array (matrix) needed
}


impl CameraUniform {
    fn new() -> Self {
        use cgmath::SquareMatrix;
        Self {
            view_position: [0.0; 4],
            view_projection: cgmath::Matrix4::identity().into(),
        }
    }

    fn update_view_projection(&mut self, camera: &camera::Camera, projection: &camera::Projection) {
        // We're using Vector4 because of the uniforms 16 byte spacing requirement
        self.view_position = camera.position.to_homogeneous().into();
        self.view_projection = (projection.calc_matrix() * camera.calc_matrix()).into(); // Convert coordinates system from OpenGL (cgmath crate uses it) to DirectX (wgpu uses it)
    }
}




#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct LightUniform {
    position: [f32; 3],
    _padding: u32, // Due to uniforms requiring 16 byte (4 float) spacing, we need to use a padding field here
    color: [f32; 3],
}



pub struct State {
    surface: wgpu::Surface,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    pub window_size: winit::dpi::PhysicalSize<u32>,
    master_render_pipeline: wgpu::RenderPipeline,
    texture_bind_group_layout: wgpu::BindGroupLayout, // !!!
    light_obj_model: model::Model,
    obj_models: HashMap<usize, Box<model::Model>>, //LinkedList<Box<model::Model>>,
    //obj_models: Vec<model::Model>,
    camera: camera::Camera,
    projection: camera::Projection,
    camera_controller: camera::CameraController,
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    //instances: Vec<Instance>,
    //#[allow(dead_code)]
    instance_buffer: wgpu::Buffer,
    depth_texture: texture::Texture,
    light_uniform: LightUniform,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    light_render_pipeline: wgpu::RenderPipeline,
    mouse_pressed: bool,
}


impl State {
    // Creating some of the wgpu types requires async code
    async fn new(window: &Window) -> Self {
        let window_size = window.inner_size();

        // The instance is a handle to the GPU
        let instance = wgpu::Instance::new(wgpu::Backends::all()); // BackendBit::PRIMARY => Vulkan + Metal + DX12 + Browser WebGPU
        let surface = unsafe { instance.create_surface(window) }; 
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { // Options passed here are not guaranteed to work for all devices
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .unwrap();

        // [TODO: Use iteration]
        // let adapter = instance // Iterates over all possible adapters for the backend and gets first that support given surface
        //     .enumerate_adapters(wgpu::BackendBit::PRIMARY)
        //     .filter(|adapter| { // Check if this adapter supports our surface
        //         adapter.get_swap_chain_preferred_format(&surface).is_some() 
        //     })
        //     .next()
        //     .unwrap();
        
        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                features: wgpu::Features::empty(), // Allows to specify what extra features of GPU that needs to be included (e.g. depth clamping, push constants, texture compression, etc)
                limits: wgpu::Limits::default(), // Allows to specify the limit of certain types of resources that will be used (e.g. max samplers, uniform buffers, etc)
            },
            None, // Trace path
        )
        .await
        .unwrap();

        let surface_configuration = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT, // Defines how the swap_chain's underlying textures will be used
            format: surface.get_preferred_format(&adapter).unwrap(), // Defines how the swap_chain's textures will be stored on the gpu
            width: window_size.width,
            height: window_size.height,
            present_mode: wgpu::PresentMode::Fifo, // Defines how to sync the swap_chain with the display
        };

        surface.configure(&device, &surface_configuration);



        

        
        let camera = camera::Camera::new((0.0, 5.0, 10.0), cgmath::Deg(-90.0), cgmath::Deg(-20.0));
        let projection = camera::Projection::new(surface_configuration.width, surface_configuration.height, cgmath::Deg(45.0), 0.1, 100.0);
        let camera_controller = camera::CameraController::new(4.0, 0.4);
       
        let mut camera_uniform = CameraUniform::new();
        camera_uniform.update_view_projection(&camera, &projection);

        let camera_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Camera Buffer"),
            contents: bytemuck::cast_slice(&[camera_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("camera_bind_group_layout"),
        });

        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
            label: Some("camera_bind_group"),
        });









        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { // Describes a set of resources and how they can be accessed by a shader
            entries: &[
                wgpu::BindGroupLayoutEntry { // Entry for the sampled texture at binding 0
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT, // Visible only to fragment shader
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { // Entry for the sampler at binding 1
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT, // Visible only to fragment shader
                    ty: wgpu::BindingType::Sampler {
                        comparison: false,
                        filtering: true,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { // Normal map
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler { 
                        comparison: false,
                        filtering: true, 
                    },
                    count: None,
                },
            ],
            label: Some("texture_bind_group_layout"),
        });

        // let res_dir = std::path::Path::new(env!("OUT_DIR")).join("res"); // Create path from build directory to res directory
        // let obj_model = model::Model::load( // Load model and create resources for it
        //     &device,
        //     &queue,
        //     &texture_bind_group_layout,
        //     res_dir.join("../res/models/cube.obj"),
        // )
        // .unwrap();

        //let res_dir = std::path::Path::new(env!("OUT_DIR")).join("res"); // Create path from build directory to res directory
        let light_obj_model = model::Model::load( // Load model and create resources for it
            &device,
            &queue,
            &texture_bind_group_layout,
            Box::new(Path::new("D:\\Programming\\Rust\\pill_project\\pill_engine\\pill\\src\\graphics\\res\\models\\cube.obj"))
        )
        .unwrap();

       
       


        // const SPACE_BETWEEN: f32 = 3.0;
        // let instances = (0..NUM_INSTANCES_PER_ROW).flat_map(|z| {
        //     (0..NUM_INSTANCES_PER_ROW).map(move |x| {

        //         let x = SPACE_BETWEEN * (x as f32 - NUM_INSTANCES_PER_ROW as f32 / 2.0);
        //         let z = SPACE_BETWEEN * (z as f32 - NUM_INSTANCES_PER_ROW as f32 / 2.0);
        //         let position = cgmath::Vector3 { x, y: 0.0, z };

        //         let rotation = if position.is_zero() {
        //             // this is needed so an object at (0, 0, 0) won't get scaled to zero as Quaternions can effect scale if they're not created correctly
        //             cgmath::Quaternion::from_axis_angle(cgmath::Vector3::unit_z(),cgmath::Deg(0.0),)
        //         } else {
        //             cgmath::Quaternion::from_axis_angle(position.normalize(), cgmath::Deg(45.0))
        //         };

        //         Instance { position, rotation }
        //     })
        // })
        // .collect::<Vec<_>>();

        //let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<_>>(); // Convert from Instance to InstanceRaw (with matrix instead of quaternion)
        // let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        //     label: Some("Instance Buffer"),
        //     contents: bytemuck::cast_slice(&instance_data),
        //     usage: wgpu::BufferUsage::VERTEX,
        // });

        let mut instance_start_data: Vec<Instance> = Vec::new();
        instance_start_data.push( 
            Instance { 
                position: cgmath::Vector3 { x: 0.0, y: 0.0, z: 0.0 },
                rotation: cgmath::Quaternion::zero(),
            } 
        );
        let instance_start_data_raw = instance_start_data.iter().map(Instance::to_raw).collect::<Vec<_>>();
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Instance Buffer"),
            contents: bytemuck::cast_slice(&instance_start_data_raw),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });







        let light_uniform = LightUniform {
            position: [2.0, 2.0, 2.0],
            _padding: 0,
            color: [1.0, 1.0, 1.0],
        };

        let light_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light VB"),
            contents: bytemuck::cast_slice(&[light_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, // To enable updating this buffer COPY_DST flag is needed
        });

        let light_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
            label: None,
        });

        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &light_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: light_buffer.as_entire_binding(),
            }],
            label: None,
        });








        let depth_texture = texture::Texture::create_depth_texture(&device, &surface_configuration, "depth_texture");

        

        let master_render_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Render Pipeline Layout"),
                bind_group_layouts: &[
                    &texture_bind_group_layout,
                    &camera_bind_group_layout,
                    &light_bind_group_layout,
                ],
                push_constant_ranges: &[],
            });


            //let vs_module = device.create_shader_module(&wgpu::include_spirv!("../res/shaders/built/master.vert.spv"));
            //let fs_module = device.create_shader_module(&wgpu::include_spirv!("../res/shaders/built/master.frag.spv"));

            let vertex_shader = wgpu::ShaderModuleDescriptor {
                label: Some("shader.vert"),
                source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/master.vert.spv")),
            };
    
            let fragment_shader = wgpu::ShaderModuleDescriptor {
                label: Some("shader.vert"),
                source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/master.frag.spv")),
            };


            create_render_pipeline(
                &device,
                &layout,
                surface_configuration.format,
                Some(texture::Texture::DEPTH_FORMAT),
                &[model::ModelVertex::descriptor(), InstanceRaw::descriptor()],
                &vertex_shader,
                &fragment_shader,
            )
        };


        let light_render_pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Light Pipeline Layout"),
                bind_group_layouts: &[
                    &camera_bind_group_layout, 
                    &light_bind_group_layout
                ],
                push_constant_ranges: &[],
            });

            //let vs_module = device.create_shader_module(&wgpu::include_spirv!("../res/shaders/built/light.vert.spv"));
            //let fs_module = device.create_shader_module(&wgpu::include_spirv!("../res/shaders/built/light.frag.spv"));

            let vertex_shader = wgpu::ShaderModuleDescriptor {
                label: Some("shader.vert"),
                source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/light.vert.spv")),
            };
    
            let fragment_shader = wgpu::ShaderModuleDescriptor {
                label: Some("shader.vert"),
                source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/light.frag.spv")),
            };

            create_render_pipeline(
                &device,
                &layout,
                surface_configuration.format,
                Some(texture::Texture::DEPTH_FORMAT),
                &[model::ModelVertex::descriptor()],
                &vertex_shader,
                &fragment_shader,
            )
        };



       

        Self {
            surface,
            device,
            queue,
            surface_configuration,
            window_size,
            master_render_pipeline,
            texture_bind_group_layout,
            light_obj_model,
            obj_models: HashMap::new(), //Vec<model::Model>,
            camera,
            projection, 
            camera_controller,
            camera_buffer,
            camera_bind_group,
            camera_uniform,
            //instances,
            instance_buffer,
            depth_texture,
            light_uniform,
            light_buffer,
            light_bind_group,
            light_render_pipeline,
            mouse_pressed: false,
        }
        
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        if new_window_size.width > 0 && new_window_size.height > 0 {
            self.projection.resize(new_window_size.width, new_window_size.height);
            self.window_size = new_window_size;
            self.surface_configuration.width = new_window_size.width;
            self.surface_configuration.height = new_window_size.height;
            self.surface.configure(&self.device, &self.surface_configuration);
            self.depth_texture = texture::Texture::create_depth_texture(
                &self.device,
                &self.surface_configuration,
                "depth_texture",
            );
        }
    }

    fn input(&mut self, event: &DeviceEvent) -> bool { // Returns a bool to indicate whether an event has been fully processed (if the method returns true, the main loop won't process the event any further)
        match event {
            DeviceEvent::Key(
                KeyboardInput {
                    virtual_keycode: Some(key),
                    state,
                    ..
                }
            ) => self.camera_controller.process_keyboard(*key, *state),
            DeviceEvent::MouseWheel { delta, .. } => {
                self.camera_controller.process_scroll(delta);
                true
            }
            DeviceEvent::Button {
                button: 1, // Left Mouse Button
                state,
            } => {
                self.mouse_pressed = *state == ElementState::Pressed;
                true
            }
            DeviceEvent::MouseMotion { delta } => {
                if self.mouse_pressed {
                    self.camera_controller.process_mouse(delta.0, delta.1);
                }
                true
            }
            _ => false,
        }
    }

    fn update(&mut self, dt: std::time::Duration) {

        self.camera_controller.update_camera(&mut self.camera, dt);
        self.camera_uniform.update_view_projection(&self.camera, &self.projection);

        // self.camera_controller.update_camera(&mut self.camera);
        // self.camera_uniform.update_view_projection(&self.camera);
        self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::cast_slice(&[self.camera_uniform])); // [TODO: What is that?? https://sotrh.github.io/learn-wgpu/beginner/tutorial6-uniforms/#a-controller-for-our-camera]
    
    
        // Update the light
        let old_position: cgmath::Vector3<_> = self.light_uniform.position.into();


        self.light_uniform.position = (cgmath::Quaternion::from_axis_angle((0.0, 1.0, 0.0).into(), cgmath::Deg(1.0)) * old_position).into();
       // self.light_uniform.position = (cgmath::Quaternion::from_axis_angle((0.0, 1.0, 0.0).into(), cgmath::Deg(1.0)) * old_position).into(); // Change position of light
        self.queue.write_buffer(&self.light_buffer, 0, bytemuck::cast_slice(&[self.light_uniform]),);
    }

    //fn render(&mut self) -> Result<(), wgpu::SwapChainError> {
    fn render(&mut self, scene: &Scene) -> Result<(), pill_engine::RendererError> { 

        //let frame = self.swap_chain.get_current_frame(); // Get frame to render to
        //let frame = self.swap_chain.get_current_frame()?.output; // Get frame to render to

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
                label: Some("Render Encoder"),
        });

        { // Additional scope to release mutable borrow of encoder done by begin_render_pass
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor { // Use the encoder to create a RenderPass
                label: Some("Render Pass"),
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
                    view: &self.depth_texture.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: true,
                    }),
                    stencil_ops: None,
                }),
            });


            //render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
            //render_pass.set_vertex_buffer(1, self.vertex_buffer.slice(..));

            
            // --- Draw light objects ---
            use crate::model::DrawLight;
            render_pass.set_pipeline(&self.light_render_pipeline);
            render_pass.draw_light_model(
                &self.light_obj_model,
                &self.camera_bind_group,
                &self.light_bind_group,
            );


            

            // --- Draw master objects ---
            render_pass.set_pipeline(&self.master_render_pipeline);

            let mut counter: usize = 0;
            for gameobject in scene.gameobjectCollection.iter() {
                
                let gameobject = gameobject.borrow();


                // Get transform and rendering components
                // From rendering component get mesh resource id to find buffers created in renderer
                // From transform set model matrix
                // Render

                




                //println!("[Renderer] Rendering gameobject (object): {} ({}/{})", gameobject.name, counter + 1, scene.gameobjectCollection.len());
                //println!("[Renderer] Object position: {:?}", gameobject.position);
              
                // let old_position: cgmath::Vector3<_> = gameobject.position.clone().into();
                // let new_position: cgmath::Vector3<_> = (cgmath::Quaternion::from_axis_angle((0.0, 1.0, 0.0).into(), cgmath::Deg(1.0)) * old_position).into();
                // gameobject.position = new_position;
                //println!("[Renderer] Object position: {:?}", new_position);

                // Create instance
                let mut instances: Vec<Instance> = Vec::new();
                instances.push( 
                    Instance { 
                        position: gameobject.position.clone(), 
                        rotation: gameobject.rotation.clone()
                    } 
                );

                // Update instance buffer
                let instance_data = instances.iter().map(Instance::to_raw).collect::<Vec<_>>();
                self.queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instance_data));
                render_pass.set_vertex_buffer(1, self.instance_buffer.slice(..)); // This is set in shader once, so we are overwritting it in each call

                // Draw
                let model = self.obj_models.get(gameobject.resource_id.as_ref().unwrap()).unwrap().as_ref();
                render_pass.draw_model(
                    model,
                    &self.camera_bind_group,
                    &self.light_bind_group,
                );

                // Increase counter
                counter += 1;
            }
        }

        self.queue.submit(iter::once(encoder.finish())); // Finish command buffer and submit it to the GPU's render queue
        frame.present();
        Ok(())
    }
}

fn create_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    depth_format: Option<wgpu::TextureFormat>,
    vertex_layouts: &[wgpu::VertexBufferLayout],
    vertex_shader: &ShaderModuleDescriptor,
    fragment_shader: &ShaderModuleDescriptor

) -> wgpu::RenderPipeline {
    let vertex_shader = device.create_shader_module(vertex_shader);
    let fragment_shader = device.create_shader_module(fragment_shader);

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Render Pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState { 
            module: &vertex_shader,
            entry_point: "main",
            buffers: vertex_layouts, // Specifies structure of vertices that will be passed to the vertex shader
        },
        fragment: Some(wgpu::FragmentState {
            module: &fragment_shader,
            entry_point: "main",
            targets: &[wgpu::ColorTargetState { // Specifies what what color outputs wwgpu should set up
                format: color_format,
                blend: Some(wgpu::BlendState {
                    alpha: wgpu::BlendComponent::REPLACE,
                    color: wgpu::BlendComponent::REPLACE,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            }],
        }),
        primitive: wgpu::PrimitiveState { // Specifies how to interpret vertices when converting them into triangles
            topology: wgpu::PrimitiveTopology::TriangleList, // Each three vertices will correspond to one triangle
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw, // Specifies how to determine whether a given triangle is facing forward or not (FrontFace::Ccw means that a triangle is facing forward if the vertices are arranged in a counter clockwise direction)
            cull_mode: Some(wgpu::Face::Back), // Triangles that are not considered facing forward are culled (not included in the render) as specified by CullMode::Back            
            polygon_mode: wgpu::PolygonMode::Fill, // Setting this to anything other than Fill requires Features::NON_FILL_POLYGON_MODE     
            clamp_depth: false, // Requires Features::DEPTH_CLAMPING
            conservative: false, // Requires Features::CONSERVATIVE_RASTERIZATION
        },
        depth_stencil: depth_format.map(|format| wgpu::DepthStencilState { // [TODO: Investigate this map]
            format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less, // Specifies when to discard a new pixel. Using LESS means pixels will be drawn front to back
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: 1, // Determines how many samples pipeline will use (Multisampling)
            mask: !0, // Specifies which samples should be active
            alpha_to_coverage_enabled: false,
        },
    })
}