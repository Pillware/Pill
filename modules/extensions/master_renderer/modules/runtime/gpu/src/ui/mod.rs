//! The UI painter: draws [`pill_ui_frame`] draw lists with wgpu.
//!
//! # Responsibilities
//!
//! - [`UiRenderer`]: draws a [`pill_ui_frame::DrawList`] with one pipeline,
//!   whole-image texture uploads and scissor clipping.
//!
//! # Design
//!
//! The painter turns plain data into GPU work. It keeps no global state,
//! opens no window and handles no events. It does not know who built the
//! lists. A game's egui hands over `pill_ui_frame` values the same way any
//! other producer would. It draws on the renderer's own device and into the
//! renderer's target, so no wgpu object crosses a DLL boundary. It is the
//! `overlay` module's sibling and draws after that module's pass.

/// The draw-list renderer: one pipeline, whole-image texture uploads and scissor clipping.
mod renderer;

pub(crate) use renderer::UiRenderer;
