//! Software sprite renderer for display-controller targets, with no GPU.
//!
//! # Responsibilities
//!
//! - Defines the sprite components ([`Position`], [`Color`], [`Sprite`]), the
//!   viewport types ([`RenderViewport`], [`VirtualResolution`]) and the
//!   per-entity record the rasterizer draws ([`SpriteInstance`]).
//! - Rasterizes one frame from an [`Engine`](pill_engine::engine::Engine) into
//!   an RGB565 framebuffer and hands over its bytes ([`SpriteRasterizer`],
//!   [`Framebuffer`], [`Renderer`]).
//!
//! # Design
//!
//! This crate follows the same renderer contract as `pill_2d_renderer`, in
//! software, for machines that have a display but no usable GPU - a Raspberry
//! Pi driving an SPI panel, for example. The crate stops at the framebuffer:
//! it owns no display, no SPI bus and no GPIO pin, and depends on no hardware
//! crate.
//!
//! [`component`] copies `pill_2d_renderer`'s module instead of sharing a
//! dependency, so a display target never links wgpu just to name `Sprite`. The
//! two `#[repr(C)]` copies must stay identical. The methods match the wgpu
//! backend's: construction takes dimensions instead of a window handle, and
//! `render` cannot fail.

// =============================================================================
// Modules
// =============================================================================

/// The sprite components, the viewport types and the per-entity instance record.
pub mod component;

/// The RGB565 pixel buffer and its colour packing.
pub mod framebuffer;

/// The software rasterizer that turns sprite instances into pixels.
pub mod rasterizer;

/// The renderer that owns the framebuffer and draws one frame per call.
pub mod renderer;

// =============================================================================
// Re-exports
// =============================================================================

// The renderer's public surface, so callers write
// `pill_embedded_renderer::Sprite` instead of reaching into the module that
// declares it. `Position` and `Color` belong to this renderer, not to the
// engine: a sprite's pixel placement and fill are the renderer's business.
// They sit here so one `use` covers both `register_components` and the
// components it registers.
pub use component::{
    register_components, sprite_instances, Color, Position, RenderViewport, Sprite, SpriteInstance,
    VirtualResolution,
};
pub use framebuffer::{pack_rgb565, unpack_rgb565, Framebuffer, BYTES_PER_PIXEL};
pub use rasterizer::SpriteRasterizer;
pub use renderer::Renderer;

// NOTE: this crate has no `RendererError` and no `RendererWindow`, the two
// types `pill_2d_renderer` declares for wgpu.
//
// That error type covers surface, adapter, device and frame-acquisition
// failures, and most of its variants hold a `wgpu` type. Drawing into a buffer
// this crate owns has none of those steps, so an enum here would be empty and
// `new` and `render` would return a `Result` that is always `Ok`. Presenting
// the frame can fail, but that belongs to the caller's display driver, which
// reports failures in its own terms.
//
// `RendererWindow` stands for a window handle that wgpu can bind a surface to.
// This backend has no surface and no window: it draws to a panel described by
// its pixel dimensions.
