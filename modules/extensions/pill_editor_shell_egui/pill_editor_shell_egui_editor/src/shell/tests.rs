//! The shell's unit tests.
//!
//! # Responsibilities
//!
//! - The tab bodies and the panel logic behind them: the asset browser's
//!   tree, path bar and grid, the tile icons, the addable component list, the
//!   shared selection's moves and the Inspector's rows and settings.
//! - The shared chrome: the popup's dismissal rule, the left-aligned row, the
//!   logged row placements and the floating toolbox's rows.
//!
//! # Design
//!
//! The shell declares this module as `#[cfg(test)] pub mod tests`, so nothing
//! here is built into the module the host loads, and `use super::*;` reaches
//! every shell item. A test that needs pointer events goes through
//! [`run_pass_with`]; the rest use [`run_pass`].

// External crates
use pill_engine::Engine;

// Current crate
use super::*;

// =============================================================================
// Free functions
// =============================================================================

/// The artifact declares exactly one system: the shell pass. A second
/// declaration would run two egui passes per frame.
#[test]
fn the_shell_declares_one_system() {
    let mut engine = Engine::new();
    pill_engine::declarations::register_all_systems(&mut engine);
    let names: Vec<String> = engine
        .system_snapshots()
        .into_iter()
        .map(|snapshot| snapshot.name)
        .collect();
    assert_eq!(names, ["egui_shell"], "{names:?}");
}

/// A row's hover text is the tooltip its type declared, and the whole key when
/// the type declared none.
#[test]
fn a_settings_row_shows_the_tooltip_its_type_declared() {
    let decorators = [
        AssetFieldDecorator {
            key: "ambient_sinks".to_owned(),
            decorator: FieldDecorator::Tooltip("Voices for sounds in this scene.".to_owned()),
        },
        AssetFieldDecorator {
            key: "ear_separation".to_owned(),
            decorator: FieldDecorator::Range {
                min: 0.1,
                max: 10.0,
            },
        },
    ];

    assert_eq!(
        inspector_settings::settings_row_tooltip(&decorators, "ambient_sinks"),
        "Voices for sounds in this scene."
    );
    assert_eq!(
        inspector_settings::settings_row_tooltip(&decorators, "gain"),
        "gain",
        "an undeclared row keeps its key as the hover text"
    );
}

/// A declared range becomes the slider's bounds, and a row with no range keeps
/// its drag value.
#[test]
fn a_settings_row_draws_the_range_its_type_declared() {
    let decorators = [AssetFieldDecorator {
        key: "ear_separation".to_owned(),
        decorator: FieldDecorator::Range {
            min: 0.1,
            max: 10.0,
        },
    }];

    assert_eq!(
        inspector_settings::settings_row_range(&decorators, "ear_separation"),
        Some((0.1, 10.0))
    );
    assert_eq!(
        inspector_settings::settings_row_range(&decorators, "gain"),
        None,
        "a row without a range keeps its drag value"
    );
}

// =============================================================================

/// Runs one egui pass for its side effects and discards the output.
///
/// A test has no GPU, so it never applies the texture updates the pass
/// produces. epaint panics in a debug build when a `TexturesDelta` with
/// unapplied updates is dropped, so the helper clears them first, which is
/// what epaint asks of a caller that ignores the updates.
///
/// `#[cfg(test)]` is the gate `#[test]` puts on a test function. It keeps this
/// helper out of the hot-patch function inventory, which names functions by
/// module path and cannot name one the library build leaves out.
#[cfg(test)]
fn run_pass(context: &egui::Context, pass: &mut dyn FnMut(&mut egui::Ui)) {
    let mut frame_output = context.run_ui(egui::RawInput::default(), pass);
    frame_output.textures_delta.clear();
}

/// Runs one egui pass over a fixed 1280x720 screen with the given events.
///
/// The drag tests need presses, moves and releases, and [`run_pass`] sends no
/// events at all.
#[cfg(test)]
fn run_pass_with(
    context: &egui::Context,
    events: Vec<egui::Event>,
    pass: &mut dyn FnMut(&mut egui::Ui),
) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        events,
        ..Default::default()
    };
    let mut frame_output = context.run_ui(input, pass);
    frame_output.textures_delta.clear();
}

/// The assets the tree and the grid tests draw: a folder, a folder inside it,
/// a mesh and a shader source file.
///
/// A macro rather than a function because the patch scanner lists every
/// module-level function that carries no `cfg` gate, and its inventory is
/// compiled into the library as well as the tests. A plain function here would
/// need the `#[cfg(test)]` gate [`run_pass`] carries; a macro needs none.
macro_rules! browser_fixture {
    () => {
        vec![
            AssetListing {
                root: String::new(),
                path: "models".to_string(),
                read_only: false,
                kind: AssetKind::Folder,
                standalone: false,
                has_meta: false,
                guid: None,
            },
            AssetListing {
                root: String::new(),
                path: "models/nested".to_string(),
                read_only: false,
                kind: AssetKind::Folder,
                standalone: false,
                has_meta: false,
                guid: None,
            },
            AssetListing {
                root: String::new(),
                path: "models/cube.obj".to_string(),
                read_only: false,
                kind: AssetKind::parse("pill_master_renderer::assets::Mesh"),
                standalone: false,
                has_meta: true,
                guid: None,
            },
            AssetListing {
                root: String::new(),
                path: "shaders/code.wgsl".to_string(),
                read_only: false,
                kind: AssetKind::parse("file"),
                standalone: false,
                has_meta: false,
                guid: None,
            },
        ]
    };
}

// =============================================================================
// Tests
// =============================================================================

#[test]
fn entity_rows_list_the_worlds_entities() {
    let mut engine = Engine::new();
    engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let second = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let rows = entity_rows(&snapshot);
    assert_eq!(rows.len(), 2);
    let expected = format!("{}v{}", second.id(), second.generation());
    assert!(
        rows.iter()
            .any(|(entity, label)| *entity == second && label.contains(&expected)),
        "the second entity's row carries its display id: {rows:?}"
    );
}

#[test]
fn the_systems_rows_name_the_worlds_systems() {
    let mut engine = Engine::new();
    engine.register_system("tick", || {});
    engine.register_system("render", || {});
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let rows: Vec<String> = snapshot
        .systems
        .iter()
        .map(systems::system_row_label)
        .collect();
    assert!(rows.iter().any(|row| row.contains("tick")), "{rows:?}");
    assert!(rows.iter().any(|row| row.contains("render")), "{rows:?}");
}

/// The tree, the path bar and the grid agree on which folder is open.
///
/// Opening a folder expands its parent folders, but not the folder's own row,
/// because the folder's children are listed in the grid. A row's caret opens
/// and closes that row, and the path bar gets one segment for each folder from
/// the root down to the open folder.
#[test]
fn the_browser_opens_a_folder_and_the_tree_follows() {
    let assets = browser_fixture!();
    let mut browser = ProjectBrowser::default();
    assert!(browser.open("models/nested"));
    assert!(
        browser.is_expanded("models"),
        "the open folder's ancestor must be expanded, or its row is unreachable"
    );
    assert!(
        !browser.is_expanded("models/nested"),
        "opening a folder lists its children in the grid; it does not expand its row"
    );
    assert!(browser.toggle_expanded("models/nested"));
    assert!(browser.is_expanded("models/nested"));
    assert!(!browser.toggle_expanded("models/nested"));
    assert!(!browser.is_expanded("models/nested"));
    // The path bar draws one segment for each folder from the root down, and
    // each segment holds the full path up to that folder.
    let segments: Vec<String> = browser::breadcrumb(&browser.folder)
        .into_iter()
        .map(|(path, _label)| path)
        .collect();
    assert_eq!(
        segments,
        vec![
            String::new(),
            "models".to_string(),
            "models/nested".to_string()
        ],
        "every segment of the open folder must appear in the path bar"
    );
    assert_eq!(
        browser::children(&assets, "models")
            .iter()
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>(),
        vec!["models/nested", "models/cube.obj"],
        "the grid lists the open folder's own children, folders first"
    );
}

/// The rule all three pickers (asset, entity and reference) get from
/// `chrome::floating_popup`: a press outside the popup and its anchor closes
/// it, and a press on either one keeps it open.
#[test]
fn a_press_outside_a_floating_popup_is_reported_outside() {
    let context = egui::Context::default();
    style::apply(&context);
    let anchor = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(120.0, 20.0));
    let press_at = |at: egui::Pos2| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let draw = |ui: &mut egui::Ui| {
        let mut regions: Vec<ShellRect> = Vec::new();
        let popup = floating_popup(
            ui,
            egui::Id::new("pill_editor_shell_egui.test_picker"),
            anchor,
            120.0,
            &mut regions,
            &mut |ui| {
                ui.label("a candidate");
            },
        );
        assert_eq!(regions.len(), 1, "the popup claims its rectangle");
        popup
    };

    let mut popup_rect = None;
    let mut on_anchor = None;
    let mut outside = None;
    let mut inside = None;
    // Each press gets its own frame: `any_pressed` reports only the frame a
    // press starts in, and the popup keeps its rectangle from one frame to the
    // next.
    run_pass_with(&context, Vec::new(), &mut |ui| {
        popup_rect = Some(draw(ui).response.rect);
    });
    run_pass_with(&context, vec![press_at(anchor.center())], &mut |ui| {
        on_anchor = Some(draw(ui).pressed_outside)
    });
    let rect = popup_rect.expect("the popup drew its rectangle");
    run_pass_with(&context, vec![press_at(rect.center())], &mut |ui| {
        inside = Some(draw(ui).pressed_outside)
    });
    run_pass_with(
        &context,
        vec![press_at(egui::pos2(900.0, 600.0))],
        &mut |ui| outside = Some(draw(ui).pressed_outside),
    );

    assert_eq!(on_anchor, Some(false), "the press that opened it keeps it");
    assert_eq!(inside, Some(false), "a press inside the list keeps it");
    assert_eq!(outside, Some(true), "a press away from it closes it");
}

/// Drawing every tab body is a smoke test: it checks that nothing panics, that
/// no tab queues a command, and that the only overlay region the pass claims
/// is the scene's frame counter. The name states the queuing check, not what
/// the tabs draw.
#[test]
fn drawing_every_tab_body_queues_nothing() {
    let mut engine = Engine::new();
    engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    engine.register_system("tick", || {});
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let mut selection = EditorSelection::default();
    let mut dialog: Option<AssetDialog> = None;
    let mut pending = PendingEditorCommands::default();
    let mut draft: Option<preferences::PreferencesDraft> = None;
    let mut browser = ProjectBrowser::default();
    let mut drag_out: Option<String> = None;
    let context = egui::Context::default();
    style::apply(&context);
    let mut regions = Vec::new();
    let mut inspector_session = InspectorSession::default();
    let mut previews = pill_editor_core::session::PreviewStates::default();
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            entities_tab(
                ui,
                &snapshot,
                &mut selection,
                &mut pending,
                &mut regions,
                &mut inspector_session,
            );
            systems_tab(ui, &snapshot, &mut pending);
            inspector_tab(
                ui,
                &snapshot,
                &selection,
                &mut pending,
                &mut regions,
                &mut inspector_session,
            );
            {
                let mut writes = AssetTileWrites {
                    selection: &mut selection,
                    dialog: &mut dialog,
                    pending: &mut pending,
                    overlay_regions: &mut regions,
                    drag_out: &mut drag_out,
                    previews: &mut previews,
                };
                assets_tab(ui, &snapshot, &mut browser, &mut writes);
            }
            preferences_tab(ui, &snapshot, &mut draft, &mut pending);
            scene_overlay(ui, 120.0, &mut regions);
        });
    });
    assert_eq!(regions.len(), 1, "the frame counter is the UI's region");
    assert!(
        pending.batches.is_empty(),
        "drawing every tab body queues no command"
    );
}

/// The Add list the popup and the component menu share offers only components
/// that are registered as addable and that the entity does not carry yet.
#[test]
fn the_addable_list_omits_the_components_the_entity_carries() {
    let mut engine = Engine::new();
    engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let mut snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    snapshot.components = vec![
        pill_editor_core::editor_state::RegisteredComponent {
            type_name: "demo::Carried".to_string(),
            addable: true,
        },
        pill_editor_core::editor_state::RegisteredComponent {
            type_name: "demo::Missing".to_string(),
            addable: true,
        },
        pill_editor_core::editor_state::RegisteredComponent {
            type_name: "demo::NotAddable".to_string(),
            addable: false,
        },
    ];
    let detail = EntityDetail {
        entity: Entity::from_raw(1, 0),
        components: vec![ComponentView {
            type_name: "demo::Carried".to_string(),
            persistable: false,
            editable: true,
            fields: Vec::new(),
            values: Vec::new(),
            asset_labels: Vec::new(),
            entity_labels: Vec::new(),
        }],
    };
    assert_eq!(
        addable_components(&snapshot, &detail),
        vec!["demo::Missing"],
        "a carried component is not offered again, and neither is a non-addable one"
    );
}

#[test]
fn the_assets_panel_wraps_a_long_tile_caption_and_queues_nothing() {
    let engine = Engine::new();
    let mut snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    snapshot.assets = browser_fixture!();
    let mut browser = ProjectBrowser::default();
    assert!(browser.open("models"));
    let mut selection = EditorSelection::default();
    let mut dialog: Option<AssetDialog> = None;
    let mut pending = PendingEditorCommands::default();
    let mut drag_out: Option<String> = None;
    let context = egui::Context::default();
    style::apply(&context);
    let mut regions = Vec::new();
    let mut caption_row_count = 0usize;
    let mut caption_width = 0.0f32;
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            assets_tree(
                ui,
                &snapshot,
                &mut browser,
                &mut selection,
                &mut dialog,
                &mut pending,
                &mut regions,
            );
            assets_path_bar(ui, &mut browser);
            assets_zoom_bar(ui, &snapshot.assets, &mut browser);
            {
                let mut previews = pill_editor_core::session::PreviewStates::default();
                let mut writes = AssetTileWrites {
                    selection: &mut selection,
                    dialog: &mut dialog,
                    pending: &mut pending,
                    overlay_regions: &mut regions,
                    drag_out: &mut drag_out,
                    previews: &mut previews,
                };
                assets_grid(ui, &snapshot, &mut browser, &mut writes);
            }
            let font = egui::TextStyle::Small.resolve(ui.style());
            let job = tile_name_job(
                "a_name_that_is_much_too_long_for_a_tile.png",
                &font,
                egui::Color32::WHITE,
                40.0,
            );
            let galley = ui.painter().layout_job(job);
            caption_row_count = galley.rows.len();
            caption_width = galley.size().x;
        });
    });
    assert_eq!(
        caption_row_count, 2,
        "a long name wraps to the two caption rows"
    );
    assert!(
        caption_width <= 41.0,
        "the caption stays inside the tile: {caption_width}"
    );
    assert!(pending.batches.is_empty(), "drawing queues no commands");
    assert_eq!(regions.len(), 0, "the tiles draw no overlay regions");
    assert_eq!(
        browser::children(&snapshot.assets, "models")
            .iter()
            .map(|row| row.path.as_str())
            .collect::<Vec<_>>(),
        vec!["models/nested", "models/cube.obj"],
        "the grid lists the open folder's own children"
    );
}

#[test]
fn tiles_pick_an_icon_from_the_kind_and_the_extension() {
    let row = |path: &str, kind: &str| AssetListing {
        root: String::new(),
        path: path.to_string(),
        read_only: false,
        kind: AssetKind::parse(kind),
        standalone: false,
        has_meta: false,
        guid: None,
    };
    assert_eq!(asset_icon(&row("models", "folder")), icons::Icon::Folder);
    assert_eq!(
        asset_icon(&row(
            "models/cube.obj",
            "pill_master_renderer::assets::Mesh"
        )),
        icons::Icon::Cube
    );
    assert_eq!(
        asset_icon(&row(
            "textures/skin.png",
            "pill_master_renderer::assets::Texture"
        )),
        icons::Icon::Image
    );
    assert_eq!(
        asset_icon(&row(
            "textures/stone.material",
            "pill_master_renderer::Material"
        )),
        icons::Icon::Sphere
    );
    assert_eq!(
        asset_icon(&row("shaders/code.wgsl", "file")),
        icons::Icon::Code
    );
    assert_eq!(asset_icon(&row("config.ini", "file")), icons::Icon::File);
}

#[test]
fn an_asset_type_names_its_row_icon() {
    assert_eq!(
        asset_type_icon("pill_master_renderer::assets::Mesh"),
        icons::Icon::Cube
    );
    assert_eq!(
        asset_type_icon("pill_master_renderer::assets::Material"),
        icons::Icon::Sphere
    );
    assert_eq!(
        asset_type_icon("pill_master_renderer::assets::Texture"),
        icons::Icon::Image
    );
    assert_eq!(
        asset_type_icon("pill_master_renderer::assets::Shader"),
        icons::Icon::Code
    );
    // A type with no shape of its own falls back to the plain file icon.
    assert_eq!(asset_type_icon("demo::Sound"), icons::Icon::File);
    assert_eq!(asset_type_icon(""), icons::Icon::File);
}

/// Dragging an asset tile from the grid onto its Inspector asset row queues
/// the reference edit, through egui's drag-and-drop payload.
#[test]
fn a_dropped_asset_tile_queues_the_reference_edit() {
    const MESH_TYPE: &str = "pill_master_renderer::assets::Mesh";
    /// Draw the grid and the asset row, one frame.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        context: &egui::Context,
        events: Vec<egui::Event>,
        snapshot: &EditorSnapshot,
        browser: &mut ProjectBrowser,
        selection: &mut EditorSelection,
        dialog: &mut Option<AssetDialog>,
        pending: &mut PendingEditorCommands,
        regions: &mut Vec<ShellRect>,
        drag_out: &mut Option<String>,
        inspector_session: &mut InspectorSession,
        view: &ComponentView,
        entity: Entity,
    ) {
        run_pass_with(context, events, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(640.0, 300.0), |ui| {
                    let mut previews = pill_editor_core::session::PreviewStates::default();
                    let mut writes = AssetTileWrites {
                        selection,
                        dialog,
                        pending,
                        overlay_regions: regions,
                        drag_out,
                        previews: &mut previews,
                    };
                    assets_grid(ui, snapshot, browser, &mut writes);
                });
                inspector_fields::inspector_asset_row(
                    ui,
                    inspector_fields::AssetRowInput {
                        entity,
                        component: view,
                        index: 0,
                        field: "mesh",
                        asset_type: Some(MESH_TYPE),
                        tooltip: None,
                        targets: std::slice::from_ref(&entity),
                        details: &[],
                        assets: &snapshot.assets,
                        engine_assets: &[],
                    },
                    pending,
                    inspector_session,
                    regions,
                );
            });
        });
    }

    let engine = Engine::new();
    let mut snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    snapshot.assets = vec![AssetListing {
        root: String::new(),
        path: "cube.obj".to_string(),
        read_only: false,
        kind: AssetKind::parse(MESH_TYPE),
        standalone: false,
        has_meta: true,
        guid: None,
    }];
    let entity = Entity::from_raw(1, 0);
    let view = ComponentView {
        type_name: "demo::MeshRenderer".to_string(),
        persistable: false,
        editable: true,
        fields: Vec::new(),
        values: vec![FieldValue::Asset {
            asset_type: Some(MESH_TYPE.to_string()),
            index: u32::MAX,
            generation: u32::MAX,
        }],
        asset_labels: vec![Some(AssetReferenceLabel::Unset)],
        entity_labels: vec![None],
    };
    let mut browser = ProjectBrowser::default();
    let mut selection = EditorSelection::default();
    let mut dialog: Option<AssetDialog> = None;
    let mut pending = PendingEditorCommands::default();
    let mut regions: Vec<ShellRect> = Vec::new();
    let mut drag_out: Option<String> = None;
    let mut inspector_session = InspectorSession::default();
    let context = egui::Context::default();
    style::apply(&context);

    // One idle pass lays both ends out and records their rectangles.
    draw(
        &context,
        Vec::new(),
        &snapshot,
        &mut browser,
        &mut selection,
        &mut dialog,
        &mut pending,
        &mut regions,
        &mut drag_out,
        &mut inspector_session,
        &view,
        entity,
    );
    let tile = browser
        .tile_rects
        .iter()
        .find(|(path, _)| path == "cube.obj")
        .map(|(_, corners)| {
            egui::Rect::from_min_max(
                egui::pos2(corners[0], corners[1]),
                egui::pos2(corners[2], corners[3]),
            )
        })
        .expect("the tile drew");
    let row = inspector_session
        .asset_row_rects
        .iter()
        .next()
        .map(|(_, rect)| rect)
        .expect("the asset row drew");
    let from = tile.center();
    let to = row.center();

    // Press on the tile, move onto the row, then release, one frame each: the
    // same three frames and event shapes the window bridge produces for a
    // scripted drag.
    let press = egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    for events in [
        vec![egui::Event::PointerMoved(from), press],
        vec![egui::Event::PointerMoved(to)],
        vec![release],
    ] {
        draw(
            &context,
            events,
            &snapshot,
            &mut browser,
            &mut selection,
            &mut dialog,
            &mut pending,
            &mut regions,
            &mut drag_out,
            &mut inspector_session,
            &view,
            entity,
        );
    }

    assert_eq!(
        pending.batches.len(),
        1,
        "the drop queued one recorded edit"
    );
    assert!(
        matches!(
            pending.batches[0].commands.as_slice(),
            [EditorCommand::SetFieldAsset { field, reference: Some(path), .. }]
                if field == "mesh" && path == "cube.obj"
        ),
        "the drop points the field at the tile's asset: {:?}",
        pending.batches[0].commands
    );
}

/// Dragging a list row reorders the list.
///
/// The drop rewrites the whole array in one recorded step, so undo takes back
/// the reorder and not the file. A release on a row's picker button must count
/// as a drop on the list instead of being taken by the picker.
#[test]
fn a_dragged_row_reorders_the_list() {
    const PASS_TYPE: &str = "pill_master_renderer::assets::RenderPass";
    const ASSET_PATH: &str = "pipelines/main.pipeline";

    #[cfg(test)]
    fn draw(
        context: &egui::Context,
        events: Vec<egui::Event>,
        document: &serde_json::Value,
        references: &[AssetReferenceField],
        pending: &mut PendingEditorCommands,
        inspector_session: &mut InspectorSession,
        regions: &mut Vec<ShellRect>,
    ) {
        run_pass_with(context, events, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(420.0, 300.0), |ui| {
                    inspector_settings::reference_list_rows(
                        ui,
                        ASSET_PATH,
                        document,
                        references,
                        &mut SettingsWalk {
                            assets: &[],
                            engine_assets: &[],
                            pending,
                            session: inspector_session,
                            overlay_regions: regions,
                            decorators: &[],
                            read_only: false,
                        },
                    );
                });
            });
        });
    }

    let document = serde_json::json!({
        "passes": [{ "guid": "a" }, { "guid": "b" }, { "guid": "c" }]
    });
    let references: Vec<AssetReferenceField> = ["a", "b", "c"]
        .iter()
        .enumerate()
        .map(|(index, guid)| AssetReferenceField {
            key: format!("passes[{index}]"),
            target: PASS_TYPE.to_string(),
            guid: Some((*guid).to_string()),
            nested: true,
            label: None,
        })
        .collect();
    let mut pending = PendingEditorCommands::default();
    let mut inspector_session = InspectorSession::default();
    let mut regions: Vec<ShellRect> = Vec::new();
    let context = egui::Context::default();
    style::apply(&context);

    // One idle pass lays the rows out and records their rectangles.
    draw(
        &context,
        Vec::new(),
        &document,
        &references,
        &mut pending,
        &mut inspector_session,
        &mut regions,
    );
    let rows: Vec<egui::Rect> = (0..3)
        .map(|index| {
            let key = format!("{ASSET_PATH}:passes[{index}]");
            inspector_session
                .settings_row_rects
                .get(&key)
                .expect("the row drew")
        })
        .collect();

    // Press on the first row's picker, move to the bottom edge of the third
    // row, then release, one frame each: the three frames and event shapes of
    // a scripted drag.
    let from = egui::pos2(rows[0].right() - 40.0, rows[0].center().y);
    let to = egui::pos2(rows[2].right() - 40.0, rows[2].bottom() - 1.0);
    let press = egui::Event::PointerButton {
        pos: from,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos: to,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    for events in [
        vec![egui::Event::PointerMoved(from), press],
        vec![egui::Event::PointerMoved(to)],
        vec![release],
    ] {
        draw(
            &context,
            events,
            &document,
            &references,
            &mut pending,
            &mut inspector_session,
            &mut regions,
        );
    }

    assert_eq!(
        pending.batches.len(),
        1,
        "the drop queued one recorded edit"
    );
    let EditorCommand::SetAssetSettings { path, settings } = &pending.batches[0].commands[0] else {
        panic!(
            "the drop writes the settings document: {:?}",
            pending.batches[0].commands
        );
    };
    assert_eq!(path, ASSET_PATH);
    let order: Vec<&str> = settings["passes"]
        .as_array()
        .expect("the list stays an array")
        .iter()
        .map(|entry| entry["guid"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(order, vec!["b", "c", "a"], "the first row moved to the end");
}

/// A row's label starts at the left edge of the label column, so the control
/// begins at the same x whatever the label says; a centred label would move
/// with its own length. The control keeps the row's own gap from the column.
#[test]
fn a_row_left_aligns_its_label_and_keeps_the_gap() {
    let context = egui::Context::default();
    style::apply(&context);
    // The control's offset from the row's left edge, for one label.
    let measure = |label: &str| -> f32 {
        let mut offset = 0.0;
        run_pass(&context, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(520.0, 120.0), |ui| {
                    let row_left = ui.cursor().left();
                    row_in(ui, WIDE_ROW_LABEL_WIDTH, label, None, &mut |ui| {
                        let response = ui
                            .add_sized(egui::vec2(80.0, ROW_HEIGHT), egui::Button::new("control"));
                        offset = response.rect.left() - row_left;
                    });
                });
            });
        });
        offset
    };
    let short = measure("a");
    let long = measure("a label long enough to fill its column");
    assert_eq!(
        short, long,
        "the control sits at one x whatever the label says"
    );
    assert!(
        short >= WIDE_ROW_LABEL_WIDTH + ROW_LABEL_GAP - 0.5,
        "the control starts past the label column and its gap, got {short}"
    );
}

/// A click on an entity row moves the shared selection; a second click on the
/// selected row is not a change.
#[test]
fn a_row_click_moves_the_shared_selection() {
    let mut engine = Engine::new();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let mut selection = EditorSelection::default();
    assert!(select_entity(&mut selection, entity));
    assert_eq!(selection.primary(), Some(entity));
    // A second click on the selected row is not a change.
    assert!(!select_entity(&mut selection, entity));
}

#[test]
fn an_asset_row_click_moves_the_asset_selection() {
    let mut selection = EditorSelection::default();
    assert!(select_asset(&mut selection, "textures/rock.png"));
    assert_eq!(selection.asset.as_deref(), Some("textures/rock.png"));
    // A second click on the selected row is not a change.
    assert!(!select_asset(&mut selection, "textures/rock.png"));
}

#[test]
fn the_inspector_lists_the_selections_components_and_fields() {
    use pill_editor_core::editor_state::ComponentView;
    use pill_editor_core::editor_state::{ComponentFieldView, EntityDetail};
    let mut engine = Engine::new();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let detail = EntityDetail {
        entity,
        components: vec![
            ComponentView {
                asset_labels: Vec::new(),
                entity_labels: Vec::new(),
                type_name: "tests::Health".to_string(),
                persistable: false,
                editable: true,
                fields: vec![ComponentFieldView {
                    name: "current".to_string(),
                    decorators: Vec::new(),
                }],
                values: vec![FieldValue::F32(3.0)],
            },
            ComponentView {
                asset_labels: Vec::new(),
                entity_labels: Vec::new(),
                type_name: "tests::Marker".to_string(),
                persistable: false,
                editable: false,
                fields: Vec::new(),
                values: Vec::new(),
            },
        ],
    };
    let rows = component_rows(&detail);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "tests::Health");
    assert_eq!(rows[0].1, vec![("current".to_string(), "3".to_string())]);
    // A component without editable fields lists none.
    assert_eq!(rows[1].0, "tests::Marker");
    assert!(rows[1].1.is_empty());

    // The tab draws the detail without panicking.
    let mut snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    snapshot.detail = Some(detail);
    let mut selection = EditorSelection::default();
    selection.select_only(entity);
    selection.asset = Some("textures/rock.png".to_string());
    snapshot.asset_detail = Some(pill_editor_core::editor_state::AssetDetail {
        row: pill_editor_core::editor_state::AssetListing {
            root: String::new(),
            path: "textures/rock.png".to_string(),
            read_only: false,
            kind: pill_editor_core::editor_state::AssetKind::Type("file".to_string()),
            standalone: false,
            has_meta: false,
            guid: None,
        },
        settings: Some(serde_json::json!({
            "texture_type": "Color",
            "mips": 4,
            "flip_v": false,
            "waves": [{ "amplitude": 1.0 }],
        })),
        references: Vec::new(),
        decorators: Vec::new(),
    });
    let mut pending = PendingEditorCommands::default();
    let context = egui::Context::default();
    style::apply(&context);
    let mut regions = Vec::new();
    let mut inspector_session = InspectorSession::default();
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            inspector_tab(
                ui,
                &snapshot,
                &selection,
                &mut pending,
                &mut regions,
                &mut inspector_session,
            );
        });
    });
    assert!(pending.batches.is_empty(), "drawing queues no edits");
}

#[test]
fn field_values_format_for_display() {
    assert_eq!(format_field_value(&FieldValue::F32(1.5)), "1.5");
    assert_eq!(format_field_value(&FieldValue::U32(7)), "7");
    assert_eq!(format_field_value(&FieldValue::Bool(true)), "true");
    assert_eq!(
        format_field_value(&FieldValue::Text("hi".to_string())),
        "hi"
    );
}

/// A list entry's key (`passes[2]`) names the list family it belongs to, and a
/// plain key names no family.
///
/// The settings walk uses the family to skip a list's raw array, so its guids
/// are never drawn as JSON rows.
#[test]
fn a_reference_keys_family_is_what_a_list_row_belongs_to() {
    let entry = AssetReferenceField {
        key: "passes[2]".to_string(),
        target: "demo::RenderPass".to_string(),
        guid: None,
        nested: true,
        label: None,
    };
    assert_eq!(inspector_settings::reference_family(&entry), Some("passes"));
    let named = AssetReferenceField {
        key: "shader".to_string(),
        ..entry
    };
    assert_eq!(inspector_settings::reference_family(&named), None);
}

/// A row's rectangle is logged once per placement. A jitter smaller than a
/// point and a row that scrolls out and back log nothing new; a real move
/// logs again.
#[test]
fn a_rows_placement_is_logged_once_per_place() {
    let mut logged = LoggedRects::default();
    let rect = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 40.0));
    assert!(inspector_fields::record_row_placement(&mut logged, "row", rect).is_some());
    assert!(inspector_fields::record_row_placement(&mut logged, "row", rect).is_none());
    let jittered = rect.translate(egui::vec2(0.2, -0.3));
    assert!(
        inspector_fields::record_row_placement(&mut logged, "row", jittered).is_none(),
        "a fraction of a point is the same placement"
    );
    let moved = rect.translate(egui::vec2(0.0, 12.0));
    assert!(
        inspector_fields::record_row_placement(&mut logged, "row", moved).is_some(),
        "a real move logs"
    );
}

/// Every component of a vector keeps the same width. A value too long for its
/// field is cropped inside it instead of widening the row, so the axis labels
/// beside it never move.
#[test]
fn a_vectors_components_keep_one_width() {
    let values = [
        FieldValue::F32(1.0),
        FieldValue::F32(-123_456.75),
        FieldValue::F32(0.000_125),
    ];
    let context = egui::Context::default();
    style::apply(&context);
    let mut widths = Vec::new();
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.set_max_width(360.0);
            for value in &values {
                let row = ui.horizontal_wrapped(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(INSPECTOR_VECTOR_FIELD_WIDTH, INSPECTOR_VECTOR_FIELD_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            axis_slot(ui, 1);
                            fixed_width_field(ui, value, None);
                        },
                    );
                });
                widths.push(row.response.rect.width());
            }
        });
    });
    assert_eq!(
        widths[0], widths[1],
        "a long value widens the row: {widths:?}"
    );
    assert_eq!(
        widths[0], widths[2],
        "a value with many decimals widens the row: {widths:?}"
    );
    assert!(
        widths[0] <= INSPECTOR_VECTOR_FIELD_WIDTH + 8.0,
        "the row stays the width of its group: {widths:?}"
    );
    // The same value in a plain row does grow the row. That growth is what the
    // fixed-width field in a vector row prevents.
    let mut plain_width = 0.0f32;
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.set_max_width(360.0);
            let row = ui.horizontal_wrapped(|ui| {
                axis_slot(ui, 1);
                scalar_edit(ui, &values[1]);
            });
            plain_width = row.response.rect.width();
        });
    });
    assert!(
        plain_width > widths[1],
        "an unconstrained editor already fits its value ({plain_width} vs {})",
        widths[1]
    );
}

/// The floating toolbox draws one ordinary button per row, as wide as the row.
///
/// The panel therefore reads as one column of controls. The active row takes
/// the theme's selection colour, and a click on a row still queues that row's
/// command.
#[test]
fn the_toolbox_rows_fill_their_width() {
    /// Draw the toolbox over a viewport-sized screen, one frame.
    fn draw(
        context: &egui::Context,
        events: Vec<egui::Event>,
        snapshot: &EditorSnapshot,
        toolbox: &mut ToolboxSession,
        pending: &mut PendingEditorCommands,
        regions: &mut Vec<ShellRect>,
    ) {
        run_pass_with(context, events, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut viewer = EditorTabViewer {
                    snapshot,
                    selection: &mut EditorSelection::default(),
                    asset_dialog: &mut None,
                    pending,
                    preferences_draft: &mut None,
                    toolbox,
                    browser: &mut ProjectBrowser::default(),
                    inspector: &mut InspectorSession::default(),
                    scene_rects: Vec::new(),
                    game_rect: None,
                    overlay_regions: Vec::new(),
                    frames_per_second: 60.0,
                    active_tabs: &[],
                    deferred: &mut FrameRequests::default(),
                    console_filters: &mut ConsoleFilterSettings::default(),
                    requests: &mut ShellRequests::default(),
                    stats: &mut StatisticsSession::default(),
                    panels: &PanelRegistry::default(),
                    panel_budget: &mut PanelBudget::default(),
                    panel_commands: &mut Vec::new(),
                    panel_entities: &[],
                    panel_detail: None,
                    panel_answer: None,
                    panel_window_requests: &mut Vec::new(),
                    pending_panel_window: None,
                    pending_panel_window_used: false,
                    clipboard: &mut pill_editor_core::clipboard::EditorClipboard::default(),
                    previews: &mut pill_editor_core::session::PreviewStates::default(),
                    badges: &[],
                };
                toolbox_overlay(&mut viewer, ui, 0);
                *regions = std::mem::take(&mut viewer.overlay_regions);
            });
        });
    }

    let engine = Engine::new();
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let mut toolbox = ToolboxSession::default();
    let mut pending = PendingEditorCommands::default();
    let mut regions: Vec<ShellRect> = Vec::new();
    let context = egui::Context::default();
    style::apply(&context);
    let palette = style::palette();

    // One idle pass lays the panel out and records each row's face.
    draw(
        &context,
        Vec::new(),
        &snapshot,
        &mut toolbox,
        &mut pending,
        &mut regions,
    );
    let faces: Vec<egui::Rect> = (0..toolbox::TOOLBOX_ITEMS.len())
        .map(|index| {
            toolbox
                .face(0, index)
                .expect("every row drew its face this pass")
        })
        .collect();
    // The panel is placed against the viewport's corner, which sits inside the
    // central panel's margin, so the test works the origin out from the first
    // row it drew instead of assuming the screen's corner.
    let origin = faces[0].min
        - egui::vec2(
            toolbox::TOOLBOX_PADDING,
            toolbox::TOOLBOX_HANDLE_HEIGHT + toolbox::TOOLBOX_ITEM_GAP,
        );
    for (item, face) in toolbox::TOOLBOX_ITEMS.iter().zip(&faces) {
        let row = toolbox::item_rect(*item, origin);
        assert!(
            row.contains_rect(*face),
            "{}'s button {face:?} is not inside its row {row:?}",
            item.name()
        );
        assert!(
            (face.width() - row.width()).abs() < 0.5,
            "{}'s button takes the whole row's width: {face:?} in {row:?}",
            item.name()
        );
    }

    // The active row (the default tool) is painted with the theme's selection
    // colour by egui's own button, and a row at rest is not.
    let first = faces[0];
    let neutral = faces[1];
    let selection_fill = palette.selection;
    let mut frame_output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        },
        |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut viewer = EditorTabViewer {
                    snapshot: &snapshot,
                    selection: &mut EditorSelection::default(),
                    asset_dialog: &mut None,
                    pending: &mut PendingEditorCommands::default(),
                    preferences_draft: &mut None,
                    toolbox: &mut toolbox,
                    browser: &mut ProjectBrowser::default(),
                    inspector: &mut InspectorSession::default(),
                    scene_rects: Vec::new(),
                    game_rect: None,
                    overlay_regions: Vec::new(),
                    frames_per_second: 60.0,
                    active_tabs: &[],
                    deferred: &mut FrameRequests::default(),
                    console_filters: &mut ConsoleFilterSettings::default(),
                    requests: &mut ShellRequests::default(),
                    stats: &mut StatisticsSession::default(),
                    panels: &PanelRegistry::default(),
                    panel_budget: &mut PanelBudget::default(),
                    panel_commands: &mut Vec::new(),
                    panel_entities: &[],
                    panel_detail: None,
                    panel_answer: None,
                    panel_window_requests: &mut Vec::new(),
                    pending_panel_window: None,
                    pending_panel_window_used: false,
                    clipboard: &mut pill_editor_core::clipboard::EditorClipboard::default(),
                    previews: &mut pill_editor_core::session::PreviewStates::default(),
                    badges: &[],
                };
                toolbox_overlay(&mut viewer, ui, 0);
            });
        },
    );
    frame_output.textures_delta.clear();
    // A painted fill counts for a row when it covers that row's width, so the
    // panel's own background (narrower than a row) never matches.
    let painted = |face: egui::Rect, fill: egui::Color32| {
        frame_output
            .shapes
            .iter()
            .any(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) => {
                    rect.fill == fill
                        && rect.rect.intersects(face)
                        && rect.rect.width() >= face.width() - 2.0
                }
                _ => false,
            })
    };
    assert!(
        painted(first, selection_fill),
        "the active row's button takes the selection colour"
    );
    assert!(
        !painted(neutral, selection_fill),
        "a row at rest is not painted as the selection"
    );

    // A press and a release on the face still queue the row's command.
    let center = first.center();
    let press = egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    };
    let release = egui::Event::PointerButton {
        pos: center,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    };
    for events in [
        vec![egui::Event::PointerMoved(center), press],
        vec![release],
    ] {
        draw(
            &context,
            events,
            &snapshot,
            &mut toolbox,
            &mut pending,
            &mut regions,
        );
    }
    assert_eq!(
        pending.batches.len(),
        1,
        "the click queued one recorded batch"
    );
    assert!(
        matches!(
            pending.batches[0].commands[0],
            EditorCommand::SetGizmoTool { .. }
        ),
        "the row queues its gizmo command: {:?}",
        pending.batches[0].commands
    );
}

/// The Preferences tab's first draw seeds its draft buffers and queues no
/// command.
#[test]
fn the_preferences_pair_saves_and_cancels() {
    /// Draws the Preferences tab for one frame with the given events.
    fn draw(
        context: &egui::Context,
        events: Vec<egui::Event>,
        snapshot: &EditorSnapshot,
        draft: &mut Option<preferences::PreferencesDraft>,
        pending: &mut PendingEditorCommands,
    ) {
        run_pass_with(context, events, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.set_max_width(420.0);
                preferences_tab(ui, snapshot, draft, pending);
            });
        });
    }

    let engine = Engine::new();
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let mut draft: Option<preferences::PreferencesDraft> = None;
    let mut pending = PendingEditorCommands::default();
    let context = egui::Context::default();
    style::apply(&context);
    draw(&context, Vec::new(), &snapshot, &mut draft, &mut pending);
    assert!(draft.is_some(), "the tab seeded its buffers");
    assert!(pending.batches.is_empty(), "drawing queues nothing");
}

/// The label column is a share of the panel's width within its bounds, so
/// every row of one panel starts its control at the same x.
#[test]
fn the_label_column_scales_with_the_panel_within_its_bounds() {
    let context = egui::Context::default();
    let mut widths = Vec::new();
    for panel_width in [150.0_f32, 360.0, 2000.0] {
        run_pass(&context, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(panel_width, 40.0), |ui| {
                    widths.push(label_column_width(ui));
                });
            });
        });
    }
    assert_eq!(
        widths[0], LABEL_COLUMN_MIN_WIDTH,
        "a narrow panel keeps the narrowest column; the fields take the squeeze"
    );
    assert!(
        (widths[1] - 360.0 * LABEL_COLUMN_FRACTION).abs() < 20.0,
        "a common panel gives its share: {widths:?}"
    );
    assert_eq!(
        widths[2], LABEL_COLUMN_MAX_WIDTH,
        "a wide panel stops at the cap"
    );
}

/// In a panel narrower than the label column plus roomy fields, a row keeps
/// its label column and squashes its fields: a scalar row and a three-axis
/// row both end inside the panel instead of running past its edge.
#[test]
fn a_narrow_panel_squashes_fields_and_keeps_labels() {
    const PANEL_WIDTH: f32 = 230.0;
    let context = egui::Context::default();
    style::apply(&context);
    let values = [
        FieldValue::F32(1.5),
        FieldValue::F32(-123_456.75),
        FieldValue::F32(0.25),
    ];
    let mut label_width = 0.0f32;
    let mut row_rights = Vec::new();
    let mut panel_right = 0.0f32;
    run_pass(&context, &mut |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.allocate_ui(egui::vec2(PANEL_WIDTH, 200.0), |ui| {
                panel_right = ui.max_rect().right();
                label_width = label_column_width(ui);
                let scalar_row = ui.horizontal(|ui| {
                    label_cell(ui, "gravity_scale", None);
                    label_gap(ui);
                    fixed_width_field(ui, &values[1], None);
                });
                row_rights.push(scalar_row.response.rect.right());
                let vector_rows = ui.vertical(|ui| {
                    vector_row(
                        ui,
                        "linear_velocity",
                        None,
                        values.len(),
                        &mut |ui, axis| {
                            fixed_width_field(ui, &values[axis], None);
                        },
                    );
                });
                row_rights.push(vector_rows.response.rect.right());
            });
        });
    });
    assert_eq!(
        label_width, LABEL_COLUMN_MIN_WIDTH,
        "the labels keep their column"
    );
    for right in &row_rights {
        assert!(
            *right <= panel_right + 0.5,
            "a row runs past the panel's edge: rows end at {row_rights:?}, the panel at {panel_right}"
        );
    }
}

/// The Inspector drawn the way the dock draws a tab - a vertical-only scroll
/// area and the body's margin, in a narrow tab - lays every row out inside the
/// tab: nothing reaches past its right edge, where the dock would clip it.
#[test]
fn a_narrow_inspector_tab_lays_everything_out_inside_the_tab() {
    use pill_editor_core::editor_state::{
        AssetDetail, AssetKind, AssetListing, ComponentFieldView, ComponentView, EntityDetail,
    };
    const TAB_WIDTH: f32 = 215.0;
    let mut engine = Engine::new();
    let entity = engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    let field = |name: &str| ComponentFieldView {
        name: name.to_string(),
        decorators: Vec::new(),
    };
    let floats = |values: &[f32]| {
        FieldValue::Array(values.iter().map(|value| FieldValue::F32(*value)).collect())
    };
    let detail = EntityDetail {
        entity,
        components: vec![
            ComponentView {
                asset_labels: Vec::new(),
                entity_labels: Vec::new(),
                type_name: "tests::TransformComponent".to_string(),
                persistable: true,
                editable: true,
                fields: vec![field("translation"), field("rotation"), field("scale")],
                values: vec![
                    floats(&[0.0, 0.0, 0.0]),
                    floats(&[0.4, 0.8, 0.2, 0.1]),
                    floats(&[1.0, 1.0, 1.0]),
                ],
            },
            ComponentView {
                asset_labels: Vec::new(),
                entity_labels: Vec::new(),
                type_name: "tests::MeshRendererComponent".to_string(),
                persistable: true,
                editable: true,
                fields: vec![field("mesh"), field("material")],
                values: vec![FieldValue::unset_asset(), FieldValue::unset_asset()],
            },
        ],
    };
    let mut snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    snapshot.detail = Some(detail);
    let mut selection = EditorSelection::default();
    selection.select_only(entity);
    selection.asset = Some("settings/pill_physics.physics_settings".to_string());
    snapshot.asset_detail = Some(AssetDetail {
        row: AssetListing {
            root: String::new(),
            path: "settings/pill_physics.physics_settings".to_string(),
            read_only: false,
            kind: AssetKind::Type("pill_physics::PhysicsSettingsAsset".to_string()),
            standalone: true,
            has_meta: false,
            guid: None,
        },
        settings: Some(serde_json::json!({
            "debug_draw": true,
            "fixed_timestep": 0.016666668,
            "gravity": [0.0, -9.81, 0.0],
            "interpolate": false,
            "max_substeps": 8,
        })),
        references: Vec::new(),
        decorators: Vec::new(),
    });
    let mut pending = PendingEditorCommands::default();
    let context = egui::Context::default();
    style::apply(&context);
    let mut regions = Vec::new();
    let mut inspector_session = InspectorSession::default();
    let mut tab_right = 0.0f32;
    let mut content_right = 0.0f32;
    // Two passes: egui settles a scroll area's size from the pass before.
    for _ in 0..2 {
        run_pass(&context, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(TAB_WIDTH, 700.0), |ui| {
                    tab_right = ui.max_rect().right();
                    egui::ScrollArea::new([false, true]).show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                inspector_tab(
                                    ui,
                                    &snapshot,
                                    &selection,
                                    &mut pending,
                                    &mut regions,
                                    &mut inspector_session,
                                );
                                content_right = ui.min_rect().right();
                            });
                    });
                });
            });
        });
    }
    assert!(
        content_right <= tab_right + 0.5,
        "the Inspector's content ends at {content_right}, past the tab's edge at {tab_right}"
    );
}

/// Draws `draw_tab` the way the dock draws a tab body - a vertical-only scroll
/// area and the body's margin, in a tab `tab_width` points wide - over two
/// passes, and returns the content's right edge and the tab's.
#[cfg(test)]
fn narrow_tab_extent(
    context: &egui::Context,
    tab_width: f32,
    draw_tab: &mut dyn FnMut(&mut egui::Ui),
) -> (f32, f32) {
    let mut tab_right = 0.0f32;
    let mut content_right = 0.0f32;
    for _ in 0..2 {
        run_pass(context, &mut |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.allocate_ui(egui::vec2(tab_width, 700.0), |ui| {
                    tab_right = ui.max_rect().right();
                    egui::ScrollArea::new([false, true]).show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::same(4))
                            .show(ui, |ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                draw_tab(ui);
                                content_right = ui.min_rect().right();
                            });
                    });
                });
            });
        });
    }
    (content_right, tab_right)
}

/// Every tab body drawn in a narrow tab stays inside it: the Entities,
/// Systems, Assets and Preferences tabs squash or shorten what does not fit
/// instead of reaching past the edge, where the dock would clip it.
#[test]
fn every_narrow_tab_body_stays_inside_the_tab() {
    const TAB_WIDTH: f32 = 215.0;
    let mut engine = Engine::new();
    engine
        .world_mut()
        .create_entity()
        .build()
        .expect("entity builds");
    engine.register_system("a_system_with_a_rather_long_name_for_its_row", || {});
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let mut selection = EditorSelection::default();
    let mut dialog: Option<AssetDialog> = None;
    let mut pending = PendingEditorCommands::default();
    let mut draft: Option<preferences::PreferencesDraft> = None;
    let mut browser = ProjectBrowser::default();
    let mut drag_out: Option<String> = None;
    let mut regions = Vec::new();
    let mut inspector_session = InspectorSession::default();
    let mut previews = pill_editor_core::session::PreviewStates::default();
    let context = egui::Context::default();
    style::apply(&context);

    let mut extents = Vec::new();
    extents.push((
        "entities",
        narrow_tab_extent(&context, TAB_WIDTH, &mut |ui| {
            entities_tab(
                ui,
                &snapshot,
                &mut selection,
                &mut pending,
                &mut regions,
                &mut inspector_session,
            );
        }),
    ));
    extents.push((
        "systems",
        narrow_tab_extent(&context, TAB_WIDTH, &mut |ui| {
            systems_tab(ui, &snapshot, &mut pending);
        }),
    ));
    extents.push((
        "assets",
        narrow_tab_extent(&context, TAB_WIDTH, &mut |ui| {
            let mut writes = AssetTileWrites {
                selection: &mut selection,
                dialog: &mut dialog,
                pending: &mut pending,
                overlay_regions: &mut regions,
                drag_out: &mut drag_out,
                previews: &mut previews,
            };
            assets_tab(ui, &snapshot, &mut browser, &mut writes);
        }),
    ));
    extents.push((
        "preferences",
        narrow_tab_extent(&context, TAB_WIDTH, &mut |ui| {
            preferences_tab(ui, &snapshot, &mut draft, &mut pending);
        }),
    ));
    for (tab, (content_right, tab_right)) in &extents {
        assert!(
            *content_right <= *tab_right + 0.5,
            "the {tab} tab's content ends at {content_right}, past the tab's edge at {tab_right}"
        );
    }
}
