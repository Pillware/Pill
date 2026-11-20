//! A frame of UI output, made of draw lists per surface and the textures
//! they sample.
//!
//! # Responsibilities
//!
//! - [`UiFrame`]: everything a UI producer (a game's overlay, a tool's
//!   panels) built this update, as owned plain data.
//! - [`UiDraw`]: one surface's draw list and the monitor scale its logical
//!   pixels are in.
//!
//! # Design
//!
//! The value crosses an artifact boundary: a project's system builds it
//! inside one DLL, and a renderer in another DLL draws it. Everything it
//! carries is therefore owned data. The draw list holds plain vertex data,
//! and the textures are a versioned set of copies (see [`crate::texture`]),
//! never a pointer into the producer's atlas. The renderer keeps its own GPU
//! textures, and it uploads an image only when the set carries a revision it
//! has not applied yet.

// Current crate
use crate::draw::DrawList;
use crate::texture::UiTextureSet;

// =============================================================================
// UiDraw
// =============================================================================

/// One surface's UI for a frame.
#[derive(Clone, Debug, Default)]
pub struct UiDraw {
    /// The id of the surface to draw into. The primary surface has id `0`.
    pub surface: u32,
    /// The scale factor of the monitor this list was built for. The list's
    /// positions are in logical pixels, and the backend multiplies them by
    /// this factor when it draws.
    pub scale_factor: f32,
    /// The vertices, indices and commands to draw for this surface.
    pub list: DrawList,
}

// =============================================================================
// UiFrame
// =============================================================================

/// The UI output for one frame. It holds the lists to draw on the surfaces
/// and the textures those lists sample.
#[derive(Clone, Debug, Default)]
pub struct UiFrame {
    /// The lists to draw, in draw order (later entries draw over earlier).
    pub draws: Vec<UiDraw>,
    /// The textures the lists sample, and the ids of the ones to free.
    /// The set holds a texture only when its content changed since the last
    /// time the producer sent it, so an empty set means there is nothing to
    /// upload or free. A renderer that was just rebuilt holds no textures
    /// and asks for them again (see [`crate::texture`]).
    pub textures: UiTextureSet,
}

impl UiFrame {
    /// Returns `true` when there is nothing to draw, upload or free.
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty() && self.textures.is_empty()
    }

    /// Forget the previous frame's work, keeping the allocations.
    pub fn clear(&mut self) {
        self.draws.clear();
        self.textures.clear();
    }
}
