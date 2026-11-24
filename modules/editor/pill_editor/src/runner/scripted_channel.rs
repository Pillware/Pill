//! The scripted test channel: the verbs, their dumps, and the injections.
//!
//! # Responsibilities
//!
//! - Read the commands a suite appends to `PILL_EDITOR_TEST_COMMANDS` and run
//!   each through [`run_scripted`](EditorWindow::run_scripted).
//! - Answer a command with a log line a suite can wait for, or refuse it
//!   through the one rejection token a suite treats as failure.
//! - Inject the input a suite cannot produce otherwise: keys, clicks and
//!   drags.
//!
//! # Design
//!
//! Development only. The verb grammar is `pill_editor_core::scripted`, which
//! the browser editor shares too. This module adds the native executable's
//! dispatch and its dumps: every verb runs the code a real press runs, and
//! every dump shows state that no press can reach.
//!
//! A line is refused when its own target is wrong: a handle no entity answers
//! to, a field that is not editable, a value the field's kind cannot read, or
//! a drag asked to start while another is still running. Every refusal prints
//! the one rejection token, so a suite's typo fails like a malformed verb
//! instead of waiting out a timeout. A correct line that arrives before the
//! shell module attaches is not refused: it warns or does nothing, and the
//! same line works a moment later.

// Current crate
use super::*;
use pill_editor_core::tools;
use pill_engine_core::gizmos::GizmoRegistry;

// =============================================================================
// Scripted Refusals
// =============================================================================

/// Refuses one scripted line, naming it and why it cannot run.
///
/// Every refusal prints the same token: an unknown verb, an unknown target, a
/// missing or malformed argument, or a handle that resolves to nothing. A
/// suite can then tell a rejected line from a slow answer without matching
/// every message, and `EditorSession` raises on the token at once instead of
/// timing out.
fn reject_scripted_command(command: &str, reason: &str) {
    warn!(
        target: telemetry_target::EDITOR,
        command,
        reason,
        "[editor] scripted command rejected"
    );
}

/// Refuses a failed scripted step through [`reject_scripted_command`].
///
/// A handler that answers `Result<(), String>` fails for a reason about the
/// line itself: a handle that resolves to nothing, a field that is not
/// editable, or a value the field's kind cannot read. Neither this function
/// nor [`reject_scripted_command`] takes the window, so an arm of
/// [`EditorWindow::run_scripted`] can pass a handler's result here without
/// borrowing the window a second time.
fn reject_on_failure(line: &str, outcome: Result<(), String>) {
    if let Err(reason) = outcome {
        reject_scripted_command(line, &reason);
    }
}

// =============================================================================
// EditorWindow
// =============================================================================

impl EditorWindow {
    /// Advances a scripted drag by one frame: first the press, then the move,
    /// then the release. The line it logs on the release tells a suite that
    /// the drag is over and the pointer is free again.
    ///
    /// A drag that starts inside a scene viewport is a scene gesture as well
    /// as a UI one: its press, move and release reach the viewport's own
    /// handlers, which is how a suite drags a handle or a marquee. The
    /// viewport is decided at the press and every later stage keeps it,
    /// because the pointer may leave the viewport mid-drag.
    pub(super) fn advance_scripted_drag(&mut self) {
        let Some(drag) = self.scripted_drag.as_ref() else {
            return;
        };
        let (from, to, stage) = (drag.from, drag.to, drag.stage);
        let scene_view = self.scripted_scene_view(from);
        match stage {
            ScriptedDragStage::Press => {
                self.queue_input(InputEvent::CursorMoved {
                    position: pill_core::math::Vector2f::new(from.0, from.1),
                });
                self.queue_input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Pressed,
                });
                if let Some(view) = scene_view {
                    self.activate_view(view);
                    let (origin_x, origin_y) = self.view_origin(view);
                    self.press_scene(view, from.0 - origin_x, from.1 - origin_y);
                }
                if let Some(drag) = self.scripted_drag.as_mut() {
                    drag.stage = ScriptedDragStage::Move;
                }
            }
            ScriptedDragStage::Move => {
                self.queue_input(InputEvent::CursorMoved {
                    position: pill_core::math::Vector2f::new(to.0, to.1),
                });
                if let Some(view) = scene_view {
                    let (origin_x, origin_y) = self.view_origin(view);
                    self.scene_pointer_moved(view, (to.0 - origin_x, to.1 - origin_y));
                }
                if let Some(drag) = self.scripted_drag.as_mut() {
                    drag.stage = ScriptedDragStage::Release;
                }
            }
            ScriptedDragStage::Release => {
                self.queue_input(InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Released,
                });
                if let Some(view) = scene_view {
                    self.release_scene(view);
                }
                self.scripted_drag = None;
                info!(
                    target: telemetry_target::EDITOR,
                    from_x = from.0,
                    from_y = from.1,
                    to_x = to.0,
                    to_y = to.1,
                    "[editor] pointer drag injected"
                );
            }
        }
    }

    /// The scene view a scripted point in the main window falls in, when the
    /// point is not also over the shell's own UI.
    ///
    /// A point over an overlay region belongs to the UI, exactly as it does
    /// for a scripted click, so the drag is not a scene gesture there.
    fn scripted_scene_view(&self, point: (f32, f32)) -> Option<usize> {
        let view = self.scene_view_at(point.0, point.1)?;
        let scale_factor = self.scale_factor;
        let over_ui = self
            .overlay_regions
            .iter()
            .any(|region| region.contains(point.0 / scale_factor, point.1 / scale_factor));
        (!over_ui).then_some(view)
    }

    /// Logs the asset browser's rows: the count first, then one line per row
    /// with its kind and flags. A suite reads the listing from these lines.
    pub(super) fn dump_assets(&mut self) {
        info!(
            target: telemetry_target::EDITOR,
            count = self.asset_entries.len(),
            "[test] assets"
        );
        for row in &self.asset_entries {
            // The suites' patterns read the other fields first, in this order,
            // so the root and the read-only flag come last.
            info!(
                target: telemetry_target::EDITOR,
                path = row.key().as_str(),
                kind = row.kind.text(),
                standalone = row.standalone,
                has_meta = row.has_meta,
                root = row.root.as_str(),
                read_only = row.read_only,
                "[test] asset"
            );
        }
    }

    /// Logs the Assets tab's open folder and the rows its grid lists there.
    /// A suite can check tree and grid navigation from these lines, with no
    /// pixel clicks.
    pub(super) fn dump_browser(&mut self) {
        let folder = {
            let world = self.host.host_mut().engine_mut().world_mut();
            match world.get_resource::<EguiShellState>() {
                Some(state) => state.browser.folder.clone(),
                None => String::new(),
            }
        };
        let browser_rows = pill_editor_core::browser::children(&self.asset_entries, &folder);
        info!(
            target: telemetry_target::EDITOR,
            folder = folder.as_str(),
            count = browser_rows.len(),
            "[test] browser"
        );
        for row in browser_rows {
            info!(
                target: telemetry_target::EDITOR,
                path = row.path.as_str(),
                kind = row.kind.text(),
                "[test] browser item"
            );
        }
    }

    /// Logs the redraw counters, for the suites.
    ///
    /// Two dumps separated by an idle stretch show which redraw mode the loop
    /// is in. When the loop waits instead of presenting every frame, the frame
    /// count grows at the idle poll's pace, not at the display's.
    pub(super) fn dump_cadence(&mut self) {
        let paused = self.host.host().engine().systems_paused();
        info!(
            target: telemetry_target::EDITOR,
            continuous = self.cadence.continuous(),
            frames = self.cadence.frames(),
            demanded = self.cadence.demanded(),
            waits = self.cadence.waits(),
            paused,
            input = self.last_demand.input,
            scripted = self.last_demand.scripted,
            snapshot = self.last_demand.snapshot,
            resized = self.last_demand.resized,
            repaint = self.last_demand.repaint_requested,
            animating = self.last_demand.animating,
            "[test] cadence"
        );
    }

    /// Logs the Console tab's filters, the counts they leave and the first
    /// visible row, for the suites.
    pub(super) fn dump_console(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(console_state) = world.get_resource::<ConsoleState>() else {
            return;
        };
        let view = console_state.view();
        // The per-channel counts prove that a project's own line reached the
        // history. The game channel only fills when something outside the
        // engine logs.
        let engine = console_state
            .lines
            .iter()
            .filter(|line| line.channel == ConsoleChannel::Engine)
            .count();
        let game = console_state.lines.len() - engine;
        let first = view
            .lines
            .first()
            .map(|line| format!("{} {}: {}", line.level_name(), line.target, line.message))
            .unwrap_or_default();
        info!(
            target: telemetry_target::EDITOR,
            level = pill_editor_core::console::level_name(view.minimum_level),
            channel = view.channel.map_or("both", ConsoleChannel::name),
            search = view.search.as_str(),
            total = view.total,
            visible = view.visible,
            engine,
            game,
            first = first.as_str(),
            "[test] console"
        );
    }

    /// Logs the store's own assets: the ones no file backs. Every picker
    /// offers them beside the listing's rows, so a suite can check what a
    /// picker reaches beyond the project's `res`.
    pub(super) fn dump_engine_assets(&mut self) {
        let engine_assets = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world
                .get_resource::<EditorSnapshot>()
                .map(|snapshot| snapshot.engine_assets.clone())
                .unwrap_or_default()
        };
        info!(
            target: telemetry_target::EDITOR,
            count = engine_assets.len(),
            "[test] engine assets"
        );
        for row in &engine_assets {
            info!(
                target: telemetry_target::EDITOR,
                kind = row.kind.as_str(),
                name = row.name.as_str(),
                guid = row.guid.as_deref().unwrap_or(""),
                "[test] engine asset"
            );
        }
    }

    /// Logs the entity list and each row's component names, for the suites. A
    /// collection test finds its subject this way.
    pub(super) fn dump_entities(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let entities = world
            .get_resource::<EditorSnapshot>()
            .map(|snapshot| snapshot.entities.clone())
            .unwrap_or_default();
        info!(
            target: telemetry_target::EDITOR,
            count = entities.len(),
            "[test] entities"
        );
        for entry in &entities {
            info!(
                target: telemetry_target::EDITOR,
                display = entry.display.as_str(),
                components = entry.archetype_label.as_str(),
                "[test] entity"
            );
        }
    }

    /// Logs one component field's value, for the scripted
    /// `dump field <entity> <component> <field>`. The entity is a display
    /// handle, and the component name may be abbreviated to its `::Name`
    /// suffix.
    pub(super) fn dump_field(&mut self, entity_text: &str, component: &str, field: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let resolved_target = resolve_display_entity(world, entity_text)
            .and_then(|entity| resolve_component_name(world, component).map(|name| (entity, name)));
        let Some((entity, component)) = resolved_target else {
            warn!(
                target: telemetry_target::EDITOR,
                entity = entity_text,
                component,
                field,
                "[test] field unreadable"
            );
            return;
        };
        match world.read_component_field(entity, &component, field) {
            Ok(value) => info!(
                target: telemetry_target::EDITOR,
                value = ?value,
                "[test] field"
            ),
            Err(_) => warn!(
                target: telemetry_target::EDITOR,
                entity = entity_text,
                component = component.as_str(),
                field,
                "[test] field unreadable"
            ),
        }
    }

    /// Logs the scene grid's state: whether it is on, its cell size, and the
    /// surfaces the renderer draws it on. This is the same resolution the
    /// renderer makes from the same bindings.
    pub(super) fn dump_grid(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let cameras = world.get_resource::<SurfaceCameras>();
        let grid = cameras.map(|cameras| cameras.grid).unwrap_or_default();
        // The renderer draws the grid on the frame's view-bound editor views,
        // and only while the switch is on. With the switch off, the log line
        // carries no surfaces.
        let surface_ids_text = match cameras {
            Some(cameras) if grid.enabled => editor_view_surfaces(cameras)
                .iter()
                .map(|surface| surface.0.to_string())
                .collect::<Vec<_>>()
                .join(","),
            _ => String::new(),
        };
        info!(
            target: telemetry_target::EDITOR,
            enabled = grid.enabled,
            cell = grid.cell_size,
            surfaces = %surface_ids_text,
            "[test] scene grid"
        );
    }

    /// Logs every registered component gizmo: its type, when it draws, whether
    /// an editor hid it, and how many entities it drew in the last frame.
    ///
    /// The line also names the surfaces that show the drawing: the frame's
    /// editor views, which are the surfaces the renderer draws chrome on. A
    /// game's own surface never is one, so the game view never shows a gizmo.
    pub(super) fn dump_gizmos(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let surface_ids_text = world
            .get_resource::<SurfaceCameras>()
            .map(|cameras| {
                editor_view_surfaces(cameras)
                    .iter()
                    .map(|surface| surface.0.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default();
        let Some(registry) = world.get_resource::<GizmoRegistry>() else {
            info!(
                target: telemetry_target::EDITOR,
                count = 0,
                hidden = 0,
                enabled = false,
                surfaces = %surface_ids_text,
                "[test] gizmos"
            );
            return;
        };
        info!(
            target: telemetry_target::EDITOR,
            count = registry.len(),
            hidden = registry.hidden_types().count(),
            enabled = registry.is_enabled(),
            surfaces = %surface_ids_text,
            "[test] gizmos"
        );
        for entry in registry.entries() {
            info!(
                target: telemetry_target::EDITOR,
                gizmo_type = %entry.type_name,
                owner = %entry.owner,
                when = %entry.when.label(),
                visible = registry.is_type_visible(&entry.type_name),
                drew = entry.drew_last_frame(),
                "[test] gizmo"
            );
        }
    }

    /// Logs the play session's state: the run mode, the pause flag and the
    /// world's restore epoch, which moves on every Stop.
    pub(super) fn dump_play(&mut self) {
        let engine = self.host.host_mut().engine_mut();
        let mode = match engine.run_mode() {
            pill_engine_core::RunMode::Editing => "editing",
            pill_engine_core::RunMode::Playing => "playing",
        };
        // The history's depths are part of the session's contract: the
        // document's stack goes aside at Play and comes back at Stop, so a
        // suite compares these across a session.
        let (undo, redo) = engine
            .world()
            .get_resource::<CommandHistory>()
            .map(|history| (history.undo_depth(), history.redo_depth()))
            .unwrap_or((0, 0));
        info!(
            target: telemetry_target::EDITOR,
            mode,
            paused = engine.systems_paused(),
            restore_epoch = engine.world().restore_epoch(),
            undo,
            redo,
            "[test] play"
        );
    }

    /// Logs the module panels, for the suites.
    pub(super) fn dump_panels(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(registry) = world.get_resource::<PanelRegistry>() else {
            return;
        };
        let panel_budget = world
            .get_resource::<EguiShellState>()
            .map(|state| state.panel_budget.clone())
            .unwrap_or_default();
        info!(
            target: telemetry_target::EDITOR,
            count = registry.len(),
            live = registry
                .entries()
                .iter()
                .filter(|entry| entry.alive())
                .count(),
            "[test] panels"
        );
        for (slot, entry) in registry.entries().iter().enumerate() {
            let timing = panel_budget.get(slot as u16);
            info!(
                target: telemetry_target::EDITOR,
                slot,
                id = entry.id.as_str(),
                source = entry.source.as_str(),
                alive = entry.alive(),
                epoch = entry.epoch,
                micros = timing.map(|timing| timing.micros).unwrap_or(0.0),
                passes = timing.map(|timing| timing.passes).unwrap_or(0),
                slow = panel_budget.is_slow(slot as u16),
                title = registry.tab_title(slot as u16).as_str(),
                "[test] panel"
            );
        }
    }

    /// Logs the preferences file's rows as the Preferences tab's view holds
    /// them, then the project key and whether the file remembers it. This is
    /// the tab's exact data, so a suite can check a save or a tool choice
    /// without reading the tab's pixels.
    pub(super) fn dump_preferences(&mut self) {
        let view = PreferencesView::from_document(
            &self.preferences_document,
            self.gizmo_project.as_deref(),
        );
        info!(
            target: telemetry_target::EDITOR,
            keymap_preset = view.text("keymap_preset"),
            code_editor_command = view.text("code_editor_command"),
            autosave_interval_seconds = view.text("autosave_interval_seconds"),
            snapping_translation = view.text("snapping_steps.translation"),
            snapping_rotation_degrees = view.text("snapping_steps.rotation_degrees"),
            snapping_scale = view.text("snapping_steps.scale"),
            history_step_cap = view.text("history_step_cap"),
            history_memory_budget_mb = view.text("history_memory_budget_mb"),
            "[test] preferences"
        );
        info!(
            target: telemetry_target::EDITOR,
            project = view.project.as_deref().unwrap_or("-"),
            remembered = view.project_remembered,
            mode = ?view.project_gizmo.mode,
            space = ?view.project_gizmo.space,
            snap = ?view.project_gizmo.snap,
            "[test] preferences project"
        );
    }

    /// Log the Problems count and the first forty rows, for the suites.
    ///
    /// Each row carries its index in the panel, so a suite can open the row it
    /// matched instead of assuming which row came first. The window is wide
    /// enough that a build's own warnings - the shared workspace's dead code
    /// and unused imports, reported before a project's error - cannot push
    /// that error past the end of the dump.
    pub(super) fn dump_problems(&mut self) {
        info!(
            target: telemetry_target::EDITOR,
            count = self.notices.problems.len(),
            "[test] problems"
        );
        for (index, row) in self.notices.problems.iter().take(40).enumerate() {
            info!(
                target: telemetry_target::EDITOR,
                module = row.module.as_str(),
                severity = row.problem.severity.name(),
                file = row.problem.file.as_deref().unwrap_or(""),
                line = row.problem.line.unwrap_or(0),
                index = index,
                text = row.problem.message.as_str(),
                "[test] problem row"
            );
        }
    }

    /// Logs the editable resources and their values, for the suites.
    pub(super) fn dump_resources(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let resources = world
            .get_resource::<EditorSnapshot>()
            .map(|snapshot| snapshot.resources.clone())
            .unwrap_or_default();
        info!(
            target: telemetry_target::EDITOR,
            count = resources.len(),
            "[test] resources"
        );
        for view in &resources {
            info!(
                target: telemetry_target::EDITOR,
                name = view.name.as_str(),
                label = view.label.as_str(),
                fields = view.fields.len(),
                "[test] resource"
            );
            for field in &view.fields {
                info!(
                    target: telemetry_target::EDITOR,
                    resource = view.name.as_str(),
                    field = field.field.as_str(),
                    label = field.label.as_str(),
                    value = inspector::value_text(&field.value).as_str(),
                    "[test] resource field"
                );
            }
        }
    }

    /// Logs the placed rectangle of every scene view, in the main window's
    /// physical pixels, for the suite's scripted clicks. The game view's
    /// rectangle follows when the game window exists.
    pub(super) fn dump_scenes(&mut self) {
        for view in 0..self.views.len() {
            let Some(scene) = &self.views[view] else {
                continue;
            };
            let (origin_x, origin_y) = self.view_origin(view);
            let (width, height) = scene.window.size();
            debug!(
                target: telemetry_target::EDITOR,
                view,
                x = origin_x as i32,
                y = origin_y as i32,
                width,
                height,
                visible = scene.window.visible,
                "[editor] scene view rect"
            );
        }
        if let Some(game) = &self.game_surface {
            let (width, height) = game.size();
            debug!(
                target: telemetry_target::EDITOR,
                x = game.origin.0,
                y = game.origin.1,
                width,
                height,
                visible = game.visible,
                "[editor] game view rect"
            );
        }
    }

    /// Logs the selected entities, primary last, the selected asset and the
    /// selected elements (`dump selection`).
    ///
    /// An element row carries the world translation the active tool reports for
    /// it, which is how a suite reads what a drag wrote. An element no tool
    /// reports has no transform to show, and its three numbers are `nan`.
    pub(super) fn dump_selection(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let (entities, asset, element_kind, elements) = world
            .get_resource::<EditorSelection>()
            .map(|selection| {
                (
                    selection.entities().to_vec(),
                    selection.asset.clone(),
                    selection.elements().kind().to_string(),
                    selection.elements().elements().to_vec(),
                )
            })
            .unwrap_or_default();
        // A copy of the registry, so the report can read the world it lives in.
        let reported: Vec<tools::ToolElement> = match (
            world.get_resource::<tools::ToolRegistry>().cloned(),
            elements.first(),
        ) {
            (Some(registry), Some(first)) => registry.active_elements(first.entity, world),
            _ => Vec::new(),
        };
        info!(
            target: telemetry_target::EDITOR,
            count = entities.len(),
            asset = asset.as_deref().unwrap_or("none"),
            primary = entities
                .last()
                .map(|entity| entity.to_string())
                .unwrap_or_else(|| "none".to_string())
                .as_str(),
            elements = elements.len(),
            "[test] selection"
        );
        for entity in &entities {
            info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                "[test] selected entity"
            );
        }
        for element in &elements {
            let translation = reported
                .iter()
                .find(|row| row.id == element.id)
                .map(|row| row.transform.translation)
                .unwrap_or([f32::NAN; 3]);
            info!(
                target: telemetry_target::EDITOR,
                entity = %element.entity,
                id = element.id,
                kind = element_kind.as_str(),
                x = translation[0],
                y = translation[1],
                z = translation[2],
                "[test] selected element"
            );
        }
    }

    /// Logs the live standalone types the create dialog offers. A suite reads
    /// the first one to drive a create.
    pub(super) fn dump_standalone_types(&mut self) {
        for standalone_type in &self.standalone_types {
            info!(
                target: telemetry_target::EDITOR,
                type_name = standalone_type.type_name.as_str(),
                display = standalone_type.display_name.as_str(),
                extension = standalone_type.extension.as_str(),
                "[test] standalone type"
            );
        }
    }

    /// Logs the Statistics tab's numbers, for the suites.
    pub(super) fn dump_stats(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(shell_state) = world.get_resource::<EguiShellState>() else {
            return;
        };
        let stats = &shell_state.stats;
        info!(
            target: telemetry_target::EDITOR,
            fps = stats.frames_per_second,
            shell_us = stats.shell_micros,
            passes = stats.shell_passes,
            tabs = stats.tabs.len(),
            "[test] stats"
        );
        for entry in &stats.tabs {
            info!(
                target: telemetry_target::EDITOR,
                name = entry.tab.name(),
                us = entry.micros,
                passes = entry.passes,
                "[test] stats tab"
            );
        }
    }

    /// Logs every registered system with the state the Systems tab draws, and
    /// puts the engine's pause flag on the count line. This is the tab's exact
    /// data, so a suite can check a toggle or a pause without reading the
    /// tab's pixels.
    pub(super) fn dump_systems(&mut self) {
        let (systems, paused) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            match world.get_resource::<EditorSnapshot>() {
                Some(snapshot) => (snapshot.systems.clone(), snapshot.paused),
                None => (Vec::new(), false),
            }
        };
        info!(
            target: telemetry_target::EDITOR,
            count = systems.len(),
            paused,
            "[test] systems"
        );
        for system in &systems {
            info!(
                target: telemetry_target::EDITOR,
                index = system.index,
                name = system.name.as_str(),
                owner = system.owner_label.as_str(),
                enabled = system.enabled,
                hot_patchable = system.hot_patchable,
                ambiguous = system.name_is_ambiguous,
                "[test] system"
            );
        }
    }

    /// Logs the dock's layout for the suite's assertions: the surface count,
    /// one line per leaf and one line per popped-out tab.
    pub(super) fn dump_tabs(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(shell_state) = world.get_resource::<EguiShellState>() else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] dump needs the shell module's state"
            );
            return;
        };
        let surfaces = shell_state.dock.state().surfaces_count();
        info!(
            target: telemetry_target::EDITOR,
            surfaces,
            "[editor] egui dock dump"
        );
        // One line per leaf, so a suite can tell a restored layout from the
        // default. A leaf line names the tabs its group holds and which one is
        // in front.
        let leaves: Vec<(usize, usize, String, String)> = shell_state
            .dock
            .state()
            .iter_all_nodes()
            .filter_map(|(path, node)| match node {
                pill_editor_core::egui_dock::Node::Leaf(leaf) => {
                    let tabs = leaf
                        .tabs
                        .iter()
                        .map(|tab| tab.name())
                        .collect::<Vec<_>>()
                        .join(",");
                    let active = leaf
                        .tabs
                        .get(leaf.active.0)
                        .map_or_else(String::new, |tab| tab.name().to_string());
                    Some((path.surface.0, path.node.0, active, tabs))
                }
                _ => None,
            })
            .collect();
        for (surface, node, active, tabs) in leaves {
            info!(
                target: telemetry_target::EDITOR,
                surface,
                node,
                active = %active,
                tabs = %tabs,
                "[editor] egui dock leaf"
            );
        }
        // Popped-out tabs are not in the tree, so they get their own lines.
        // A suite reads which tabs left the dock and which came back.
        let popouts: Vec<String> = shell_state
            .dock
            .popouts()
            .iter()
            .map(|tab| tab.name())
            .collect();
        for tab in popouts {
            info!(
                target: telemetry_target::EDITOR,
                tab = tab.as_str(),
                "[editor] egui dock popout"
            );
        }
    }

    /// Logs the theme the editor paints with: the layers it was merged from,
    /// the generation that produced it, and the accent, panel and corner
    /// values it resolved (`dump theme`).
    ///
    /// These are the values the executable hands the shell, so a suite can
    /// check a layer change without reading the tab's pixels.
    pub(super) fn dump_theme(&mut self) {
        let user_layer_path = self
            .theme_layers
            .iter()
            .find(|layer_info| layer_info.layer == ThemeLayer::User)
            .map_or("none", ThemeLayerInfo::path_text);
        let project_layer_path = self
            .theme_layers
            .iter()
            .find(|layer_info| layer_info.layer == ThemeLayer::Project)
            .map_or("none", ThemeLayerInfo::path_text);
        info!(
            target: telemetry_target::EDITOR,
            layers = %layers_text(&self.theme_layers),
            user = %user_layer_path,
            project = %project_layer_path,
            generation = self.theme_generation,
            accent = %self.theme.palette.accent.hex(),
            panel = %self.theme.palette.panel.hex(),
            corner = self.theme.shape.corner_radius,
            "[test] theme"
        );
    }

    /// Logs the active viewport, the interaction state, the hovered handle and
    /// every view's camera. These are the inputs to navigation and viewport
    /// routing, and a suite can read them with no pixel clicks.
    pub(super) fn dump_viewport(&mut self) {
        let hovered_handle_text = self
            .hovered_handle
            .map(|handle| handle.to_string())
            .unwrap_or_else(|| "-".to_string());
        // The shell's own frame counter. A suite that waits for "a frame ran
        // since N" asks for one dump instead of waiting for the rows
        // heartbeat, which prints once every 210 frames.
        let frame = self
            .host
            .host_mut()
            .engine_mut()
            .world_mut()
            .get_resource::<EguiShellState>()
            .map_or(0, |state| state.frame_index);
        info!(
            target: telemetry_target::EDITOR,
            active = self.active_view,
            interaction = self.interaction_name(),
            hovered = hovered_handle_text.as_str(),
            keymap = %self.keymap,
            frame,
            "[test] viewport"
        );
        // The value a callback reads: the same numbers, published as a
        // resource at the frame boundary. A suite that moves the pointer and
        // dumps again proves the publish, not just the runner's own fields.
        let published = self
            .host
            .host_mut()
            .engine_mut()
            .world()
            .get_resource::<EditorViewportState>()
            .copied()
            .unwrap_or_default();
        let pointer_text = published
            .pointer_in_view
            .map(|(x, y)| format!("{x:.1},{y:.1}"))
            .unwrap_or_else(|| "-".to_string());
        let rect_text = published
            .view_rect
            .map(|rect| {
                format!(
                    "{:.1},{:.1},{:.1},{:.1}",
                    rect.x, rect.y, rect.width, rect.height
                )
            })
            .unwrap_or_else(|| "-".to_string());
        info!(
            target: telemetry_target::EDITOR,
            active = published.active_view.map_or(-1, |view| view as i64),
            pointer = pointer_text.as_str(),
            rect = rect_text.as_str(),
            views = published.view_count,
            "[test] viewport state"
        );
        for view in 0..self.views.len() {
            let camera = self.cameras[view];
            let eye = camera.eye();
            let pivot = camera.pivot;
            info!(
                target: telemetry_target::EDITOR,
                view,
                eye = format!("{:.3},{:.3},{:.3}", eye[0], eye[1], eye[2]).as_str(),
                pivot = format!("{:.3},{:.3},{:.3}", pivot[0], pivot[1], pivot[2]).as_str(),
                distance = camera.distance,
                yaw = camera.yaw,
                pitch = camera.pitch,
                "[test] viewport camera"
            );
        }
    }

    /// Logs the panel windows on screen, for the suites.
    pub(super) fn dump_windows(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let windows = world
            .get_resource::<EguiShellState>()
            .map(|state| state.panel_windows.windows.clone())
            .unwrap_or_default();
        info!(
            target: telemetry_target::EDITOR,
            count = windows.len(),
            "[test] windows"
        );
        for window in &windows {
            info!(
                target: telemetry_target::EDITOR,
                id = window.id.as_str(),
                source = window.source.as_str(),
                slot = window.slot,
                modal = window.modal,
                "[test] window"
            );
        }
    }

    /// Injects a full left click at a point of the window's client area, in
    /// its physical pixels.
    ///
    /// The click enters through the same input path as a real one: the shell's
    /// egui pass hit-tests it and routes it, so an overlay widget takes it,
    /// while a point with no widget reaches the scene. Press and release
    /// arrive in one input frame, which the engine reports as both edges, and
    /// the egui bridge forwards them in order.
    pub(super) fn inject_click(&mut self, x: f32, y: f32) {
        // Step 1: Queue the press and the release, as one input frame.
        let position = pill_core::math::Vector2f::new(x, y);
        self.queue_input(InputEvent::CursorMoved { position });
        self.queue_input(InputEvent::MouseButton {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
        });
        self.queue_input(InputEvent::MouseButton {
            button: MouseButton::Left,
            state: ButtonState::Released,
        });
        info!(
            target: telemetry_target::EDITOR,
            x,
            y,
            "[editor] click injected"
        );
        // Step 2: A click inside a visible scene viewport activates that view
        // and asks what is under the point, through the same press a window
        // event sends. The pick's answer moves the shared selection, and the
        // outline follows it. A point over an overlay region - a chip, the
        // floating toolbox or an open menu - belongs to the UI, not the scene.
        if let Some(view) = self.scene_view_at(x, y) {
            let scale_factor = self.scale_factor;
            let over_ui = self
                .overlay_regions
                .iter()
                .any(|region| region.contains(x / scale_factor, y / scale_factor));
            if !over_ui {
                let (origin_x, origin_y) = self.view_origin(view);
                info!(
                    target: telemetry_target::EDITOR,
                    view,
                    x,
                    y,
                    "[editor] scene view click"
                );
                // A scripted click is a press and a release in one command.
                // The pick answers later, with the button already up, so a
                // handle under the point does not start a drag.
                self.activate_view(view);
                self.press_scene(view, x - origin_x, y - origin_y);
                self.release_scene(view);
            }
        }
    }

    /// Injects a keyboard chord through the same input path a real key enters:
    /// the modifiers, the press, the typed text and the release, in the order
    /// a window reports them.
    ///
    /// The editor's own tool shortcuts read winit events, not the engine's
    /// input stream, so the chord also runs the handler a real press runs.
    /// Without that, `key w` would reach the shell but never switch the tool.
    pub(super) fn inject_key(&mut self, chord: &KeyChord) {
        // Step 1: Queue the modifiers, the press, the typed text and the
        // release, in the order a window reports them.
        let held_modifiers = EngineModifiers {
            shift: chord.shift,
            control: chord.control,
            alt: chord.alt,
            super_key: chord.super_key,
        };
        self.queue_input(InputEvent::ModifiersChanged {
            modifiers: held_modifiers,
        });
        self.queue_input(InputEvent::Key {
            key: chord.key,
            state: ButtonState::Pressed,
        });
        if let Some(character) = chord.typed_character() {
            self.queue_input(InputEvent::TextInput {
                text: character.to_string(),
            });
        }
        self.queue_input(InputEvent::Key {
            key: chord.key,
            state: ButtonState::Released,
        });
        // The chord releases its modifiers as it ends, so a later injection
        // does not inherit a held Control from this one.
        self.queue_input(InputEvent::ModifiersChanged {
            modifiers: EngineModifiers {
                shift: false,
                control: false,
                alt: false,
                super_key: false,
            },
        });
        info!(
            target: telemetry_target::EDITOR,
            chord = chord.label().as_str(),
            "[editor] key injected"
        );
        // Step 2: Run the command the chord stands for. A real press runs the
        // same one: the command the keybinding registry resolves, with the
        // modifiers the chord named.
        self.run_bound_command(
            chord.key,
            EngineModifiers {
                shift: chord.shift,
                control: chord.control,
                alt: chord.alt,
                super_key: chord.super_key,
            },
        );
    }

    /// Injects a full right click at a point of the window's client area,
    /// through the same input path a real click enters.
    ///
    /// The shell's egui pass hit-tests and routes it, so a context menu opens
    /// over whatever is under the pointer. It never reaches the scene.
    pub(super) fn inject_rclick(&mut self, x: f32, y: f32) {
        let position = pill_core::math::Vector2f::new(x, y);
        self.queue_input(InputEvent::CursorMoved { position });
        self.queue_input(InputEvent::MouseButton {
            button: MouseButton::Right,
            state: ButtonState::Pressed,
        });
        self.queue_input(InputEvent::MouseButton {
            button: MouseButton::Right,
            state: ButtonState::Released,
        });
        info!(
            target: telemetry_target::EDITOR,
            x,
            y,
            "[editor] right click injected"
        );
    }

    /// Returns the viewport interaction state's name. Only one owner holds the
    /// pointer at a time, so the dump and the logs say who has it.
    pub(super) fn interaction_name(&self) -> &'static str {
        if self.camera_drag.is_some() {
            return "navigating";
        }
        match self.gizmo_drag.as_ref().map(|drag| drag.kind) {
            Some(HandleKind::Plane { .. }) => "dragging-plane",
            Some(HandleKind::Axis { .. }) => "dragging-axis",
            None if self.hovered_handle.is_some() => "hovering",
            None => "idle",
        }
    }

    /// Runs the scripted commands appended since the last frame.
    ///
    /// The grammar lives in [`pill_editor_core::scripted`]. This parses each
    /// line and dispatches it, and a line that does not parse is refused at
    /// once, so a suite sees the failure instead of waiting out a timeout.
    ///
    /// The commands a line queued are applied before the next line runs. An
    /// append reaches the loop as one batch, so without that order an append
    /// that edits and then reads would answer the value the edit replaced.
    pub(super) fn poll_test_channel(&mut self) {
        // Step 1: Read the batch the channel appended.
        let lines = match &mut self.test_channel {
            Some(channel) => channel.poll(),
            None => return,
        };
        // Step 2: Parse and run each line, then apply what it queued.
        for line in lines {
            // A scripted line arrives between frames. The loop must not stay
            // asleep over it.
            self.demand.scripted = true;
            match ScriptedCommand::parse(&line) {
                Ok(command) => self.run_scripted(&line, command),
                Err(error) => reject_scripted_command(&line, &error.reason),
            }
            self.apply_pending_commands();
        }
    }

    /// Queues one scripted event for the engine and for the shell's own list.
    ///
    /// A scripted click or key stands in for a user's, so the UI has to see it
    /// the way a real event arrives, through the shell's list. The game still
    /// receives what a user's event would.
    pub(super) fn queue_input(&mut self, event: InputEvent) {
        if let InputEvent::CursorMoved { position } = event {
            // The scripted pointer is the shell's pointer too, so the seed the
            // next frame carries must match what the shell just saw.
            self.shell_cursor = Some(position);
        }
        self.host
            .host_mut()
            .engine_mut()
            .push_input_event(event.clone());
        self.shell_events.push(event);
    }

    /// Runs one parsed scripted command.
    ///
    /// No grammar is left here: each arm names its side effect, so the verbs
    /// and their arguments have one home, the core parser.
    pub(super) fn run_scripted(&mut self, line: &str, command: ScriptedCommand) {
        match command {
            ScriptedCommand::Quit => {
                info!(target: telemetry_target::EDITOR, "[editor] quit requested");
                self.request_quit();
            }
            ScriptedCommand::Undock(tab) => self.undock_tab(tab),
            ScriptedCommand::ActivateTab(tab) => self.activate_tab(tab),
            ScriptedCommand::Palette(request) => self.scripted_palette(request),
            ScriptedCommand::ConsoleFilter(level) => self.scripted_console_filter(&level),
            ScriptedCommand::ConsoleSearch(text) => self.scripted_console_search(text),
            ScriptedCommand::ProblemOpen(index) => self.scripted_problem_open(index),
            ScriptedCommand::SafeModeRetry => self.retry_shell_module(),
            ScriptedCommand::PanelUnload(source) => self.scripted_panel_unload(&source),
            ScriptedCommand::PanelRestore(source) => self.scripted_panel_restore(&source),
            ScriptedCommand::PanelWindow { slot, id } => self.scripted_panel_window(slot, &id),
            ScriptedCommand::PanelField {
                component,
                field,
                value,
            } => reject_on_failure(line, self.scripted_panel_field(&component, &field, &value)),
            ScriptedCommand::Command { id, target } => {
                reject_on_failure(line, self.scripted_command_invocation(&id, &target))
            }
            ScriptedCommand::Reveal(path) => {
                let world = self.host.host_mut().engine_mut().world_mut();
                if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                    pending.reveal(path.clone());
                }
                info!(
                    target: telemetry_target::EDITOR,
                    path = path.as_str(),
                    "[editor] reveal queued"
                );
            }
            ScriptedCommand::PickReference {
                asset,
                key,
                candidate,
            } => {
                // This makes the choice a picker row makes: it writes the
                // candidate's own guid through the same settings edit the row
                // queues.
                let guid = self
                    .asset_entries
                    .iter()
                    .find(|row| row.path == candidate || row.key() == candidate)
                    .and_then(|row| row.guid.clone());
                match guid {
                    Some(guid) => {
                        info!(
                            target: telemetry_target::EDITOR,
                            key = key.as_str(),
                            candidate = candidate.as_str(),
                            guid = guid.as_str(),
                            "[editor] reference pick queued"
                        );
                        self.pick_reference(&asset, &key, &guid);
                    }
                    None => reject_scripted_command(
                        line,
                        "pick reference needs a candidate carrying a guid",
                    ),
                }
            }
            ScriptedCommand::Click { x, y } => self.inject_click(x, y),
            ScriptedCommand::RightClick { x, y } => self.inject_rclick(x, y),
            ScriptedCommand::Drag { from, to } => {
                reject_on_failure(line, self.start_scripted_drag(from, to))
            }
            ScriptedCommand::GizmoDrag { target, delta } => {
                match self.scripted_gizmo_handle(&target) {
                    Some(handle) => self.start_synthetic_gizmo_drag(handle, Some(delta)),
                    None => reject_scripted_command(line, "gizmo drag needs a known axis or plane"),
                }
            }
            ScriptedCommand::DragElement { id, delta } => {
                reject_on_failure(line, self.scripted_drag_element(id, delta))
            }
            ScriptedCommand::ElementMenu { item } => {
                reject_on_failure(line, self.scripted_element_menu(&item))
            }
            ScriptedCommand::GizmoPress { target } => match self.scripted_gizmo_handle(&target) {
                Some(handle) => self.start_synthetic_gizmo_drag(handle, None),
                None => reject_scripted_command(line, "gizmo press needs a known axis or plane"),
            },
            ScriptedCommand::GizmoRelease => reject_on_failure(line, self.scripted_gizmo_release()),
            ScriptedCommand::GizmoTool(mode) => self.set_gizmo_tool(mode),
            ScriptedCommand::Key(chord) => self.inject_key(&chord),
            ScriptedCommand::Dump(dump) => match dump {
                Dump::Tabs => self.dump_tabs(),
                Dump::Scenes => self.dump_scenes(),
                Dump::Viewport => self.dump_viewport(),
                Dump::Grid => self.dump_grid(),
                Dump::Gizmos => self.dump_gizmos(),
                Dump::Play => self.dump_play(),
                Dump::Assets => self.dump_assets(),
                Dump::EngineAssets => self.dump_engine_assets(),
                Dump::Browser => self.dump_browser(),
                Dump::StandaloneTypes => self.dump_standalone_types(),
                Dump::Systems => self.dump_systems(),
                Dump::Tools => self.dump_tools(),
                Dump::Elements => self.dump_elements(),
                Dump::Preferences => self.dump_preferences(),
                Dump::ProjectSettings => self.dump_project_settings(),
                Dump::Theme => self.dump_theme(),
                Dump::Console => self.dump_console(),
                Dump::Problems => self.dump_problems(),
                Dump::Stats => self.dump_stats(),
                Dump::Cadence => self.dump_cadence(),
                Dump::Panels => self.dump_panels(),
                Dump::Windows => self.dump_windows(),
                Dump::Resources => self.dump_resources(),
                Dump::Entities => self.dump_entities(),
                Dump::Selection => self.dump_selection(),
                Dump::Field {
                    entity,
                    component,
                    field,
                } => self.dump_field(&entity, &component, &field),
            },
            ScriptedCommand::Browser(folder) => self.scripted_browser(&folder),
            ScriptedCommand::Camera { navigation, delta } => {
                self.scripted_camera(navigation, delta.0, delta.1)
            }
            ScriptedCommand::CameraZoom(lines) => self.zoom_camera(self.active_view, lines),
            ScriptedCommand::Hover { view, x, y } => {
                reject_on_failure(line, self.scripted_hover(view, x, y))
            }
            ScriptedCommand::HoverUi { x, y } => self.scripted_hover_ui(x, y),
            ScriptedCommand::Drop(path) => self.drop_files(vec![std::path::PathBuf::from(path)]),
            ScriptedCommand::HoverFile(path) => self.preview_drop(std::path::Path::new(&path)),
            ScriptedCommand::DragOut(path) => self.start_drag_out(&path, false),
            ScriptedCommand::SelectAsset(path) => self.scripted_select_asset(&path),
            ScriptedCommand::SelectEntity(entity_text) => {
                reject_on_failure(line, self.scripted_select_entity(&entity_text))
            }
            ScriptedCommand::SelectAdd(entity_text) => reject_on_failure(
                line,
                self.scripted_select_extend(&entity_text, SelectionClick::Add),
            ),
            ScriptedCommand::SelectRange(entity_text) => reject_on_failure(
                line,
                self.scripted_select_extend(&entity_text, SelectionClick::Range),
            ),
            ScriptedCommand::SelectResource(name) => self.scripted_select_resource(&name),
            ScriptedCommand::Tool(name) => reject_on_failure(line, self.scripted_tool(name)),
            ScriptedCommand::Extensions { enabled } => self.scripted_extensions(enabled),
            ScriptedCommand::SelectElement { ids, add } => {
                reject_on_failure(line, self.scripted_select_elements(ids, add))
            }
            ScriptedCommand::EditResource {
                resource,
                field,
                value,
            } => reject_on_failure(line, self.scripted_edit_resource(&resource, &field, &value)),
            ScriptedCommand::Toolbar(action) => self.scripted_toolbar(action),
            ScriptedCommand::SceneNew => self.scripted_scene(EditorCommand::SceneNew),
            ScriptedCommand::SceneOpen(path) => {
                self.scripted_scene(EditorCommand::SceneOpen { path })
            }
            ScriptedCommand::SceneSave(path) => match path {
                Some(path) => self.scripted_scene(EditorCommand::SceneSaveAs { path }),
                None => self.scripted_scene(EditorCommand::SceneSave),
            },
            ScriptedCommand::DialogSetName(text) => self.scripted_dialog_set_name(&text),
            ScriptedCommand::DialogSubmit => self.scripted_dialog_submit(),
            ScriptedCommand::EditAsset(edit) => self.scripted_asset_edit(edit),
            ScriptedCommand::PickEntity {
                entity,
                component,
                field,
            } => reject_on_failure(line, self.scripted_pick_entity(&entity, &component, &field)),
            ScriptedCommand::EditSet {
                entity,
                component,
                field,
                value,
            } => reject_on_failure(
                line,
                self.scripted_edit_set(&entity, &component, &field, &value),
            ),
            ScriptedCommand::EditArray {
                entity,
                component,
                field,
                index,
                value,
            } => reject_on_failure(
                line,
                self.scripted_edit_array(&entity, &component, &field, index, &value),
            ),
            ScriptedCommand::EditSelection {
                component,
                field,
                value,
            } => reject_on_failure(
                line,
                self.scripted_edit_selection(&component, &field, &value),
            ),
            ScriptedCommand::EditAssetSettings { path, key, value } => {
                reject_on_failure(line, self.scripted_asset_settings(&path, &key, &value))
            }
            ScriptedCommand::EditSystem { index, enabled } => {
                self.scripted_edit_system(index, enabled)
            }
            ScriptedCommand::EditPreference { key, value } => {
                self.scripted_edit_preference(&key, &value)
            }
            ScriptedCommand::EditProjectSetting { path, value, local } => reject_on_failure(
                line,
                self.scripted_edit_project_setting(&path, &value, local),
            ),
            ScriptedCommand::ResetProjectSetting { path } => {
                reject_on_failure(line, self.scripted_reset_project_setting(&path))
            }
            ScriptedCommand::EditComponent {
                entity,
                component,
                add,
            } => reject_on_failure(line, self.scripted_component_edit(&entity, &component, add)),
            ScriptedCommand::Undo => self.scripted_undo(),
            ScriptedCommand::Redo => self.scripted_redo(),
            ScriptedCommand::Copy => self.scripted_clipboard(ClipboardAction::Copy),
            ScriptedCommand::Cut => self.scripted_clipboard(ClipboardAction::Cut),
            ScriptedCommand::Paste => self.scripted_clipboard(ClipboardAction::Paste),
            ScriptedCommand::PauseToggle => self.scripted_pause_toggle(),
            ScriptedCommand::Play => self.scripted_play(),
            ScriptedCommand::Stop => self.scripted_stop(),
            ScriptedCommand::PlayToggle => self.scripted_play_toggle(),
            ScriptedCommand::NewSceneView => {
                reject_on_failure(line, self.scripted_new_scene_view())
            }
            ScriptedCommand::Popout { tab, redock } => {
                reject_on_failure(line, self.scripted_popout(tab, redock))
            }
            ScriptedCommand::Step => self.scripted_step(),
            ScriptedCommand::EntityCreate => self.scripted_entity_create(),
            ScriptedCommand::EntityDelete => self.scripted_entity_delete(),
            ScriptedCommand::SnapToggle => {
                let snap = self.gizmo_settings().snap;
                self.scripted_gizmo_tool(None, None, Some(!snap))
            }
            ScriptedCommand::SpaceToggle => self.scripted_space_cycle(),
        }
    }

    /// Queues one `edit asset` command as the recorded edit the Assets tab's
    /// actions also queue.
    pub(super) fn scripted_asset_edit(&mut self, edit: AssetEdit) {
        let (edit_command, label) = match edit {
            AssetEdit::Create {
                type_name,
                folder,
                name,
            } => (
                EditorCommand::CreateStandaloneAsset {
                    folder: if folder == "-" { String::new() } else { folder },
                    name: name.clone(),
                    type_name,
                    overwrite: None,
                },
                format!("create asset {name}"),
            ),
            AssetEdit::Delete { path } => (
                EditorCommand::DeleteAssetFile { path: path.clone() },
                format!("delete asset {path}"),
            ),
            AssetEdit::Move { from, to } => (
                EditorCommand::MoveAssetFile {
                    from: from.clone(),
                    to,
                },
                format!("move asset {from}"),
            ),
            AssetEdit::Meta { path } => (
                EditorCommand::WriteAssetMeta { path: path.clone() },
                format!("write meta {path}"),
            ),
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(vec![edit_command], label, None);
        }
    }

    /// Runs one scripted asset-settings edit (`edit asset-settings <path>
    /// <key> <value>`): the same recorded `SetAssetSettings` batch the
    /// Inspector's settings rows queue. The text is parsed against the leaf's
    /// current JSON kind.
    ///
    /// # Errors
    ///
    /// Returns a message when the key path is malformed, when no asset at
    /// `path` has a settings document, or when the text is not a value of the
    /// leaf's kind or the path cannot be reached in the document.
    pub(super) fn scripted_asset_settings(
        &mut self,
        path: &str,
        key: &str,
        value_text: &str,
    ) -> Result<(), String> {
        let key_path = inspector::parse_settings_key_path(key)?;
        let Some(document) = self.read_asset_settings(path) else {
            return Err(format!("no asset at '{path}' has a settings document"));
        };
        // A leaf the document does not hold yet is read as text. That happens
        // for a slot with no reference yet, whose picker value is a guid
        // string.
        let kind = inspector::settings_field(&document, &key_path)
            .cloned()
            .unwrap_or_else(|| serde_json::Value::String(String::new()));
        let parsed_value = inspector::parse_settings_value(value_text, &kind)?;
        let mut patched_document = document;
        // The same inserting rule the Inspector's settings rows use.
        inspector::patch_settings_inserting(&mut patched_document, &key_path, parsed_value)?;
        self.queue_settings_value(path, key, patched_document);
        Ok(())
    }

    /// Opens a folder in the Assets tab's browser (`root` opens the asset
    /// root), through the shell's session state. It stands in for a click in
    /// the folder tree.
    pub(super) fn scripted_browser(&mut self, folder: &str) {
        let folder = if folder == "root" { "" } else { folder };
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(shell_state) = world.get_resource_mut::<EguiShellState>() else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] browser navigation needs the shell module's state"
            );
            return;
        };
        if !shell_state.browser.open(folder) {
            info!(
                target: telemetry_target::EDITOR,
                folder,
                "[editor] browser folder unchanged"
            );
        }
    }

    /// Applies a scripted camera gesture to the active viewport, through the
    /// same math a drag uses.
    pub(super) fn scripted_camera(
        &mut self,
        navigation: CameraNavigation,
        delta_x: f32,
        delta_y: f32,
    ) {
        let view = self.active_view;
        self.apply_camera_gesture(view, navigation, delta_x, delta_y);
        info!(
            target: telemetry_target::EDITOR,
            view,
            navigation = camera_navigation_name(navigation),
            "[editor] camera gesture applied"
        );
    }

    /// Runs one scripted clipboard command (`command editor.copy/cut/paste`).
    /// It queues the same request the Edit menu's rows queue, so the test
    /// channel drives the menu's own path.
    pub(super) fn scripted_clipboard(&mut self, action: ClipboardAction) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let selection = world
            .get_resource::<EditorSelection>()
            .cloned()
            .unwrap_or_default();
        let snapshot = world
            .get_resource::<EditorSnapshot>()
            .cloned()
            .unwrap_or_default();
        let summary = match world.get_resource_mut::<EditorClipboard>() {
            Some(clipboard) => clipboard::queue_request(clipboard, action, &selection, &snapshot),
            None => None,
        };
        // The cut's delete is queued in its own borrow, because the two
        // resources cannot be held mutably at once.
        if action == ClipboardAction::Cut {
            if let (Some((commands, label)), Some(pending)) = (
                clipboard::cut_commands(&selection),
                world.get_resource_mut::<PendingEditorCommands>(),
            ) {
                pending.push_recorded(commands, label, None);
            }
        }
        info!(
            target: telemetry_target::EDITOR,
            action = match action {
                ClipboardAction::Copy => "copy",
                ClipboardAction::Cut => "cut",
                ClipboardAction::Paste => "paste",
            },
            payload = summary.as_deref().unwrap_or("nothing"),
            "[editor] clipboard queued"
        );
    }

    /// Runs `edit add <entity> <component>` or `edit remove <entity>
    /// <component>` from the scripted channel. It queues the same recorded
    /// component attach or detach the Inspector's menu items queue.
    ///
    /// # Errors
    ///
    /// Returns a message when the display handle names no entity or the
    /// component name resolves to no component.
    pub(super) fn scripted_component_edit(
        &mut self,
        entity_text: &str,
        component_text: &str,
        add: bool,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(entity) = resolve_display_entity(world, entity_text) else {
            return Err(format!("no entity named '{entity_text}'"));
        };
        let Some(component) = resolve_component_name(world, component_text) else {
            return Err(format!("no component named '{component_text}'"));
        };
        let (edit, label) = if add {
            inspector::component_add(entity, &component)
        } else {
            inspector::component_remove(entity, &component)
        };
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(vec![edit], label, None);
        }
        info!(
            target: telemetry_target::EDITOR,
            component = component.as_str(),
            add,
            "[editor] component edit queued"
        );
        Ok(())
    }

    /// Runs `console filter <level>`, which sets the lowest level the Console
    /// tab shows. The next frame applies it to the history.
    pub(super) fn scripted_console_filter(&mut self, level: &str) {
        let Some(minimum_level) = pill_editor_core::console::level_from_name(level) else {
            debug!(
                target: telemetry_target::EDITOR,
                level,
                "[editor] console filter ignored"
            );
            return;
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(state) = world.get_resource_mut::<EguiShellState>() {
            state.console_filters.minimum_level = minimum_level;
        }
        info!(
            target: telemetry_target::EDITOR,
            level,
            "[editor] console filter"
        );
    }

    /// Runs `console search <text>`, which sets the Console tab's substring
    /// filter.
    pub(super) fn scripted_console_search(&mut self, text: String) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(state) = world.get_resource_mut::<EguiShellState>() {
            state.console_filters.search = text.clone();
        }
        info!(
            target: telemetry_target::EDITOR,
            search = text.as_str(),
            "[editor] console search"
        );
    }

    /// Sets the open dialog's name field, as typing into the window's text
    /// field does.
    pub(super) fn scripted_dialog_set_name(&mut self, dialog_name: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(state) = world.get_resource_mut::<EguiShellState>() {
            match state.asset_dialog.as_mut() {
                Some(AssetDialog::Create { name: field, .. })
                | Some(AssetDialog::Rename { name: field, .. }) => {
                    *field = dialog_name.to_string();
                }
                None => warn!(
                    target: telemetry_target::EDITOR,
                    "[editor] dialog set without an open dialog"
                ),
            }
        }
    }

    /// Runs the open dialog's submit button.
    ///
    /// The dialog is taken out of the session, submitted, and put back,
    /// because the submit also needs the pending command queue. The world
    /// cannot lend two mutable resources at once.
    pub(super) fn scripted_dialog_submit(&mut self) {
        // Step 1: Take the dialog out of the shell state, so the session is
        // free while the submit runs.
        let mut dialog = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world
                .get_resource_mut::<EguiShellState>()
                .and_then(|state| state.asset_dialog.take())
        };
        if dialog.is_none() {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] dialog submit without an open dialog"
            );
            return;
        }
        // Step 2: Submit it against the pending command queue.
        {
            let world = self.host.host_mut().engine_mut().world_mut();
            if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                match dialog.as_ref() {
                    Some(AssetDialog::Create { .. }) => {
                        assets::submit_create(&mut dialog, pending);
                    }
                    Some(AssetDialog::Rename { .. }) => {
                        assets::submit_rename(&mut dialog, pending);
                    }
                    None => {}
                }
            }
        }
        // Step 3: Store the dialog back as the submit left it. An accepted
        // submit leaves none, so the window closes.
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(shell_state) = world.get_resource_mut::<EguiShellState>() {
            shell_state.asset_dialog = dialog;
        }
    }

    /// Runs one scripted array-element edit (`edit array <entity> <component>
    /// <field> <index> <value>`). It queues the same recorded
    /// `SetArrayElement` batch an inspector element widget queues.
    ///
    /// # Errors
    ///
    /// Returns a message when the target does not resolve - no entity, no
    /// component or no such field - when the index is outside the array, or
    /// when the text does not parse against the element's kind.
    pub(super) fn scripted_edit_array(
        &mut self,
        entity_text: &str,
        component_text: &str,
        field: &str,
        index: usize,
        value_text: &str,
    ) -> Result<(), String> {
        let (entity, component, current_value) =
            self.resolve_edit_target(entity_text, component_text, field)?;
        let recorded =
            inspector::element_write(entity, &component, field, &current_value, index, value_text)?;
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(recorded.commands, recorded.label, Some(recorded.coalesce));
        }
        Ok(())
    }

    /// Runs `reset setting <path>`, which queues the same reset the tab's row
    /// action queues: the key leaves both settings files.
    ///
    /// # Errors
    ///
    /// Returns a message when the host was not configured from a project
    /// directory, so there is no settings file to reset.
    pub(super) fn scripted_reset_project_setting(&mut self, path: &str) -> Result<(), String> {
        let Some(directory) = self
            .host
            .host()
            .project_directory()
            .map(|directory| directory.display().to_string())
        else {
            return Err("the editor runs no project, so it has no settings file".to_string());
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::ResetProjectSetting {
                directory,
                path: path.to_string(),
            }]);
        }
        info!(
            target: telemetry_target::EDITOR,
            setting = path,
            "[editor] project setting reset queued"
        );
        Ok(())
    }

    /// Logs every declared project setting the way the Project Settings tab
    /// draws it: the entry, the path, the value in effect and where it comes
    /// from, one line per setting.
    ///
    /// The rows are the tab's own data, so a suite can check a write or a reset
    /// without reading the tab's pixels.
    pub(super) fn dump_project_settings(&mut self) {
        let view = capture_project_settings(self.host.host());
        info!(
            target: telemetry_target::EDITOR,
            directory = view.directory.as_deref().unwrap_or("-"),
            entries = view.entries().len(),
            rows = view.rows.len(),
            refusal = view.refusal.as_deref().unwrap_or("-"),
            "[test] settings"
        );
        for row in &view.rows {
            info!(
                target: telemetry_target::EDITOR,
                entry = row.entry.as_str(),
                path = row.full_path.as_str(),
                value = row.effective_text.as_str(),
                default = row.default_text.as_str(),
                project = row.project_text.as_deref().unwrap_or("-"),
                local = row.local_text.as_deref().unwrap_or("-"),
                source = row.provenance.icon(),
                timing = row.timing.as_str(),
                "[test] setting"
            );
        }
    }

    /// Runs one scripted preferences row (`edit preference <key> <value...>`):
    /// the same `SetPreferences` batch the tab's Save button queues, holding
    /// just that row. The value is everything after the key, so a text value
    /// keeps its spaces.
    pub(super) fn scripted_edit_preference(&mut self, key: &str, value: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::SetPreferences {
                values: vec![(key.to_string(), value.to_string())],
            }]);
        }
        info!(
            target: telemetry_target::EDITOR,
            key,
            value,
            "[editor] preference edit queued"
        );
    }

    /// Runs `edit setting <path> <value> [local]`, which queues the same
    /// `SetProjectSetting` the tab's buttons queue.
    ///
    /// # Errors
    ///
    /// Returns a message when the host was not configured from a project
    /// directory, so there is no settings file to write.
    pub(super) fn scripted_edit_project_setting(
        &mut self,
        path: &str,
        value: &str,
        local: bool,
    ) -> Result<(), String> {
        let Some(directory) = self
            .host
            .host()
            .project_directory()
            .map(|directory| directory.display().to_string())
        else {
            return Err("the editor runs no project, so it has no settings file".to_string());
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::SetProjectSetting {
                directory,
                path: path.to_string(),
                value: value.to_string(),
                local,
            }]);
        }
        info!(
            target: telemetry_target::EDITOR,
            setting = path,
            value,
            local,
            "[editor] project setting edit queued"
        );
        Ok(())
    }

    /// # Errors
    ///
    /// Returns a message when the resource has no such editable field, or when
    /// the text does not parse against the field's current kind.
    pub(super) fn scripted_edit_resource(
        &mut self,
        resource: &str,
        field: &str,
        value_text: &str,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let current_value = world.get_resource::<GizmoState>().and_then(|state| {
            pill_editor_core::gizmo::resource_field_value(&state.settings, resource, field).ok()
        });
        let Some(current_value) = current_value else {
            return Err(format!("'{resource}' has no editable field '{field}'"));
        };
        let parsed_value = inspector::parse_field_value(value_text, &current_value)?;
        let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() else {
            return Ok(());
        };
        pending.push_recorded(
            vec![EditorCommand::SetResourceField {
                resource: resource.to_string(),
                field: field.to_string(),
                value: parsed_value,
            }],
            format!("set {resource}.{field}"),
            Some(format!("resource.{resource}.{field}")),
        );
        info!(
            target: telemetry_target::EDITOR,
            resource,
            field,
            value = value_text,
            "[editor] resource edit queued"
        );
        Ok(())
    }

    /// Runs one scripted whole-field edit over the whole selection
    /// (`edit selection <component> <field> <value>`): the recorded batch an
    /// Inspector row queues when several entities are selected.
    ///
    /// The value is parsed against the primary entity's field. The write
    /// reaches every selected entity whose detail carries the component, so
    /// one recorded step covers the whole set and undoes in one step.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is selected, when the target does not
    /// resolve against the primary entity, or when the text does not name a
    /// value for the field.
    pub(super) fn scripted_edit_selection(
        &mut self,
        component_text: &str,
        field: &str,
        value_text: &str,
    ) -> Result<(), String> {
        let (primary_entity, selected_details) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let Some(primary_entity) = world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.primary())
            else {
                return Err("edit selection needs a selected entity".to_string());
            };
            let selected_details = world
                .get_resource::<EditorSnapshot>()
                .map(|snapshot| snapshot.selected_details.clone())
                .unwrap_or_default();
            (primary_entity, selected_details)
        };
        let (entity, component, current_value) =
            self.resolve_edit_target(&primary_entity.to_string(), component_text, field)?;
        let targets = inspector::selection_targets(&selected_details, entity, &component);
        let recorded = inspector::selection_write(
            &targets,
            entity,
            &component,
            field,
            &current_value,
            value_text,
        )?;
        let written_command_count = recorded.commands.len();
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(recorded.commands, recorded.label, Some(recorded.coalesce));
        }
        info!(
            target: telemetry_target::EDITOR,
            component = component.as_str(),
            field,
            entities = written_command_count,
            "[editor] selection field edit queued"
        );
        Ok(())
    }

    /// Runs one scripted whole-field edit (`edit set <entity> <component>
    /// <field> <value>`): the same recorded field edit an inspector widget
    /// queues, which is a `SetField` or its asset or entity reference variant.
    /// The text is parsed against the field's current kind.
    ///
    /// # Errors
    ///
    /// Returns a message when the target does not resolve, or when the text
    /// does not name a value for the field.
    pub(super) fn scripted_edit_set(
        &mut self,
        entity_text: &str,
        component_text: &str,
        field: &str,
        value_text: &str,
    ) -> Result<(), String> {
        let (entity, component, current_value) =
            self.resolve_edit_target(entity_text, component_text, field)?;
        let recorded =
            inspector::field_write(entity, &component, field, &current_value, value_text)?;
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(recorded.commands, recorded.label, Some(recorded.coalesce));
        }
        Ok(())
    }

    /// Runs `edit system <index> on|off` from the scripted channel: the same
    /// recorded, undoable toggle the Systems tab's on/off buttons queue.
    pub(super) fn scripted_edit_system(&mut self, index: usize, enabled: bool) {
        let (edit, label) = systems::system_toggle(index, enabled);
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(vec![edit], label, None);
        }
        info!(
            target: telemetry_target::EDITOR,
            index,
            enabled,
            "[editor] system edit queued"
        );
    }

    /// Runs `editor.entity_create` from the scripted channel: the recorded
    /// edit that creates one empty entity, as the Entities menu's New Entity
    /// does.
    pub(super) fn scripted_entity_create(&mut self) {
        let (command, label) = entities::entity_create();
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(vec![command], label, None);
        }
        info!(
            target: telemetry_target::EDITOR,
            "[editor] new entity queued"
        );
    }

    /// Runs `editor.entity_delete`: the selected entity's recorded delete, with
    /// the destroy and the selection clear as one step. When the selection
    /// holds no entity, it logs a note and does nothing.
    pub(super) fn scripted_entity_delete(&mut self) {
        // An element selection belongs to the active tool: Delete removes
        // those elements through the tool and leaves the entity alone, which is
        // what makes a point delete undoable as one step.
        if self.delete_selected_elements() {
            return;
        }
        let (entity, asset) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world
                .get_resource::<EditorSelection>()
                .map(|selection| (selection.primary(), selection.asset.clone()))
                .unwrap_or((None, None))
        };
        let Some(entity) = entity else {
            info!(
                target: telemetry_target::EDITOR,
                "[editor] delete entity skipped nothing selected"
            );
            return;
        };
        let (commands, label) = entities::entity_delete(entity, asset);
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(commands, label, None);
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            "[editor] delete entity queued"
        );
    }

    /// Runs `drag element <id> <dx> <dy> <dz>`: a direct nudge of one element
    /// by a world delta, queued as the recorded write a gizmo drag ends with.
    ///
    /// The element's new transform comes from the active tool's own report, so
    /// the editor never guesses where a tool keeps its data, and the write goes
    /// back through the same `set_element_transforms` hook a drag uses.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is selected, when the tool reports no
    /// elements, or when it reports no element with that id.
    pub(super) fn scripted_drag_element(&mut self, id: u64, delta: [f32; 3]) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some((entity, kind, _selected)) = tools::element_selection_subject(world) else {
            return Err("drag element needs a selected entity to own the elements".to_string());
        };
        let registry = tools::tool_registry_mut(world).clone();
        let reported = registry.active_elements(entity, world);
        let Some(element) = reported.iter().find(|element| element.id == id) else {
            return Err(format!("the active tool reports no element {id}"));
        };
        let moved = tools::ElementMove {
            element: id,
            transform: tools::ElementTransform {
                translation: gizmo::add(element.transform.translation, delta),
                ..element.transform
            },
        };
        let translation = moved.transform.translation;
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(
                vec![EditorCommand::SetElementTransforms {
                    entity,
                    kind,
                    moves: vec![moved],
                }],
                "nudge element",
                None,
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            element = id,
            x = translation[0],
            y = translation[1],
            z = translation[2],
            "[editor] element nudge queued"
        );
        Ok(())
    }

    /// Runs `element menu <item>`: the row of the active tool's element context
    /// menu a click on it queues.
    ///
    /// The row is checked against the tool's own report first, so a line that
    /// names a row the tool does not offer is refused instead of queueing a
    /// request nothing answers. The queued command is the same one the menu
    /// draws, and the frame boundary asks the tool what it means.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing is selected, when the tool adds no rows,
    /// or when it does not offer that row.
    pub(super) fn scripted_element_menu(&mut self, item: &str) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some((entity, kind, selected)) = tools::element_selection_subject(world) else {
            return Err("element menu needs a selected entity to own the elements".to_string());
        };
        let registry = tools::tool_registry_mut(world).clone();
        if !registry
            .active_menu(entity, &kind, &selected, world)
            .iter()
            .any(|entry| entry.id == item)
        {
            return Err(format!(
                "the active tool offers no menu row '{item}' for this selection"
            ));
        }
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(
                vec![EditorCommand::RunElementMenu {
                    item: item.to_string(),
                }],
                format!("tool {item}"),
                None,
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            item,
            "[editor] element menu queued"
        );
        Ok(())
    }

    /// Logs what the active tool reports (`dump elements`): one line per
    /// element with its world place, then one line per menu row it offers.
    ///
    /// A run with no selected entity or no reporting tool logs the count line
    /// alone, so a suite can tell "nothing selected" from "the tool reports
    /// nothing".
    pub(super) fn dump_elements(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let subject = tools::element_selection_subject(world);
        let Some((entity, kind, selected)) = subject else {
            info!(
                target: telemetry_target::EDITOR,
                count = 0,
                entity = "none",
                "[test] elements"
            );
            return;
        };
        let registry = tools::tool_registry_mut(world).clone();
        let reported = registry.active_elements(entity, world);
        let kinds = registry.active_kinds(entity, world);
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            count = reported.len(),
            kinds = kinds.len(),
            kind = kind.as_str(),
            selected = selected.len(),
            "[test] elements"
        );
        for element in &reported {
            let translation = element.transform.translation;
            info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                id = element.id,
                kind = element.kind.as_str(),
                x = translation[0],
                y = translation[1],
                z = translation[2],
                selected = selected.contains(&element.id),
                "[test] element"
            );
        }
        for entry in registry.active_menu(entity, &kind, &selected, world) {
            info!(
                target: telemetry_target::EDITOR,
                item = entry.id.as_str(),
                label = entry.label.as_str(),
                "[test] element menu"
            );
        }
    }

    /// Queues the tool's recorded removal of the element selection, and
    /// answers whether there was one to remove.
    ///
    /// The batch is the removal and then the slot's clear, in that order: the
    /// undo runs the other way round, so it puts the full selection back first
    /// and re-inserts the elements after it. A run with no element selection,
    /// or a tool that cannot remove, answers `false` and leaves Delete to the
    /// entity rule.
    pub(super) fn delete_selected_elements(&mut self) -> bool {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some((entity, kind, selected)) = tools::element_selection_subject(world) else {
            return false;
        };
        if selected.is_empty() {
            return false;
        }
        let registry = tools::tool_registry_mut(world).clone();
        if !registry.active_removes_elements(entity, world) {
            return false;
        }
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(
                vec![
                    EditorCommand::RemoveElements {
                        entity,
                        kind: kind.clone(),
                        elements: selected.clone(),
                    },
                    EditorCommand::SetElementSelection {
                        kind: String::new(),
                        elements: Vec::new(),
                    },
                ],
                "delete elements",
                None,
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            kind = kind.as_str(),
            count = selected.len(),
            "[editor] delete elements queued"
        );
        true
    }

    /// Offers one key press to the active tool's element selection, and
    /// answers whether the tool took it.
    ///
    /// The tool hears the key before the keybindings do, so a component tool
    /// can own a chord while its element selection is up; every other key - and
    /// every key with no element selection - falls through to the editor.
    pub(super) fn tool_element_key(&mut self, key: KeyCode) -> bool {
        let name = pill_editor_core::scripted::key_code_name(key);
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some((entity, kind, selected)) = tools::element_selection_subject(world) else {
            return false;
        };
        if selected.is_empty() {
            return false;
        }
        let registry = tools::tool_registry_mut(world).clone();
        let response = registry.active_key(entity, &kind, name, &selected, world);
        match response {
            tools::KeyResponse::Default => false,
            tools::KeyResponse::Consume => {
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %entity,
                    kind = kind.as_str(),
                    key = name,
                    "[editor] element key consumed"
                );
                true
            }
            tools::KeyResponse::Commands(commands) => {
                let count = commands.len();
                crate::runner::element_tools::queue_tool_commands(world, commands);
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %entity,
                    kind = kind.as_str(),
                    key = name,
                    commands = count,
                    "[editor] element key commands queued"
                );
                true
            }
        }
    }

    /// Resolves a scripted handle name to its handle id. `x`/`y`/`z` name the
    /// active tool's axes, and `xy`/`xz`/`yz` name the translation planes,
    /// which only the translate tool offers.
    pub(super) fn scripted_gizmo_handle(&mut self, handle_name: &str) -> Option<u32> {
        let gizmo_settings = self.gizmo_settings();
        if let Some(axis) = parse_gizmo_axis(handle_name) {
            return Some(gizmo::axis_handle(gizmo_settings.mode, axis));
        }
        let (first_axis, second_axis) = match handle_name {
            "xy" => (0, 1),
            "xz" => (0, 2),
            "yz" => (1, 2),
            _ => return None,
        };
        if gizmo_settings.mode != GizmoMode::Translate {
            warn!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(gizmo_settings.mode),
                "[editor] plane handles belong to the translate tool"
            );
            return None;
        }
        Some(gizmo::plane_handle(first_axis, second_axis))
    }

    /// Finishes the drag a `gizmo press` started, like a mouse release.
    ///
    /// # Errors
    ///
    /// Returns a message when no gizmo drag is in flight.
    pub(super) fn scripted_gizmo_release(&mut self) -> Result<(), String> {
        if self.gizmo_drag.is_none() {
            return Err("gizmo release needs a drag in flight".to_string());
        }
        self.synthetic_gizmo_press = None;
        self.end_gizmo_drag();
        Ok(())
    }

    /// Runs `editor.tool_space_toggle` from the scripted channel: it queues the
    /// same `CycleGizmoSpace` edit the toolbox's space row queues, so a suite
    /// reaches a tool's own frame the way a click does.
    pub(super) fn scripted_space_cycle(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::CycleGizmoSpace]);
        }
    }

    /// Runs `editor.tool_*` from the scripted channel: it queues the same
    /// `SetGizmoTool` edit the toolbox rows and the viewport menu queue. The
    /// edit is applied but never recorded, because a tool choice is a control,
    /// not an edit.
    pub(super) fn scripted_gizmo_tool(
        &mut self,
        mode: Option<GizmoMode>,
        space: Option<GizmoSpace>,
        snap: Option<bool>,
    ) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::SetGizmoTool { mode, space, snap }]);
        }
    }

    /// Moves one view's pointer and updates the hovered handle, standing in for
    /// a real pointer move.
    ///
    /// # Errors
    ///
    /// Returns a message when `view` is not a scene view index.
    pub(super) fn scripted_hover(&mut self, view: usize, x: f32, y: f32) -> Result<(), String> {
        if view >= self.views.len() {
            return Err(format!("there is no scene view {view}"));
        }
        self.scene_cursor[view] = Some((x, y));
        self.update_hover(view, (x, y));
        info!(
            target: telemetry_target::EDITOR,
            view,
            x,
            y,
            "[editor] hover injected"
        );
        Ok(())
    }

    /// Moves the shell's own pointer, as a real move arrives.
    ///
    /// egui learns a pointer position only from an event, so a suite that wants
    /// a tooltip or a resize cursor moves the pointer here. The stored cursor
    /// keeps the position on later frames, just like a real move that has
    /// stopped moving.
    pub(super) fn scripted_hover_ui(&mut self, x: f32, y: f32) {
        let position = pill_core::math::Vector2f::new(x, y);
        self.shell_cursor = Some(position);
        self.shell_events.push(InputEvent::CursorMoved { position });
        self.demand.input = true;
        info!(
            target: telemetry_target::EDITOR,
            x,
            y,
            "[editor] pointer hovered"
        );
    }

    /// Docks one more scene view, as the Window menu asks: the tab goes where
    /// the menu would put it, and the executable's activation path brings it to
    /// the front.
    ///
    /// # Errors
    ///
    /// Returns a message when every scene view is already open. A shell that
    /// has not attached yet only warns, and the same line works once it has.
    pub(super) fn scripted_new_scene_view(&mut self) -> Result<(), String> {
        let scene_view_choice = {
            let world = self.host.host_mut().engine_mut().world_mut();
            world.get_resource::<EguiShellState>().map(|state| {
                (
                    state.dock.next_scene_view(),
                    state.dock.can_open_scene_view(),
                )
            })
        };
        // A shell that is not attached yet is a passing moment, not a refused
        // line, because the same command works once it is.
        let Some((tab, has_room)) = scene_view_choice else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] scene view needs the shell module's state"
            );
            return Ok(());
        };
        if !has_room {
            return Err(format!(
                "the editor shows at most {} scene views",
                pill_editor_core::boundary::SCENE_VIEWS_MAX
            ));
        }
        self.activate_tab(tab);
        info!(
            target: telemetry_target::EDITOR,
            tab = tab.name().as_str(),
            view = tab.scene_view().unwrap_or_default(),
            "[editor] scene view added"
        );
        Ok(())
    }

    /// Runs `palette <query>`, `palette run` or `palette close` from the
    /// scripted channel.
    ///
    /// This touches only the palette's model state. The shell's pass draws the
    /// popup and runs the selected row, so a scripted run and a click take the
    /// same action path.
    pub(super) fn scripted_palette(&mut self, request: PaletteRequest) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(shell_state) = world.get_resource_mut::<EguiShellState>() else {
            return;
        };
        match request {
            PaletteRequest::Query(query) => {
                shell_state.palette.open = true;
                shell_state.palette.query = query;
                shell_state.palette.selected = 0;
                shell_state.palette.logged_query = None;
                info!(
                    target: telemetry_target::EDITOR,
                    query = shell_state.palette.query.as_str(),
                    "[editor] palette opened"
                );
            }
            PaletteRequest::Run => shell_state.palette.run_requested = true,
            PaletteRequest::Close => {
                shell_state.palette.open = false;
                shell_state.palette.run_requested = false;
                info!(
                    target: telemetry_target::EDITOR,
                    "[editor] palette dismissed"
                );
            }
        }
    }

    /// Runs `command <id> [entity <name> | asset <path>]` for a command a
    /// module registered.
    ///
    /// The line names its subject the way a person would ("the Mesh"); this
    /// resolves it to the handle the module reads, so a module never sees a
    /// name and a suite never sees a handle it cannot predict.
    ///
    /// # Errors
    ///
    /// Returns a message when nothing registers `id`, or when the line names an
    /// entity that does not exist.
    pub(super) fn scripted_command_invocation(
        &mut self,
        id: &str,
        target: &ScriptedTarget,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let registered = world
            .get_resource::<CommandRegistry>()
            .is_some_and(|registry| registry.command(id).is_some());
        if !registered {
            return Err(format!("no command is registered as {id}"));
        }
        let resolved = match target {
            ScriptedTarget::None => CommandTarget::None,
            ScriptedTarget::Entity(name) => {
                let Some(entity) = resolve_display_entity(world, name) else {
                    return Err(format!("no entity named '{name}'"));
                };
                CommandTarget::Entity {
                    entity,
                    component: None,
                }
            }
            ScriptedTarget::Asset(key) => CommandTarget::Asset(key.clone()),
        };
        let Some(inbox) = world.get_resource_mut::<CommandInbox>() else {
            return Err("the command inbox is missing".to_string());
        };
        inbox.push(CommandInvocation {
            id: id.to_string(),
            target: resolved.clone(),
        });
        info!(
            target: telemetry_target::EDITOR,
            command = id,
            target = command_target_label(&resolved).as_str(),
            "[editor] command invoked"
        );
        Ok(())
    }

    /// Queues the component-field write a module panel's own row would queue
    /// (`panel field <component> <field> <value>`).
    ///
    /// The row is a drag value the test channel cannot press, so the write goes
    /// where the row puts it: into the shared `PanelCommands` queue.
    /// [`Self::apply_panel_commands`] applies that queue at the next frame
    /// boundary. A test therefore drives the panel contract itself - the queue,
    /// the recorded field edit and its undo step - and not a second write path.
    ///
    /// # Errors
    ///
    /// Returns a message when the selection holds no primary entity.
    pub(super) fn scripted_panel_field(
        &mut self,
        component: &str,
        field: &str,
        value: &str,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(entity) = world
            .get_resource::<EditorSelection>()
            .and_then(|selection| selection.primary())
        else {
            return Err("panel field needs a selected entity".to_string());
        };
        let mut queued = false;
        if let Some(commands) = world.get_resource_mut::<PanelCommands>() {
            commands.push(PanelCommand::SetField {
                entity,
                component: component.to_string(),
                field: field.to_string(),
                value: value.to_string(),
            });
            queued = true;
        }
        if queued {
            info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                component,
                field,
                value,
                "[editor] panel field queued"
            );
        }
        Ok(())
    }

    /// Runs `panel restore <source>`, which binds that module's panels again.
    ///
    /// The requests the executable kept are registered against a fresh liveness
    /// marker, which is what a reload's re-registration does.
    pub(super) fn scripted_panel_restore(&mut self, source: &str) {
        // Step 1: Report a source that is still bound and stop.
        if self.module_markers.contains_key(source) {
            info!(
                target: telemetry_target::EDITOR,
                source,
                restored = 0,
                "[editor] panel source restored"
            );
            return;
        }
        // Step 2: Register the kept requests against a fresh liveness marker.
        let liveness_marker = ModuleLiveness::new();
        let kept_requests: Vec<PanelRequest> = self
            .panel_requests
            .iter()
            .copied()
            .filter(|request| request.source == source)
            .collect();
        let world = self.host.host_mut().engine_mut().world_mut();
        let mut restored = 0;
        if let Some(registry) = world.get_resource_mut::<PanelRegistry>() {
            for request in &kept_requests {
                registry.register(
                    request.id,
                    request.title,
                    request.source,
                    request.draw,
                    &liveness_marker,
                );
                restored += 1;
            }
        }
        // Step 3: Remember the marker and report the count.
        if restored > 0 {
            self.module_markers
                .insert(source.to_string(), liveness_marker);
        }
        info!(
            target: telemetry_target::EDITOR,
            source,
            restored,
            "[editor] panel source restored"
        );
    }

    /// Runs `panel unload <source>`, which drops one module's liveness marker.
    ///
    /// This leaves the registry in the state a module unload leaves it in,
    /// because the marker is what an unload drops. The tab then reads
    /// "module not loaded", and neither the panel's draw function nor the
    /// module's editor-API callbacks are handed out.
    pub(super) fn scripted_panel_unload(&mut self, source: &str) {
        let dropped = self.module_markers.remove(source).is_some();
        info!(
            target: telemetry_target::EDITOR,
            source,
            dropped,
            "[editor] panel source unloaded"
        );
    }

    /// Runs `panel window <slot> <id>`, which asks a panel to open one of its
    /// windows.
    ///
    /// The panel decides what the id means, so this drives the same code its
    /// button does.
    pub(super) fn scripted_panel_window(&mut self, slot: u16, id: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(shell_state) = world.get_resource_mut::<EguiShellState>() else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] panel window request needs the shell module's state"
            );
            return;
        };
        shell_state.pending_panel_window = Some((slot, id.to_string()));
        info!(
            target: telemetry_target::EDITOR,
            slot,
            window = id,
            "[editor] panel window requested"
        );
    }

    /// Runs `editor.pause_toggle` from the scripted channel: it queues the flip
    /// of the engine's pause flag, applied but never recorded. That is the same
    /// walk the Systems tab's Pause/Resume button takes.
    pub(super) fn scripted_pause_toggle(&mut self) {
        let paused = self.host.host_mut().engine_mut().systems_paused();
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![systems::pause_toggle_command(paused)]);
        }
        info!(
            target: telemetry_target::EDITOR,
            paused = !paused,
            "[editor] pause toggled"
        );
    }

    /// Runs `editor.play` from the scripted channel: the same queued
    /// `EnterPlay` the toolbar's Play button queues.
    pub(super) fn scripted_play(&mut self) {
        self.queue_play_command(EditorCommand::EnterPlay);
    }

    /// Runs `editor.stop` from the scripted channel: the same queued `ExitPlay`
    /// the toolbar's Stop button queues.
    pub(super) fn scripted_stop(&mut self) {
        self.queue_play_command(EditorCommand::ExitPlay);
    }

    /// Runs `editor.play_toggle`: Play while editing, Stop while playing.
    pub(super) fn scripted_play_toggle(&mut self) {
        let run_mode = self.host.host_mut().engine_mut().run_mode();
        let command = match run_mode {
            pill_engine_core::RunMode::Editing => EditorCommand::EnterPlay,
            pill_engine_core::RunMode::Playing => EditorCommand::ExitPlay,
        };
        self.queue_play_command(command);
    }

    /// Queues one play command for the frame boundary's drain.
    fn queue_play_command(&mut self, command: EditorCommand) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![command]);
        }
    }

    /// Arms an entity reference row (`pick entity <entity> <component>
    /// <field>`): the same `ArmEntityPick` the row's target button queues, so
    /// the next scene press names the field's reference.
    ///
    /// # Errors
    ///
    /// Returns a message when the target does not resolve, or when the field
    /// does not hold an entity reference.
    pub(super) fn scripted_pick_entity(
        &mut self,
        entity_text: &str,
        component_text: &str,
        field: &str,
    ) -> Result<(), String> {
        let (entity, component, current_value) =
            self.resolve_edit_target(entity_text, component_text, field)?;
        if !matches!(current_value, FieldValue::Entity { .. }) {
            return Err(format!("'{field}' does not hold an entity reference"));
        }
        let command = EditorCommand::ArmEntityPick {
            entity,
            component,
            field: field.to_owned(),
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![command]);
        }
        Ok(())
    }

    /// Runs `popout <tab>` or `popout <tab> redock`: the same dock move the tab
    /// menu makes, driven from the scripted channel.
    ///
    /// # Errors
    ///
    /// Returns a message for the game view, which is the primary surface's
    /// window, and when the tab is already docked or already out.
    pub(super) fn scripted_popout(&mut self, tab: EditorTab, redock: bool) -> Result<(), String> {
        if tab == EditorTab::Game {
            return Err("the game view is the primary surface's window".to_string());
        }
        let world = self.host.host_mut().engine_mut().world_mut();
        // A shell that is not attached yet is a passing moment, not a refused
        // line.
        let Some(shell_state) = world.get_resource_mut::<EguiShellState>() else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] popout needs the shell module's state"
            );
            return Ok(());
        };
        let moved = if redock {
            shell_state.dock.redock(tab)
        } else {
            shell_state.dock.pop_out(tab)
        };
        if moved {
            info!(
                target: telemetry_target::EDITOR,
                tab = tab.name().as_str(),
                redock,
                "[editor] popout moved"
            );
        } else {
            return Err(format!(
                "the {} tab is already {}",
                tab.name(),
                if redock { "docked" } else { "out" }
            ));
        }
        Ok(())
    }

    /// Runs `problem open <index>`, the same action a click on the row takes.
    ///
    /// The row opens at once, not at the next frame boundary. A suite that sends
    /// this line waits for the editor it launched, and one frame of delay would
    /// make the assertion race the launch. The body is the one the frame drain
    /// calls, so the two paths differ only in when they run.
    pub(super) fn scripted_problem_open(&mut self, index: usize) {
        info!(
            target: telemetry_target::EDITOR,
            index,
            "[editor] problem open scripted"
        );
        self.open_problem(index);
    }

    /// Runs `editor.redo` from the scripted channel: the same walk, which
    /// queues the next step's forward batch.
    pub(super) fn scripted_redo(&mut self) {
        drain::step_history(self.host.host_mut().engine_mut(), HistoryDirection::Redo);
    }

    /// Runs `scene new`, `scene open` or `scene save` from the scripted channel:
    /// the same command the File menu queues, applied at the next frame
    /// boundary.
    ///
    /// Document commands are not recorded, because they replace the world
    /// instead of editing it, so the batch is queued unrecorded.
    pub(super) fn scripted_scene(&mut self, command: EditorCommand) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![command]);
        }
    }

    /// Sets the scripted asset selection: the same shared resource an Assets row
    /// click writes. The line is logged so a suite can watch either path.
    pub(super) fn scripted_select_asset(&mut self, path: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(selection) = world.get_resource_mut::<EditorSelection>() {
            selection.asset = Some(path.to_string());
        }
        info!(
            target: telemetry_target::EDITOR,
            path,
            "[editor] asset selected"
        );
    }

    /// Sets the scripted entity selection: the same recorded step the Entities
    /// tab's row click queues, resolved against the live entity table.
    ///
    /// A screen-space click cannot re-select an entity that a gizmo drag moved,
    /// but the handle a pick logged still resolves here.
    ///
    /// # Errors
    ///
    /// Returns a message when the display handle names no entity.
    pub(super) fn scripted_select_entity(&mut self, entity_text: &str) -> Result<(), String> {
        // A plain click is the reset form of the same rule, so the two scripted
        // verbs and the pointer path share one path.
        self.scripted_select_extend(entity_text, SelectionClick::Reset)
    }

    /// Extends the scripted selection (`select add` / `select range`): the set a
    /// Ctrl- or Shift-click on the Entities list produces, recorded as the same
    /// single `SetSelection` step the row click queues.
    ///
    /// The click rule itself lives in the core's `EditorSelection::clicked`, so
    /// the pointer path and this channel cannot disagree about what a click
    /// means. What the modifiers do is the shell's `click_from_modifiers`.
    ///
    /// # Errors
    ///
    /// Returns a message when the display handle names no entity.
    pub(super) fn scripted_select_extend(
        &mut self,
        entity_text: &str,
        click: SelectionClick,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(entity) = resolve_display_entity(world, entity_text) else {
            return Err(format!("no entity named '{entity_text}'"));
        };
        // The Entities list's row order, which a range click reads.
        let entity_rows: Vec<Entity> = world
            .get_resource::<EditorSnapshot>()
            .map(|snapshot| snapshot.entities.iter().map(|row| row.entity).collect())
            .unwrap_or_default();
        let Some(selection) = world.get_resource::<EditorSelection>() else {
            return Ok(());
        };
        let entities = selection.clicked(click, entity, &entity_rows);
        let asset = selection.asset.clone();
        let count = entities.len();
        let label = match click {
            SelectionClick::Reset if count <= 1 => "select entity",
            _ => "select entities",
        };
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(
                vec![EditorCommand::SetSelection { entities, asset }],
                label,
                Some("editor.selection".to_string()),
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %entity,
            count,
            "[editor] selection changed"
        );
        Ok(())
    }

    /// Runs `select element <ids>` / `select element add <id>`: the recorded
    /// element selection a tool's own click makes.
    ///
    /// While a tool reports elements, every id is resolved against its report
    /// and applied through the same click rule a pointer press uses: the first
    /// id is the click the verb names, the rest extend it, and a kind mismatch
    /// logs the same refusal a Ctrl-click across kinds does. Without a
    /// reporting tool the ids are taken as given, under the kind the current
    /// selection carries.
    ///
    /// # Errors
    ///
    /// Returns a message when no entity is selected to own the elements, and
    /// when a reporting tool does not report one of the ids.
    pub(super) fn scripted_select_elements(
        &mut self,
        ids: Vec<u64>,
        add: bool,
    ) -> Result<(), String> {
        // Step 1: the owner and the active tool's report, in one borrow. The
        // element selection's own owner wins: a tool edits the elements of
        // the entity it was entered on.
        let (owner, reported) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let Some(selection) = world.get_resource::<EditorSelection>() else {
                return Ok(());
            };
            let Some(owner) = selection.elements().owner().or_else(|| selection.primary()) else {
                return Err(
                    "select element needs a selected entity to own the elements".to_string()
                );
            };
            let reported = match world.get_resource::<tools::ToolRegistry>() {
                Some(registry) if registry.active_reports_elements(owner, world) => {
                    registry.active_elements(owner, world)
                }
                _ => Vec::new(),
            };
            (owner, reported)
        };
        if reported.is_empty() {
            return self.scripted_select_unreported_elements(owner, ids, add);
        }
        // Step 2: apply the ids in the order given, through the click rule.
        // The report is the order a range reads, exactly as the pointer's own
        // path passes it.
        let report: Vec<ElementRef> = reported
            .iter()
            .map(|element| ElementRef {
                entity: owner,
                id: element.id,
            })
            .collect();
        for (index, id) in ids.iter().enumerate() {
            let Some(element) = reported.iter().find(|element| element.id == *id) else {
                return Err(format!("the active tool reports no element {id}"));
            };
            let click = if add || index > 0 {
                SelectionClick::Add
            } else {
                SelectionClick::Reset
            };
            self.apply_element_click_with(
                gizmo::ElementHandleTarget {
                    entity: owner,
                    element: element.id,
                    kind: element.kind.clone(),
                },
                click,
                &report,
            );
        }
        Ok(())
    }

    /// The element selection of a run with no reporting tool: the ids are
    /// taken as given, under the kind the current selection names.
    ///
    /// # Errors
    ///
    /// Returns a message when the selection resource is missing; the owner has
    /// already been resolved by the caller.
    fn scripted_select_unreported_elements(
        &mut self,
        owner: Entity,
        ids: Vec<u64>,
        add: bool,
    ) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(selection) = world.get_resource::<EditorSelection>() else {
            return Ok(());
        };
        let kind = if selection.elements().kind().is_empty() {
            "element".to_string()
        } else {
            selection.elements().kind().to_string()
        };
        let mut elements: Vec<ElementRef> = if add {
            selection.elements().elements().to_vec()
        } else {
            Vec::new()
        };
        for id in &ids {
            let element = ElementRef {
                entity: owner,
                id: *id,
            };
            if !elements.contains(&element) {
                elements.push(element);
            }
        }
        let count = elements.len();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(
                vec![EditorCommand::SetElementSelection { kind, elements }],
                "select elements",
                Some("editor.elements".to_string()),
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %owner,
            count,
            "[editor] element selection queued"
        );
        Ok(())
    }

    /// Runs `tool <name>` / `tool none`: the recorded activation a toolbox row
    /// makes.
    ///
    /// A built-in's name also queues the gizmo-mode command its row would, so
    /// the scripted channel, the toolbox and the shortcuts all end in the same
    /// place. An unknown name is refused with the tools that exist, which is
    /// what the grammar's own line says.
    ///
    /// # Errors
    ///
    /// Returns the registry's refusal when no tool has that name.
    pub(super) fn scripted_tool(&mut self, name: Option<String>) -> Result<(), String> {
        let world = self.host.host_mut().engine_mut().world_mut();
        tools::activate_tool(world, name.as_deref())?;
        let active = tools::tool_registry_mut(world).active().map(str::to_string);
        // A built-in tool is a gizmo mode, so activating one queues the same
        // command its toolbox row does. Any other tool's effect is its element
        // reporting, which the next frame's publish reads.
        if let Some(mode) = active
            .as_deref()
            .and_then(tools::BuiltinTool::from_name)
            .map(tools::BuiltinTool::gizmo_mode)
        {
            if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                pending.push_recorded(
                    vec![EditorCommand::SetGizmoTool {
                        mode: Some(mode),
                        space: None,
                        snap: None,
                    }],
                    format!("tool {}", tools::BuiltinTool::from(mode).name()),
                    Some("editor.tool".to_string()),
                );
            }
        }
        info!(
            target: telemetry_target::EDITOR,
            tool = active.as_deref().unwrap_or("none"),
            "[editor] tool activated"
        );
        Ok(())
    }

    /// Logs the registered tools and which one is active (`dump tools`).
    ///
    /// Every extension follows the tool it extends, with the place it takes in
    /// that tool's chain and whether it is attached to what is selected right
    /// now. `attached` is what a reader compares between two entities: an
    /// extension that requires a component is only attached to an entity that
    /// carries it.
    pub(super) fn dump_tools(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let registry = tools::tool_registry_mut(world).clone();
        let active = registry.active().map(str::to_string);
        let subject = tools::element_selection_subject(world);
        info!(
            target: telemetry_target::EDITOR,
            count = registry.tools().len(),
            active = active.as_deref().unwrap_or("none"),
            extensions = registry.extensions().len(),
            "[test] tools"
        );
        for tool in registry.tools() {
            info!(
                target: telemetry_target::EDITOR,
                name = tool.name.as_str(),
                label = tool.label.as_str(),
                shortcut = tool.shortcut.as_deref().unwrap_or("none"),
                global = tool.global,
                active = active.as_deref() == Some(tool.name.as_str()),
                "[test] tool"
            );
            for extension in registry
                .extensions()
                .iter()
                .filter(|extension| extension.base_tool.eq_ignore_ascii_case(&tool.name))
            {
                let attached = subject.as_ref().is_some_and(|(entity, _, _)| {
                    registry.is_extension_attached(&extension.name, *entity, world)
                });
                info!(
                    target: telemetry_target::EDITOR,
                    name = extension.name.as_str(),
                    base = extension.base_tool.as_str(),
                    order = extension.order,
                    requires = extension.requires_component.as_deref().unwrap_or("none"),
                    kinds = extension.for_kinds.join(","),
                    keys = extension.keys.join(","),
                    attached,
                    "[test] tool extension"
                );
            }
        }
    }

    /// Runs `select resource <name>`: the recorded selection a resource row
    /// makes.
    pub(super) fn scripted_select_resource(&mut self, resource_name: &str) {
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() else {
            return;
        };
        pending.push_recorded(
            vec![EditorCommand::SelectResource {
                resource: Some(resource_name.to_string()),
            }],
            format!("select resource {resource_name}"),
            None,
        );
        info!(
            target: telemetry_target::EDITOR,
            resource = resource_name,
            "[editor] resource selection queued"
        );
    }

    /// Runs `editor.step` from the scripted channel: it queues one single step,
    /// applied but never recorded. That is the same walk the Step button takes.
    pub(super) fn scripted_step(&mut self) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push(vec![EditorCommand::StepOnce]);
        }
        info!(target: telemetry_target::EDITOR, "[editor] step queued");
    }

    /// Runs `extensions on|off`: registers or drops the fixture tool's two
    /// extensions.
    ///
    /// Every other suite area drives the plain fixture tool, so the extensions
    /// arrive only when an area asks for them. The registration is the one a
    /// module's editor crate makes at load time, which is why what it logs about
    /// a chord clash is the same line.
    pub(super) fn scripted_extensions(&mut self, enabled: bool) {
        let world = self.host.host_mut().engine_mut().world_mut();
        crate::runner::tool_fixture::set_extensions(world, enabled);
        let registered = tools::tool_registry_mut(world).extensions().len();
        info!(
            target: telemetry_target::EDITOR,
            enabled,
            extensions = registered,
            "[editor] tool extensions"
        );
    }

    /// Runs one Assets toolbar button from the scripted channel, through the
    /// same core helpers the buttons call, enablement rules included.
    pub(super) fn scripted_toolbar(&mut self, action: ToolbarAction) {
        // Step 1: Clone out the rows, the types and the selection the helpers
        // read, because the world's mutable resources are borrowed below.
        let (asset_rows, standalone_types, selected_asset_path) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let (asset_rows, standalone_types) = match world.get_resource::<EditorSnapshot>() {
                Some(snapshot) => (snapshot.assets.clone(), snapshot.standalone_types.clone()),
                None => (Vec::new(), Vec::new()),
            };
            let selected_asset_path = world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.asset.clone());
            (asset_rows, standalone_types, selected_asset_path)
        };
        // Step 2: Build a selection view for the shared asset helpers. Only the
        // asset half belongs to them, and the entity set is empty because
        // nothing here acts on entities.
        let mut selection = EditorSelection::default();
        selection.asset = selected_asset_path;
        // Step 3: Run the chosen button through its shared helper.
        match action {
            ToolbarAction::Create => {
                let world = self.host.host_mut().engine_mut().world_mut();
                if let Some(state) = world.get_resource_mut::<EguiShellState>() {
                    assets::open_create(
                        &mut state.asset_dialog,
                        &selection,
                        &asset_rows,
                        &standalone_types,
                    );
                }
            }
            ToolbarAction::Rename => {
                let world = self.host.host_mut().engine_mut().world_mut();
                if let Some(state) = world.get_resource_mut::<EguiShellState>() {
                    assets::open_rename(&mut state.asset_dialog, &selection, &asset_rows);
                }
            }
            ToolbarAction::Delete => {
                let world = self.host.host_mut().engine_mut().world_mut();
                if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                    assets::delete_selection(pending, &selection, &asset_rows);
                }
            }
            ToolbarAction::Meta => {
                let world = self.host.host_mut().engine_mut().world_mut();
                if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                    assets::write_meta_selection(pending, &selection, &asset_rows);
                }
            }
        }
    }

    /// Runs `editor.undo` from the scripted channel. The shared core walk pops
    /// the newest step and queues its inverse, applied but never recorded, which
    /// is the same walk the browser editor and the native editor take.
    pub(super) fn scripted_undo(&mut self) {
        drain::step_history(self.host.host_mut().engine_mut(), HistoryDirection::Undo);
    }

    /// Queues a scripted left-button drag. The press, the move and the release
    /// arrive one per frame through [`Self::advance_scripted_drag`], which is
    /// the shape egui's drag detection needs. The events enter through the same
    /// input path a real drag enters, so a panel's drag handle reacts exactly as
    /// it does for a user.
    ///
    /// # Errors
    ///
    /// Returns a message when a scripted drag is already in flight.
    pub(super) fn start_scripted_drag(
        &mut self,
        from: (f32, f32),
        to: (f32, f32),
    ) -> Result<(), String> {
        if self.scripted_drag.is_some() {
            return Err("a scripted drag is already in flight".to_string());
        }
        self.scripted_drag = Some(ScriptedDrag {
            from,
            to,
            stage: ScriptedDragStage::Press,
        });
        info!(
            target: telemetry_target::EDITOR,
            from_x = from.0,
            from_y = from.1,
            to_x = to.0,
            to_y = to.1,
            "[editor] pointer drag queued"
        );
        Ok(())
    }
}
