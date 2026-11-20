//! Window-surface renderer: the wgpu surface, device and frame loop a
//! frontend drives.
//!
//! # Responsibilities
//!
//! - Creates the wgpu instance, surface, adapter, device and queue.
//! - Picks the surface's present and alpha modes, and falls back to safer
//!   pairs when the driver refuses the preferred one.
//! - Reconfigures the surface after frontend resize notifications.
//! - Acquires, draws and presents one frame from the current [`Engine`].
//!
//! # Design
//!
//! Frontends keep their own event loop and window. They pass a window handle
//! to [`Renderer::new`], then call [`Renderer::resize`],
//! [`Renderer::set_viewport`], [`Renderer::set_virtual_resolution`] and
//! [`Renderer::surface_size`] as their window changes, and [`Renderer::render`]
//! once per frame. A frontend never needs to name wgpu or run an async executor
//! itself.

// External crates
use pill_engine::engine::Engine;

// Current crate
use crate::component::{RenderViewport, VirtualResolution};
use crate::sprite::SpriteRenderer;

// =============================================================================
// Re-exports
// =============================================================================

/// Rendering initialization or presentation failure.
///
/// The enum is declared in [`crate::error::RendererError`] and re-exported
/// here so a caller can name it through the `renderer` module.
pub use crate::error::RendererError;

// =============================================================================
// RendererWindow
// =============================================================================

/// Window handle the renderer can create a surface for.
///
/// Every value wgpu accepts as a window implements this trait, so a frontend
/// can pass an `Arc<winit::window::Window>` without naming wgpu.
pub trait RendererWindow: wgpu::WindowHandle {}

impl<T> RendererWindow for T where T: wgpu::WindowHandle {}

// =============================================================================
// Renderer
// =============================================================================

/// GPU state for one frontend window surface.
///
/// Holds everything one frame needs: the surface, device, queue and sprite
/// renderer, plus the optional viewport and logical-size overrides a frontend
/// installs.
pub struct Renderer {
    /// The GPU surface bound to the frontend's window handle.
    surface: wgpu::Surface<'static>,
    /// Logical GPU device used for all rendering commands.
    device: wgpu::Device,
    /// Command queue that submits rendered frames to the device.
    queue: wgpu::Queue,
    /// Surface configuration, applied at creation and again after a resize or a
    /// lost surface.
    surface_config: wgpu::SurfaceConfiguration,
    /// Draws the sprite entities into a texture view each frame.
    sprite_renderer: SpriteRenderer,
    /// Physical-pixel crop rectangle, or `None` for full-surface rendering.
    viewport: Option<RenderViewport>,
    /// Logical scene size filling the viewport, or `None` for one-to-one pixels.
    virtual_resolution: Option<VirtualResolution>,
}

impl Renderer {
    /// Create the GPU surface and all resources needed to draw an engine world.
    ///
    /// The supplied handle is kept by wgpu for as long as the surface lives; an
    /// `Arc<winit::window::Window>` satisfies it without this crate depending
    /// on winit.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceCreation`] or
    /// [`RendererError::AdapterRequest`] if the surface or an adapter for it
    /// cannot be created, [`RendererError::DeviceCreation`] if the device
    /// cannot be created, [`RendererError::NoTextureFormats`] or
    /// [`RendererError::NoAlphaModes`] if the surface offers no texture format
    /// or no alpha mode, and [`RendererError::SurfaceConfigure`] if every
    /// presentation configuration is refused.
    pub fn new<W>(window: W, width: u32, height: u32) -> Result<Self, RendererError>
    where
        W: RendererWindow + 'static,
    {
        // Step 1: create the wgpu instance and bind it to the frontend window.
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|error| RendererError::SurfaceCreation { source: error })?;

        // Step 2: acquire an adapter compatible with the surface.
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|error| RendererError::AdapterRequest { source: error })?;

        // Step 3: request the device and queue from the adapter.
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ECS renderer device"),
            required_features: wgpu::Features::empty(),
            required_limits:
                wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::default(),
            ..Default::default()
        }))
        .map_err(|error| RendererError::DeviceCreation { source: error })?;

        // Step 4: derive the surface format, alpha mode, and presentation mode.
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .first()
            .copied()
            .ok_or(RendererError::NoTextureFormats)?;
        let alpha_mode =
            select_alpha_mode(&capabilities.alpha_modes).ok_or(RendererError::NoAlphaModes)?;
        let present_mode = select_present_mode(&capabilities.present_modes);

        let mut surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };

        // Step 5: configure the surface, with a safer fallback when refused.
        //
        // `Surface::configure` returns nothing. A refused configuration shows
        // up as a device error instead, and the device panics on those by
        // default, which would end the whole host before a frontend could see
        // a `RendererError`. A refusal is possible here: an uncapped present
        // mode needs a flip-model swapchain, and a compositing alpha mode
        // needs a compositing window, and the capability list promises neither.
        // Each candidate pair therefore gets its own error scopes, and the
        // first one the driver accepts wins.
        let mut configured = false;
        for (candidate_present, candidate_alpha) in configure_candidates(present_mode, alpha_mode) {
            surface_config.present_mode = candidate_present;
            surface_config.alpha_mode = candidate_alpha;

            // One error scope per filter, so a refusal is caught here instead
            // of reaching the device's uncaptured-error handler.
            device.push_error_scope(wgpu::ErrorFilter::Validation);
            device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            device.push_error_scope(wgpu::ErrorFilter::Internal);
            surface.configure(&device, &surface_config);
            // The swapchain is only created when a frame is acquired, so the
            // probe takes one. wgpu can accept the configuration and still fail
            // here, and without a scope around this call the refusal would be
            // fatal.
            let probe = surface.get_current_texture();
            let internal = pollster::block_on(device.pop_error_scope());
            let out_of_memory = pollster::block_on(device.pop_error_scope());
            let validation = pollster::block_on(device.pop_error_scope());

            let mut failure = internal
                .or(out_of_memory)
                .or(validation)
                .map(|error| error.to_string());
            let probe_frame = match probe {
                Ok(frame) => Some(frame),
                Err(error) => {
                    failure.get_or_insert_with(|| error.to_string());
                    None
                }
            };

            match (failure, probe_frame) {
                (None, Some(frame)) => {
                    // The probe frame is discarded; the frontend acquires its
                    // own once the first real frame is drawn.
                    drop(frame);
                    configured = true;
                    break;
                }
                (_, _) => {}
            }
        }

        if !configured {
            // Every candidate was refused.
            return Err(RendererError::SurfaceConfigure);
        }

        // Step 6: build the sprite renderer and assemble the renderer state.
        let sprite_renderer = SpriteRenderer::new(&device, format);
        Ok(Self {
            surface,
            device,
            queue,
            surface_config,
            sprite_renderer,
            viewport: None,
            virtual_resolution: None,
        })
    }

    /// Reconfigure the presentation surface for a new physical window size.
    ///
    /// Zero-sized notifications occur while a window is minimized and are
    /// ignored because wgpu surfaces cannot be configured with zero dimensions.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.configure_surface();
    }

    /// Return the physical dimensions of the currently configured surface.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }

    /// Restrict rendering to a physical-pixel rectangle within the surface.
    ///
    /// Passing `None` restores full-surface rendering. Frontends embedding the
    /// surface behind UI should update this rectangle whenever their layout or
    /// window scale changes.
    pub fn set_viewport(&mut self, viewport: Option<RenderViewport>) {
        self.viewport = viewport;
    }

    /// Select the logical scene size that should fill the physical viewport.
    ///
    /// `None` keeps one logical unit per surface pixel. Invalid dimensions are
    /// rejected by disabling the override.
    pub fn set_virtual_resolution(&mut self, resolution: Option<VirtualResolution>) {
        self.virtual_resolution = resolution.filter(|resolution| resolution.is_valid());
    }

    /// Draw and present every `(Position, Sprite)` entity in the engine world.
    ///
    /// A lost or outdated surface is reconfigured and the frame is skipped.
    /// A timeout is temporary and skips the frame too. A fatal allocation
    /// failure, or any other surface failure, goes back to the frontend to be
    /// reported.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceTextureFailed`] when the frame texture
    /// cannot be acquired due to an out-of-memory condition or an unknown
    /// backend failure. Lost, outdated, and timed-out surfaces are recovered
    /// internally and never produce an error.
    pub fn render(&mut self, engine: &mut Engine) -> Result<(), RendererError> {
        // Step 1: acquire the next frame texture, recovering transient errors.
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.configure_surface();
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(error @ (wgpu::SurfaceError::OutOfMemory | wgpu::SurfaceError::Other)) => {
                return Err(RendererError::SurfaceTextureFailed { source: error });
            }
        };

        // Step 2: build the texture view and resolve the viewport and projection.
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let viewport = self
            .viewport
            .unwrap_or_else(|| {
                RenderViewport::full(self.surface_config.width, self.surface_config.height)
            })
            .clamped_to(self.surface_config.width, self.surface_config.height)
            .unwrap_or_default();

        let virtual_resolution = resolve_virtual_resolution(self.virtual_resolution, viewport);

        // Step 3: draw the sprite world into the view and present the frame.
        self.sprite_renderer.render_in_viewport_with_resolution(
            engine.world_mut(),
            &self.device,
            &self.queue,
            &view,
            viewport,
            virtual_resolution,
        );
        frame.present();
        Ok(())
    }

    /// Reapply the stored surface configuration after a resize or a lost or
    /// outdated surface.
    fn configure_surface(&self) {
        self.surface.configure(&self.device, &self.surface_config);
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Return the logical size to project into: the configured one when it is
/// valid, otherwise the viewport's size in pixels.
fn resolve_virtual_resolution(
    configured_resolution: Option<VirtualResolution>,
    viewport: RenderViewport,
) -> VirtualResolution {
    configured_resolution
        .filter(|resolution| resolution.is_valid())
        .unwrap_or_else(|| {
            VirtualResolution::new(viewport.width.max(1) as f32, viewport.height.max(1) as f32)
        })
}

/// Present and alpha mode pairs to try, most preferred first.
///
/// The first pair is the one the selection helpers chose. The later pairs give
/// up one preference at a time, because the capability list does not promise
/// either of them: a compositing alpha mode needs a compositing window, and an
/// uncapped present mode needs a flip-model swapchain. `Fifo` with `Opaque` is
/// the pair every surface must support, so it comes last.
///
/// The list is never empty, because the caller relies on trying at least one
/// pair.
fn configure_candidates(
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
) -> Vec<(wgpu::PresentMode, wgpu::CompositeAlphaMode)> {
    let mut candidates = vec![(present_mode, alpha_mode)];
    if alpha_mode != wgpu::CompositeAlphaMode::Opaque {
        candidates.push((present_mode, wgpu::CompositeAlphaMode::Opaque));
    }
    if present_mode != wgpu::PresentMode::Fifo {
        candidates.push((wgpu::PresentMode::Fifo, wgpu::CompositeAlphaMode::Opaque));
    }
    candidates
}

/// Return the present mode to configure the surface with.
///
/// The answer is always `Fifo`. It is the only mode a Vulkan surface must
/// support, and a mode that `Surface::get_capabilities` lists may still fail to
/// create: the NVIDIA Vulkan driver on Windows advertises `Immediate` and then
/// fails the swapchain with an out-of-memory error. wgpu reports that through
/// the device's uncaptured-error path, which ends the host before any
/// `RendererError` can be built. The advertised list is therefore ignored.
fn select_present_mode(_supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    wgpu::PresentMode::Fifo
}

/// Pick an alpha mode that lets a host draw its own UI over the rendered image.
///
/// A window the platform keeps opaque simply shows the rendered image as it
/// is. A host that asks its window for transparency can draw its own layers on
/// top and let this surface show through.
fn select_alpha_mode(supported: &[wgpu::CompositeAlphaMode]) -> Option<wgpu::CompositeAlphaMode> {
    supported
        .iter()
        .copied()
        .find(|mode| *mode == wgpu::CompositeAlphaMode::PostMultiplied)
        .or_else(|| {
            supported
                .iter()
                .copied()
                .find(|mode| *mode == wgpu::CompositeAlphaMode::PreMultiplied)
        })
        .or_else(|| supported.first().copied())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The guaranteed mode wins even when the surface advertises an uncapped one.
    ///
    /// An advertised mode is not a promise. The NVIDIA Vulkan driver on Windows
    /// lists `Immediate` and then fails to create the swapchain for it, so the
    /// selector must not prefer it.
    #[test]
    fn present_mode_always_selects_fifo() {
        let advertised = [
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Fifo,
        ];
        assert_eq!(select_present_mode(&advertised), wgpu::PresentMode::Fifo);
        assert_eq!(
            select_present_mode(&[wgpu::PresentMode::Fifo]),
            wgpu::PresentMode::Fifo
        );
    }

    /// Transparent UI hosts prefer an explicitly composited alpha mode.
    #[test]
    fn alpha_mode_prefers_composited_surface() {
        let supported = [
            wgpu::CompositeAlphaMode::Opaque,
            wgpu::CompositeAlphaMode::PreMultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied,
        ];
        assert_eq!(
            select_alpha_mode(&supported),
            Some(wgpu::CompositeAlphaMode::PostMultiplied)
        );
    }

    /// Platforms without composited modes retain their first supported mode.
    #[test]
    fn alpha_mode_falls_back_to_first_supported_mode() {
        assert_eq!(
            select_alpha_mode(&[wgpu::CompositeAlphaMode::Opaque]),
            Some(wgpu::CompositeAlphaMode::Opaque)
        );
    }

    /// Embedded viewports cannot extend beyond their native surface.
    #[test]
    fn render_viewport_clamps_to_surface_bounds() {
        assert_eq!(
            RenderViewport::new(80, 40, 50, 70).clamped_to(100, 90),
            Some(RenderViewport::new(80, 40, 20, 50))
        );
        assert_eq!(
            RenderViewport::new(100, 0, 20, 20).clamped_to(100, 90),
            None
        );
    }

    /// A configured project coordinate space remains stable as the panel changes.
    #[test]
    fn virtual_resolution_is_independent_of_physical_viewport_size() {
        let configured_resolution = VirtualResolution::new(800.0, 600.0);

        assert_eq!(
            resolve_virtual_resolution(
                Some(configured_resolution),
                RenderViewport::new(240, 80, 517, 463)
            ),
            configured_resolution
        );
        assert_eq!(
            resolve_virtual_resolution(None, RenderViewport::new(240, 80, 517, 463)),
            VirtualResolution::new(517.0, 463.0)
        );
    }
}
