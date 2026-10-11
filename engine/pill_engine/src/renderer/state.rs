#![allow(clippy::too_many_arguments)]
use crate::{
    app_config::EngineConfig,
    ecs::{EntityHandle, Scene},
    graphics::{
        BufferDesc, FrameTarget, Pass, PassPBROpaque, PillRenderer, Pipeline, PipelineDesc,
        RendererTargetDesc, RendererTextureHandle, TextureData, WorldQuery,
    },
    renderer::resources::{RendererMesh, RendererTexture, Vertex},
    resources::ResourceManager,
};

use pill_core::Result;
use pill_core::{
    debug, info, LogContext, PillError, PillSlotMap, PillStyle, PillTypeMap, RendererError, Timer,
};
use std::sync::Arc;

pub struct Renderer {
    pub state: State,
}

impl Renderer {
    /// Creates the renderer asynchronously; required on WASM where `block_on` is unavailable.
    pub async fn new_async(
        window: Arc<winit::window::Window>,
        config: EngineConfig,
    ) -> Result<Self> {
        info!(LogContext::Rendering => "Initializing {}", "Renderer".module_object_style());
        let state = State::new(window, config).await?;
        Ok(Self { state })
    }
}

impl PillRenderer for Renderer {
    #[cfg(not(target_arch = "wasm32"))]
    fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self> {
        info!(LogContext::Rendering => "Initializing {}", "Renderer".module_object_style());
        let state = pollster::block_on(State::new(window, config))?;
        Ok(Self { state })
    }

    #[cfg(target_arch = "wasm32")]
    fn new(_window: Arc<winit::window::Window>, _config: EngineConfig) -> Result<Self> {
        panic!("Use Renderer::new_async on WASM")
    }

    // --- Other ---

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        info!(LogContext::Rendering => "Resizing {} resources", "Renderer".module_object_style());
        self.state.resize(new_window_size)
    }

    fn get_window(&self) -> Arc<winit::window::Window> {
        self.state.window.clone()
    }

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        scene: &Scene,
        globals: &PillTypeMap,
        delta_time: f32,
        timer: &mut Timer,
        resource_manager: &ResourceManager,
    ) -> Result<()> {
        debug!(LogContext::Frame => "Starting frame render");

        timer.record("Get frame");

        let frame = self.state.surface.get_current_texture();
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => match error {
                wgpu::SurfaceError::Lost => return Err(RendererError::SurfaceLost.into()),
                wgpu::SurfaceError::OutOfMemory => {
                    return Err(RendererError::SurfaceOutOfMemory.into())
                }
                _ => return Err(RendererError::SurfaceOther.into()),
            },
        };

        let frame_target = FrameTarget {
            view: frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default()),
            srgb_view: frame.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.state.color_format.add_srgb_suffix()),
                ..Default::default()
            }),
        };

        let mut encoder =
            self.state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("render_encoder"),
                });

        let world = WorldQuery::new(
            active_camera_entity_handle,
            scene,
            globals,
            delta_time,
            resource_manager,
        );

        timer.begin_context("Scene Passes");

        let mut passes = std::mem::take(&mut self.state.passes);
        for pass in &mut passes {
            timer.record(pass.get_label());
            pass.draw(&mut encoder, self, &frame_target, &world)?;
        }
        self.state.passes = passes;

        timer.end_context()?;

        timer.record("Submit commands and present frame");

        self.state.frame_counter += 1;
        let capture = self.state.screenshot.as_ref().and_then(|(target, path)| {
            if self.state.frame_counter == *target {
                Some(path.clone())
            } else {
                None
            }
        });

        let screenshot_buf = if let Some(ref _path) = capture {
            let w = self.state.surface_configuration.width;
            let h = self.state.surface_configuration.height;
            let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let bytes_per_row = (4 * w).div_ceil(align) * align;
            let buf = self.state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("screenshot_readback"),
                size: (bytes_per_row * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                frame.texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bytes_per_row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            Some((buf, w, h, bytes_per_row))
        } else {
            None
        };

        self.state.queue.submit(std::iter::once(encoder.finish()));

        if let (Some((buf, w, h, bytes_per_row)), Some(path)) = (screenshot_buf, capture) {
            let slice = buf.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            let _ = self.state.device.poll(wgpu::MaintainBase::Wait);
            let data = slice.get_mapped_range();
            #[cfg(not(target_arch = "wasm32"))]
            save_screenshot(&path, &data, w, h, bytes_per_row, self.state.color_format);
            drop(data);
            buf.unmap();
            self.state.screenshot = None;
        }

        frame.present();

        Ok(())
    }

    // --- Pass API ---

    fn set_passes(&mut self, mut passes: Vec<Box<dyn Pass>>) -> Result<()> {
        for pass in &mut passes {
            pass.init(self)?;
        }
        self.state.passes = passes;
        Ok(())
    }

    fn get_surface_size(&self) -> (u32, u32) {
        (
            self.state.surface_configuration.width,
            self.state.surface_configuration.height,
        )
    }

    fn get_device(&self) -> &wgpu::Device {
        &self.state.device
    }

    fn get_queue(&self) -> &wgpu::Queue {
        &self.state.queue
    }

    fn get_surface_format(&self) -> wgpu::TextureFormat {
        self.state.color_format
    }

    fn create_buffer(&mut self, desc: BufferDesc) -> Result<wgpu::Buffer> {
        let buffer = self.state.device.create_buffer(&wgpu::BufferDescriptor {
            label: desc.label,
            size: desc.byte_size,
            usage: desc.usage,
            mapped_at_creation: false,
        });
        Ok(buffer)
    }

    fn create_pipeline(&mut self, desc: PipelineDesc) -> Result<Pipeline> {
        let bind_group_layouts: Vec<wgpu::BindGroupLayout> = desc
            .bind_groups
            .iter()
            .map(|entries| {
                self.state
                    .device
                    .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: None,
                        entries,
                    })
            })
            .collect();

        let bind_group_layout_refs: Vec<&wgpu::BindGroupLayout> =
            bind_group_layouts.iter().collect();

        let pipeline_layout =
            self.state
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: desc.label,
                    bind_group_layouts: &bind_group_layout_refs,
                    push_constant_ranges: &[],
                });

        let vs_module = self
            .state
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.vs.entry_func),
                source: wgpu::ShaderSource::Wgsl(desc.vs.source.into()),
            });
        let fs_module = self
            .state
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(desc.ps.entry_func),
                source: wgpu::ShaderSource::Wgsl(desc.ps.source.into()),
            });

        let pipeline = self
            .state
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: desc.label,
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vs_module,
                    entry_point: Some(desc.vs.entry_func),
                    buffers: desc.vertex_buffers,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fs_module,
                    entry_point: Some(desc.ps.entry_func),
                    targets: desc.targets,
                    compilation_options: Default::default(),
                }),
                primitive: desc.primitive,
                depth_stencil: desc.depth_stencil,
                multisample: desc.multisample,
                multiview: None,
                cache: None,
            });

        Ok(Pipeline {
            pipeline,
            bind_group_layouts,
        })
    }

    fn create_render_target(&mut self, desc: RendererTargetDesc) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_render_target(
            &self.state.device,
            &desc.name,
            desc.width,
            desc.height,
            desc.format,
        )?;
        let handle = self.state.pass_textures.insert(texture);
        Ok(handle)
    }

    fn create_depth_texture(&mut self, label: &str) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_depth_texture(
            &self.state.device,
            &self.state.surface_configuration,
            label,
        )?;
        let handle = self.state.pass_textures.insert(texture);
        Ok(handle)
    }

    fn get_render_target_view(&self, handle: RendererTextureHandle) -> Option<&wgpu::TextureView> {
        self.state
            .pass_textures
            .get(handle)
            .map(|t| &t.texture_view)
    }

    fn create_texture_from_pixels(
        &mut self,
        name: &str,
        data: TextureData,
    ) -> Result<RendererTextureHandle> {
        let texture = RendererTexture::new_from_pixels(
            &self.state.device,
            &self.state.queue,
            name,
            data.mips,
            data.width,
            data.height,
            data.format,
        )?;
        Ok(self.state.pass_textures.insert(texture))
    }

    fn get_texture_view(&self, handle: RendererTextureHandle) -> Option<wgpu::TextureView> {
        self.state.pass_textures.get(handle).map(|t| {
            t.texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        })
    }
}

pub struct State {
    pass_textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_configuration: wgpu::SurfaceConfiguration,
    color_format: wgpu::TextureFormat,
    passes: Vec<Box<dyn Pass>>,
    window: Arc<winit::window::Window>,
    // Optional frame capture: (target_frame, output_path). Set via PILL_SCREENSHOT env var.
    screenshot: Option<(u32, String)>,
    frame_counter: u32,
}

impl State {
    async fn new(window: Arc<winit::window::Window>, config: EngineConfig) -> Result<Self> {
        let window_width = config.get_int("WINDOW_WIDTH")? as u32;
        let window_height = config.get_int("WINDOW_HEIGHT")? as u32;
        let window_size = winit::dpi::PhysicalSize::new(window_width, window_height);

        // Create wgpu instance and window surface
        let (instance, surface) = {
            let backends = match std::env::var("WGPU_BACKENDS").as_deref() {
                Ok("VULKAN") => wgpu::Backends::VULKAN,
                Ok("DX12") => wgpu::Backends::DX12,
                Ok("METAL") => wgpu::Backends::METAL,
                Ok("GL") => wgpu::Backends::GL,
                Ok("BROWSER_WEBGPU") => wgpu::Backends::BROWSER_WEBGPU,
                _ => wgpu::Backends::all(),
            };

            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends,
                flags: wgpu::InstanceFlags::from_build_config().with_env(),
                backend_options: wgpu::BackendOptions::default(),
            });
            let surface =
                instance
                    .create_surface(window.clone())
                    .map_err(|error| -> PillError {
                        PillError::from(format!("Failed to create surface: {}", error))
                    })?;
            (instance, surface)
        };

        // Select GPU adapter and request logical device
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| -> PillError {
                PillError::from(format!("Failed to request adapter: {}", error))
            })?;

        let info = adapter.get_info();
        info!(LogContext::Rendering => "Using GPU: {} ({:?})", info.name, info.backend);

        let (device, queue) = {
            let wanted = wgpu::Features::DEPTH_CLIP_CONTROL
                | wgpu::Features::TIMESTAMP_QUERY
                | wgpu::Features::PIPELINE_STATISTICS_QUERY
                | wgpu::Features::FLOAT32_FILTERABLE;
            let features = wanted & adapter.features();

            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: None,
                    required_features: features,
                    required_limits: wgpu::Limits::default(),
                    memory_hints: wgpu::MemoryHints::default(),
                    trace: wgpu::Trace::default(),
                })
                .await
                .map_err(|error| -> PillError {
                    PillError::from(format!("Failed to request device: {}", error))
                })?
        };

        // Configure swap chain format and present mode
        let (surface_configuration, color_format) = {
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                format,
                width: window_size.width,
                height: window_size.height,
                desired_maximum_frame_latency: 2,
                present_mode,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: if format.is_srgb() {
                    vec![]
                } else {
                    vec![format.add_srgb_suffix()]
                },
            };
            surface.configure(&device, &surface_configuration);
            let color_format = surface_configuration.format;
            (surface_configuration, color_format)
        };

        let pass_textures = PillSlotMap::with_capacity_and_key(10);

        let screenshot = std::env::var("PILL_SCREENSHOT").ok().map(|s| {
            let frame = std::env::var("PILL_SCREENSHOT_FRAME")
                .ok()
                .and_then(|f| f.parse::<u32>().ok())
                .unwrap_or(10);
            (frame, s)
        });

        Ok(Self {
            pass_textures,
            surface,
            device,
            queue,
            surface_configuration,
            color_format,
            passes: Vec::new(),
            window,
            screenshot,
            frame_counter: 0,
        })
    }

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        if new_window_size.width > 0 && new_window_size.height > 0 {
            self.surface_configuration.width = new_window_size.width;
            self.surface_configuration.height = new_window_size.height;
            self.surface
                .configure(&self.device, &self.surface_configuration);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save_screenshot(
    path: &str,
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: u32,
    format: wgpu::TextureFormat,
) {
    use std::io::BufWriter;

    let is_bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );

    let mut rgba: Vec<u8> = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        let row_data = &data[start..start + (width * 4) as usize];
        for pixel in row_data.chunks(4) {
            if is_bgra {
                rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            } else {
                rgba.extend_from_slice(pixel);
            }
        }
    }

    let file = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("screenshot: cannot create {path}: {e}");
            return;
        }
    };
    let mut enc = png::Encoder::new(BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    if let Ok(mut writer) = enc.write_header() {
        let _ = writer.write_image_data(&rgba);
        println!("screenshot saved: {path}");
    }
}
