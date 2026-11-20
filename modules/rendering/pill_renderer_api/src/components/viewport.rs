//! The rectangle of the target a frame draws into.
//!
//! # Responsibilities
//!
//! - Define [`RenderViewport`]: the pixel area inside a surface that the
//!   renderer draws into when a frontend asks for only part of it.
//!
//! # Design
//!
//! It is neither a component nor an asset. It is a plain value that the
//! renderer stores, so it has no ECS derives and nothing registers it. It sits
//! in `components/` beside the camera, the contract's other scene type.

// =============================================================================
// RenderViewport
// =============================================================================

/// A rectangle of the target that a frame draws into, in pixels.
///
/// The renderer may hold one for a surface and draw only inside it. When it
/// holds none, or the one it holds has no area left inside the target, it
/// draws the whole target instead.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RenderViewport {
    /// Left edge of the rectangle within the target, in pixels.
    pub x: u32,
    /// Top edge of the rectangle within the target, in pixels.
    pub y: u32,
    /// Width of the rectangle, in pixels.
    pub width: u32,
    /// Height of the rectangle, in pixels.
    pub height: u32,
}

impl RenderViewport {
    /// Creates a viewport from explicit pixel bounds.
    ///
    /// Nothing is checked here: the values are stored unchanged. To fit the
    /// rectangle to a target that may be smaller, use
    /// [`RenderViewport::clamped_to`].
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// Returns a viewport that covers the whole target.
    ///
    /// It starts at the origin and measures `target_width` by
    /// `target_height`. The renderer substitutes it when no viewport is set,
    /// or when an explicit one would draw nothing.
    pub const fn full(target_width: u32, target_height: u32) -> Self {
        Self::new(0, 0, target_width, target_height)
    }

    /// Clips the rectangle to a target of the given pixel size.
    ///
    /// Returns `None` when nothing of the rectangle stays inside the target:
    /// the origin is at or past the target's edge, or the visible width or
    /// height is zero. The renderer treats `None` as "draw the full target".
    pub fn clamped_to(self, target_width: u32, target_height: u32) -> Option<Self> {
        let x = self.x.min(target_width);
        let y = self.y.min(target_height);
        let width = self.width.min(target_width - x);
        let height = self.height.min(target_height - y);
        (width > 0 && height > 0).then_some(Self::new(x, y, width, height))
    }
}
