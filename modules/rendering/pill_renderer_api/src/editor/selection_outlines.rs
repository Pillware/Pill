//! Selection outlines: the entities an editor draws an outline around.
//!
//! # Responsibilities
//!
//! - Name the entities to outline and the colour to outline them in
//!   ([`SelectionOutlines`], a world resource).
//!
//! # Design
//!
//! The outline is asked for through the world like every other editor input: a
//! frontend writes the selection into the resource and any renderer draws it.
//! An entity that draws nothing shows no outline, so a stale entity in the list
//! costs nothing.

// External crates
use pill_engine::{Entity, Resource};

// =============================================================================
// SelectionOutlines
// =============================================================================

/// The entities a renderer outlines, in one colour.
#[derive(Debug, Clone)]
pub struct SelectionOutlines {
    /// The entities to outline. An entity that draws nothing shows no outline.
    pub entities: Vec<Entity>,
    /// The outline colour, linear RGBA.
    pub color: [f32; 4],
}

impl Default for SelectionOutlines {
    fn default() -> Self {
        Self {
            entities: Vec::new(),
            color: [1.0, 0.55, 0.1, 1.0],
        }
    }
}

impl Resource for SelectionOutlines {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_renderer_api::SelectionOutlines")
    }
}
