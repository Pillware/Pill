//! The pointer-driven resize of the editor's main window.
//!
//! # Responsibilities
//!
//! - [`WindowRect`]: a window's rectangle on the desktop, in physical pixels.
//! - [`ResizeDrag`]: one resize a caption-strip border band started - the edge,
//!   the rectangle at the press, the press point and the smallest size the
//!   drag may ask for - and [`ResizeDrag::rect_at`], the rectangle the drag
//!   asks for at a pointer position.
//!
//! # Design
//!
//! The editor's main window has no system frame, so its whole rectangle is its
//! client area and one rectangle describes both. Nothing here touches a
//! window, a clock or a GPU: the drag is arithmetic on three points, and the
//! executable applies the answer with `set_outer_position` and
//! `request_inner_size`. That is what makes every edge checkable in a unit
//! test and what makes the gesture follow the pointer on the editor's own
//! frame loop.

// Current crate
use crate::requests::WindowEdge;

// =============================================================================
// Types
// =============================================================================

/// A window's rectangle on the desktop, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowRect {
    /// The left edge, in screen pixels.
    pub x: i32,
    /// The top edge, in screen pixels.
    pub y: i32,
    /// The width, in pixels.
    pub width: u32,
    /// The height, in pixels.
    pub height: u32,
}

impl WindowRect {
    /// Returns the right edge, in screen pixels, and the vertical edge a
    /// west-drag keeps still.
    fn right(&self) -> i32 {
        self.x + self.width as i32
    }

    /// Returns the bottom edge, in screen pixels, and the vertical edge a
    /// north-drag keeps still.
    fn bottom(&self) -> i32 {
        self.y + self.height as i32
    }
}

/// One resize a caption-strip border band started, driven by the pointer.
///
/// The rectangle the window had at the press and the press point are held
/// together, so the answer depends only on where the pointer is now: a drag
/// that is stepped repeatedly resolves the same way and cannot accumulate a
/// drift.
#[derive(Clone, Copy, Debug)]
pub struct ResizeDrag {
    /// The edge or corner the press landed on.
    edge: WindowEdge,
    /// The window's rectangle when the press arrived.
    start: WindowRect,
    /// Where the press landed, in screen pixels.
    press: (i32, i32),
    /// The smallest size the drag may ask for, in physical pixels.
    floor: (u32, u32),
}

impl ResizeDrag {
    /// Starts a drag from `edge`, for a window at `rect` whose border band was
    /// pressed at `press`, in screen pixels.
    ///
    /// `floor` is the smallest rectangle the drag may ask for. A press that has
    /// not moved yet asks for `rect` itself.
    #[must_use]
    pub fn new(edge: WindowEdge, rect: WindowRect, press: (i32, i32), floor: (u32, u32)) -> Self {
        Self {
            edge,
            start: rect,
            press,
            floor,
        }
    }

    /// Returns the edge or corner this drag started from.
    #[must_use]
    pub fn edge(&self) -> WindowEdge {
        self.edge
    }

    /// Returns the rectangle this drag asks for while the pointer is at
    /// `pointer`, in screen pixels.
    ///
    /// The edge opposite the one dragged stays where it was, and the dragged
    /// edge follows the pointer, so a west or north drag moves the window and a
    /// shrink clamps against that fixed edge. A size below `floor` never comes
    /// out: a window narrower than the band that grabs it could not be resized
    /// back.
    #[must_use]
    pub fn rect_at(&self, pointer: (i32, i32)) -> WindowRect {
        let (pointer_x_delta, pointer_y_delta) =
            (pointer.0 - self.press.0, pointer.1 - self.press.1);
        let mut width = self.start.width as i32;
        let mut height = self.start.height as i32;
        if self.edge.grows_westward() {
            width -= pointer_x_delta;
        } else if self.edge.grows_eastward() {
            width += pointer_x_delta;
        }
        if self.edge.grows_northward() {
            height -= pointer_y_delta;
        } else if self.edge.grows_southward() {
            height += pointer_y_delta;
        }
        let width = width.max(self.floor.0 as i32);
        let height = height.max(self.floor.1 as i32);
        // The corner opposite the dragged one does not move, so a clamped
        // shrink walks the dragged edge back to where the floor leaves it
        // instead of moving the whole window.
        let x = if self.edge.grows_westward() {
            self.start.right() - width
        } else {
            self.start.x
        };
        let y = if self.edge.grows_northward() {
            self.start.bottom() - height
        } else {
            self.start.y
        };
        WindowRect {
            x,
            y,
            width: width as u32,
            height: height as u32,
        }
    }
}

/// Which way one edge or corner gives the window more room.
impl WindowEdge {
    /// Whether dragging this edge takes room off the left, which moves the
    /// window's own left edge.
    fn grows_westward(self) -> bool {
        matches!(self, Self::West | Self::NorthWest | Self::SouthWest)
    }

    /// Whether dragging this edge adds room on the right.
    fn grows_eastward(self) -> bool {
        matches!(self, Self::East | Self::NorthEast | Self::SouthEast)
    }

    /// Whether dragging this edge takes room off the top, which moves the
    /// window's own top edge.
    fn grows_northward(self) -> bool {
        matches!(self, Self::North | Self::NorthWest | Self::NorthEast)
    }

    /// Whether dragging this edge adds room at the bottom.
    fn grows_southward(self) -> bool {
        matches!(self, Self::South | Self::SouthWest | Self::SouthEast)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A 800x600 window at the desktop origin, with a 320x240 floor.
    fn start_rect() -> WindowRect {
        WindowRect {
            x: 100,
            y: 50,
            width: 800,
            height: 600,
        }
    }

    const FLOOR: (u32, u32) = (320, 240);

    #[test]
    fn a_press_that_has_not_moved_asks_for_the_start_rectangle() {
        let rect = start_rect();
        let press = (500, 350);
        for edge in WindowEdge::ALL {
            let drag = ResizeDrag::new(edge, rect, press, FLOOR);
            assert_eq!(
                drag.rect_at(press),
                rect,
                "{} moved a window the pointer had not moved",
                edge.name()
            );
            assert_eq!(drag.edge(), edge);
        }
    }

    #[test]
    fn the_east_edge_follows_the_pointer_and_leaves_the_other_edges_alone() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::East, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((620, 410));
        assert_eq!(moved, WindowRect { width: 920, ..rect });
    }

    #[test]
    fn the_west_edge_moves_the_window_and_keeps_its_right_edge_still() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::West, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((380, 350));
        assert_eq!(
            moved,
            WindowRect {
                x: -20,
                width: 920,
                ..rect
            }
        );
        assert_eq!(moved.right(), rect.right());
    }

    #[test]
    fn the_north_edge_moves_the_window_and_keeps_its_bottom_edge_still() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::North, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((500, 300));
        assert_eq!(
            moved,
            WindowRect {
                y: 0,
                height: 650,
                ..rect
            }
        );
        assert_eq!(moved.bottom(), rect.bottom());
    }

    #[test]
    fn the_north_west_corner_moves_both_edges() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::NorthWest, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((460, 320));
        assert_eq!(
            moved,
            WindowRect {
                x: 60,
                y: 20,
                width: 840,
                height: 630,
            }
        );
    }

    #[test]
    fn the_south_east_corner_grows_both_edges() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::SouthEast, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((540, 380));
        assert_eq!(
            moved,
            WindowRect {
                width: 840,
                height: 630,
                ..rect
            }
        );
    }

    #[test]
    fn a_shrink_stops_at_the_floor_without_moving_the_window() {
        let rect = start_rect();
        let east = ResizeDrag::new(WindowEdge::East, rect, (500, 350), FLOOR);
        assert_eq!(
            east.rect_at((-5_000, 350)),
            WindowRect {
                width: FLOOR.0,
                ..rect
            }
        );
        let south_east = ResizeDrag::new(WindowEdge::SouthEast, rect, (500, 350), FLOOR);
        assert_eq!(
            south_east.rect_at((-5_000, -5_000)),
            WindowRect {
                width: FLOOR.0,
                height: FLOOR.1,
                ..rect
            }
        );
    }

    #[test]
    fn a_clamped_shrink_against_the_west_edge_keeps_the_right_edge_still() {
        let rect = start_rect();
        let drag = ResizeDrag::new(WindowEdge::West, rect, (500, 350), FLOOR);
        let moved = drag.rect_at((5_000, 350));
        assert_eq!(
            moved,
            WindowRect {
                x: rect.right() - FLOOR.0 as i32,
                width: FLOOR.0,
                ..rect
            }
        );
        assert_eq!(moved.right(), rect.right());
    }
}
