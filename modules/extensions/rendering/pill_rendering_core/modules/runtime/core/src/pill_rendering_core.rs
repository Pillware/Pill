//! The rendering logic every renderer shares, with no graphics stack behind it.
//!
//! # Responsibilities
//!
//! - Build a frame from the world: the pass chain, the camera each surface
//!   draws through, the instances to draw and the clocks and UI the shaders and
//!   painter read ([`frame`]).
//! - Turn a camera and a transform into the matrices a shader reads, replacing
//!   projection inputs no matrix can use ([`camera`]).
//! - Project debug geometry to the screen and clip it at the camera's near side
//!   ([`projection`]).
//! - Build an instance's model matrix ([`instance`]) and the one key a draw
//!   queue sorts by ([`draw_order`]).
//! - Read a material's slots by name, the way both renderers draw them
//!   ([`materials`]).
//! - Answer a screen point with a world-space ray and test it against a mesh
//!   ([`picking`]).
//!
//! # Design
//!
//! This crate is the shared half of every renderer pair. It links the
//! rendering data (`pill_rendering`) and the contract (`pill_renderer_api`),
//! and it links no graphics stack: a renderer compiles it into its own image
//! and calls it, so two renderers draw the same frame from the same code.
//!
//! It registers nothing - no component, no resource, no system - and holds no
//! graphics state. Everything here is a plain function of values a renderer
//! passes in and gets back, which is what lets it be linked into a module
//! image without registering anything twice.
//!
//! Editing this crate reaches a running renderer the way any of its sources
//! does: the renderer depends on it by path, so saving here rebuilds and
//! reloads that renderer.

/// Turning a camera and a transform into the matrices a shader reads.
pub mod camera;
/// The key a draw queue sorts by, and the slot identity it packs.
pub mod draw_order;

/// The 5 by 7 bitmap font debug labels are drawn in, shared by every renderer.
pub mod debug_font;

/// Building a frame from the world.
pub mod frame;

/// The per-instance model matrix.
pub mod instance;

/// Reading a material's slots by name.
pub mod materials;

/// A world-space ray for a screen point, and a test against a mesh.
pub mod picking;

/// Projecting debug geometry to the screen, and clipping it at the near side.
pub mod projection;
