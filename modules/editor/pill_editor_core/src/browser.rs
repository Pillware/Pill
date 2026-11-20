//! The Assets tab's project browser: the folder the grid lists, the folders
//! the tree has open, and the listing helpers the panes use.
//!
//! # Responsibilities
//!
//! - [`ProjectBrowser`]: the browser's session state - the folder the grid
//!   lists, the folders the tree shows open, and the tile size the grid
//!   draws.
//! - The panel's roots ([`ROOTS`], [`is_root`], [`root_of`]): the project's
//!   own tree, and every extension's read-only resources beside it.
//! - The listing helpers the panes use: [`children`], [`child_folders`],
//!   [`breadcrumb`], [`parent_folder`], [`file_name`] and [`tile_columns`].
//! - [`drop_folder_relative`]: where a dropped file goes, checked so a
//!   folder outside the asset directory is refused instead of joined.
//!
//! # Design
//!
//! The snapshot lists every file and folder in one flat list of
//! [`AssetListing`] rows. Each row names the root it came from and its path
//! inside that root, so both panes build what they show from the same list:
//! the tree walks the folder rows, the grid keeps the open folder's direct
//! children, and the breadcrumb cuts the open folder's path into segments.
//! A row whose key is a root is never listed as another row's child, so the
//! extensions' rows reach the tree only through their own root row. Nothing
//! here touches egui or the disk, which lets the tree, the grid, the
//! breadcrumb and the scripted channel agree on one state.

// Standard library
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::path::PathBuf;

// External crates
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::editor_state::{AssetListing, EXTENSIONS_ROOT};

// =============================================================================
// Constants
// =============================================================================

/// The label of the project's root row. Inside the project's tree it is also
/// the breadcrumb's first segment.
pub const ROOT_LABEL: &str = "Assets";

/// The panel's roots, in the order the tree draws them: the project's own
/// assets first, then every extension's read-only resources.
pub const ROOTS: [AssetRoot; 2] = [
    AssetRoot {
        key: "",
        label: ROOT_LABEL,
        read_only: false,
    },
    AssetRoot {
        key: EXTENSIONS_ROOT,
        label: EXTENSIONS_ROOT,
        read_only: true,
    },
];

/// The tile size the grid starts at, in points.
pub const DEFAULT_TILE_SIZE: f32 = 72.0;

/// The tile sizes the grid's slider spans, in points.
pub const TILE_SIZE_RANGE: RangeInclusive<f32> = 40.0..=120.0;

// =============================================================================
// Types
// =============================================================================

/// One root of the Assets panel: the key that opening its row sets, and the
/// label the row shows.
///
/// A root sits at the panel's own level. It is not a folder inside another
/// root's tree: the tree draws both roots at the left edge, and a row whose
/// key is a root is never listed as another row's child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetRoot {
    /// The key the root's row opens in the grid. Empty for the project's own
    /// tree.
    pub key: &'static str,
    /// The label the root's row and the breadcrumb's first segment show.
    pub label: &'static str,
    /// Whether everything below the root is an extension's resource that may
    /// be listed and inspected but never written into.
    pub read_only: bool,
}

/// The Assets tab's project browser: the state both panes share.
///
/// It lives for one session, like the viewport toolbox: a restart starts the
/// browser at the project's asset root. The state sits in the shared `dylib`
/// with the rest of the session, so a shell reload keeps the folder the user
/// was looking at.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectBrowser {
    /// The folder the grid lists, as a path from the asset root: empty for the
    /// root itself.
    pub folder: String,
    /// The folders whose tree rows are open, so their children are shown.
    pub expanded: BTreeSet<String>,
    /// The tile size the grid draws, in points.
    pub tile_size: f32,
    /// Each tile's last logged rectangle, keyed by the tile's path. A
    /// rectangle is logged once per placement, so a test suite can drag a
    /// tile without reading pixels.
    pub tile_rects: Vec<(String, [f32; 4])>,
    /// Each folder tree row's last logged rectangle, keyed by the folder. A
    /// test suite right-clicks a row through it without reading pixels.
    pub tree_rects: Vec<(String, [f32; 4])>,
    /// The grid pane's last logged rectangle. A right-click on the empty space
    /// inside it opens the open folder's menu.
    pub pane_rect: Option<[f32; 4]>,
}

impl Default for ProjectBrowser {
    fn default() -> Self {
        Self {
            folder: String::new(),
            expanded: BTreeSet::new(),
            tile_size: DEFAULT_TILE_SIZE,
            tile_rects: Vec::new(),
            tree_rects: Vec::new(),
            pane_rect: None,
        }
    }
}

impl ProjectBrowser {
    /// Opens `folder` in the grid and returns whether the open folder changed.
    ///
    /// The call logs the `[editor] browser folder` line the suites watch. The
    /// folder's ancestors open in the tree too, so the tree always shows where
    /// the grid is looking.
    pub fn open(&mut self, folder: &str) -> bool {
        let folder = folder.trim_matches('/').to_string();
        self.expand_ancestors(&folder);
        if self.folder == folder {
            return false;
        }
        self.folder = folder;
        pill_core::info!(
            target: telemetry_target::EDITOR,
            path = self.folder.as_str(),
            "[editor] browser folder"
        );
        true
    }

    /// Opens a closed folder's tree row, or closes an open one, and returns
    /// whether the folder is now open.
    pub fn toggle_expanded(&mut self, folder: &str) -> bool {
        if self.expanded.contains(folder) {
            self.expanded.remove(folder);
            false
        } else {
            self.expanded.insert(folder.to_string());
            true
        }
    }

    /// Returns whether the tree shows `folder` open.
    pub fn is_expanded(&self, folder: &str) -> bool {
        self.expanded.contains(folder)
    }

    /// Opens every ancestor of `path` in the tree, so the tree shows the
    /// folder that holds it.
    pub fn expand_ancestors(&mut self, path: &str) {
        let mut ancestor = parent_folder(path);
        while !ancestor.is_empty() {
            self.expanded.insert(ancestor.to_string());
            ancestor = parent_folder(ancestor);
        }
    }

    /// Moves the open folder to the nearest one the listing still holds, when
    /// the open folder has vanished: it was deleted or renamed, or the project
    /// reloaded onto another tree. Returns whether the open folder changed.
    pub fn settle(&mut self, assets: &[AssetListing]) -> bool {
        // Step 1: check that the open folder still exists. A root always
        // does: the panel draws its row itself, so no listing row can vanish.
        if self.folder.is_empty() {
            return false;
        }
        let exists = is_root(&self.folder)
            || assets
                .iter()
                .any(|row| is_folder(row) && row.key() == self.folder);
        if exists {
            return false;
        }
        // Step 2: climb to the nearest ancestor the listing still holds.
        let mut fallback_folder = parent_folder(&self.folder).to_string();
        while !fallback_folder.is_empty() {
            if is_root(&fallback_folder)
                || assets
                    .iter()
                    .any(|row| is_folder(row) && row.key() == fallback_folder)
            {
                return self.open(&fallback_folder);
            }
            fallback_folder = parent_folder(&fallback_folder).to_string();
        }
        // Step 3: nothing above it survived, so open the asset root.
        self.open("")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns whether `key` is one of the panel's roots.
pub fn is_root(key: &str) -> bool {
    ROOTS.iter().any(|root| root.key == key)
}

/// Returns the root `folder` belongs to: the extensions root for everything
/// under [`EXTENSIONS_ROOT`], the project's tree for anything else.
pub fn root_of(folder: &str) -> AssetRoot {
    ROOTS
        .iter()
        .copied()
        .rfind(|root| {
            !root.key.is_empty()
                && (folder == root.key || folder.starts_with(&format!("{}/", root.key)))
        })
        .unwrap_or(ROOTS[0])
}

/// Returns whether a row is a folder: the one kind the tree and the grid treat
/// differently.
pub fn is_folder(row: &AssetListing) -> bool {
    row.kind.is_folder()
}

/// Returns the folder a path lives in: its parent, or empty for the asset
/// root.
pub fn parent_folder(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((parent, _)) => parent,
        None => "",
    }
}

/// Returns a path's own name: its last `/`-separated segment.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Returns whether `path` is a direct child of `folder`. The empty folder is
/// the asset root.
pub fn is_direct_child(path: &str, folder: &str) -> bool {
    parent_folder(path) == folder
}

/// Returns the direct children of `folder`: folders first, then files, each
/// group in case-insensitive name order. Both panes list rows in this order.
///
/// A root's row is never a child, because the panel draws its roots at its own
/// level. That keeps the project's tree to the project's own content.
pub fn children<'a>(assets: &'a [AssetListing], folder: &str) -> Vec<&'a AssetListing> {
    let mut rows: Vec<&AssetListing> = assets
        .iter()
        .filter(|row| {
            let key = row.key();
            !key.is_empty() && !is_root(&key) && is_direct_child(&key, folder)
        })
        .collect();
    sort_rows(&mut rows);
    rows
}

/// Returns the direct child folders of `folder`, in the order [`children`]
/// lists them.
pub fn child_folders<'a>(assets: &'a [AssetListing], folder: &str) -> Vec<&'a AssetListing> {
    let mut rows: Vec<&AssetListing> = assets
        .iter()
        .filter(|row| {
            let key = row.key();
            is_folder(row) && !is_root(&key) && is_direct_child(&key, folder)
        })
        .collect();
    sort_rows(&mut rows);
    rows
}

/// Builds the breadcrumb of `folder`: the root it belongs to, then one
/// `(path, label)` pair per segment. Each path holds the segments up to it, so
/// a click on a label can open that ancestor.
pub fn breadcrumb(folder: &str) -> Vec<(String, String)> {
    let root = root_of(folder);
    let mut segments = vec![(root.key.to_string(), root.label.to_string())];
    let mut cumulative = root.key.to_string();
    // The root's segment is already in place, so only the part after it is
    // split.
    let path_below_root = if root.key.is_empty() {
        folder
    } else {
        folder
            .strip_prefix(root.key)
            .unwrap_or(folder)
            .trim_start_matches('/')
    };
    for segment in path_below_root
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if !cumulative.is_empty() {
            cumulative.push('/');
        }
        cumulative.push_str(segment);
        segments.push((cumulative.clone(), segment.to_string()));
    }
    segments
}

/// Returns how many tiles fit across `available_width` points when each cell
/// takes `cell_width` points. The count is never zero, so a narrow pane still
/// draws its rows.
pub fn tile_columns(available_width: f32, cell_width: f32) -> usize {
    // A zero, negative or non-finite cell width gives no sensible column
    // count.
    if !cell_width.is_finite() || cell_width <= 0.0 {
        return 1;
    }
    ((available_width / cell_width).floor() as usize).max(1)
}

/// Returns the folder a drop lands in, relative to the asset root, from the
/// browser's folder label.
///
/// `res` itself is the asset root, so it becomes an empty relative path. Every
/// other folder goes through the same validator every asset command uses. A
/// folder that climbed out of the root (`res/../..`) is therefore refused
/// instead of joined onto it, so the copy cannot land outside the project.
///
/// # Errors
///
/// The validator's message when the folder is not a path inside the asset
/// directory.
pub fn drop_folder_relative(folder: &str) -> Result<PathBuf, String> {
    let relative_folder = match folder {
        "res" => "",
        other => other.strip_prefix("res/").unwrap_or(other),
    };
    if relative_folder.is_empty() {
        return Ok(PathBuf::new());
    }
    pill_engine::asset_metadata::checked_relative(relative_folder)
}

/// Sorts rows with folders first, then by name without regard to case.
///
/// The name is read from the row's key, not from its path: a module root's row
/// carries an empty path, so the key is the only place its name exists.
fn sort_rows(rows: &mut [&AssetListing]) {
    rows.sort_by_cached_key(|row| (!is_folder(row), file_name(&row.key()).to_lowercase()));
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor_state::AssetKind;

    fn row(path: &str, kind: &str) -> AssetListing {
        AssetListing {
            root: String::new(),
            path: path.to_string(),
            read_only: false,
            kind: AssetKind::parse(kind),
            standalone: kind == "Material",
            has_meta: false,
            guid: None,
        }
    }

    fn listing() -> Vec<AssetListing> {
        vec![
            row("models", "folder"),
            row("models/cube.obj", "Mesh"),
            row("models/plane.obj", "Mesh"),
            row("models/nested", "folder"),
            row("models/nested/deep.obj", "Mesh"),
            row("shaders", "folder"),
            row("shaders/code.wgsl", "file"),
            row("config.ini", "file"),
            row("textures", "folder"),
            row("textures/skin.png", "Texture"),
            row("textures/stone.material", "Material"),
        ]
    }

    /// A row of an extension's read-only resources, at `path` inside `root`.
    fn module_row(root: &str, path: &str) -> AssetListing {
        AssetListing {
            root: root.to_string(),
            path: path.to_string(),
            read_only: true,
            kind: AssetKind::Folder,
            standalone: false,
            has_meta: false,
            guid: None,
        }
    }

    /// An extension's module and one folder inside it, as the host lists them.
    fn listing_with_a_module() -> Vec<AssetListing> {
        let mut assets = listing();
        assets.push(module_row("pill_master_renderer_data", ""));
        assets.push(module_row("pill_master_renderer_data", "defaults"));
        assets
    }

    /// Only the exact label `folder` makes a row a folder. A type path that
    /// ends in `Folder`, or a near miss of the label, is a file: the tree lists
    /// it, and the toolbar may write to it.
    #[test]
    fn only_the_folder_label_makes_a_folder() {
        assert!(is_folder(&row("models", "folder")));
        assert!(!is_folder(&row("models/odd.folder", "demo::Folder")));
        assert!(!is_folder(&row("models/odd.folder", "Folder")));
        assert!(!is_folder(&row("models/odd.folder", "folders")));
        assert!(!is_folder(&row("models/odd.folder", "file")));
    }

    /// The panel has two roots, and an extension's resources sit under their
    /// own root rather than inside the project's tree.
    #[test]
    fn the_extensions_root_sits_beside_the_project_tree() {
        let assets = listing_with_a_module();

        let top: Vec<String> = children(&assets, "").iter().map(|row| row.key()).collect();
        assert_eq!(top, vec!["models", "shaders", "textures", "config.ini"]);
        assert!(top.iter().all(|key| !is_root(key)));

        let modules: Vec<String> = children(&assets, EXTENSIONS_ROOT)
            .iter()
            .map(|row| row.key())
            .collect();
        assert_eq!(modules, vec!["extensions/pill_master_renderer_data"]);

        let inside: Vec<String> = children(&assets, "extensions/pill_master_renderer_data")
            .iter()
            .map(|row| row.key())
            .collect();
        assert_eq!(
            inside,
            vec!["extensions/pill_master_renderer_data/defaults".to_string()]
        );

        // A browser opened inside a module's tree keeps the folder it lists.
        let mut browser = ProjectBrowser::default();
        assert!(browser.open("extensions/pill_master_renderer_data/defaults"));
        assert!(!browser.settle(&assets));
    }

    /// A root has no row of its own, so it never falls back. A folder whose
    /// module unloaded moves to the root that held it.
    #[test]
    fn the_extensions_root_outlives_the_rows_below_it() {
        let mut browser = ProjectBrowser::default();
        assert!(browser.open(EXTENSIONS_ROOT));
        assert!(!browser.settle(&listing()));

        assert!(browser.open("extensions/unloaded/defaults"));
        assert!(browser.settle(&listing()));
        assert_eq!(browser.folder, EXTENSIONS_ROOT);
    }

    /// The breadcrumb starts at the root the folder belongs to, and its first
    /// segment opens that root.
    #[test]
    fn the_breadcrumb_names_the_root_the_folder_belongs_to() {
        let segments = breadcrumb("extensions/pill_master_renderer_data/defaults");
        let paths: Vec<&str> = segments.iter().map(|(path, _)| path.as_str()).collect();
        let labels: Vec<&str> = segments.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "extensions",
                "extensions/pill_master_renderer_data",
                "extensions/pill_master_renderer_data/defaults",
            ]
        );
        assert_eq!(
            labels,
            vec!["extensions", "pill_master_renderer_data", "defaults"]
        );
        assert_eq!(
            breadcrumb(EXTENSIONS_ROOT),
            vec![(EXTENSIONS_ROOT.to_string(), EXTENSIONS_ROOT.to_string())]
        );
        assert_eq!(root_of("models/nested").key, "");
        assert_eq!(root_of(EXTENSIONS_ROOT).key, EXTENSIONS_ROOT);
    }

    #[test]
    fn children_keep_folders_first_then_names() {
        let assets = listing();
        let names: Vec<&str> = children(&assets, "")
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(names, vec!["models", "shaders", "textures", "config.ini"]);
        let model_names: Vec<&str> = children(&assets, "models")
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(
            model_names,
            vec!["models/nested", "models/cube.obj", "models/plane.obj"]
        );
    }

    #[test]
    fn child_folders_are_the_tree_rows() {
        let assets = listing();
        let folders: Vec<&str> = child_folders(&assets, "models")
            .iter()
            .map(|row| row.path.as_str())
            .collect();
        assert_eq!(folders, vec!["models/nested"]);
        assert_eq!(child_folders(&assets, "").len(), 3);
        assert!(child_folders(&assets, "shaders").is_empty());
    }

    #[test]
    fn breadcrumb_starts_at_the_root_and_accumulates() {
        let segments = breadcrumb("models/nested");
        let paths: Vec<&str> = segments.iter().map(|(path, _)| path.as_str()).collect();
        let labels: Vec<&str> = segments.iter().map(|(_, label)| label.as_str()).collect();
        assert_eq!(paths, vec!["", "models", "models/nested"]);
        assert_eq!(labels, vec!["Assets", "models", "nested"]);
        assert_eq!(breadcrumb("").len(), 1);
    }

    #[test]
    fn parents_and_names_come_off_the_slashes() {
        assert_eq!(parent_folder("models/cube.obj"), "models");
        assert_eq!(parent_folder("config.ini"), "");
        assert_eq!(file_name("models/nested/deep.obj"), "deep.obj");
        assert_eq!(file_name("models"), "models");
        assert!(is_direct_child("models/cube.obj", "models"));
        assert!(!is_direct_child("models/nested/deep.obj", "models"));
    }

    #[test]
    fn opening_a_folder_expands_its_ancestors() {
        let mut browser = ProjectBrowser::default();
        assert!(browser.open("models/nested"));
        assert!(browser.is_expanded("models"));
        assert!(!browser.is_expanded("models/nested"));
        assert_eq!(browser.folder, "models/nested");
        // Opening the same folder again moves nothing.
        assert!(!browser.open("/models/nested/"));
    }

    #[test]
    fn a_vanished_folder_falls_back_to_its_nearest_ancestor() {
        let mut browser = ProjectBrowser::default();
        browser.open("models/nested");
        let assets = vec![
            row("models", "folder"),
            row("models/cube.obj", "Mesh"),
            row("config.ini", "file"),
        ];
        assert!(browser.settle(&assets));
        assert_eq!(browser.folder, "models");
        // A tree that lacks the folder entirely falls back to the asset root.
        let mut browser = ProjectBrowser::default();
        browser.open("gone/deeper");
        assert!(browser.settle(&assets));
        assert_eq!(browser.folder, "");
        // A root never falls back.
        assert!(!browser.settle(&assets));
    }

    #[test]
    fn toggling_a_tree_row_flips_its_state() {
        let mut browser = ProjectBrowser::default();
        assert!(browser.toggle_expanded("models"));
        assert!(browser.is_expanded("models"));
        assert!(!browser.toggle_expanded("models"));
        assert!(!browser.is_expanded("models"));
    }

    #[test]
    fn tile_columns_fit_the_pane_and_never_reach_zero() {
        assert_eq!(tile_columns(400.0, 80.0), 5);
        assert_eq!(tile_columns(79.0, 80.0), 1);
        assert_eq!(tile_columns(0.0, 80.0), 1);
        assert_eq!(tile_columns(400.0, 0.0), 1);
    }

    #[test]
    fn a_drop_in_the_root_folder_is_the_asset_root() {
        assert_eq!(drop_folder_relative("res"), Ok(PathBuf::new()));
    }

    #[test]
    fn a_drop_in_a_named_folder_stays_under_the_root() {
        assert_eq!(
            drop_folder_relative("res/models/nested"),
            Ok(PathBuf::from("models/nested"))
        );
    }

    #[test]
    fn a_drop_folder_that_climbs_out_is_refused() {
        for folder in ["res/../..", "res/models/../../..", "../..", "/res/models"] {
            assert!(
                drop_folder_relative(folder).is_err(),
                "`{folder}` was accepted as a drop folder"
            );
        }
    }
}
