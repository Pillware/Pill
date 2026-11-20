//! Draws engine frames into an RGB565 pixel buffer for display-controller
//! targets.
//!
//! # Responsibilities
//!
//! - Owns the framebuffer and the software rasterizer ([`Renderer`]).
//! - Resizes the framebuffer when the caller reports a new display size.
//! - Draws one frame from the current [`Engine`] ([`Renderer::render`]).
//! - Hands the finished frame to the caller as RGB565 bytes
//!   ([`Renderer::frame_rgb565`]).
//!
//! # Design
//!
//! The renderer produces a frame and stops. It owns no display, no SPI bus and
//! no GPIO pin, and it depends on no hardware crate - the caller takes the
//! bytes and writes them wherever they belong, typically an ST7789 over SPI:
//!
//! ```ignore
//! let mut renderer = Renderer::new(240, 280);
//! renderer.render(&mut engine);
//! display.draw(renderer.frame_rgb565())?;
//! ```
//!
//! Because the driver stays outside, the crate compiles and its tests run on
//! any machine, not only on one wired to a panel.

// External crates
use pill_engine::engine::Engine;

// Current crate
use crate::component::{RenderViewport, VirtualResolution};
use crate::framebuffer::Framebuffer;
use crate::rasterizer::SpriteRasterizer;

// =============================================================================
// Renderer
// =============================================================================

/// Draws the engine world into an RGB565 framebuffer on the CPU.
///
/// Holds the pixel buffer, the rasterizer, and the two overrides a frontend
/// can install: the viewport and the virtual resolution.
pub struct Renderer {
    /// Pixel buffer the world is drawn into each frame.
    framebuffer: Framebuffer,
    /// Turns sprite instances into pixels.
    rasterizer: SpriteRasterizer,
    /// Rectangle to draw in, in physical pixels, or `None` for the whole
    /// framebuffer.
    viewport: Option<RenderViewport>,
    /// Logical scene size that fills the viewport, or `None` for one project
    /// unit per pixel.
    virtual_resolution: Option<VirtualResolution>,
}

impl Renderer {
    /// Create a renderer for a display `width` by `height` pixels in size.
    ///
    /// A 240x280 ST7789 panel is `Renderer::new(240, 280)`. A zero dimension is
    /// raised to one pixel instead of being rejected. Nothing in this call can
    /// fail, and an obviously wrong frame tells the caller about a bad size
    /// more clearly than an error it has no way to fix.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            framebuffer: Framebuffer::new(width, height),
            rasterizer: SpriteRasterizer::new(),
            viewport: None,
            virtual_resolution: None,
        }
    }

    /// Resize the framebuffer to a new display size.
    ///
    /// When the size changes, the old contents are dropped and the next
    /// [`Self::render`] repaints every pixel. A zero dimension is raised to one
    /// pixel.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.framebuffer.resize(width, height);
    }

    /// The framebuffer dimensions in pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.framebuffer.width(), self.framebuffer.height())
    }

    /// Restrict drawing to a physical-pixel rectangle within the framebuffer.
    ///
    /// `None` restores full-framebuffer drawing. Pixels outside the rectangle
    /// keep the background colour rather than stale contents, because each
    /// frame clears the whole buffer before drawing.
    pub fn set_viewport(&mut self, viewport: Option<RenderViewport>) {
        self.viewport = viewport;
    }

    /// Select the logical scene size that should fill the physical viewport.
    ///
    /// `None` keeps one project unit per pixel. This is how a project authored
    /// for, say, 800x600 fills a 240x280 panel without the project knowing the
    /// panel exists. Invalid dimensions disable the override.
    pub fn set_virtual_resolution(&mut self, resolution: Option<VirtualResolution>) {
        self.virtual_resolution = resolution.filter(|resolution| resolution.is_valid());
    }

    /// Draw every `(Position, Sprite)` entity in the engine world.
    ///
    /// This cannot fail: there is no device to lose and no frame to acquire,
    /// so unlike the wgpu backend there is nothing to report. Read the finished
    /// pixels with [`Self::frame_rgb565`].
    ///
    /// # Examples
    ///
    /// ```ignore
    /// let mut renderer = Renderer::new(240, 280);
    /// renderer.set_virtual_resolution(Some(VirtualResolution::new(800.0, 600.0)));
    /// renderer.render(host.engine_mut());
    /// display.draw(renderer.frame_rgb565())?;
    /// ```
    pub fn render(&mut self, engine: &mut Engine) {
        // Step 1: Work out where to draw: the installed viewport clamped to the
        // framebuffer, and the project resolution mapped onto it.
        let (width, height) = (self.framebuffer.width(), self.framebuffer.height());
        let viewport = self
            .viewport
            .unwrap_or_else(|| RenderViewport::full(width, height))
            .clamped_to(width, height)
            .unwrap_or_default();
        let virtual_resolution = resolve_virtual_resolution(self.virtual_resolution, viewport);

        // Step 2: Rasterize the world into that rectangle.
        self.rasterizer.render_in_viewport_with_resolution(
            engine.world(),
            &mut self.framebuffer,
            viewport,
            virtual_resolution,
        );
    }

    /// The last rendered frame as little-endian RGB565 bytes.
    ///
    /// The slice is `width * height * 2` bytes, row-major and top-down, which
    /// is the layout an ST7789 `draw(&[u8])` expects. It stays current until
    /// the next `render` or `resize`. The call takes `&mut self` because it
    /// fills a byte buffer kept from the previous frame instead of allocating
    /// a new one.
    pub fn frame_rgb565(&mut self) -> &[u8] {
        self.framebuffer.as_rgb565_le()
    }

    /// Read-only access to the framebuffer, for callers that need the packed
    /// pixels.
    pub fn framebuffer(&self) -> &Framebuffer {
        &self.framebuffer
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Return the project resolution to draw into, given the viewport's size.
///
/// The rule is the same as the wgpu backend's, so a project looks the same on
/// both: a configured resolution that is valid wins, and otherwise every
/// viewport pixel holds one project unit.
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The framebuffer adopts the requested display size.
    #[test]
    fn renderer_targets_the_requested_display_size() {
        let renderer = Renderer::new(240, 280);
        assert_eq!(renderer.surface_size(), (240, 280));
    }

    /// The frame is two bytes per pixel, as the display controller expects.
    #[test]
    fn frame_bytes_match_the_display_size() {
        let mut renderer = Renderer::new(240, 280);
        assert_eq!(renderer.frame_rgb565().len(), 240 * 280 * 2);
    }

    /// Resizing updates both the reported size and the frame length.
    #[test]
    fn resizing_updates_the_frame_length() {
        let mut renderer = Renderer::new(240, 280);
        renderer.resize(128, 64);

        assert_eq!(renderer.surface_size(), (128, 64));
        assert_eq!(renderer.frame_rgb565().len(), 128 * 64 * 2);
    }

    /// An invalid virtual resolution is rejected rather than stored.
    #[test]
    fn invalid_virtual_resolutions_are_ignored() {
        let mut renderer = Renderer::new(64, 64);
        renderer.set_virtual_resolution(Some(VirtualResolution::new(0.0, 600.0)));
        assert_eq!(renderer.virtual_resolution, None);

        renderer.set_virtual_resolution(Some(VirtualResolution::new(800.0, 600.0)));
        assert_eq!(
            renderer.virtual_resolution,
            Some(VirtualResolution::new(800.0, 600.0))
        );
    }

    /// A configured project resolution stays the same as the display size
    /// changes.
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

    /// Rendering an empty world still produces a full background frame.
    #[test]
    fn rendering_an_empty_world_produces_a_full_frame() {
        let mut engine = Engine::new();
        let mut renderer = Renderer::new(16, 8);
        renderer.render(&mut engine);

        let frame = renderer.frame_rgb565();
        assert_eq!(frame.len(), 16 * 8 * 2);
        // Every pixel carries the same background colour, so every two-byte
        // pixel matches the first one.
        assert!(frame.chunks_exact(2).all(|pixel| pixel == &frame[0..2]));
    }
}
