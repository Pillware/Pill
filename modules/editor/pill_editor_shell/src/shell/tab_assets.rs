//! The Assets tab: the folder tree, the open folder's grid, and the bars
//! around them.
//!
//! # Responsibilities
//!
//! - [`assets_tab`]: the tab's body, its count row and the panes inside it.
//! - [`assets_tree`] and [`folder_tree_rows`]: the folder rows, their carets
//!   and the drag targets.
//! - [`assets_path_bar`], [`assets_zoom_bar`] and [`assets_grid`]: the path
//!   strip, the item count with the tile size slider, and the tiles.
//!
//! # Design
//!
//! The two `use` lines below name this crate by its package path. A hot patch
//! builds a patch crate that replays this file's opening lines exactly as they
//! are, so an import written any other way would not resolve there.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The width the tree pane starts at, in points.
pub const ASSETS_TREE_WIDTH: f32 = 176.0;

/// The widths the splitter drags the tree pane through, in points.
pub const ASSETS_TREE_WIDTH_RANGE: std::ops::RangeInclusive<f32> = 96.0..=320.0;

/// The indent one tree level adds, in points.
pub const ASSETS_TREE_INDENT: f32 = 12.0;

/// The width a row's caret slot takes, in points: either the caret, or the
/// empty space that keeps a childless row's label in line with the others.
pub const ASSETS_TREE_CARET_WIDTH: f32 = 14.0;

/// The caret glyph's size, in points.
pub const ASSETS_TREE_CARET_SIZE: f32 = 10.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the Assets tab: Unity's project panel. A folder tree sits on the
/// left, and on the right the open folder's path over a grid of tiles. A
/// count row across the top says how many entries the listing holds. A tile
/// dragged onto a folder row or a folder tile moves the file into that
/// folder. A folder's menu, and the strip under the tiles, offer the
/// container menu: make an asset in that folder, or open the folder in the
/// file manager. A file's menu renames and deletes it.
#[pill_hot_fn]
pub fn assets_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    browser: &mut ProjectBrowser,
    writes: &mut AssetTileWrites<'_>,
) {
    // Step 1: move the open folder to the nearest one the listing still
    // holds, so the grid never lists a path that is gone. That happens when
    // the folder was deleted or renamed, or the project reloaded onto another
    // tree; when nothing above it survived, the tab opens the asset root.
    browser.settle(&snapshot.assets);
    // Step 2: claim the tab's whole body before anything is drawn in it. The
    // panes are laid out inside this rectangle, so whatever they leave bare -
    // the count row, the strip beside the zoom bar, the space below the last
    // tile - belongs to no widget of its own. A right-click there opens the
    // open folder's container menu, the same one the grid's spare space
    // offers, and a tile dropped there moves into the open folder. The tree,
    // the tiles and every control register after this claim, so their own
    // menus and clicks win wherever they cover it.
    let tab_body_rect = ui.available_rect_before_wrap();
    let tab_body_response = ui.interact(
        tab_body_rect,
        ui.id().with("pill_editor_shell.assets_tab"),
        egui::Sense::click_and_drag(),
    );
    if let Some(dragged) = tab_body_response.dnd_release_payload::<String>() {
        if let Some(to) = assets::drop_destination(&dragged, &browser.folder) {
            assets::queue_move(writes.pending, &dragged, &to);
        }
    }
    context_menu(
        "assets",
        &tab_body_response,
        writes.overlay_regions,
        &mut |ui| {
            asset_container_menu(
                ui,
                snapshot,
                writes.selection,
                writes.dialog,
                writes.pending,
                &browser.folder,
            );
        },
    );
    // Step 3: draw the count row, then the panes. The row carries no
    // management buttons - a folder's menu creates an asset in it or opens it
    // in the file manager, and a file's menu renames and deletes it - so all
    // that is left there is the count of entries the listing walk found.
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{} entries", snapshot.assets.len())).weak());
    });
    ui.separator();
    egui::Panel::left("pill_editor_shell.assets_tree")
        .resizable(true)
        .default_size(ASSETS_TREE_WIDTH)
        .size_range(ASSETS_TREE_WIDTH_RANGE)
        .show(ui, |ui| {
            assets_tree(
                ui,
                snapshot,
                browser,
                writes.selection,
                writes.dialog,
                writes.pending,
                writes.overlay_regions,
            );
        });
    egui::Panel::top("pill_editor_shell.assets_path").show(ui, |ui| {
        assets_path_bar(ui, browser);
    });
    egui::Panel::bottom("pill_editor_shell.assets_zoom").show(ui, |ui| {
        assets_zoom_bar(ui, &snapshot.assets, browser);
    });
    egui::CentralPanel::default().show(ui, |ui| {
        assets_grid(ui, snapshot, browser, writes);
    });
}

/// Draws the Assets tab's folder tree: the panel's two roots - the project's
/// assets and the extensions' read-only resources - then every folder below
/// them at its own indent, with the open folder's row highlighted.
///
/// A row click opens that folder in the grid and selects it, and the create
/// dialog's target folder follows that selection. The caret opens and closes
/// the row's children. A dropped tile moves its file into the row's folder,
/// except on the extensions side, which takes no drops at all. A row's menu is
/// the container's: make an asset in that folder, or open it in the file
/// manager. The spare space below the rows offers the same for the open
/// folder.
pub fn assets_tree(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    browser: &mut ProjectBrowser,
    selection: &mut EditorSelection,
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for root in browser::ROOTS {
                // Both roots sit at the panel's own level: every extension's
                // resources are listed beside the project's tree, not inside
                // it.
                let root_row = ui.selectable_label(
                    browser.folder == root.key,
                    icons::rich(icons::Icon::Folder, root.label),
                );
                if root_row.clicked() {
                    browser.open(root.key);
                }
                if let Some(dragged) = root_row.dnd_release_payload::<String>() {
                    if !root.read_only {
                        if let Some(to) = assets::drop_destination(&dragged, root.key) {
                            assets::queue_move(pending, &dragged, &to);
                        }
                    }
                }
                root_row.on_hover_text(if root.read_only {
                    "The extensions' resources: listed and inspected, never written"
                } else {
                    "The project's asset root"
                });
                folder_tree_rows(
                    ui,
                    &snapshot.assets,
                    browser,
                    selection,
                    dialog,
                    pending,
                    overlay_regions,
                    snapshot,
                    root.key,
                    1,
                );
            }
            // The spare space under the rows opens the same container menu,
            // for the folder the grid has open.
            let spare_space_rect = ui.available_rect_before_wrap();
            let spare_space_response = ui.interact(
                spare_space_rect,
                ui.id().with("pill_editor_shell.assets_tree_space"),
                egui::Sense::click(),
            );
            let open_folder = browser.folder.clone();
            context_menu(
                "assets tree",
                &spare_space_response,
                overlay_regions,
                &mut |ui| {
                    asset_container_menu(ui, snapshot, selection, dialog, pending, &open_folder);
                },
            );
        });
}

/// Draws one level of the folder tree: a row per folder directly under
/// `parent` and, for each row the tree shows expanded, that row's own
/// children under it.
#[allow(clippy::too_many_arguments)]
pub fn folder_tree_rows(
    ui: &mut egui::Ui,
    assets: &[AssetListing],
    browser: &mut ProjectBrowser,
    selection: &mut EditorSelection,
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
    overlay_regions: &mut Vec<ShellRect>,
    snapshot: &EditorSnapshot,
    parent: &str,
    depth: usize,
) {
    for folder in browser::child_folders(assets, parent) {
        let key = folder.key();
        let path = key.as_str();
        let read_only = folder.read_only;
        let expanded = browser.is_expanded(path);
        let has_children = !browser::child_folders(assets, path).is_empty();
        let mut caret_clicked = false;
        let mut row_clicked = false;
        let mut dropped_asset = None;
        ui.horizontal(|ui| {
            ui.add_space(ASSETS_TREE_INDENT * depth as f32);
            if has_children {
                let icon = if expanded {
                    icons::Icon::CaretDown
                } else {
                    icons::Icon::CaretRight
                };
                let caret = egui::RichText::new(icon.glyph().to_string())
                    .family(egui::FontFamily::Name(icons::FAMILY.into()))
                    .size(ASSETS_TREE_CARET_SIZE);
                if ui.add(egui::Button::new(caret).frame(false)).clicked() {
                    caret_clicked = true;
                }
            } else {
                ui.add_space(ASSETS_TREE_CARET_WIDTH);
            }
            let label = ui.selectable_label(
                browser.folder == path,
                icons::rich(icons::Icon::Folder, browser::file_name(path)),
            );
            if label.clicked() {
                row_clicked = true;
            }
            // The row is a container: its menu makes an asset inside that
            // folder, or opens the folder in the file manager.
            context_menu("assets tree", &label, overlay_regions, &mut |ui| {
                asset_container_menu(ui, snapshot, selection, dialog, pending, path);
            });
            // A module's resource tree takes no drops, because nothing may be
            // written into it.
            if let Some(dragged) = label.dnd_release_payload::<String>() {
                if !read_only {
                    dropped_asset = Some(dragged);
                }
            }
            let row_rect = label.rect;
            label.on_hover_text(path);
            remember_tree_row_rect(browser, path, row_rect);
        });
        if caret_clicked {
            browser.toggle_expanded(path);
        }
        if row_clicked {
            browser.open(path);
            select_asset(selection, path);
        }
        if let Some(dragged) = dropped_asset {
            if let Some(to) = assets::drop_destination(&dragged, path) {
                assets::queue_move(pending, &dragged, &to);
            }
        }
        if expanded {
            folder_tree_rows(
                ui,
                assets,
                browser,
                selection,
                dialog,
                pending,
                overlay_regions,
                snapshot,
                path,
                depth + 1,
            );
        }
    }
}

/// Draws the path strip over the grid: the asset root, then one segment per
/// part of the open folder. Every segment but the last is a link, so one
/// click moves back up to that ancestor.
pub fn assets_path_bar(ui: &mut egui::Ui, browser: &mut ProjectBrowser) {
    ui.horizontal(|ui| {
        let segments = browser::breadcrumb(&browser.folder);
        let last_segment_index = segments.len().saturating_sub(1);
        for (index, (path, label)) in segments.iter().enumerate() {
            if index > 0 {
                ui.label(egui::RichText::new(">").weak());
            }
            if index == last_segment_index {
                ui.label(egui::RichText::new(label).strong());
            } else if ui.link(label).clicked() {
                browser.open(path);
            }
        }
    });
}

/// Draws the strip under the grid: how many items the open folder lists
/// directly, and the tile size slider, which resizes every tile as it moves.
pub fn assets_zoom_bar(ui: &mut egui::Ui, assets: &[AssetListing], browser: &mut ProjectBrowser) {
    ui.horizontal(|ui| {
        let item_count = browser::children(assets, &browser.folder).len();
        ui.label(egui::RichText::new(format!("{item_count} items")).weak());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Slider::new(&mut browser.tile_size, browser::TILE_SIZE_RANGE)
                    .show_value(false),
            )
            .on_hover_text("Tile size");
        });
    });
}

/// Draws the open folder's tiles: its folders first, then its files, each an
/// icon with its own name under it. The pane below the last row takes a
/// dropped tile into the open folder, and a right-click there opens the
/// container menu of the open folder.
pub fn assets_grid(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    browser: &mut ProjectBrowser,
    writes: &mut AssetTileWrites<'_>,
) {
    let cell = egui::vec2(
        browser.tile_size + ASSETS_TILE_GAP,
        browser.tile_size + ASSETS_TILE_LABEL_HEIGHT,
    );
    let open_folder = browser.folder.clone();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // The gap lives inside each cell, so the tiles sit edge to edge.
            // egui's item spacing would add a gutter between tiles and push a
            // full row's last column past the pane's width.
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            let column_count = browser::tile_columns(ui.available_width(), cell.x);
            let child_rows = browser::children(&snapshot.assets, &open_folder);
            // Step 1: claim the pane behind the tiles. A right-click anywhere
            // the tiles do not cover - the gap after a row's last tile, the
            // space under the last row, whatever a short folder leaves -
            // opens the container menu, and a tile dropped there moves into
            // the open folder. Tiles register after this claim, so a tile sits
            // on top and its own menu wins.
            //
            // The claimed rectangle reaches past the visible pane down to the
            // last row: a folder with more rows than the panel shows keeps its
            // lower rows outside the scroll area's visible slice, and a gap in
            // one of those rows has to answer too.
            let pane_rect = ui.available_rect_before_wrap();
            let row_count = child_rows.len().div_ceil(column_count.max(1));
            let covered_rect = egui::Rect::from_min_size(
                pane_rect.min,
                egui::vec2(
                    pane_rect.width().max(column_count as f32 * cell.x),
                    pane_rect.height().max(row_count as f32 * cell.y),
                ),
            );
            let pane_response = ui.interact(
                covered_rect,
                ui.id().with("pill_editor_shell.assets_pane"),
                egui::Sense::click_and_drag(),
            );
            remember_pane_rect(browser, covered_rect);
            if let Some(dragged) = pane_response.dnd_release_payload::<String>() {
                if let Some(to) = assets::drop_destination(&dragged, &open_folder) {
                    assets::queue_move(writes.pending, &dragged, &to);
                }
            }
            context_menu(
                "assets",
                &pane_response,
                writes.overlay_regions,
                &mut |ui| {
                    asset_container_menu(
                        ui,
                        snapshot,
                        writes.selection,
                        writes.dialog,
                        writes.pending,
                        &open_folder,
                    );
                },
            );
            // Step 2: draw the tiles, or the line that says the folder is
            // empty.
            if child_rows.is_empty() {
                ui.label(egui::RichText::new("This folder is empty").weak());
            }
            for row in child_rows.chunks(column_count) {
                ui.horizontal(|ui| {
                    for entry in row {
                        asset_tile(ui, entry, cell, browser, snapshot, writes);
                    }
                });
            }
        });
}
