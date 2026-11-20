//! The editor's tabs and the default dock layout.
//!
//! # Responsibilities
//!
//! - [`EditorTab`]: the editor's built-in tabs - the scene views' and the
//!   game viewport's placeholders, entities, systems, inspector, assets,
//!   console, problems, statistics and preferences - with their titles and
//!   wire names.
//! - [`EditorDock`]: the `egui_dock` state with the default layout (the
//!   scene and game views in the center, the entity column on the left, the
//!   inspector above the system column on the right, the assets row below),
//!   the schema version its documents carry, and
//!   [`EditorDock::settled`], which replaces the placeholder rectangles of a
//!   layout that has not been drawn yet with finite ones.
//! - [`activate_tab`]: bringing a tab to the front of its group, for the
//!   Window menu and the scripted `tab` command.
//!
//! # Design
//!
//! The dock state is the shell's long-lived state, so it lives in the shared
//! core crate rather than the reloadable shell module. A reload of the shell
//! then finds the layout exactly where the user left it, and persistence
//! carries it across restarts.

// External crates
use egui::Rect;
use egui_dock::{DockState, Node, NodeIndex};
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// The fraction of the window's width one side column takes in the default
/// layout.
pub const SIDE_COLUMN_FRACTION: f32 = 0.22;

/// The fraction of the window's height the bottom row takes in the default
/// layout.
pub const BOTTOM_ROW_FRACTION: f32 = 0.3;

/// The fraction of the right column's height the Inspector takes in the
/// default layout. The remaining height holds the Systems and Preferences
/// tabs.
pub const INSPECTOR_COLUMN_FRACTION: f32 = 0.4;

// =============================================================================
// Types
// =============================================================================

/// One tab of the egui editor.
///
/// The enum is serialized into the saved layout, so adding a tab is a
/// schema change (`LAYOUT_SCHEMA_VERSION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum EditorTab {
    /// A scene view's placeholder: the shell measures this tab's body and
    /// draws nothing, so that view's child window shows through. The payload
    /// is the view's index, which is what pairs the tab with a child window,
    /// a renderer surface and a camera.
    Scene(u8),
    /// The game viewport's placeholder: like the scene tabs, but its child
    /// window carries the primary surface, so it shows the project through
    /// the project's own camera.
    Game,
    /// The world's entities: id, name, click to select.
    Entities,
    /// The world's systems: name, stage, enabled.
    Systems,
    /// The selected entity's detail: its components and their field values.
    Inspector,
    /// The project's assets: the folder tree, the tile grid and the toolbar.
    Assets,
    /// The engine's and the project's recent log lines.
    Console,
    /// Build errors and warnings, grouped by module.
    Problems,
    /// Frame, pass and per-tab timings.
    Statistics,
    /// The preferences file's editor-wide rows and the current project's
    /// remembered gizmo choices.
    Preferences,
    /// A panel a module registered: the payload is the panel's slot in the
    /// registry, which stays the same across that module's reload. The tab's
    /// title comes from the registry, which adds a note once the panel's
    /// module has unloaded.
    Module(u16),
}

impl EditorTab {
    /// Every built-in tab, in the order the Window menu lists them.
    pub const ALL: [EditorTab; 11] = [
        EditorTab::Scene(0),
        EditorTab::Scene(1),
        EditorTab::Game,
        EditorTab::Entities,
        EditorTab::Systems,
        EditorTab::Inspector,
        EditorTab::Assets,
        EditorTab::Console,
        EditorTab::Problems,
        EditorTab::Statistics,
        EditorTab::Preferences,
    ];

    /// The view index, when this is a scene view's tab.
    pub fn scene_view(self) -> Option<u8> {
        match self {
            EditorTab::Scene(index) => Some(index),
            _ => None,
        }
    }

    /// The tab's display title.
    ///
    /// A title, not a `&'static str`: the scene views are numbered, so the
    /// second and later ones build their own name.
    pub fn title(self) -> String {
        match self {
            EditorTab::Scene(0) => "Scene".to_string(),
            EditorTab::Scene(index) => format!("Scene {}", index as u16 + 1),
            EditorTab::Game => "Game".to_string(),
            EditorTab::Entities => "Entities".to_string(),
            EditorTab::Systems => "Systems".to_string(),
            EditorTab::Inspector => "Inspector".to_string(),
            EditorTab::Assets => "Assets".to_string(),
            EditorTab::Console => "Console".to_string(),
            EditorTab::Problems => "Problems".to_string(),
            EditorTab::Statistics => "Statistics".to_string(),
            EditorTab::Preferences => "Preferences".to_string(),
            // A module panel's title belongs to the registry, not here: the
            // shell asks the registry, because the title changes when the
            // module unloads.
            EditorTab::Module(_) => "Module".to_string(),
        }
    }

    /// The wire name the logs and the scripted channel use: `scene`, then
    /// `scene2` for the second view and on.
    pub fn name(self) -> String {
        match self {
            EditorTab::Scene(0) => "scene".to_string(),
            EditorTab::Scene(index) => format!("scene{}", index as u16 + 1),
            EditorTab::Game => "game".to_string(),
            EditorTab::Entities => "entities".to_string(),
            EditorTab::Systems => "systems".to_string(),
            EditorTab::Inspector => "inspector".to_string(),
            EditorTab::Assets => "assets".to_string(),
            EditorTab::Console => "console".to_string(),
            EditorTab::Problems => "problems".to_string(),
            EditorTab::Statistics => "statistics".to_string(),
            EditorTab::Preferences => "preferences".to_string(),
            EditorTab::Module(_) => "module".to_string(),
        }
    }

    /// The panel slot, when this is a module panel's tab.
    pub fn module_slot(self) -> Option<u16> {
        match self {
            EditorTab::Module(slot) => Some(slot),
            _ => None,
        }
    }

    /// The tab a wire name names: the inverse of [`EditorTab::name`].
    ///
    /// The scripted channel and the frontend's popout windows share this
    /// parser, so a name that parses in one place parses in the other. A
    /// module panel's tab carries its slot (`module:3`), because the bare
    /// word cannot identify one. A scene view is `scene`, `scene2`,
    /// `scene3`: the printed number is one more than the view's index,
    /// because a person counts views from one. Returns `None` for a name no
    /// tab answers to.
    pub fn from_wire(name: &str) -> Option<EditorTab> {
        if let Some(slot_text) = name.strip_prefix("module:") {
            return slot_text.parse().ok().map(EditorTab::Module);
        }
        if let Some(view_number_text) = name.strip_prefix("scene") {
            if view_number_text.is_empty() {
                return Some(EditorTab::Scene(0));
            }
            return view_number_text
                .parse::<u16>()
                .ok()
                .and_then(|view_number| view_number.checked_sub(1))
                .and_then(|index| u8::try_from(index).ok())
                .map(EditorTab::Scene);
        }
        EditorTab::ALL.into_iter().find(|tab| tab.name() == name)
    }
}

impl<'de> Deserialize<'de> for EditorTab {
    /// Reads a tab from the saved layout: either the form this build writes
    /// (`{Scene: 2}`, `Game`) or a bare name an older document holds
    /// (`Scene`, `SecondScene`). A bare name reads as a tab without an
    /// index, so an older document keeps its layout.
    ///
    /// The value is read through the shared value tree, not through this
    /// format's own type, so the same rule serves the YAML file and the tests
    /// that drive the impl directly.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let serialized_tab = serde_json::Value::deserialize(deserializer)?;
        if let Some(name) = serialized_tab.as_str() {
            return match name {
                // The two scene viewports an older document names without an
                // index.
                "Scene" => Ok(EditorTab::Scene(0)),
                "SecondScene" => Ok(EditorTab::Scene(1)),
                "Game" => Ok(EditorTab::Game),
                "Entities" => Ok(EditorTab::Entities),
                "Systems" => Ok(EditorTab::Systems),
                "Inspector" => Ok(EditorTab::Inspector),
                "Assets" => Ok(EditorTab::Assets),
                "Console" => Ok(EditorTab::Console),
                "Problems" => Ok(EditorTab::Problems),
                "Statistics" => Ok(EditorTab::Statistics),
                "Preferences" => Ok(EditorTab::Preferences),
                other => Err(D::Error::custom(format!("unknown editor tab {other}"))),
            };
        }
        let Some((name, index)) = serialized_tab
            .as_object()
            .and_then(|object| object.iter().next())
            .and_then(|(name, index)| Some((name.as_str(), index.as_u64()?)))
        else {
            return Err(D::Error::custom(
                "an editor tab is a name, or a name with an index",
            ));
        };
        let index = u16::try_from(index)
            .map_err(|_| D::Error::custom(format!("editor tab {name} is out of range")))?;
        match name {
            "Scene" => Ok(EditorTab::Scene(index as u8)),
            "Module" => Ok(EditorTab::Module(index)),
            other => Err(D::Error::custom(format!(
                "editor tab {other} takes no index"
            ))),
        }
    }
}

/// The editor's dock: an `egui_dock` state with the editor's default
/// layout.
///
/// The shell keeps one of these in its session resource, so a reload of the
/// shell module finds the dock exactly where the user left it. Persistence
/// carries it across restarts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EditorDock {
    /// The schema this document was written with.
    schema_version: u32,
    /// The panel id that each module tab's slot meant when the document was
    /// written. A slot number means nothing to the next run, but the id does,
    /// so a panel that registers again finds its tab.
    #[serde(default)]
    panel_ids: Vec<String>,
    /// The tabs popped out into their own OS windows. A popped-out tab is
    /// *not* in the dock tree; redocking puts it back in the focused leaf.
    #[serde(default)]
    popouts: Vec<EditorTab>,
    /// The dock's tabs and splits.
    state: DockState<EditorTab>,
}

impl EditorDock {
    /// A dock with the default layout: the game and scene viewports tabbed
    /// together in the center, the entity list as the left column, the
    /// inspector above the system list on the right, and the assets, console,
    /// problems and statistics tabs as a bottom row across the window.
    pub fn new() -> Self {
        let mut state = DockState::new(vec![
            EditorTab::Scene(0),
            EditorTab::Scene(1),
            EditorTab::Game,
        ]);
        let tree = state.main_surface_mut();
        // Step 1: split the bottom row off the root first, because it spans
        // the full window width. The node that was there keeps the space
        // above it.
        let [upper, _assets] = tree.split_below(
            NodeIndex::root(),
            1.0 - BOTTOM_ROW_FRACTION,
            vec![
                EditorTab::Assets,
                EditorTab::Console,
                EditorTab::Problems,
                EditorTab::Statistics,
            ],
        );
        // Step 2: split the entity column off the upper node. For
        // `split_below` and `split_right` the fraction is the share the node
        // already there keeps; `split_left` instead gives the fraction-sized
        // slot on the left to the new node, so this fraction is the column's
        // own share.
        let [middle, _entities] =
            tree.split_left(upper, SIDE_COLUMN_FRACTION, vec![EditorTab::Entities]);
        // Step 3: split the right column off the middle node at the share of
        // what is left that makes it as wide as the entity column.
        let center_fraction = 1.0 - SIDE_COLUMN_FRACTION / (1.0 - SIDE_COLUMN_FRACTION);
        let [_center, right_column] =
            tree.split_right(middle, center_fraction, vec![EditorTab::Inspector]);
        // Step 4: stack the inspector above the systems list in the right
        // column. `split_below`'s fraction goes to the node already there,
        // so the inspector keeps it, and the systems list - with the
        // preferences form tabbed beside it - takes what is left.
        let [_inspector, _systems] = tree.split_below(
            right_column,
            INSPECTOR_COLUMN_FRACTION,
            vec![EditorTab::Systems, EditorTab::Preferences],
        );
        Self {
            schema_version: super::LAYOUT_SCHEMA_VERSION,
            panel_ids: Vec::new(),
            popouts: Vec::new(),
            state,
        }
    }

    /// The dock state, for reading. `egui_dock` edits the tree while it
    /// draws, so the shell goes through [`EditorDock::state_mut`] instead and
    /// this handle is for queries.
    pub fn state(&self) -> &DockState<EditorTab> {
        &self.state
    }

    /// The dock state, mutably: the shell runs the tabs through this, and
    /// `egui_dock` applies drags and closes to it directly.
    pub fn state_mut(&mut self) -> &mut DockState<EditorTab> {
        &mut self.state
    }

    /// The schema version the document carries.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Stamp the document with `version`: what a migration does before it is
    /// written back.
    pub fn set_schema_version(&mut self, version: u32) {
        self.schema_version = version;
    }

    /// Whether `tab` is docked somewhere: the main surface or a window.
    pub fn contains(&self, tab: EditorTab) -> bool {
        // `DockState::find_tab` reports the surface and node holding the tab.
        self.state.find_tab(&tab).is_some()
    }

    /// The tab a new scene view takes: one past the highest view index the
    /// dock holds today.
    ///
    /// Highest plus one, rather than the number of views, so a view the user
    /// closed in the middle never hands its index to a second tab. The
    /// frontend keeps one window, surface and camera per view index for the
    /// whole session, so a new tab on a reused index would take over the
    /// closed view's window and camera.
    pub fn next_scene_view(&self) -> EditorTab {
        let highest_view_index = self
            .state
            .iter_all_tabs()
            .filter_map(|(_path, tab)| tab.scene_view())
            .chain(self.popouts.iter().filter_map(|tab| tab.scene_view()))
            .max();
        EditorTab::Scene(highest_view_index.map_or(0, |index| index.saturating_add(1)))
    }

    /// How many scene view slots the dock needs: one past its highest view
    /// index. The frontend holds one window, surface and camera per slot, so
    /// the count has to reach the highest index in use, not the number of
    /// views.
    ///
    /// A popped-out view counts too: it is still a view, with its child
    /// window replaced by a window of its own, and it reports its rectangle
    /// through that slot.
    pub fn scene_view_count(&self) -> usize {
        self.state
            .iter_all_tabs()
            .filter_map(|(_path, tab)| tab.scene_view())
            .chain(self.popouts.iter().filter_map(|tab| tab.scene_view()))
            .max()
            .map_or(0, |index| index as usize + 1)
    }

    /// Whether one more scene view fits. Each view is a child window and a
    /// renderer surface, so the frontend's limit is the dock's too.
    pub fn can_open_scene_view(&self) -> bool {
        self.scene_view_count() < crate::boundary::SCENE_VIEWS_MAX
    }

    /// Whether the document was written by this build's schema.
    pub(crate) fn is_current(&self) -> bool {
        self.schema_version == super::LAYOUT_SCHEMA_VERSION
    }

    /// Record the registry's panel ids, so the module tabs a save writes can be
    /// found again by id on the next run.
    pub fn set_panel_ids(&mut self, panel_ids: Vec<String>) {
        self.panel_ids = panel_ids;
    }

    /// The panel ids the document carried.
    pub fn panel_ids(&self) -> &[String] {
        &self.panel_ids
    }

    /// The tabs popped out into their own OS windows.
    pub fn popouts(&self) -> &[EditorTab] {
        &self.popouts
    }

    /// Whether `tab` is currently popped out.
    pub fn is_popped_out(&self, tab: EditorTab) -> bool {
        self.popouts.contains(&tab)
    }

    /// Pops `tab` out into its own window: it leaves the dock tree, so the
    /// dock stops drawing it, and joins `popouts` so the next save remembers
    /// it. Reports whether the tab was docked and moved.
    pub fn pop_out(&mut self, tab: EditorTab) -> bool {
        if self.popouts.contains(&tab) {
            return false;
        }
        // The last tab is never popped out: an empty dock has no focused leaf
        // for a redock to return to.
        if self.state.iter_all_tabs().count() <= 1 {
            return false;
        }
        let Some(path) = self.state.find_tab(&tab) else {
            return false;
        };
        if self.state.remove_tab(path).is_none() {
            return false;
        }
        self.popouts.push(tab);
        true
    }

    /// Puts a popped-out `tab` back into the focused leaf and reports whether
    /// it was popped out.
    pub fn redock(&mut self, tab: EditorTab) -> bool {
        let Some(index) = self.popouts.iter().position(|popped| *popped == tab) else {
            return false;
        };
        self.popouts.remove(index);
        self.state.push_to_focused_leaf(tab);
        true
    }

    /// Rewrite every module tab's slot from the ids the document carried to
    /// the slots the registry assigns today.
    ///
    /// A tab whose panel is not registered today points past the registry
    /// (`u16::MAX`): the tab is the user's and stays, and the shell titles it
    /// `Module panel` and draws its body as not loaded.
    pub fn remap_panel_slots(&mut self, panel_ids: &[String]) {
        let recorded_panel_ids = std::mem::take(&mut self.panel_ids);
        for (_path, tab) in self.state.iter_all_tabs_mut() {
            let Some(slot) = tab.module_slot() else {
                continue;
            };
            let resolved_slot = recorded_panel_ids
                .get(slot as usize)
                .and_then(|id| panel_ids.iter().position(|known| known == id))
                .map_or(u16::MAX, |index| index as u16);
            *tab = EditorTab::Module(resolved_slot);
        }
        self.set_panel_ids(panel_ids.to_vec());
    }

    /// Returns a copy with placeholder geometry replaced by finite values.
    ///
    /// A fresh dock's nodes start with `Rect::NOTHING` (`±INF`) until the
    /// first `DockArea::show` lays them out, and a non-finite coordinate is
    /// not a value a saved document reads back. The values are recomputed on
    /// the next show, so a zero placeholder is safe.
    pub(crate) fn settled(&self) -> Self {
        let mut dock = self.clone();
        for (_surface, node) in dock.state.iter_all_nodes_mut() {
            match node {
                Node::Leaf(leaf) => {
                    leaf.rect = finite_rect(leaf.rect);
                    leaf.viewport = finite_rect(leaf.viewport);
                }
                Node::Vertical(split) | Node::Horizontal(split) => {
                    split.rect = finite_rect(split.rect);
                }
                // An empty node has no rectangle to make finite.
                Node::Empty => {}
            }
        }
        dock
    }
}

impl Default for EditorDock {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns `rect`, or a zero rect when it still holds the placeholder
/// infinities.
fn finite_rect(rect: Rect) -> Rect {
    let finite = |coordinate: f32| coordinate.is_finite();
    if finite(rect.min.x) && finite(rect.min.y) && finite(rect.max.x) && finite(rect.max.y) {
        rect
    } else {
        Rect::ZERO
    }
}

/// Brings `tab` to the front of its group and reports whether the dock held
/// it.
///
/// The Window menu and the scripted `tab` command share this path.
/// `set_active_tab` takes the tab's index inside its leaf; focusing the node
/// needs the tab's whole path, because a node index only addresses the
/// surface whose tree holds it - a floating surface's tree is separate.
pub fn activate_tab(dock: &mut DockState<EditorTab>, tab: EditorTab) -> bool {
    let Some(path) = dock.find_tab(&tab) else {
        return false;
    };
    // A path the dock no longer holds is a no-op, not an error to report.
    if dock.set_active_tab(path).is_err() {
        return false;
    }
    dock.set_focused_node_and_surface(path.node_path());
    true
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::SurfaceIndex;

    #[test]
    fn the_default_layout_holds_every_tab() {
        let dock = EditorDock::new();
        for tab in EditorTab::ALL {
            assert!(
                dock.contains(tab),
                "{tab:?} is missing from the default layout"
            );
        }
    }

    #[test]
    fn the_default_layout_centers_scene_with_columns_and_a_bottom_row() {
        let dock = EditorDock::new();
        let state = dock.state();
        let surface = SurfaceIndex::main();
        let tabs_at = |index: NodeIndex| -> Vec<EditorTab> {
            state[surface][index]
                .get_leaf()
                .expect("the layout index points at a leaf")
                .tabs
                .clone()
        };
        // A split's fraction is the share its top or left child keeps;
        // each `split_*` call decides which node that is.
        let fraction_at = |index: NodeIndex| -> f32 {
            match &state[surface][index] {
                Node::Vertical(split) | Node::Horizontal(split) => split.fraction,
                _ => panic!("the layout index points at a split"),
            }
        };
        // The root is a vertical split: the assets row sits below everything
        // else.
        let root = NodeIndex::root();
        assert!(
            state[surface][root].is_vertical(),
            "the assets row is not a bottom split"
        );
        assert_eq!(
            tabs_at(root.right()),
            vec![
                EditorTab::Assets,
                EditorTab::Console,
                EditorTab::Problems,
                EditorTab::Statistics
            ]
        );
        // The node above splits into the entity column and the rest.
        let upper = root.left();
        assert!(
            state[surface][upper].is_horizontal(),
            "the entity column is not a side split"
        );
        assert_eq!(tabs_at(upper.left()), vec![EditorTab::Entities]);
        // The rest splits into the scene center and the system column. The
        // two scene viewports and the game view share the center group.
        let rest = upper.right();
        assert!(
            state[surface][rest].is_horizontal(),
            "the system column is not a side split"
        );
        assert_eq!(
            tabs_at(rest.left()),
            vec![EditorTab::Scene(0), EditorTab::Scene(1), EditorTab::Game]
        );
        // The right column stacks the inspector above the systems list,
        // with the preferences form tabbed beside the list.
        let column = rest.right();
        assert!(
            state[surface][column].is_vertical(),
            "the inspector is not stacked above the systems list"
        );
        assert_eq!(tabs_at(column.left()), vec![EditorTab::Inspector]);
        assert_eq!(
            tabs_at(column.right()),
            vec![EditorTab::Systems, EditorTab::Preferences]
        );
        // Each side column ends up one `SIDE_COLUMN_FRACTION` of the window's
        // width: the entity split gives its column that share directly, and
        // the right column takes a larger share of a node that is one side
        // column narrower. The bottom row is one `BOTTOM_ROW_FRACTION` of the
        // window, the inspector one `INSPECTOR_COLUMN_FRACTION` of its
        // column, and the scene keeps the rest.
        assert!((fraction_at(root) - (1.0 - BOTTOM_ROW_FRACTION)).abs() < f32::EPSILON);
        assert!((fraction_at(upper) - SIDE_COLUMN_FRACTION).abs() < f32::EPSILON);
        let scene_share = 1.0 - SIDE_COLUMN_FRACTION / (1.0 - SIDE_COLUMN_FRACTION);
        assert!((fraction_at(rest) - scene_share).abs() < f32::EPSILON);
        assert!((fraction_at(column) - INSPECTOR_COLUMN_FRACTION).abs() < f32::EPSILON);
    }

    #[test]
    fn a_layout_round_trips_through_serde() {
        let dock = EditorDock::new();
        // The save path replaces the placeholder rects first; a fresh dock is
        // the worst case and must round-trip.
        let json = serde_json::to_string(&dock.settled()).expect("serialize the dock");
        let restored: EditorDock = serde_json::from_str(&json).expect("deserialize the dock");
        for tab in EditorTab::ALL {
            assert!(restored.contains(tab));
        }
        assert_eq!(restored.schema_version(), dock.schema_version());
    }

    #[test]
    fn popping_a_tab_out_removes_it_from_the_tree_and_remembers_it() {
        let mut dock = EditorDock::new();
        assert!(dock.pop_out(EditorTab::Inspector));
        assert!(
            !dock.contains(EditorTab::Inspector),
            "the tab is still docked"
        );
        assert_eq!(dock.popouts(), [EditorTab::Inspector]);
        // Popping the same tab out twice changes nothing: it is already in
        // `popouts`, so the second call returns false straight away.
        assert!(!dock.pop_out(EditorTab::Inspector));
    }

    #[test]
    fn a_popped_out_tab_redocks_into_the_focused_leaf() {
        let mut dock = EditorDock::new();
        assert!(dock.pop_out(EditorTab::Inspector));
        assert!(dock.redock(EditorTab::Inspector));
        assert!(
            dock.contains(EditorTab::Inspector),
            "the tab did not come back"
        );
        assert!(dock.popouts().is_empty(), "the dock still lists a pop-out");
        // A tab that was never popped out has nothing to redock.
        assert!(!dock.redock(EditorTab::Console));
    }

    #[test]
    fn the_last_docked_tab_is_never_popped_out() {
        let mut dock = EditorDock::new();
        for tab in [
            EditorTab::Scene(0),
            EditorTab::Scene(1),
            EditorTab::Game,
            EditorTab::Entities,
            EditorTab::Systems,
            EditorTab::Preferences,
            EditorTab::Inspector,
            EditorTab::Console,
            EditorTab::Problems,
            EditorTab::Statistics,
        ] {
            assert!(dock.pop_out(tab), "{tab:?} could not be popped out");
        }
        assert_eq!(dock.state().iter_all_tabs().count(), 1);
        assert!(
            !dock.pop_out(EditorTab::Assets),
            "the last docked tab left the dock without a leaf to return to"
        );
    }

    #[test]
    fn a_popped_out_scene_view_keeps_its_slot() {
        let mut dock = EditorDock::new();
        // The default layout opens two views, so the third is the highest
        // slot.
        dock.state_mut().push_to_focused_leaf(EditorTab::Scene(2));
        assert_eq!(dock.scene_view_count(), 3);
        assert!(dock.pop_out(EditorTab::Scene(2)));
        // The view left the tree but its slot stays, so the frontend keeps
        // its window and the next view does not take its index.
        assert_eq!(dock.scene_view_count(), 3);
        assert_eq!(dock.next_scene_view(), EditorTab::Scene(3));
    }

    #[test]
    fn activating_a_tab_brings_it_to_its_leaf_s_front() {
        let mut dock = EditorDock::new();
        assert!(activate_tab(dock.state_mut(), EditorTab::Systems));
        let path = dock
            .state()
            .find_tab(&EditorTab::Systems)
            .expect("the systems tab is in the default dock");
        let leaf = dock.state()[path.surface][path.node]
            .get_leaf()
            .expect("the tab sits in a leaf");
        let active = leaf.tabs[leaf.active.0];
        assert_eq!(active, EditorTab::Systems);
    }

    #[test]
    fn activating_a_tab_the_dock_lacks_reports_it() {
        let mut state = DockState::new(vec![EditorTab::Scene(0)]);
        assert!(!activate_tab(&mut state, EditorTab::Systems));
        assert!(activate_tab(&mut state, EditorTab::Scene(0)));
    }

    #[test]
    fn a_new_scene_view_takes_the_next_index() {
        let dock = EditorDock::new();
        assert_eq!(dock.next_scene_view(), EditorTab::Scene(2));
        let mut dock = dock;
        dock.state_mut().push_to_focused_leaf(EditorTab::Scene(2));
        assert_eq!(dock.next_scene_view(), EditorTab::Scene(3));
        assert_eq!(dock.scene_view_count(), 3);
    }

    #[test]
    fn a_second_view_survives_as_its_own_tab_and_title() {
        let dock = EditorDock::new();
        assert!(dock.contains(EditorTab::Scene(0)));
        assert!(dock.contains(EditorTab::Scene(1)));
        assert_eq!(EditorTab::Scene(0).title(), "Scene");
        assert_eq!(EditorTab::Scene(1).title(), "Scene 2");
        assert_eq!(EditorTab::Scene(2).title(), "Scene 3");
        assert_eq!(EditorTab::Scene(0).name(), "scene");
        assert_eq!(EditorTab::Scene(1).name(), "scene2");
        assert_eq!(EditorTab::Scene(2).name(), "scene3");
    }

    #[test]
    fn an_older_document_reads_its_unit_named_tabs() {
        // The unit-name form a stored document may hold: a bare variant
        // name, read as a tab rather than dropped, so the layout survives.
        let tab: EditorTab = serde_json::from_str("\"Scene\"").expect("the legacy scene tab");
        assert_eq!(tab, EditorTab::Scene(0));
        let tab: EditorTab =
            serde_json::from_str("\"SecondScene\"").expect("the legacy second tab");
        assert_eq!(tab, EditorTab::Scene(1));
        let tab: EditorTab = serde_json::from_str("\"Systems\"").expect("a legacy unit tab");
        assert_eq!(tab, EditorTab::Systems);
        let tab: EditorTab = serde_json::from_str("{\"Scene\":3}").expect("an indexed scene tab");
        assert_eq!(tab, EditorTab::Scene(3));
        let tab: EditorTab = serde_json::from_str("{\"Module\":2}").expect("an indexed panel tab");
        assert_eq!(tab, EditorTab::Module(2));
        // An unknown name is a refusal, not a silently dropped tab.
        assert!(serde_json::from_str::<EditorTab>("\"Nonsense\"").is_err());
        assert!(serde_json::from_str::<EditorTab>("{\"Game\":1}").is_err());
    }

    #[test]
    fn every_tab_parses_back_from_the_name_it_logs() {
        // The parser and the writer are one contract: a name in a log line, a
        // scripted command or a window title has to reach its tab again.
        for tab in EditorTab::ALL {
            let name = tab.name();
            assert_eq!(EditorTab::from_wire(&name), Some(tab), "{name}");
        }
        // A module panel's tab carries its slot, because the bare word names
        // none of them.
        assert_eq!(EditorTab::from_wire("module:3"), Some(EditorTab::Module(3)));
        assert_eq!(EditorTab::from_wire("module:0"), Some(EditorTab::Module(0)));
        // Scene views count from one: `scene` is the first, `scene4` the
        // fourth.
        assert_eq!(EditorTab::from_wire("scene"), Some(EditorTab::Scene(0)));
        assert_eq!(EditorTab::from_wire("scene4"), Some(EditorTab::Scene(3)));
        // A name no tab answers to is a refusal, not a guess.
        assert_eq!(EditorTab::from_wire("module"), None);
        assert_eq!(EditorTab::from_wire("scene0"), None);
        assert_eq!(EditorTab::from_wire("nonsense"), None);
    }
}
