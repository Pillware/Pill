//! The plain draw data a UI producer builds and a renderer draws.
//!
//! # Responsibilities
//!
//! - [`draw`]: the draw vocabulary a backend consumes. [`DrawList`] holds the
//!   vertices, the indices and one command list of clip changes and draws,
//!   with the values they use ([`UiVertex`], [`DrawCommand`], [`Color`],
//!   [`Rect`], [`TextureId`]).
//! - [`frame`]: a frame of UI output. [`UiFrame`] holds one [`UiDraw`] per
//!   surface, and the frame carries the textures those lists sample.
//! - [`texture`]: the versioned texture set ([`UiTextureSet`], [`UiTexture`],
//!   [`UiTextureFilter`]). A backend uploads a texture again when its revision
//!   changes, so a rebuilt backend uploads what it lacks instead of drawing
//!   nothing.
//! - [`slot`]: the world resource ([`UiFrameSlot`]) that carries a frame from
//!   the producer to the renderer.
//!
//! # Design
//!
//! These types cross DLL boundaries: one DLL builds the UI data and another
//! draws it (`pill_renderer_api::RenderFrame.ui`). They live in their own
//! shared crate so every artifact that builds or draws UI uses the same
//! types, and so the renderer contract can carry them without depending on
//! the UI library. Everything here is owned plain data.

/// The draw vocabulary a backend consumes.
pub mod draw;
/// A frame of UI output.
pub mod frame;
/// The world resource that carries a frame from the producer to the renderer.
pub mod slot;
/// The versioned textures a frame carries.
pub mod texture;

pub use draw::{Color, DrawCommand, DrawList, Rect, TextureId, UiVertex};
pub use frame::{UiDraw, UiFrame};
pub use slot::UiFrameSlot;
pub use texture::{UiTexture, UiTextureFilter, UiTextureSet};
