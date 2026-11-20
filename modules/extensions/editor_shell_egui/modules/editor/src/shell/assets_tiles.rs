//! The Assets tab's tiles: what a tile draws, the menus it offers and the
//! create and rename dialog.
//!
//! # Responsibilities
//!
//! - [`asset_tile`]: one grid tile - its preview or icon, its wrapped name, the
//!   highlight behind a selected or hovered tile, and the clicks, menu and
//!   drags it answers.
//! - [`asset_icon`], [`asset_type_icon`] and [`tile_name_job`]: the icon a row
//!   draws and the layout of its name.
//! - [`asset_menu_items`] and [`asset_container_menu`]: the menus a file tile
//!   and a container offer.
//! - [`asset_dialog_window`]: the create and rename dialogs.
//! - [`remember_tile_rect`], [`remember_tree_row_rect`] and
//!   [`remember_pane_rect`]: the rectangles a test suite aims a drag or a
//!   right-click at.
//! - [`AssetTileWrites`]: the handles a tile writes through.
//! - The tile metrics: the cell padding, the name band, the icon inset, the
//!   highlight's corner radius, the name's row count and the preview's badge.
//!
//! # Design
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The horizontal padding inside a tile's cell, in points.
pub const ASSETS_TILE_GAP: f32 = 8.0;

/// The band under a tile's icon that holds its name, in points.
pub const ASSETS_TILE_LABEL_HEIGHT: f32 = 26.0;

/// The gap between a tile's icon and its name, in points.
pub const ASSETS_TILE_LABEL_GAP: f32 = 2.0;

/// The inset between a tile's cell edge and its icon, in points.
pub const ASSETS_TILE_ICON_INSET: f32 = 6.0;

/// The corner radius of the highlight behind a selected or hovered tile, in
/// points.
pub const ASSETS_TILE_CORNER_RADIUS: f32 = 4.0;

/// The rows a tile's name may wrap to before it gets an ellipsis; Unity's tiles
/// use two.
pub const ASSETS_TILE_LABEL_ROWS: usize = 2;

/// The width and height of the type icon a finished preview keeps in its
/// bottom-left corner, in points.
pub const ASSETS_TILE_BADGE_SIZE: f32 = 14.0;

// =============================================================================
// Types
// =============================================================================

/// The handles a grid tile writes through: the selection a click moves, the
/// dialog a menu entry opens, the commands it queues, the regions an open menu
/// marks as the UI's, the path a drag out reports and the previews it draws.
/// One bundle keeps [`asset_tile`]'s signature short enough for clippy's
/// argument budget.
pub struct AssetTileWrites<'a> {
    /// The shared selection the tile's clicks move.
    pub selection: &'a mut EditorSelection,
    /// The create/rename dialog the tile's menu can open.
    pub dialog: &'a mut Option<AssetDialog>,
    /// The recorded file commands the tile's menu queues.
    pub pending: &'a mut PendingEditorCommands,
    /// The regions an open menu marks so a click inside is never a scene pick.
    pub overlay_regions: &'a mut Vec<ShellRect>,
    /// The asset path a drag just took out of the window: the shell writes it
    /// once and the executable starts the system's drag with it.
    pub drag_out: &'a mut Option<String>,
    /// The preview state of every tile. A tile draws a texture when one exists
    /// and its type icon otherwise, and records itself as drawn so the
    /// executable asks only for the previews that are on screen.
    pub previews: &'a mut pill_editor_core::session::PreviewStates,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Logs a tile's rectangle each time it moves, so a test suite can drag a file
/// onto a drop target (an Inspector asset row, a folder) without reading
/// pixels.
pub fn remember_tile_rect(browser: &mut ProjectBrowser, path: &str, rect: egui::Rect) {
    let corners = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
    if let Some((_, logged)) = browser
        .tile_rects
        .iter_mut()
        .find(|(logged_path, _)| logged_path == path)
    {
        if *logged == corners {
            return;
        }
        *logged = corners;
    } else {
        browser.tile_rects.push((path.to_owned(), corners));
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        path,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] assets tile rect"
    );
}

/// Logs a folder tree row's rectangle each time it moves, keyed by the folder,
/// so a test suite can right-click the row without reading pixels.
pub fn remember_tree_row_rect(browser: &mut ProjectBrowser, path: &str, rect: egui::Rect) {
    let corners = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
    if let Some((_, logged)) = browser
        .tree_rects
        .iter_mut()
        .find(|(logged_path, _)| logged_path == path)
    {
        if *logged == corners {
            return;
        }
        *logged = corners;
    } else {
        browser.tree_rects.push((path.to_owned(), corners));
    }
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        path,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] assets tree row rect"
    );
}

/// Logs the grid pane's rectangle each time it moves: the area where a
/// right-click on empty space opens the container menu.
pub fn remember_pane_rect(browser: &mut ProjectBrowser, rect: egui::Rect) {
    let corners = [rect.min.x, rect.min.y, rect.max.x, rect.max.y];
    if browser.pane_rect == Some(corners) {
        return;
    }
    browser.pane_rect = Some(corners);
    pill_core::debug!(
        target: telemetry_target::EDITOR,
        min_x = rect.min.x,
        min_y = rect.min.y,
        max_x = rect.max.x,
        max_y = rect.max.y,
        "[editor] assets pane rect"
    );
}

/// Draws one grid tile: its preview or type icon, its wrapped name, the
/// highlight behind a selected or hovered tile, and the clicks, menu and drags
/// it answers. A double click opens a folder, or a scene in the editor.
pub fn asset_tile(
    ui: &mut egui::Ui,
    row: &AssetListing,
    cell: egui::Vec2,
    browser: &mut ProjectBrowser,
    snapshot: &EditorSnapshot,
    writes: &mut AssetTileWrites<'_>,
) {
    // Step 1: claim the cell, remember its rectangle, and paint the highlight
    // a selected or hovered tile gets.
    let (rect, response) = ui.allocate_exact_size(cell, egui::Sense::click_and_drag());
    let key = row.key();
    remember_tile_rect(browser, &key, rect);
    let visuals = ui.visuals().clone();
    let selected = writes.selection.asset.as_deref() == Some(key.as_str());
    if selected || response.hovered() {
        let fill = if selected {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.hovered.weak_bg_fill
        };
        ui.painter()
            .rect_filled(rect, ASSETS_TILE_CORNER_RADIUS, fill);
    }
    // Step 2: draw the preview image, or the type icon alone, then the name
    // under it.
    let icon_side = (cell.x - ASSETS_TILE_GAP - ASSETS_TILE_ICON_INSET * 2.0).max(8.0);
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.center().x - icon_side * 0.5,
            rect.min.y + ASSETS_TILE_ICON_INSET,
        ),
        egui::vec2(icon_side, icon_side),
    );
    let tint = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    // A finished preview fills the tile's picture area. The type icon stays in
    // the preview's bottom-left corner, so a person still sees what the file
    // is; until a preview exists, the icon alone is drawn.
    let preview = writes.previews.ready(&key).cloned();
    writes.previews.push_visible(&key);
    match preview {
        Some(preview) => {
            let size = preview.size_vec2();
            let scale = (icon_rect.width() / size.x).min(icon_rect.height() / size.y);
            let drawn = egui::Rect::from_center_size(icon_rect.center(), size * scale);
            ui.painter().image(
                preview.id(),
                drawn,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let badge = egui::Rect::from_min_size(
                egui::pos2(drawn.min.x, drawn.max.y - ASSETS_TILE_BADGE_SIZE),
                egui::Vec2::splat(ASSETS_TILE_BADGE_SIZE),
            );
            icons::paint_duotone(ui.painter(), badge, asset_icon(row), tint);
        }
        None => icons::paint_duotone(ui.painter(), icon_rect, asset_icon(row), tint),
    }
    let font = egui::TextStyle::Small.resolve(ui.style());
    let job = tile_name_job(
        browser::file_name(&key),
        &font,
        visuals.text_color(),
        cell.x - ASSETS_TILE_GAP,
    );
    let label = ui.painter().layout_job(job);
    let label_origin = egui::pos2(
        rect.center().x - label.size().x * 0.5,
        icon_rect.max.y + ASSETS_TILE_LABEL_GAP,
    );
    ui.painter()
        .galley(label_origin, label, visuals.text_color());
    // Step 3: answer the clicks - select the row, open a folder, open a scene.
    if response.clicked() {
        select_asset(writes.selection, &key);
    }
    if response.double_clicked() && browser::is_folder(row) {
        browser.open(&key);
    }
    // A scene tile opens on the second click, like a document elsewhere. It
    // queues the File menu's Open Scene command, so a click and the menu share
    // one implementation.
    if response.double_clicked()
        && !browser::is_folder(row)
        && row.path.to_ascii_lowercase().ends_with(".scene")
    {
        let command = pill_editor_core::editor_state::EditorCommand::SceneOpen {
            path: row.path.clone(),
        };
        writes.pending.push(vec![command]);
    }
    // Step 4: select the tile on a right-click, then open its menu. A folder
    // tile offers the container's menu; a file tile offers rename and delete. A
    // module's file opens no menu at all: its contents may not be written, and
    // it is no folder to open.
    if response.secondary_clicked() {
        select_asset(writes.selection, &key);
    }
    if browser::is_folder(row) || !row.read_only {
        context_menu(
            "assets tile",
            &response,
            writes.overlay_regions,
            &mut |ui| {
                if browser::is_folder(row) {
                    asset_container_menu(
                        ui,
                        snapshot,
                        writes.selection,
                        writes.dialog,
                        writes.pending,
                        &key,
                    );
                } else {
                    asset_menu_items(
                        ui,
                        snapshot,
                        writes.selection,
                        writes.dialog,
                        writes.pending,
                    );
                }
            },
        );
    }
    // Step 5: carry the tile. A folder tile is a drop target, and every writable
    // tile is a drag source, so a file moves by dragging its tile onto a
    // folder. A module's folder takes no drop, because nothing may be written
    // into it.
    if browser::is_folder(row) {
        if let Some(dragged) = response.dnd_release_payload::<String>() {
            if !row.read_only {
                if let Some(to) = assets::drop_destination(&dragged, &key) {
                    assets::queue_move(writes.pending, &dragged, &to);
                }
            }
        }
    }
    // A module's file is not a drag source either: the payload would name a
    // path that no file command may change.
    if !row.read_only {
        response.dnd_set_drag_payload(key.clone());
    }
    // A file dragged past the window edge leaves the editor: the tile reports
    // its path once and the executable starts the system drag. Once the pointer
    // has left the window its release never arrives, so a drag out looks like
    // "button still down, no pointer position".
    if response.dragged() && writes.drag_out.is_none() && !browser::is_folder(row) {
        let left_window =
            ui.input(|input| input.pointer.any_down() && input.pointer.hover_pos().is_none());
        if left_window {
            *writes.drag_out = Some(row.path.clone());
            pill_core::info!(
                target: telemetry_target::EDITOR,
                path = row.path.as_str(),
                "[editor] drag out requested"
            );
        }
    }
    response.on_hover_text(key.as_str());
}

/// Returns the icon a row's tile draws: a folder icon, an asset type's icon by
/// its last type-name segment, or a plain file's icon by its extension.
pub fn asset_icon(row: &AssetListing) -> icons::Icon {
    if browser::is_folder(row) {
        return icons::Icon::Folder;
    }
    let type_icon = asset_type_icon(row.kind.text());
    if !matches!(type_icon, icons::Icon::File) {
        return type_icon;
    }
    let extension = row
        .path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "hdr" => icons::Icon::Image,
        "obj" | "gltf" | "glb" => icons::Icon::Cube,
        "hlsl" | "wgsl" | "glsl" | "frag" | "vert" | "comp" => icons::Icon::Code,
        _ => icons::Icon::File,
    }
}

/// Returns the icon for an asset type, matched by its last type-name segment. A
/// tile of that type and an Inspector asset row both draw it.
pub fn asset_type_icon(asset_type: &str) -> icons::Icon {
    match asset_type.rsplit("::").next().unwrap_or(asset_type) {
        "Mesh" => icons::Icon::Cube,
        "Texture" => icons::Icon::Image,
        "Material" => icons::Icon::Sphere,
        "Shader" => icons::Icon::Code,
        "RenderPass" => icons::Icon::Faders,
        _ => icons::Icon::File,
    }
}

/// Returns the layout job for a tile's name: wrapped to
/// [`ASSETS_TILE_LABEL_ROWS`] rows at `width` points, broken inside a word when
/// it must (a long file name has no spaces), with an ellipsis where the last
/// row runs out.
pub fn tile_name_job(
    text: &str,
    font: &egui::FontId,
    color: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font.clone(), color, width);
    job.wrap.max_rows = ASSETS_TILE_LABEL_ROWS;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

/// Draws the menu a file tile offers: rename and delete. Creating an asset and
/// opening a folder belong to a container's menu ([`asset_container_menu`]), so
/// a file's menu never carries them.
pub fn asset_menu_items(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    selection: &EditorSelection,
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
) {
    if ui
        .add(icons::IconButton::new(
            icons::Icon::PencilSimple,
            "Rename Asset...",
        ))
        .clicked()
    {
        assets::open_rename(dialog, selection, &snapshot.assets);
        ui.close();
    }
    if ui
        .add_enabled(
            assets::can_delete(selection, &snapshot.assets),
            icons::IconButton::new(icons::Icon::Trash, "Delete Asset"),
        )
        .clicked()
    {
        assets::delete_selection(pending, selection, &snapshot.assets);
        ui.close();
    }
}

/// Draws the menu a container offers: the grid's empty strip and a folder tile.
/// A folder can have an asset created in it and can be opened in the system's
/// file manager.
///
/// A module's folder can be opened but not written into: its resources are
/// shown and inspected, never created beside.
pub fn asset_container_menu(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    selection: &EditorSelection,
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
    folder: &str,
) {
    if !assets::is_read_only_key(&snapshot.assets, folder)
        && ui
            .add(icons::IconButton::new(
                icons::Icon::Plus,
                "Create New Asset...",
            ))
            .clicked()
    {
        assets::open_create(
            dialog,
            selection,
            &snapshot.assets,
            &snapshot.standalone_types,
        );
        ui.close();
    }
    if ui
        .add(icons::IconButton::new(
            icons::Icon::FolderOpen,
            "Open Folder in File Explorer",
        ))
        .clicked()
    {
        pending.reveal(folder.to_owned());
        pill_core::info!(
            target: telemetry_target::EDITOR,
            path = folder,
            "[editor] reveal queued"
        );
        ui.close();
    }
}

/// Draws the create or rename dialog over the dock while one is open.
///
/// The create dialog lists the live standalone types and a name field; the
/// rename dialog shows the current path and the new name. Their buttons run the
/// shared submit helpers, which queue the recorded batch and close the dialog.
///
/// Public because [`run_frame`]'s generated hot patch calls it through the
/// package path.
///
/// While the dialog is open, its rectangle joins the frame's overlay regions, so
/// a click on a button or in its name field belongs to the dialog and never
/// becomes a scene pick behind it.
pub fn asset_dialog_window(
    context: &egui::Context,
    dialog: &mut Option<AssetDialog>,
    pending: &mut PendingEditorCommands,
    snapshot: &EditorSnapshot,
    overlay_regions: &mut Vec<ShellRect>,
) {
    let Some(current) = dialog.as_ref() else {
        return;
    };
    // Step 1: work out the title from the collision, then draw the dialog's
    // body and its buttons. An existing name is asked about first: the same
    // window adds a warning and the keep-the-guid checkbox, and its submit
    // button reads Replace.
    let collision = assets::create_collision(current, &snapshot.standalone_types, &snapshot.assets);
    let (title, submit_label) = match current {
        AssetDialog::Create { .. } if collision.is_some() => ("Replace Asset", "Replace"),
        AssetDialog::Create { .. } => ("Create Asset", "Create"),
        AssetDialog::Rename { .. } => ("Rename Asset", "Rename"),
    };
    let mut submit = false;
    let mut cancel = false;
    let window = egui::Window::new(title)
        .id(egui::Id::new("pill_editor_shell_egui.asset_dialog"))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .collapsible(false)
        .resizable(false)
        .show(context, |ui| {
            let Some(current) = dialog.as_mut() else {
                return;
            };
            match current {
                AssetDialog::Create {
                    folder,
                    type_name,
                    name,
                    kind,
                    overwrite,
                } => {
                    ui.label(format!(
                        "In res{}{}",
                        if folder.is_empty() { "" } else { "/" },
                        folder
                    ));
                    ui.separator();
                    if snapshot.standalone_types.is_empty() {
                        ui.label("No asset types are loaded.");
                    }
                    for info in &snapshot.standalone_types {
                        let chosen = *type_name == info.type_name;
                        let marker = if chosen { ">" } else { " " };
                        let label = format!("{marker} {} (.{})", info.display_name, info.extension);
                        if ui.selectable_label(chosen, label).clicked() {
                            *type_name = info.type_name.clone();
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                    });
                    if *kind != assets::CreateKind::Asset {
                        ui.label(
                            egui::RichText::new(match kind {
                                assets::CreateKind::EmptyScene => {
                                    "Creates an empty scene and opens it."
                                }
                                assets::CreateKind::SaveWorld => {
                                    "Writes the current world to this file."
                                }
                                assets::CreateKind::Asset => "",
                            })
                            .weak()
                            .italics(),
                        );
                    }
                    if let Some(path) = collision.as_deref() {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(format!("{path} already exists."))
                                .color(style::warning()),
                        );
                        let mut keep = overwrite.unwrap_or(true);
                        // Showing the warning is also the answer to it: the
                        // keep-the-guid default goes into `overwrite` at once.
                        // The checkbox only moves the choice away from that
                        // default, and an unset `overwrite` would make the
                        // queued create fail with "already exists" - the
                        // refusal the warning exists to avoid.
                        if overwrite.is_none() {
                            *overwrite = Some(keep);
                        }
                        if ui
                            .checkbox(&mut keep, "Keep the existing guid")
                            .on_hover_text(
                                "Other assets that reference this one stay valid. Untick to write a fresh guid.",
                            )
                            .changed()
                        {
                            *overwrite = Some(keep);
                        }
                    }
                }
                AssetDialog::Rename { from, name } => {
                    ui.label(format!("Rename {from}"));
                    ui.separator();
                    ui.label("New name");
                    ui.text_edit_singleline(name);
                    ui.label(
                        egui::RichText::new("The file keeps its extension.")
                            .weak()
                            .italics(),
                    );
                }
            }
            ui.separator();
            // Stock buttons, submit first, so the action the dialog exists for
            // is the first the pointer meets.
            ui.horizontal(|ui| {
                if ui.button(submit_label).clicked() {
                    submit = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    // Step 2: claim the window's rectangle, so a press inside it is the UI's.
    if let Some(window) = window {
        claim_pointer(window.response.rect, overlay_regions);
    }
    // Step 3: run the answer the buttons recorded.
    if submit {
        match dialog.as_ref() {
            Some(AssetDialog::Create { .. }) => {
                assets::submit_create(dialog, pending);
            }
            Some(AssetDialog::Rename { .. }) => {
                assets::submit_rename(dialog, pending);
            }
            None => {}
        }
    }
    if cancel {
        assets::cancel(dialog);
    }
}
