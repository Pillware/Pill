//! The viewport state a hook reads.
//!
//! # Responsibilities
//!
//! - Define [`EditorViewportState`]: the active scene view, the pointer in it,
//!   that view's rectangle and how many views exist.
//! - Give both frontends one place to publish it, so a callback reads the same
//!   value whichever executable runs it.
//!
//! # Design
//!
//! The active view and the pointer never leave the executable today: the shell
//! draws and the runner tracks. A callback runs inside `pill_editor_core`,
//! which owns neither, so the executable publishes this plain value each frame
//! before it services the editor's requests. The browser has no scene viewport
//! at all and publishes the empty posture - `active_view: None`, no pointer,
//! no rectangle, `view_count: 0` - rather than leaving the resource absent, so
//! a callback can read it unconditionally.
//!
//! The resource is `Keep`: it describes the editor's windows, not the game, so
//! Play and Stop leave it alone.

// External crates
use pill_engine::{Resource, World};

// Current crate
use crate::boundary::ShellRect;

/// The editor's viewports, as one callback-readable value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorViewportState {
    /// The active scene view's index, when a viewport is active.
    pub active_view: Option<usize>,
    /// The pointer inside the active view, in logical points relative to that
    /// view's rectangle.
    pub pointer_in_view: Option<(f32, f32)>,
    /// The active view's rectangle, in logical points relative to the editor's
    /// client area.
    pub view_rect: Option<ShellRect>,
    /// How many scene views exist.
    pub view_count: usize,
}

impl EditorViewportState {
    /// Registers the resource, inserting the empty posture only when the world
    /// has none.
    pub fn ensure(world: &mut World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl Resource for EditorViewportState {
    /// Describes the editor's windows, not the game: a play session leaves it
    /// alone and Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_editor_core::EditorViewportState")
    }
}
