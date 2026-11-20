//! A wgpu sprite renderer kept in its own crate. It holds the sprite components
//! and the GPU pipeline that draws them.
//!
//! # Responsibilities
//!
//! - Defines the data a sprite is made of ([`Position`], [`Color`], [`Sprite`]),
//!   the region it is drawn into ([`RenderViewport`], [`VirtualResolution`]) and
//!   the per-entity record the pipeline draws ([`SpriteInstance`]).
//! - Creates the GPU surface, adapter, device and queue for a window and draws
//!   one frame through them ([`Renderer`]).
//! - Owns the sprite pipeline and its buffers ([`SpriteRenderer`]) and declares
//!   the failure type ([`RendererError`]).
//!
//! # Design
//!
//! A sprite is something only a renderer understands, so its components live
//! here with the code that draws them, not in `pill_engine`.
//!
//! The crate has two halves. [`component`] holds the data types and names no
//! `wgpu` type: it finds drawable entities by reading the world through
//! [`World::archetypes_iter`](pill_engine::world::World::archetypes_iter) and
//! [`World::component_registry`](pill_engine::world::World::component_registry).
//! [`sprite`] holds the GPU pipeline and its own `bytemuck` record of the bytes
//! it uploads.
//!
//! The crate is a plain `rlib`, not a hot-loadable module. A renderer needs a
//! live window handle, access to the world every frame and the frontend's event
//! loop, and the module ABI is a one-shot interface that gives it none of them.
//! The workspace excludes this folder, so no workspace build compiles it.

// =============================================================================
// Modules
// =============================================================================

/// The data half of the renderer: the sprite components, the viewport types and
/// the per-entity record the pipeline draws.
pub mod component;

/// The renderer's error type: failures from creating the surface, the adapter
/// or the device, and from presenting a frame.
pub mod error;

/// Creates the GPU surface, device and queue for a window and draws frames
/// through them.
pub mod renderer;

/// The sprite render pipeline and its GPU buffers.
pub mod sprite;

// =============================================================================
// Re-exports
// =============================================================================

// The renderer's public surface, so callers write `pill_2d_renderer::Sprite`
// instead of reaching into the module that declares it. `Position` and `Color`
// belong to this renderer, not to the engine: a sprite's pixel placement and
// fill are the renderer's business. They sit here so one `use` covers both
// `register_components` and the components it registers.
pub use component::{
    register_components, sprite_instances, Color, Position, RenderViewport, Sprite, SpriteInstance,
    VirtualResolution,
};
pub use error::RendererError;
pub use renderer::{Renderer, RendererWindow};
pub use sprite::SpriteRenderer;
