//! What exists because an editor asks for it.
//!
//! # Responsibilities
//!
//! - Name the editor's own viewports ([`views::editor_view_surfaces`]) and the
//!   grid they draw ([`grid::SceneGrid`], [`grid::frame_grid`]).
//! - Carry a selection to outline ([`selection_outlines::SelectionOutlines`]).
//! - Ask what is under a pixel and carry the answer back
//!   ([`picking::PickRequests`], [`picking::PickResults`]).
//! - Ask for an asset preview drawn off-screen and carry its pixels back
//!   ([`preview::PreviewRequests`], [`preview::PreviewResults`]).
//!
//! # Design
//!
//! Everything here is reached through `pill_renderer_api::editor::`, and
//! nothing here is re-exported at the crate root: a frontend that is not an
//! editor never needs any of it, so the contract's own surface says which types
//! only an editor uses. The values still travel as world resources between a
//! frontend and a renderer, which is why they live in the contract at all.
//!
//! The frame keeps the editor's inputs as plain fields ([`crate::RenderFrame`]'s
//! `editor_views`, `grid_cell_size` and `preview_requests`), so a renderer reads
//! one frame and needs no other editor resource.

// =============================================================================
// Modules
// =============================================================================

/// The scene grid of the view-bound surfaces: whether it is on, its line
/// spacing, and the spacing one frame uses.
pub mod grid;

/// Picking: the requests a frontend asks and the answers a renderer delivers.
pub mod picking;

/// Asset previews: off-screen renders the editor asks a renderer for.
pub mod preview;

/// The entities an editor outlines.
pub mod selection_outlines;

/// The surfaces an editor draws its own views through.
pub mod views;

// =============================================================================
// Re-exports
// =============================================================================

pub use grid::{frame_grid, SceneGrid};
pub use picking::{
    decode_pick_value, PickHit, PickRequest, PickRequests, PickResult, PickResults, PICK_HANDLE_BIT,
};
pub use preview::{
    ensure_preview_resources, PreviewAsset, PreviewGeneration, PreviewImage, PreviewRequest,
    PreviewRequests, PreviewResults,
};
pub use selection_outlines::SelectionOutlines;
pub use views::editor_view_surfaces;
