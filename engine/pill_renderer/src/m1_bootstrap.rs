use anyhow::Result;

#[derive(Clone, Debug)]
pub struct WindowComponent {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub vsync: bool,
}

pub fn run_m1(window: &WindowComponent) -> Result<()> {
    // winit event loop + wgpu init per DESIGN.md (M1)
    use winit::{
        dpi::LogicalSize,
        event::{Event, WindowEvent},
        event_loop::EventLoop,
        window::WindowBuilder,
    };

    let event_loop = EventLoop::new().unwrap();
    let winit_window = WindowBuilder::new()
        .with_title(window.title.clone())
        .with_inner_size(LogicalSize::new(window.width as f64, window.height as f64))
        .build(&event_loop)?;
    let vsync = window.vsync; // NEW
    let window = std::sync::Arc::new(winit_window);

    // Instance and surface
    let backends = wgpu::util::backend_bits_from_env().unwrap_or_default();
    let dx12_shader_compiler = wgpu::util::dx12_shader_compiler_from_env().unwrap_or_default();
    let gles_minor_version = wgpu::util::gles_minor_version_from_env().unwrap_or_default();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        dx12_shader_compiler,
        gles_minor_version,
    });
    let surface = instance.create_surface(window.clone()).unwrap();

    // Adapter
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .ok_or_else(|| anyhow::anyhow!("No compatible GPU adapter found"))?;

    // Device/queue
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("m1_device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
        },
        None,
    ))?;

    // Surface configuration per capabilities
    let surface_caps = surface.get_capabilities(&adapter);
    let preferred_formats = [wgpu::TextureFormat::Rgba8UnormSrgb, wgpu::TextureFormat::Bgra8UnormSrgb];
    let format = preferred_formats
        .into_iter()
        .find(|f| surface_caps.formats.contains(f))
        .unwrap_or(surface_caps.formats[0]);

    let present_mode = if vsync {
        wgpu::PresentMode::Fifo
    } else if surface_caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else if surface_caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else {
        wgpu::PresentMode::Fifo
    };

    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width,
        height: size.height,
        desired_maximum_frame_latency: 2,
        present_mode,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![format],
    };
    surface.configure(&device, &config);

    // Depth texture created but unused yet; recreate on resize
    let mut depth_view = create_depth_view(&device, &config);

    event_loop.run(move |event, elwt| {
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(new_size) => {
                    if new_size.width > 0 && new_size.height > 0 {
                        config.width = new_size.width;
                        config.height = new_size.height;
                        surface.configure(&device, &config);
                        depth_view = create_depth_view(&device, &config);
                    }
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    let new_size = window.inner_size();
                    if new_size.width > 0 && new_size.height > 0 {
                        config.width = new_size.width;
                        config.height = new_size.height;
                        surface.configure(&device, &config);
                        depth_view = create_depth_view(&device, &config);
                    }
                }
                _ => {}
            },
            Event::AboutToWait => {
                // Render black frame
                let frame = match surface.get_current_texture() {
                    Ok(frame) => frame,
                    Err(err) => match err {
                        wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated => {
                            surface.configure(&device, &config);
                            return;
                        }
                        wgpu::SurfaceError::Timeout => {
                            return;
                        }
                        wgpu::SurfaceError::OutOfMemory => {
                            elwt.exit();
                            return;
                        }
                    },
                };
                let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("m1_encoder") });
                {
                    let _rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("m1_clear_black"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth_view,
                            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                queue.submit([encoder.finish()]);
                frame.present();
            }
            _ => {}
        }
    })?;

    Ok(())
}

fn create_depth_view(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
    let depth_format = wgpu::TextureFormat::Depth32Float;
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("m1_depth"),
        size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: depth_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    depth.create_view(&wgpu::TextureViewDescriptor::default())
}


