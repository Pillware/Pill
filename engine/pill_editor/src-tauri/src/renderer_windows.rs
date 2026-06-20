//! wgpu renderer for Windows: green background + red triangle.
//! Runs on a dedicated thread so it never blocks the Tauri event loop.
//!
//! Uses Win32 child-window HWNDs as render targets via
//! `wgpu::SurfaceTargetUnsafe::RawHandle`.

#![cfg(target_os = "windows")]

use std::num::NonZeroIsize;
use raw_window_handle::{RawWindowHandle, RawDisplayHandle, Win32WindowHandle, WindowsDisplayHandle};

/// Manages wgpu render viewports. Each successful `register_viewport` call
/// spawns a dedicated render thread for one child-window HWND.
pub struct Renderer;

impl Renderer {
    pub fn new() -> Self {
        Renderer
    }

    /// Spawn a render thread for one child window `HWND`.
    ///
    /// Returns a `Sender` that can be used to signal a resize: send
    /// `(new_width, new_height)` and the render thread will reconfigure the
    /// wgpu surface on its next frame.
    pub fn register_viewport(
        &self,
        hwnd: isize,
        width: u32,
        height: u32,
    ) -> std::sync::mpsc::Sender<Option<(u32, u32)>> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<(u32, u32)>>();
        std::thread::Builder::new()
            .name("wgpu-viewport".into())
            .spawn(move || {
                pollster::block_on(render_loop(hwnd, width, height, rx))
            })
            .expect("failed to spawn render thread");
        tx
    }
}

// No vertex buffer at all — positions are hardcoded in the shader via
// @builtin(vertex_index).
const SHADER: &str = "
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    var pos = array<vec2<f32>, 3>(
        vec2<f32>( 0.0,  0.8),
        vec2<f32>(-0.8, -0.8),
        vec2<f32>( 0.8, -0.8),
    );
    return vec4<f32>(pos[vi], 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
";

async fn render_loop(
    hwnd: isize,
    width: u32,
    height: u32,
    resize_rx: std::sync::mpsc::Receiver<Option<(u32, u32)>>,
) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN | wgpu::Backends::DX12 | wgpu::Backends::GL,
        ..Default::default()
    });

    // Build raw window handle from the child window's HWND.
    let win_handle = Win32WindowHandle::new(NonZeroIsize::new(hwnd).unwrap());
    let display_handle = WindowsDisplayHandle::new();

    let wgpu_surface = unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_window_handle: RawWindowHandle::Win32(win_handle),
                raw_display_handle: RawDisplayHandle::Windows(display_handle),
            })
            .expect("create_surface_unsafe failed")
    };

    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&wgpu_surface),
            ..Default::default()
        })
        .await
        .expect("no suitable adapter");

    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            },
            None,
        )
        .await
        .expect("request_device failed");

    let caps = wgpu_surface.get_capabilities(&adapter);
    let format = caps.formats[0];
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width,
        height,
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: wgpu::CompositeAlphaMode::Opaque,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    wgpu_surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tri-shader"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tri-layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });

    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("tri-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs_main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            front_face: wgpu::FrontFace::Ccw,
            polygon_mode: wgpu::PolygonMode::Fill,
            strip_index_format: None,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
    });

    println!(
        "[wgpu] Windows renderer ready ({}x{}, format={:?})",
        width, height, format
    );

    let green = wgpu::Color {
        r: 0.0,
        g: 0.6,
        b: 0.0,
        a: 1.0,
    };

    loop {
        // Apply any pending resize/shutdown signal from the main thread.
        match resize_rx.try_recv() {
            Ok(Some((w, h))) => {
                config.width = w.max(1);
                config.height = h.max(1);
                wgpu_surface.configure(&device, &config);
            }
            Ok(None) => {
                // Shutdown signal — tab was closed.
                break;
            }
            Err(_) => {} // empty or disconnected — keep rendering
        }

        match wgpu_surface.get_current_texture() {
            Ok(frame) => {
                let view = frame.texture.create_view(&Default::default());
                let mut enc = device.create_command_encoder(&Default::default());
                {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("frame"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(green),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&pipeline);
                    pass.draw(0..3, 0..1);
                }
                queue.submit(Some(enc.finish()));
                frame.present();
            }
            Err(wgpu::SurfaceError::Lost) => {
                wgpu_surface.configure(&device, &config);
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                eprintln!("[wgpu] out of memory — shutting down render thread");
                break;
            }
            Err(e) => {
                eprintln!("[wgpu] surface error: {e:?}");
            }
        }
    }

    println!("[wgpu] render thread exited");
}
