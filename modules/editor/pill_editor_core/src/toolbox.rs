//! The floating viewport toolbox: its rows, its geometry and the commands a
//! click on a row queues.
//!
//! # Responsibilities
//!
//! - Name the toolbox's rows ([`ToolboxItem`]: the three gizmo tools, the
//!   space toggle, the snapping toggle and the scene grid's switch) and the
//!   order the panel stacks them in ([`TOOLBOX_ITEMS`]).
//! - The row lists the other surfaces share with the panel: the three tools
//!   ([`TOOL_ROWS`]) and the two spaces with their labels and icons
//!   ([`SPACE_ROWS`], [`SpaceRow`]), plus the edit builders for one tool and
//!   one space ([`mode_command`], [`space_command`]).
//! - The vocabulary a row reads: the label it shows for the current state
//!   ([`ToolboxItem::label`]), its icon ([`ToolboxItem::icon`]), whether it
//!   looks active ([`ToolboxItem::active`]), its name in the log
//!   ([`ToolboxItem::name`]) and the command a click queues
//!   ([`ToolboxItem::command`]).
//! - The panel's geometry: where it starts inside a viewport
//!   ([`default_origin`]), how a drag is kept inside the viewport
//!   ([`clamped_origin`]), and the rectangles of the panel, its drag handle and
//!   each row ([`panel_rect`], [`handle_rect`], [`item_rect`]).
//!
//! # Design
//!
//! The file is plain constants and pure functions over egui's geometry types.
//! Their numbers are the ones the shell draws with, so a row's click target,
//! the region that must never become a scene pick and the drag's limit all
//! come from here. A row queues the same edit the shortcuts, the viewport menu
//! and the Preferences tab queue: `SetGizmoTool` for the tools and the
//! snapping, `CycleGizmoSpace` for the space row - which walks World, Local and
//! then the active tool's own frames - and `SetGrid` for the grid.

// External crates
use egui::{Pos2, Rect, Vec2};

// Current crate
use crate::editor_state::EditorCommand;
use crate::gizmo::{GizmoMode, GizmoSettings, GizmoSpace};
use crate::icons::Icon;

// =============================================================================
// Geometry
// =============================================================================

/// The panel's width, in points. It leaves room for one button across the
/// panel, wide enough for the longest label plus the button's padding.
pub const TOOLBOX_WIDTH: f32 = 128.0;

/// The gap between the panel and the viewport's edges, in points. The starting
/// position sits one margin inside the viewport, and on a viewport big enough
/// to hold the panel the clamp keeps its far sides that same margin in.
pub const TOOLBOX_MARGIN: f32 = 10.0;

/// The height of the drag handle, in points: the full-width strip along the
/// panel's top edge.
pub const TOOLBOX_HANDLE_HEIGHT: f32 = 18.0;

/// The gap, in points, between the handle and the first row, and between two
/// rows.
pub const TOOLBOX_ITEM_GAP: f32 = 3.0;

/// The height of one row, in points. Each row is an ordinary button drawn at
/// exactly this height, so the rows read as one column of buttons.
pub const TOOLBOX_ITEM_HEIGHT: f32 = 24.0;

/// The inset, in points, between a row and the panel's sides, and between the
/// last row and the panel's bottom edge.
pub const TOOLBOX_PADDING: f32 = 6.0;

/// Returns the panel's size in points: the handle, the gap below it, one row
/// per item with a gap between rows, and the bottom inset.
pub fn panel_size() -> Vec2 {
    let row_count = TOOLBOX_ITEMS.len() as f32;
    Vec2::new(
        TOOLBOX_WIDTH,
        TOOLBOX_HANDLE_HEIGHT
            + TOOLBOX_ITEM_GAP
            + row_count * TOOLBOX_ITEM_HEIGHT
            + (row_count - 1.0) * TOOLBOX_ITEM_GAP
            + TOOLBOX_PADDING,
    )
}

/// Returns the panel's rectangle when its top-left corner sits at `origin`.
pub fn panel_rect(origin: Pos2) -> Rect {
    Rect::from_min_size(origin, panel_size())
}

/// Returns the drag handle's rectangle: the panel's top strip. It is the one
/// part of the panel where a press starts a drag instead of a click.
pub fn handle_rect(origin: Pos2) -> Rect {
    Rect::from_min_size(origin, Vec2::new(TOOLBOX_WIDTH, TOOLBOX_HANDLE_HEIGHT))
}

/// Returns one row's rectangle inside the panel.
///
/// A row that [`TOOLBOX_ITEMS`] does not list takes the first row's place.
/// Every [`ToolboxItem`] variant is listed, so that only keeps the function
/// total.
pub fn item_rect(item: ToolboxItem, origin: Pos2) -> Rect {
    let row_index = TOOLBOX_ITEMS
        .iter()
        .position(|candidate| *candidate == item)
        .unwrap_or(0) as f32;
    Rect::from_min_size(
        Pos2::new(
            origin.x + TOOLBOX_PADDING,
            origin.y
                + TOOLBOX_HANDLE_HEIGHT
                + TOOLBOX_ITEM_GAP
                + row_index * (TOOLBOX_ITEM_HEIGHT + TOOLBOX_ITEM_GAP),
        ),
        Vec2::new(TOOLBOX_WIDTH - 2.0 * TOOLBOX_PADDING, TOOLBOX_ITEM_HEIGHT),
    )
}

/// Returns where the panel starts before it is dragged: one margin in from the
/// viewport's top-left corner.
pub fn default_origin(viewport: Rect) -> Pos2 {
    Pos2::new(
        viewport.left() + TOOLBOX_MARGIN,
        viewport.top() + TOOLBOX_MARGIN,
    )
}

/// Returns the origin a drag may use: `requested_origin` moved back inside
/// `viewport`, one margin clear of its edges.
///
/// A viewport smaller than the panel cannot hold it. The clamp then pins the
/// panel's top-left corner at its starting position, so the right or bottom
/// part may stick out; the handle and the first rows stay reachable.
pub fn clamped_origin(requested_origin: Pos2, viewport: Rect) -> Pos2 {
    let panel_dimensions = panel_size();
    let minimum_origin = default_origin(viewport);
    let maximum_origin = Pos2::new(
        (viewport.right() - panel_dimensions.x - TOOLBOX_MARGIN).max(minimum_origin.x),
        (viewport.bottom() - panel_dimensions.y - TOOLBOX_MARGIN).max(minimum_origin.y),
    );
    Pos2::new(
        requested_origin.x.clamp(minimum_origin.x, maximum_origin.x),
        requested_origin.y.clamp(minimum_origin.y, maximum_origin.y),
    )
}

// =============================================================================
// Rows
// =============================================================================

/// What the toolbox's rows show: the gizmo's settings, the tool frame it is
/// in, and the scene grid's switch. A row reads this, and the command a click
/// queues changes it for the next frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ToolboxState<'a> {
    /// The gizmo's mode, space and snapping.
    pub gizmo: GizmoSettings,
    /// Whether the scene grid is on.
    pub grid: bool,
    /// The name of the tool frame the gizmo is in, when the active tool offers
    /// one and the space row has reached it.
    pub frame: Option<&'a str>,
}

/// One row of the floating toolbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolboxItem {
    /// The translate tool's row.
    Translate,
    /// The rotate tool's row.
    Rotate,
    /// The scale tool's row.
    Scale,
    /// The space toggle's row: switches the gizmo between world and local
    /// space. The label names the space the gizmo is in.
    Space,
    /// The snapping toggle's row.
    Snap,
    /// The scene grid's switch row.
    Grid,
}

/// Every row, in the order the panel stacks them.
pub const TOOLBOX_ITEMS: [ToolboxItem; 6] = [
    ToolboxItem::Translate,
    ToolboxItem::Rotate,
    ToolboxItem::Scale,
    ToolboxItem::Space,
    ToolboxItem::Snap,
    ToolboxItem::Grid,
];

/// The three tools in the order the viewport menu and the Preferences tab
/// list them. Both surfaces draw their rows from this list, so a new tool is
/// one row here.
pub const TOOL_ROWS: [ToolboxItem; 3] = [
    ToolboxItem::Translate,
    ToolboxItem::Rotate,
    ToolboxItem::Scale,
];

/// One gizmo space as the viewport menu and the Preferences tab draw it: the
/// value it selects, the long label the menu shows, the short one the
/// Preferences row shows, and the icon both use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceRow {
    /// The space this row selects.
    pub space: GizmoSpace,
    /// The long label the viewport menu shows.
    pub menu: &'static str,
    /// The short label the Preferences tab shows.
    pub short: &'static str,
    /// The row's icon.
    pub icon: Icon,
}

/// Both spaces, in the order the viewport menu and the Preferences tab list
/// them. The panel's own space row walks every space the gizmo has, these two
/// and the active tool's frames, and its label and icon come from
/// [`ToolboxItem::label`] and [`ToolboxItem::icon`], which repeat the short
/// labels and icons below.
pub const SPACE_ROWS: [SpaceRow; 2] = [
    SpaceRow {
        space: GizmoSpace::World,
        menu: "World space",
        short: "World",
        icon: Icon::GlobeHemisphereWest,
    },
    SpaceRow {
        space: GizmoSpace::Local,
        menu: "Local space",
        short: "Local",
        icon: Icon::MapPin,
    },
];

impl ToolboxItem {
    /// Returns the row's name in the log: the shell writes it with each row
    /// (`item="translate"`), and the editor's test suite finds a row by it.
    pub fn name(self) -> &'static str {
        match self {
            ToolboxItem::Translate => "translate",
            ToolboxItem::Rotate => "rotate",
            ToolboxItem::Scale => "scale",
            ToolboxItem::Space => "space",
            ToolboxItem::Snap => "snap",
            ToolboxItem::Grid => "grid",
        }
    }

    /// Returns the row's icon: the tool's own mark, a globe or a map pin for
    /// the space the gizmo is in, and a magnet or a grid for the two toggles.
    pub fn icon(self, state: &ToolboxState) -> Icon {
        match self {
            ToolboxItem::Translate => Icon::ArrowsOutCardinal,
            ToolboxItem::Rotate => Icon::ArrowsClockwise,
            ToolboxItem::Scale => Icon::ArrowsOut,
            ToolboxItem::Space => match state.gizmo.space {
                GizmoSpace::World if state.frame.is_none() => Icon::GlobeHemisphereWest,
                // A tool frame is the object's own space, so it takes the map
                // pin the local space uses.
                GizmoSpace::World | GizmoSpace::Local => Icon::MapPin,
            },
            ToolboxItem::Snap => Icon::Magnet,
            ToolboxItem::Grid => Icon::GridFour,
        }
    }

    /// Returns the label the row shows for the current state: a fixed word for
    /// the tools and the two toggles, and the space the gizmo is in for the
    /// space row - the tool frame's own name when the row has reached one.
    pub fn label<'a>(self, state: &ToolboxState<'a>) -> &'a str {
        match self {
            ToolboxItem::Translate => "Translate",
            ToolboxItem::Rotate => "Rotate",
            ToolboxItem::Scale => "Scale",
            ToolboxItem::Space => match (state.frame, state.gizmo.space) {
                (Some(frame), _) => frame,
                (None, GizmoSpace::World) => "World",
                (None, GizmoSpace::Local) => "Local",
            },
            ToolboxItem::Snap => "Snap",
            ToolboxItem::Grid => "Grid",
        }
    }

    /// Returns whether the row looks active: the selected tool, and each
    /// toggle in its non-default state (local space or a tool frame, snapping
    /// on, grid on).
    pub fn active(self, state: &ToolboxState) -> bool {
        match self {
            ToolboxItem::Translate => state.gizmo.mode == GizmoMode::Translate,
            ToolboxItem::Rotate => state.gizmo.mode == GizmoMode::Rotate,
            ToolboxItem::Scale => state.gizmo.mode == GizmoMode::Scale,
            ToolboxItem::Space => state.gizmo.space == GizmoSpace::Local || state.frame.is_some(),
            ToolboxItem::Snap => state.gizmo.snap,
            ToolboxItem::Grid => state.grid,
        }
    }

    /// Returns the label the scene viewport's menu shows: "Snapping" where the
    /// panel says "Snap", and the panel's word for the other rows. The menu
    /// lists the two spaces as rows of their own, so the space toggle only
    /// needs its own name here.
    pub fn menu_label(self) -> &'static str {
        match self {
            ToolboxItem::Translate => "Translate",
            ToolboxItem::Rotate => "Rotate",
            ToolboxItem::Scale => "Scale",
            ToolboxItem::Space => "Space",
            ToolboxItem::Snap => "Snapping",
            ToolboxItem::Grid => "Grid",
        }
    }

    /// Returns the command a click queues: a tool row selects that tool, the
    /// space row walks to the next space, and the snapping and grid rows flip
    /// their switch.
    ///
    /// The space row queues the cycle rather than one space, so it can reach a
    /// frame the active tool offers; the viewport menu and the Preferences tab
    /// still queue one named space through [`space_command`].
    pub fn command(self, state: &ToolboxState) -> EditorCommand {
        match self {
            ToolboxItem::Translate => EditorCommand::SetGizmoTool {
                mode: Some(GizmoMode::Translate),
                space: None,
                snap: None,
            },
            ToolboxItem::Rotate => EditorCommand::SetGizmoTool {
                mode: Some(GizmoMode::Rotate),
                space: None,
                snap: None,
            },
            ToolboxItem::Scale => EditorCommand::SetGizmoTool {
                mode: Some(GizmoMode::Scale),
                space: None,
                snap: None,
            },
            ToolboxItem::Space => EditorCommand::CycleGizmoSpace,
            ToolboxItem::Snap => EditorCommand::SetGizmoTool {
                mode: None,
                space: None,
                snap: Some(!state.gizmo.snap),
            },
            ToolboxItem::Grid => EditorCommand::SetGrid {
                enabled: !state.grid,
            },
        }
    }
}

// =============================================================================
// Commands
// =============================================================================

/// Builds the edit that selects one tool: `SetGizmoTool` with the mode set and
/// the space and snapping left unchanged. The tool rows queue the same edit.
pub fn mode_command(mode: GizmoMode) -> EditorCommand {
    EditorCommand::SetGizmoTool {
        mode: Some(mode),
        space: None,
        snap: None,
    }
}

/// Builds the edit that switches the gizmo to one space: `SetGizmoTool` with
/// the space set and the mode and snapping left unchanged.
pub fn space_command(space: GizmoSpace) -> EditorCommand {
    EditorCommand::SetGizmoTool {
        mode: None,
        space: Some(space),
        snap: None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    /// A viewport that does not start at the screen's corner, so a clamp that
    /// fell back to zero would fail the tests below.
    fn viewport() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(800.0, 600.0))
    }

    #[test]
    fn the_rows_stack_inside_the_panel() {
        let origin = default_origin(viewport());
        let panel = panel_rect(origin);
        assert!(panel.contains_rect(handle_rect(origin)));
        let mut previous_bottom = handle_rect(origin).bottom();
        for item in TOOLBOX_ITEMS {
            let rect = item_rect(item, origin);
            assert!(panel.contains_rect(rect), "{item:?} outside the panel");
            assert!(
                rect.top() >= previous_bottom,
                "{item:?} overlaps the row above"
            );
            assert_eq!(rect.left(), origin.x + TOOLBOX_PADDING);
            assert_eq!(rect.width(), TOOLBOX_WIDTH - 2.0 * TOOLBOX_PADDING);
            previous_bottom = rect.bottom() + TOOLBOX_ITEM_GAP;
        }
        // The last row ends exactly where the bottom inset begins.
        let last = item_rect(ToolboxItem::Grid, origin);
        assert_eq!(panel.bottom(), last.bottom() + TOOLBOX_PADDING);
    }

    #[test]
    fn the_default_origin_sits_at_the_viewports_corner() {
        assert_eq!(default_origin(viewport()), Pos2::new(110.0, 60.0));
    }

    #[test]
    fn a_drag_clamps_inside_the_viewport() {
        let viewport_rect = viewport();
        let panel_dimensions = panel_size();
        let far_origin = clamped_origin(Pos2::new(5000.0, 5000.0), viewport_rect);
        assert_eq!(
            far_origin,
            Pos2::new(
                viewport_rect.right() - panel_dimensions.x - TOOLBOX_MARGIN,
                viewport_rect.bottom() - panel_dimensions.y - TOOLBOX_MARGIN
            )
        );
        let negative_origin = clamped_origin(Pos2::new(-200.0, -200.0), viewport_rect);
        assert_eq!(negative_origin, default_origin(viewport_rect));
        // A point already inside the bounds is left alone.
        let inside = Pos2::new(300.0, 200.0);
        assert_eq!(clamped_origin(inside, viewport_rect), inside);
    }

    #[test]
    fn a_tiny_viewport_keeps_the_top_left_corner() {
        let tiny = Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(40.0, 40.0));
        let clamped = clamped_origin(Pos2::new(500.0, 500.0), tiny);
        assert_eq!(clamped, default_origin(tiny));
    }

    #[test]
    fn labels_follow_the_settings() {
        let mut state = ToolboxState::default();
        assert_eq!(ToolboxItem::Translate.label(&state), "Translate");
        assert_eq!(ToolboxItem::Space.label(&state), "World");
        state.gizmo.space = GizmoSpace::Local;
        assert_eq!(ToolboxItem::Space.label(&state), "Local");
        // A tool frame names itself, whichever settings space is behind it.
        let framed = ToolboxState {
            gizmo: state.gizmo,
            grid: false,
            frame: Some("Fixture"),
        };
        assert_eq!(ToolboxItem::Space.label(&framed), "Fixture");
        assert_eq!(ToolboxItem::Snap.label(&state), "Snap");
        assert_eq!(ToolboxItem::Grid.label(&state), "Grid");
    }

    /// Every row has its own icon; the space row's follows the space the
    /// gizmo is in.
    #[test]
    fn icons_belong_to_their_rows() {
        let mut state = ToolboxState::default();
        assert_eq!(ToolboxItem::Translate.icon(&state), Icon::ArrowsOutCardinal);
        assert_eq!(ToolboxItem::Rotate.icon(&state), Icon::ArrowsClockwise);
        assert_eq!(ToolboxItem::Scale.icon(&state), Icon::ArrowsOut);
        assert_eq!(ToolboxItem::Space.icon(&state), Icon::GlobeHemisphereWest);
        state.gizmo.space = GizmoSpace::Local;
        assert_eq!(ToolboxItem::Space.icon(&state), Icon::MapPin);
        assert_eq!(ToolboxItem::Snap.icon(&state), Icon::Magnet);
        assert_eq!(ToolboxItem::Grid.icon(&state), Icon::GridFour);
        // A tool frame wears the map pin too: it is the object's own space.
        let framed = ToolboxState {
            gizmo: GizmoSettings::default(),
            grid: false,
            frame: Some("Fixture"),
        };
        assert_eq!(ToolboxItem::Space.icon(&framed), Icon::MapPin);
    }

    #[test]
    fn active_follows_the_settings() {
        let mut state = ToolboxState {
            gizmo: GizmoSettings {
                mode: GizmoMode::Scale,
                space: GizmoSpace::Local,
                snap: true,
                ..GizmoSettings::default()
            },
            grid: true,
            frame: None,
        };
        assert!(ToolboxItem::Scale.active(&state));
        assert!(ToolboxItem::Space.active(&state));
        assert!(ToolboxItem::Snap.active(&state));
        assert!(ToolboxItem::Grid.active(&state));
        assert!(!ToolboxItem::Translate.active(&state));
        // A tool frame reads as active even from the world space.
        let framed = ToolboxState {
            gizmo: GizmoSettings {
                mode: GizmoMode::Scale,
                ..GizmoSettings::default()
            },
            grid: true,
            frame: Some("Fixture"),
        };
        assert!(ToolboxItem::Space.active(&framed));
        state.grid = false;
        state.gizmo = GizmoSettings::default();
        assert!(ToolboxItem::Translate.active(&state));
        assert!(!ToolboxItem::Rotate.active(&state));
        assert!(!ToolboxItem::Space.active(&state));
        assert!(!ToolboxItem::Snap.active(&state));
        assert!(!ToolboxItem::Grid.active(&state));
    }

    #[test]
    fn a_click_queues_the_edit_that_belongs_to_the_row() {
        let state = ToolboxState::default();
        assert_eq!(
            ToolboxItem::Rotate.command(&state),
            EditorCommand::SetGizmoTool {
                mode: Some(GizmoMode::Rotate),
                space: None,
                snap: None,
            }
        );
        assert_eq!(
            ToolboxItem::Space.command(&state),
            EditorCommand::CycleGizmoSpace,
            "the space row walks the cycle, so it can reach a tool frame"
        );
        assert_eq!(
            ToolboxItem::Snap.command(&state),
            EditorCommand::SetGizmoTool {
                mode: None,
                space: None,
                snap: Some(true),
            }
        );
        assert_eq!(
            ToolboxItem::Grid.command(&state),
            EditorCommand::SetGrid { enabled: true }
        );
        // Each toggle queues the opposite of the state it shows.
        let flipped = ToolboxState {
            gizmo: GizmoSettings {
                space: GizmoSpace::Local,
                snap: true,
                ..state.gizmo
            },
            grid: true,
            frame: None,
        };
        assert_eq!(
            ToolboxItem::Space.command(&flipped),
            EditorCommand::CycleGizmoSpace,
            "the cycle leaves a tool frame for the world space"
        );
        assert_eq!(
            ToolboxItem::Snap.command(&flipped),
            EditorCommand::SetGizmoTool {
                mode: None,
                space: None,
                snap: Some(false),
            }
        );
        assert_eq!(
            ToolboxItem::Grid.command(&flipped),
            EditorCommand::SetGrid { enabled: false }
        );
    }

    /// The viewport menu and the Preferences tab draw the panel's own rows, in
    /// the panel's order, and the spaces they list are the ones the panel's
    /// space toggle names. One list per thing serves all three surfaces, and
    /// this test keeps them from drifting apart.
    #[test]
    fn the_menu_rows_are_the_panels_rows() {
        assert_eq!(TOOL_ROWS.as_slice(), &TOOLBOX_ITEMS[..3]);
        for item in TOOL_ROWS {
            assert!(TOOLBOX_ITEMS.contains(&item), "{item:?} is not a panel row");
            assert!(!item.menu_label().is_empty(), "{item:?} has no menu label");
        }
        for row in SPACE_ROWS {
            let state = ToolboxState {
                gizmo: GizmoSettings {
                    space: row.space,
                    ..GizmoSettings::default()
                },
                grid: false,
                frame: None,
            };
            assert_eq!(ToolboxItem::Space.label(&state), row.short);
            assert_eq!(ToolboxItem::Space.icon(&state), row.icon);
            assert_eq!(
                space_command(row.space),
                EditorCommand::SetGizmoTool {
                    mode: None,
                    space: Some(row.space),
                    snap: None,
                }
            );
        }
        assert_eq!(
            mode_command(GizmoMode::Rotate),
            EditorCommand::SetGizmoTool {
                mode: Some(GizmoMode::Rotate),
                space: None,
                snap: None,
            }
        );
    }
}
