//! The floating viewport toolbox: its rows, its geometry and the commands a
//! click on a row queues.
//!
//! # Responsibilities
//!
//! - Name the toolbox's rows ([`ToolboxItem`]: the three gizmo tools, one row
//!   per component tool the selection is offered, the space toggle, the
//!   snapping toggle and the two switches) and the order the panel stacks them
//!   in ([`rows`], and [`TOOLBOX_ITEMS`] for the rows every editor has).
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
//! then the active tool's own frames - `SetGrid` and `SetGizmos` for the two
//! switches, and `SetTool` for a component tool's row.
//!
//! Every row list is a function of the state, because which component tools a
//! selection offers changes as the selection moves: [`rows`] is the one order
//! the panel draws in and its length is what the geometry is sized from.

// External crates
use egui::{Pos2, Rect, Vec2};

// Current crate
use crate::editor_state::{ComponentToolRow, EditorCommand};
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

/// Returns the panel's size in points for `row_count` rows: the handle, the
/// gap below it, one row per item with a gap between rows, and the bottom
/// inset.
pub fn panel_size(row_count: usize) -> Vec2 {
    let row_count = row_count as f32;
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
pub fn panel_rect(origin: Pos2, row_count: usize) -> Rect {
    Rect::from_min_size(origin, panel_size(row_count))
}

/// Returns the drag handle's rectangle: the panel's top strip. It is the one
/// part of the panel where a press starts a drag instead of a click.
pub fn handle_rect(origin: Pos2) -> Rect {
    Rect::from_min_size(origin, Vec2::new(TOOLBOX_WIDTH, TOOLBOX_HANDLE_HEIGHT))
}

/// Returns one row's rectangle inside the panel, by its place in [`rows`].
pub fn item_rect(row_index: usize, origin: Pos2) -> Rect {
    Rect::from_min_size(
        Pos2::new(
            origin.x + TOOLBOX_PADDING,
            origin.y
                + TOOLBOX_HANDLE_HEIGHT
                + TOOLBOX_ITEM_GAP
                + row_index as f32 * (TOOLBOX_ITEM_HEIGHT + TOOLBOX_ITEM_GAP),
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
/// `viewport`, one margin clear of its edges, for a panel of `row_count` rows.
///
/// A viewport smaller than the panel cannot hold it. The clamp then pins the
/// panel's top-left corner at its starting position, so the right or bottom
/// part may stick out; the handle and the first rows stay reachable.
pub fn clamped_origin(requested_origin: Pos2, viewport: Rect, row_count: usize) -> Pos2 {
    let panel_dimensions = panel_size(row_count);
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
/// in, the two switches and the component tools the selection is offered. A
/// row reads this, and the command a click queues changes it for the next
/// frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ToolboxState<'a> {
    /// The gizmo's mode, space and snapping.
    pub gizmo: GizmoSettings,
    /// Whether the scene grid is on.
    pub grid: bool,
    /// Whether the engine's component gizmos draw.
    pub gizmos: bool,
    /// The name of the tool frame the gizmo is in, when the active tool offers
    /// one and the space row has reached it.
    pub frame: Option<&'a str>,
    /// The component tools the selection's entity is offered, in the order
    /// their rows stack. Empty means no row for any of them.
    pub component_tools: &'a [ComponentToolRow],
}

impl ToolboxState<'_> {
    /// Whether an offered component tool is the active one.
    ///
    /// Exactly one tool is active at a time, so a component tool's row and the
    /// three built-in tool rows never both look active.
    pub fn component_tool_active(&self) -> bool {
        self.component_tools.iter().any(|row| row.active)
    }
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
    /// One component tool's row, by its place in
    /// [`ToolboxState::component_tools`].
    ComponentTool(usize),
    /// The space toggle's row: switches the gizmo between world and local
    /// space. The label names the space the gizmo is in.
    Space,
    /// The snapping toggle's row.
    Snap,
    /// The scene grid's switch row.
    Grid,
    /// The component gizmos' switch row.
    Gizmos,
}

/// Every row an editor always draws, in the order [`rows`] keeps them: the
/// three tools first, then the four toggles.
pub const TOOLBOX_ITEMS: [ToolboxItem; 7] = [
    ToolboxItem::Translate,
    ToolboxItem::Rotate,
    ToolboxItem::Scale,
    ToolboxItem::Space,
    ToolboxItem::Snap,
    ToolboxItem::Grid,
    ToolboxItem::Gizmos,
];

/// The three tools in the order the viewport menu and the Preferences tab
/// list them. Both surfaces draw their rows from this list, so a new tool is
/// one row here.
pub const TOOL_ROWS: [ToolboxItem; 3] = [
    ToolboxItem::Translate,
    ToolboxItem::Rotate,
    ToolboxItem::Scale,
];

/// The four rows below the tools: the space toggle, the snapping toggle, the
/// scene grid's switch and the component gizmos' switch.
pub const TOGGLE_ROWS: [ToolboxItem; 4] = [
    ToolboxItem::Space,
    ToolboxItem::Snap,
    ToolboxItem::Grid,
    ToolboxItem::Gizmos,
];

/// Every row the panel draws for one state, in stacking order: the three
/// built-in tools, one row per component tool the selection offers, then the
/// four toggles.
///
/// The panel's height, the drag limit and every row's rectangle are sized from
/// this list's length, so a row appearing or disappearing with the selection
/// grows and shrinks the whole panel.
pub fn rows(state: &ToolboxState) -> Vec<ToolboxItem> {
    let mut rows =
        Vec::with_capacity(TOOL_ROWS.len() + state.component_tools.len() + TOGGLE_ROWS.len());
    rows.extend(TOOL_ROWS);
    rows.extend((0..state.component_tools.len()).map(ToolboxItem::ComponentTool));
    rows.extend(TOGGLE_ROWS);
    rows
}

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
    /// A component tool's row is named by the tool itself.
    pub fn name<'a>(self, state: &ToolboxState<'a>) -> &'a str {
        match self {
            ToolboxItem::Translate => "translate",
            ToolboxItem::Rotate => "rotate",
            ToolboxItem::Scale => "scale",
            ToolboxItem::ComponentTool(index) => state
                .component_tools
                .get(index)
                .map_or("component tool", |row| row.name.as_str()),
            ToolboxItem::Space => "space",
            ToolboxItem::Snap => "snap",
            ToolboxItem::Grid => "grid",
            ToolboxItem::Gizmos => "gizmos",
        }
    }

    /// Returns the row's icon: the tool's own mark, a globe or a map pin for
    /// the space the gizmo is in, a magnet or a grid for the two switches, and
    /// the component icon the Inspector uses for a structural component on a
    /// component tool's row.
    pub fn icon(self, state: &ToolboxState) -> Icon {
        match self {
            ToolboxItem::Translate => Icon::ArrowsOutCardinal,
            ToolboxItem::Rotate => Icon::ArrowsClockwise,
            ToolboxItem::Scale => Icon::ArrowsOut,
            ToolboxItem::ComponentTool(_) => Icon::CubeTransparent,
            ToolboxItem::Space => match state.gizmo.space {
                GizmoSpace::World if state.frame.is_none() => Icon::GlobeHemisphereWest,
                // A tool frame is the object's own space, so it takes the map
                // pin the local space uses.
                GizmoSpace::World | GizmoSpace::Local => Icon::MapPin,
            },
            ToolboxItem::Snap => Icon::Magnet,
            ToolboxItem::Grid => Icon::GridFour,
            ToolboxItem::Gizmos => Icon::Cube,
        }
    }

    /// Returns the label the row shows for the current state: a fixed word for
    /// the built-in tools and the toggles, the space the gizmo is in for the
    /// space row - the tool frame's own name when the row has reached one - and
    /// the tool's own label for a component tool.
    pub fn label<'a>(self, state: &ToolboxState<'a>) -> &'a str {
        match self {
            ToolboxItem::Translate => "Translate",
            ToolboxItem::Rotate => "Rotate",
            ToolboxItem::Scale => "Scale",
            ToolboxItem::ComponentTool(index) => state
                .component_tools
                .get(index)
                .map_or("Tool", |row| row.label.as_str()),
            ToolboxItem::Space => match (state.frame, state.gizmo.space) {
                (Some(frame), _) => frame,
                (None, GizmoSpace::World) => "World",
                (None, GizmoSpace::Local) => "Local",
            },
            ToolboxItem::Snap => "Snap",
            ToolboxItem::Grid => "Grid",
            ToolboxItem::Gizmos => "Gizmos",
        }
    }

    /// Returns whether the row looks active: the selected tool, and each
    /// toggle in its non-default state (local space or a tool frame, snapping
    /// on, grid on).
    ///
    /// Exactly one tool is active at a time, so while an offered component
    /// tool is the active one none of the three built-in tool rows looks
    /// active.
    pub fn active(self, state: &ToolboxState) -> bool {
        match self {
            ToolboxItem::Translate => {
                !state.component_tool_active() && state.gizmo.mode == GizmoMode::Translate
            }
            ToolboxItem::Rotate => {
                !state.component_tool_active() && state.gizmo.mode == GizmoMode::Rotate
            }
            ToolboxItem::Scale => {
                !state.component_tool_active() && state.gizmo.mode == GizmoMode::Scale
            }
            ToolboxItem::ComponentTool(index) => state
                .component_tools
                .get(index)
                .is_some_and(|row| row.active),
            ToolboxItem::Space => state.gizmo.space == GizmoSpace::Local || state.frame.is_some(),
            ToolboxItem::Snap => state.gizmo.snap,
            ToolboxItem::Grid => state.grid,
            ToolboxItem::Gizmos => state.gizmos,
        }
    }

    /// Returns the label the scene viewport's menu shows: "Snapping" where the
    /// panel says "Snap", and the panel's word for the other rows. The menu
    /// lists the two spaces as rows of their own, so the space toggle only
    /// needs its own name here. It draws [`TOOL_ROWS`] and not a component
    /// tool's row, so only the fixed rows reach it.
    pub fn menu_label<'a>(self, state: &ToolboxState<'a>) -> &'a str {
        match self {
            ToolboxItem::Translate => "Translate",
            ToolboxItem::Rotate => "Rotate",
            ToolboxItem::Scale => "Scale",
            ToolboxItem::ComponentTool(_) => self.label(state),
            ToolboxItem::Space => "Space",
            ToolboxItem::Snap => "Snapping",
            ToolboxItem::Grid => "Grid",
            ToolboxItem::Gizmos => "Gizmos",
        }
    }

    /// Returns the command a click queues: a built-in tool row selects that
    /// tool, a component tool's row activates or clears it, the space row
    /// walks to the next space, and the other rows flip their switch.
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
            // A row whose tool is no longer offered clears the active tool,
            // which is the state `tool none` produces; the panel never draws
            // one, because its rows come from the same list.
            ToolboxItem::ComponentTool(index) => EditorCommand::SetTool {
                tool: state.component_tools.get(index).map(|row| row.name.clone()),
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
            ToolboxItem::Gizmos => EditorCommand::SetGizmos {
                enabled: !state.gizmos,
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

    /// One component tool the selection offers.
    fn component_row(name: &str, label: &str, active: bool) -> ComponentToolRow {
        ComponentToolRow {
            name: name.to_string(),
            label: label.to_string(),
            active,
        }
    }

    #[test]
    fn the_rows_stack_inside_the_panel() {
        let state = ToolboxState::default();
        let drawn = rows(&state);
        assert_eq!(drawn, TOOLBOX_ITEMS.to_vec());
        let origin = default_origin(viewport());
        let panel = panel_rect(origin, drawn.len());
        assert!(panel.contains_rect(handle_rect(origin)));
        let mut previous_bottom = handle_rect(origin).bottom();
        for (index, item) in drawn.iter().enumerate() {
            let rect = item_rect(index, origin);
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
        let last = item_rect(drawn.len() - 1, origin);
        assert_eq!(panel.bottom(), last.bottom() + TOOLBOX_PADDING);
    }

    /// An offered component tool adds one row between the built-in tools and
    /// the toggles, and the panel grows by exactly that row.
    #[test]
    fn a_component_tool_adds_one_row_between_the_tools_and_the_toggles() {
        let offered = [component_row("spline", "Spline", false)];
        let state = ToolboxState {
            component_tools: &offered,
            ..ToolboxState::default()
        };
        let drawn = rows(&state);
        assert_eq!(drawn.len(), TOOLBOX_ITEMS.len() + 1);
        assert_eq!(drawn[2], ToolboxItem::Scale);
        assert_eq!(drawn[3], ToolboxItem::ComponentTool(0));
        assert_eq!(drawn[4], ToolboxItem::Space);
        let taller = panel_size(drawn.len());
        let base = panel_size(TOOLBOX_ITEMS.len());
        assert_eq!(
            taller.y - base.y,
            TOOLBOX_ITEM_HEIGHT + TOOLBOX_ITEM_GAP,
            "one row taller, with its gap"
        );
        // The row below it moved down by the same amount the panel grew.
        let origin = default_origin(viewport());
        assert_eq!(
            item_rect(4, origin).top() - item_rect(3, origin).top(),
            item_rect(3, origin).top() - item_rect(2, origin).top()
        );
    }

    /// A component tool's row reads its label, its active flag and its
    /// activation command from the offered row.
    #[test]
    fn a_component_tool_row_reads_the_offered_tool() {
        let offered = [
            component_row("spline", "Spline", false),
            component_row("collider", "Collider", true),
        ];
        let state = ToolboxState {
            component_tools: &offered,
            ..ToolboxState::default()
        };
        let spline = ToolboxItem::ComponentTool(0);
        let collider = ToolboxItem::ComponentTool(1);
        assert_eq!(spline.name(&state), "spline");
        assert_eq!(collider.label(&state), "Collider");
        assert!(!spline.active(&state));
        assert!(collider.active(&state));
        assert_eq!(
            spline.command(&state),
            EditorCommand::SetTool {
                tool: Some("spline".to_string()),
            }
        );
        assert_eq!(spline.icon(&state), Icon::CubeTransparent);
        // Exactly one tool is active, so the built-in rows give way to it.
        assert!(state.component_tool_active());
        assert!(!ToolboxItem::Translate.active(&state));
        assert!(!ToolboxItem::Rotate.active(&state));
        assert!(!ToolboxItem::Scale.active(&state));
    }

    /// A row whose index names no offered tool answers safely: the callers
    /// only ever hand it a drawn row, and the degenerate answers keep the
    /// functions total.
    #[test]
    fn a_stale_component_row_answers_with_the_neutral_values() {
        let state = ToolboxState::default();
        let stale = ToolboxItem::ComponentTool(3);
        assert_eq!(stale.name(&state), "component tool");
        assert_eq!(stale.label(&state), "Tool");
        assert!(!stale.active(&state));
        assert_eq!(stale.command(&state), EditorCommand::SetTool { tool: None });
    }

    #[test]
    fn the_default_origin_sits_at_the_viewports_corner() {
        assert_eq!(default_origin(viewport()), Pos2::new(110.0, 60.0));
    }

    #[test]
    fn a_drag_clamps_inside_the_viewport() {
        let viewport_rect = viewport();
        let row_count = TOOLBOX_ITEMS.len();
        let panel_dimensions = panel_size(row_count);
        let far_origin = clamped_origin(Pos2::new(5000.0, 5000.0), viewport_rect, row_count);
        assert_eq!(
            far_origin,
            Pos2::new(
                viewport_rect.right() - panel_dimensions.x - TOOLBOX_MARGIN,
                viewport_rect.bottom() - panel_dimensions.y - TOOLBOX_MARGIN
            )
        );
        let negative_origin = clamped_origin(Pos2::new(-200.0, -200.0), viewport_rect, row_count);
        assert_eq!(negative_origin, default_origin(viewport_rect));
        // A point already inside the bounds is left alone.
        let inside = Pos2::new(300.0, 200.0);
        assert_eq!(clamped_origin(inside, viewport_rect, row_count), inside);
    }

    #[test]
    fn a_tiny_viewport_keeps_the_top_left_corner() {
        let tiny = Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(40.0, 40.0));
        let clamped = clamped_origin(Pos2::new(500.0, 500.0), tiny, TOOLBOX_ITEMS.len());
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
            gizmos: true,
            frame: Some("Fixture"),
            component_tools: &[],
        };
        assert_eq!(ToolboxItem::Space.label(&framed), "Fixture");
        assert_eq!(ToolboxItem::Snap.label(&state), "Snap");
        assert_eq!(ToolboxItem::Grid.label(&state), "Grid");
        assert_eq!(ToolboxItem::Gizmos.label(&state), "Gizmos");
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
        assert_eq!(ToolboxItem::Gizmos.icon(&state), Icon::Cube);
        // A tool frame wears the map pin too: it is the object's own space.
        let framed = ToolboxState {
            gizmo: GizmoSettings::default(),
            grid: false,
            gizmos: true,
            frame: Some("Fixture"),
            component_tools: &[],
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
            gizmos: true,
            frame: None,
            component_tools: &[],
        };
        assert!(ToolboxItem::Scale.active(&state));
        assert!(ToolboxItem::Space.active(&state));
        assert!(ToolboxItem::Snap.active(&state));
        assert!(ToolboxItem::Grid.active(&state));
        assert!(ToolboxItem::Gizmos.active(&state));
        assert!(!ToolboxItem::Translate.active(&state));
        // A tool frame reads as active even from the world space.
        let framed = ToolboxState {
            gizmo: GizmoSettings {
                mode: GizmoMode::Scale,
                ..GizmoSettings::default()
            },
            grid: true,
            gizmos: false,
            frame: Some("Fixture"),
            component_tools: &[],
        };
        assert!(ToolboxItem::Space.active(&framed));
        assert!(!ToolboxItem::Gizmos.active(&framed));
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
        assert_eq!(
            ToolboxItem::Gizmos.command(&state),
            EditorCommand::SetGizmos { enabled: true }
        );
        // Each toggle queues the opposite of the state it shows.
        let flipped = ToolboxState {
            gizmo: GizmoSettings {
                space: GizmoSpace::Local,
                snap: true,
                ..state.gizmo
            },
            grid: true,
            gizmos: true,
            frame: None,
            component_tools: &[],
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
        assert_eq!(
            ToolboxItem::Gizmos.command(&flipped),
            EditorCommand::SetGizmos { enabled: false }
        );
    }

    /// The viewport menu and the Preferences tab draw the panel's own rows, in
    /// the panel's order, and the spaces they list are the ones the panel's
    /// space toggle names. One list per thing serves all three surfaces, and
    /// this test keeps them from drifting apart.
    #[test]
    fn the_menu_rows_are_the_panels_rows() {
        assert_eq!(TOOL_ROWS.as_slice(), &TOOLBOX_ITEMS[..3]);
        assert_eq!(TOGGLE_ROWS.as_slice(), &TOOLBOX_ITEMS[3..]);
        for item in TOOLBOX_ITEMS {
            assert!(
                rows(&ToolboxState::default()).contains(&item),
                "{item:?} is not a panel row"
            );
            assert!(
                !item.menu_label(&ToolboxState::default()).is_empty(),
                "{item:?} has no menu label"
            );
        }
        for row in SPACE_ROWS {
            let state = ToolboxState {
                gizmo: GizmoSettings {
                    space: row.space,
                    ..GizmoSettings::default()
                },
                grid: false,
                gizmos: true,
                frame: None,
                component_tools: &[],
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
