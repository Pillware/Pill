//! The editor's windows: surfaces, child windows, popouts and presentation.
//!
//! # Responsibilities
//!
//! - Create and place each scene view's child window and the game view's, and
//!   keep every renderer surface bound to the window that shows it.
//! - Create and close the popped-out tab windows, and present each tab's frame
//!   in its own window.
//! - Present one editor frame through the overlay, drawing the fallback UI
//!   while no shell module is loaded.
//!
//! # Design
//!
//! A view's window and its renderer surface are handled together, because a
//! surface only draws into the window it is bound to. Drawing the UI is best
//! effort: the composition overlay is the normal posture, the window's own
//! surface is the fallback, and when the platform refuses both, creating the
//! composition overlay is retried every 120 frames instead of failing.

// Current crate
use super::*;

impl EditorWindow {
    /// Keeps the floating overlay window over the editor's client area.
    ///
    /// The overlay is a window of its own, so nothing moves it with the editor
    /// window: it is placed at the client origin and sized to the client area
    /// whenever either changes, and it is shown only while the editor is
    /// focused. An unmanaged window is stacked above every other window, so
    /// leaving it up while the editor is in the background would paint the UI
    /// over whatever the person switched to.
    ///
    /// Every other posture draws into a window the editor already owns, and
    /// this does nothing there.
    pub(super) fn sync_floating_overlay(&mut self) {
        let Some(window) = self
            .overlay
            .as_ref()
            .and_then(OverlaySurface::floating_window)
            .cloned()
        else {
            return;
        };
        // Step 1: the client area's screen origin. A window manager that has
        // not placed the editor yet reports no inner position, and the frames
        // before that are drawn at the placement that is still unknown.
        let Some(origin) = self.window.inner_position().ok() else {
            return;
        };
        // Step 2: move and resize only when something changed, so a still
        // frame costs nothing beyond the two getters above.
        let placement = (origin.x, origin.y, self.size.0, self.size.1);
        if self.overlay_placement != Some(placement) {
            window.set_outer_position(PhysicalPosition::new(origin.x, origin.y));
            let _ = window
                .request_inner_size(PhysicalSize::new(placement.2.max(1), placement.3.max(1)));
            self.overlay_placement = Some(placement);
        }
        // Step 3: hide it with the editor. `set_visible(true)` also raises an
        // X11 window, so it comes back above the scene viewports every time.
        window.set_visible(self.focused);
    }

    /// Handles a resize of the main window, in physical pixels.
    pub(super) fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        // The primary surface lives on the game view's window when one
        // exists, so only the fallback posture sizes the primary from the
        // main window.
        if self.game_surface.is_none() {
            self.host
                .resize_surface(SurfaceId::PRIMARY, width.max(1), height.max(1));
        }
        if let Some(overlay) = &mut self.overlay {
            overlay.resize(&self.device, self.size);
        }
        self.place_views();
    }

    /// Returns one scene view's client-area origin, in the main window's
    /// client coordinates and in physical pixels: the position its last
    /// placement applied.
    ///
    /// A resize never moves the window, so the origin stays valid even after
    /// the layout clears the placement cache. A drag across a separator
    /// therefore never makes the pointer jump.
    pub(super) fn view_origin(&self, view: usize) -> (f32, f32) {
        match &self.views[view] {
            Some(scene) => (scene.window.origin.0 as f32, scene.window.origin.1 as f32),
            None => (0.0, 0.0),
        }
    }

    /// Handles a resize of one scene view's window.
    ///
    /// The view's surface reconfigures with the window; without that, the
    /// scene would be drawn at the old size inside the tab. The placement cache
    /// is cleared, so the next layout pass re-applies the dock's rectangle.
    pub(super) fn resize_scene(&mut self, view: usize, width: u32, height: u32) {
        let Some(scene) = &mut self.views[view] else {
            return;
        };
        scene.window.placed = None;
        let (width, height) = (width.max(1), height.max(1));
        self.host.resize_surface(scene.surface_id, width, height);
        info!(
            target: telemetry_target::EDITOR,
            width,
            height,
            view,
            "[editor] scene surface resized"
        );
    }

    /// Handles a resize of the game window.
    ///
    /// The primary surface reconfigures with its window, as the scene view
    /// surfaces do; without that, the project would be drawn at the old size
    /// inside the Game tab. The placement cache is cleared, so the next layout
    /// pass re-applies the dock's rectangle.
    pub(super) fn resize_game(&mut self, width: u32, height: u32) {
        let Some(game) = &mut self.game_surface else {
            return;
        };
        game.placed = None;
        let (width, height) = (width.max(1), height.max(1));
        self.host.resize_surface(SurfaceId::PRIMARY, width, height);
        info!(
            target: telemetry_target::EDITOR,
            width,
            height,
            "[editor] game surface resized"
        );
    }

    /// Returns the game window's client-area origin, in the main window's
    /// client coordinates and in physical pixels: the same bridge the scene
    /// views use for pointer positions.
    pub(super) fn game_origin(&self) -> (f32, f32) {
        match &self.game_surface {
            Some(game) => (game.origin.0 as f32, game.origin.1 as f32),
            None => (0.0, 0.0),
        }
    }

    /// Opens the scene views the dock reports that this editor does not hold
    /// yet.
    ///
    /// The shell reports one slot per view index the dock carries, so a view
    /// the person just added is the list's last slot. Each new view gets its
    /// own window, surface and camera here, the same three the startup views
    /// get, and starts hidden: the placement that follows this frame shows it.
    pub(super) fn ensure_scene_views(
        &mut self,
        dock_view_count: usize,
        event_loop: &ActiveEventLoop,
    ) {
        let wanted_view_count = dock_view_count.min(pill_editor_core::boundary::SCENE_VIEWS_MAX);
        while self.views.len() < wanted_view_count {
            let view_index = self.views.len();
            self.cameras.push(OrbitCamera::default());
            self.left_down.push(false);
            self.camera_dirty.push(true);
            self.scene_cursor.push(None);
            let opened_scene = SceneView::open(event_loop, &self.window, view_index);
            if let Some(mut scene) = opened_scene {
                scene.attach(&mut self.host, &self.cameras[view_index]);
                info!(
                    target: telemetry_target::EDITOR,
                    view = view_index,
                    surface = scene.surface_id.0,
                    "[editor] scene view opened"
                );
                self.views.push(Some(scene));
            } else {
                // A view whose window could not be created still gets its
                // slots: the dock keeps its tab, and that tab shows only the
                // UI's chips.
                self.views.push(None);
            }
        }
    }

    /// Places the first scene view at the dock's default layout's center
    /// inside the client area.
    ///
    /// It runs at startup and on a resize, before the shell's own scene
    /// rectangles are applied. The other views stay hidden until their tabs
    /// show.
    pub(super) fn place_views(&mut self) {
        let Some(Some(scene)) = self.views.first_mut() else {
            return;
        };
        let (width, height) = (self.size.0 as i32, self.size.1 as i32);
        let margin = (SCENE_DEFAULT_MARGIN * self.scale_factor as f64) as i32;
        let side = (width as f64 * SIDE_COLUMN_FRACTION as f64) as i32;
        let top = (SCENE_DEFAULT_TOP_MARGIN * self.scale_factor as f64) as i32 + margin;
        let bottom = (height as f64 * BOTTOM_ROW_FRACTION as f64) as i32;
        let left = side + margin;
        let width = (width - 2 * side - 2 * margin).max(1) as u32;
        let height = (height - top - bottom - margin).max(1) as u32;
        scene.window.place(left, top, width, height, false);
    }

    /// Gives a popped-out scene view its top-level window and places it at the
    /// pop-out's screen position.
    ///
    /// The first call replaces the view's child window with a top-level one,
    /// because a child window can never reach another monitor. Later calls
    /// only follow the pop-out as it moves.
    pub(super) fn pop_view_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        view: usize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) {
        let needs_window = self
            .views
            .get(view)
            .and_then(|scene| scene.as_ref())
            .is_some_and(|scene| !scene.popped);
        if needs_window {
            let title = format!("Pill Editor Scene {} (popped out)", view + 1);
            match SurfaceWindow::open_top_level(event_loop, &title) {
                Ok(window) => {
                    let host = &mut self.host;
                    let camera = self.cameras[view];
                    if let Some(Some(scene)) = self.views.get_mut(view) {
                        scene.rebind_window(host, window, true, &camera);
                    }
                }
                Err(error) => {
                    warn!(
                        target: telemetry_target::EDITOR,
                        view,
                        error = %error,
                        "[editor] popped-out scene window refused"
                    );
                    return;
                }
            }
        }
        if let Some(Some(scene)) = self.views.get_mut(view) {
            scene.window.place(x, y, width, height, true);
        }
    }

    /// Gives a scene view its child window back when its tab returns to the
    /// dock.
    ///
    /// A view the platform forced onto a detached window is left alone: that
    /// window is the view's dock window, not a pop-out's.
    pub(super) fn ensure_view_docked(&mut self, event_loop: &ActiveEventLoop, view: usize) {
        let popped = self
            .views
            .get(view)
            .and_then(|scene| scene.as_ref())
            .is_some_and(|scene| scene.popped);
        if !popped {
            return;
        }
        let title = format!("Pill Editor Scene {}", view + 1);
        match SurfaceWindow::open(event_loop, &self.window, &title, "scene") {
            Ok(window) => {
                let host = &mut self.host;
                let camera = self.cameras[view];
                if let Some(Some(scene)) = self.views.get_mut(view) {
                    scene.rebind_window(host, window, false, &camera);
                }
            }
            Err(error) => warn!(
                target: telemetry_target::EDITOR,
                view,
                error = %error,
                "[editor] scene view redock refused"
            ),
        }
    }

    /// Places one scene view at the dock's scene rectangle, converted from
    /// logical points to the client area's physical pixels.
    pub(super) fn place_view_rect(&mut self, view: usize, rect: ShellRect) {
        let (x, y, width, height) = self.view_rect_pixels(rect);
        let Some(scene) = &mut self.views[view] else {
            return;
        };
        scene.body_rect = Some(rect);
        scene.window.place(x, y, width, height, false);
    }

    /// Places the game window at the dock's game rectangle, converted from
    /// logical points to the client area's physical pixels.
    pub(super) fn place_game_rect(&mut self, rect: ShellRect) {
        let (x, y, width, height) = self.view_rect_pixels(rect);
        let Some(game) = &mut self.game_surface else {
            return;
        };
        game.place(x, y, width, height, false);
    }

    /// Converts a dock rectangle from logical points to the client area's
    /// physical pixels, clamped inside the client area.
    ///
    /// The dock's rectangle can overshoot the window while a split is being
    /// dragged.
    ///
    /// A view's window is grown by [`COMPOSITION_VIEW_OVERSHOOT`] only while
    /// the composition overlay draws the UI above it. Without that overlay the
    /// view window is above the UI, so a grown window hides the tab bar and the
    /// edge of the panel next to it - the view looks too large and slightly out
    /// of place.
    pub(super) fn view_rect_pixels(&self, rect: ShellRect) -> (i32, i32, u32, u32) {
        let rect = if self.overlay_is_composition() {
            let overshoot = COMPOSITION_VIEW_OVERSHOOT as f32;
            ShellRect {
                x: rect.x - overshoot,
                y: rect.y - overshoot,
                width: rect.width + 2.0 * overshoot,
                height: rect.height + 2.0 * overshoot,
            }
        } else {
            rect
        };
        scaled_view_rect(rect, self.scale_factor, self.size)
    }

    /// Whether the UI is drawn above the scene and game windows.
    ///
    /// Only the composition overlay is: it is a visual over the whole window,
    /// while the window surface is the main window itself, which a child window
    /// covers.
    pub(super) fn overlay_is_composition(&self) -> bool {
        matches!(
            self.overlay.as_ref().map(OverlaySurface::mode),
            Some(OverlayMode::Composition)
        )
    }

    /// Hides one scene view's window because its tab is not shown this pass:
    /// another tab of its group is active, or the tab was closed.
    pub(super) fn hide_view(&mut self, view: usize) {
        if let Some(scene) = &mut self.views[view] {
            scene.window.hide();
        }
    }

    /// Hides the game window because its tab is not shown this pass, so no
    /// child window can paint over the tab that is.
    pub(super) fn hide_game(&mut self) {
        if let Some(game) = &mut self.game_surface {
            game.hide();
        }
    }

    /// Routes one scene window's event: activates the viewport on a press that
    /// is not over the UI's chips, starts camera navigation on its chord,
    /// starts a gizmo drag or asks for a pick on a left press, feeds a drag in
    /// flight its moves, and ends whatever the release ends.
    ///
    /// The viewport that owns an interaction keeps it: winit reports a drag's
    /// moves to the window whose press started it, even when the pointer
    /// leaves that window, so a gizmo drag or a navigation that crosses the
    /// viewport's edge carries on and no other viewport takes the pointer
    /// mid-drag.
    pub(super) fn scene_window_event(&mut self, view: usize, event: &WindowEvent) {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let point = (position.x as f32, position.y as f32);
                self.scene_cursor[view] = Some(point);
                if let Some(drag) = &self.camera_drag {
                    if drag.view == view {
                        self.update_camera_navigation(point);
                        return;
                    }
                }
                if let Some(drag) = &self.gizmo_drag {
                    if drag.view == view {
                        self.update_gizmo_drag(point.0, point.1);
                    }
                    return;
                }
                self.update_hover(view, point);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == winit::event::ElementState::Pressed;
                let button = match button {
                    winit::event::MouseButton::Left => CameraButton::Left,
                    winit::event::MouseButton::Middle => CameraButton::Middle,
                    winit::event::MouseButton::Right => CameraButton::Right,
                    _ => return,
                };
                if button == CameraButton::Left {
                    self.left_down[view] = pressed;
                }
                if !pressed {
                    if button == CameraButton::Left {
                        self.release_scene(view);
                    }
                    if self
                        .camera_drag
                        .as_ref()
                        .is_some_and(|drag| drag.view == view)
                    {
                        self.end_camera_navigation();
                    }
                    return;
                }
                // A press over one of the overlay's chips belongs to the UI:
                // it neither activates the viewport nor reaches the scene.
                if self.scene_point_is_ui(view) {
                    return;
                }
                self.activate_view(view);
                let Some(point) = self.scene_cursor[view] else {
                    return;
                };
                if let Some(navigation) = self.navigation_for(button) {
                    self.begin_camera_navigation(view, navigation, point);
                    return;
                }
                if button == CameraButton::Left {
                    self.press_scene(view, point.0, point.1);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, lines) => *lines,
                    winit::event::MouseScrollDelta::PixelDelta(position) => {
                        position.y as f32 / 40.0
                    }
                };
                if lines == 0.0 {
                    return;
                }
                // The wheel zooms the viewport it arrived over but does not
                // make it the active one: only a click activates.
                self.zoom_camera(view, lines);
            }
            _ => {}
        }
    }

    /// Makes `view` the active viewport: the one that camera navigation, the
    /// gizmo's handle length and the scripted viewport commands address.
    ///
    /// Only a press inside a scene view reaches here, so a click on the
    /// editor's own UI never activates a viewport.
    pub(super) fn activate_view(&mut self, view: usize) {
        if self.active_view == view {
            return;
        }
        self.active_view = view;
        info!(
            target: telemetry_target::EDITOR,
            view,
            "[editor] active viewport"
        );
    }

    /// Reconciles the popped-out windows with the shell's frames and paints
    /// them.
    ///
    /// The shell reports one frame per popped-out tab, so those frames are the
    /// windows: a tab that appears gets a window, a tab that disappears has its
    /// window closed, and every other frame is painted into its own window
    /// through its own renderer. The pass itself already ran in the shell, so
    /// nothing here reads egui's context.
    pub(super) fn present_popouts(
        &mut self,
        event_loop: &ActiveEventLoop,
        frames: &mut [pill_editor_core::boundary::PopoutFrame],
    ) {
        for frame in frames.iter() {
            if self.popouts.contains_key(&frame.tab) {
                continue;
            }
            self.open_popout(event_loop, &frame.tab, &frame.title);
        }
        // A tab that redocked (or was closed) loses its window.
        let live_tabs: Vec<String> = frames.iter().map(|frame| frame.tab.clone()).collect();
        let stale_tabs: Vec<String> = self
            .popouts
            .keys()
            .filter(|tab| !live_tabs.contains(tab))
            .cloned()
            .collect();
        for tab in stale_tabs {
            if self.popouts.remove(&tab).is_some() {
                info!(
                    target: telemetry_target::EDITOR,
                    tab = tab.as_str(),
                    "[editor] popout closed"
                );
            }
        }
        for frame in frames.iter() {
            let Some(popout) = self.popouts.get(&frame.tab) else {
                continue;
            };
            // A popped-out Scene tab reports its viewport in the pop-out's own
            // coordinates. The view's window follows it onto that window's
            // screen position, which is what lets it cross monitors.
            if frame.frame.scene_rects.iter().all(|rect| rect.is_none()) {
                continue;
            }
            let scale_factor = popout.window.scale_factor() as f32;
            let origin = popout
                .window
                .inner_position()
                .unwrap_or(PhysicalPosition::new(0, 0));
            let placements: Vec<(usize, i32, i32, u32, u32)> = frame
                .frame
                .scene_rects
                .iter()
                .enumerate()
                .filter_map(|(view, rect)| {
                    let rect = (*rect)?;
                    Some((
                        view,
                        origin.x + (rect.x * scale_factor) as i32,
                        origin.y + (rect.y * scale_factor) as i32,
                        (rect.width * scale_factor).max(1.0) as u32,
                        (rect.height * scale_factor).max(1.0) as u32,
                    ))
                })
                .collect();
            for (view, x, y, width, height) in placements {
                self.pop_view_window(event_loop, view, x, y, width, height);
            }
        }
        for frame in frames.iter_mut() {
            let Some(popout) = self.popouts.get_mut(&frame.tab) else {
                continue;
            };
            let textures = std::mem::take(&mut frame.frame.textures);
            match popout.surface.render(
                &self.device,
                &self.queue,
                &mut popout.renderer,
                &frame.frame.meshes,
                textures,
                frame.frame.pixels_per_point,
            ) {
                Ok(has_drawn) => {
                    // The suite's only signal that the window is live. It is
                    // logged once, so a broken painter shows up as a missing
                    // line instead of a window that was never painted.
                    if has_drawn && !popout.presented {
                        popout.presented = true;
                        info!(
                            target: telemetry_target::EDITOR,
                            tab = frame.tab.as_str(),
                            "[editor] popout presented"
                        );
                    }
                }
                Err(error) => warn!(
                    target: telemetry_target::EDITOR,
                    tab = frame.tab.as_str(),
                    error = %error,
                    "[editor] popout frame failed"
                ),
            }
        }
    }

    /// Opens one popped-out tab's window, a plain top-level window with its own
    /// swapchain and its own egui renderer.
    pub(super) fn open_popout(&mut self, event_loop: &ActiveEventLoop, tab: &str, title: &str) {
        let attributes = Window::default_attributes()
            .with_title(title)
            .with_inner_size(POPOUT_INITIAL_SIZE);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                warn!(
                    target: telemetry_target::EDITOR,
                    tab,
                    error = %error,
                    "[editor] popout window refused"
                );
                return;
            }
        };
        let window = Arc::new(window);
        let size = window.inner_size();
        let surface = match OverlaySurface::new_window(
            &self.instance,
            &self.adapter,
            &self.device,
            Arc::clone(&window),
            EDITOR_FORMAT,
            (size.width.max(1), size.height.max(1)),
        ) {
            Ok(surface) => surface,
            Err(error) => {
                warn!(
                    target: telemetry_target::EDITOR,
                    tab,
                    error = %error,
                    "[editor] popout surface refused"
                );
                return;
            }
        };
        let renderer = egui_wgpu::Renderer::new(
            &self.device,
            EDITOR_FORMAT,
            egui_wgpu::RendererOptions::default(),
        );
        info!(
            target: telemetry_target::EDITOR,
            tab,
            width = size.width,
            height = size.height,
            "[editor] popout opened"
        );
        self.popouts.insert(
            tab.to_string(),
            PopoutWindow {
                window,
                surface,
                renderer,
                presented: false,
            },
        );
    }

    /// Refreshes each popped-out window's input record with its geometry, its
    /// focus and this frame's rate. The record's events were appended earlier,
    /// as the window reported them.
    pub(super) fn publish_popout_geometry(&mut self) {
        if self.popouts.is_empty() {
            return;
        }
        let geometry: Vec<(String, f32, f32, f32, bool)> = self
            .popouts
            .iter()
            .map(|(tab, popout)| {
                let scale_factor = popout.window.scale_factor() as f32;
                let size = popout.window.inner_size();
                (
                    tab.clone(),
                    size.width as f32 / scale_factor.max(0.01),
                    size.height as f32 / scale_factor.max(0.01),
                    scale_factor,
                    popout.window.has_focus(),
                )
            })
            .collect();
        let elapsed_seconds = self.started.elapsed().as_secs_f64();
        let frames_per_second = self.fps;
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(inputs) = world.get_resource_mut::<EguiPopoutInputs>() else {
            return;
        };
        for (tab, width, height, scale_factor, focused) in geometry {
            let record = inputs.by_tab.entry(tab).or_default();
            record.screen_size = pill_core::math::Vector2f::new(width.max(1.0), height.max(1.0));
            record.scale_factor = scale_factor;
            record.focused = focused;
            record.time = elapsed_seconds;
            record.frames_per_second = frames_per_second;
        }
    }

    /// Closes `tab`'s window and redocks its tab. Closing a window is a
    /// redock, so the tab is never lost that way.
    pub(super) fn close_popout(&mut self, tab: &str) {
        if self.popouts.remove(tab).is_none() {
            return;
        }
        // The tab comes back through the dock's own redock rule, matched by
        // wire name. The shell owns the tab enum, so this side only knows the
        // name.
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(state) = world.get_resource_mut::<EguiShellState>() {
            if let Some(editor_tab) = EditorTab::from_wire(tab) {
                state.dock.redock(editor_tab);
            }
        }
        info!(
            target: telemetry_target::EDITOR,
            tab,
            "[editor] popout closed"
        );
    }

    /// Appends one event to `tab`'s input record, and keeps the window's own
    /// cursor position alongside it.
    pub(super) fn popout_event(&mut self, tab: &str, event: InputEvent) {
        let cursor = match &event {
            InputEvent::CursorMoved { position } => Some(*position),
            InputEvent::CursorLeft => None,
            _ => None,
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        let Some(inputs) = world.get_resource_mut::<EguiPopoutInputs>() else {
            return;
        };
        let record = inputs.by_tab.entry(tab.to_string()).or_default();
        match &event {
            InputEvent::CursorMoved { .. } => record.cursor = cursor,
            InputEvent::CursorLeft => record.cursor = None,
            _ => {}
        }
        record.events.push(event);
    }

    /// Clears every popped-out window's event list once the frame that read it
    /// has been presented, so no event is replayed into a second pass.
    pub(super) fn clear_popout_events(&mut self) {
        if self.popouts.is_empty() {
            return;
        }
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(inputs) = world.get_resource_mut::<EguiPopoutInputs>() {
            for record in inputs.by_tab.values_mut() {
                record.events.clear();
            }
        }
    }

    /// Draws one editor frame: the shell's frame through the overlay, or the
    /// runner's own fallback UI while no shell module is loaded.
    ///
    /// With no overlay at all, only the popped-out windows are painted; the
    /// main window's frame is skipped, and creating the composition overlay is
    /// retried.
    pub(super) fn present_ui(&mut self, event_loop: &ActiveEventLoop) {
        // Step 0: the floating overlay follows the editor window, and the
        // frame about to be drawn is where it must already be.
        self.sync_floating_overlay();
        // Step 1: take the shell's frame, or build the runner's own minimal
        // one while no shell module is loaded.
        let frame = {
            let world = self.host.host_mut().engine_mut().world_mut();
            match world.get_resource_mut::<EguiShellFrame>() {
                Some(resource) => std::mem::take(&mut *resource),
                None => self.fallback_frame(),
            }
        };
        let EguiShellFrame {
            meshes,
            textures,
            pixels_per_point,
            wants_pointer,
            wants_keyboard,
            cursor_icon,
            clipboard_text,
            scene_rects,
            game_rect,
            overlay_regions,
            ime_allowed,
            ime_rect,
            drag_out,
            // The pass's asks are drained at the frame boundary (Step 7b), not
            // while the frame is presented.
            requests: _,
            // The menu bar's quit is drained at the frame boundary (Step 7b),
            // not while the frame is presented.
            quit_requested: _,
            // egui's repaint request: the redraw decision at the end of the
            // frame counts it as a reason to draw.
            repaint_after,
            // One frame per popped-out tab; each is painted into its own
            // window below.
            mut popout_frames,
        } = frame;
        self.repaint_after = repaint_after;
        // Each scene viewport follows its dock rectangle, so the views track
        // the layout while a splitter is dragged, and a viewport whose tab is
        // not shown hides its window. The list has one slot per view the dock
        // holds, so a view the person just added opens here, on the frame its
        // tab first drew. A view whose tab was closed keeps its window hidden
        // instead of losing it.
        self.ensure_scene_views(scene_rects.len(), event_loop);
        for view in 0..self.views.len() {
            match scene_rects.get(view).copied().flatten() {
                Some(rect) => {
                    // A view whose tab is back in the dock needs its child
                    // window again.
                    self.ensure_view_docked(event_loop, view);
                    self.place_view_rect(view, rect);
                }
                None => self.hide_view(view),
            }
        }
        // The game viewport follows its own rectangle the same way: the
        // primary surface's window sits where the Game tab's body is, and it
        // disappears while another tab of its group is active.
        match game_rect {
            Some(rect) => self.place_game_rect(rect),
            None => self.hide_game(),
        }
        // The chips the shell drew over the viewports, kept for scene click
        // routing: a click inside one belongs to the UI, not the scene.
        self.overlay_regions = overlay_regions;
        // Step 2: log the change when egui starts or stops claiming the
        // pointer or the keyboard.
        if wants_pointer != self.claimed_pointer || wants_keyboard != self.claimed_keyboard {
            info!(
                target: telemetry_target::EDITOR,
                pointer = wants_pointer,
                keyboard = wants_keyboard,
                "[editor] egui input claims changed"
            );
            self.claimed_pointer = wants_pointer;
            self.claimed_keyboard = wants_keyboard;
        }
        self.apply_cursor_icon(cursor_icon);
        // An IME the shell asked for: the window takes the candidate window,
        // and the caret's rectangle places it. A text field that loses focus
        // turns it off again.
        self.apply_ime(ime_allowed, ime_rect);
        // A tile dragged past the window edge: the OS gets the file through
        // an OLE drag, and the editor's thread runs the modal loop until the
        // drop ends.
        if let Some(path) = drag_out {
            self.start_drag_out(&path);
        }
        if let Some(text) = clipboard_text {
            // egui's own copy (a text field's selection) goes through the
            // same clipboard writer the editor's structured copy uses.
            match crate::clipboard::write(&text) {
                Ok(()) => info!(
                    target: telemetry_target::EDITOR,
                    bytes = text.len(),
                    "[editor] clipboard written"
                ),
                Err(message) => warn!(
                    target: telemetry_target::EDITOR,
                    error = message.as_str(),
                    "[editor] clipboard write failed"
                ),
            }
        }
        // Step 2b: the popped-out windows - create the ones the shell reported
        // for the first time, close the ones it stopped reporting, and paint
        // each frame into its own window.
        self.present_popouts(event_loop, &mut popout_frames);
        // Step 3: draw through the overlay; retry a missing overlay rarely,
        // so a transient compositor failure heals.
        if self.overlay.is_none() {
            self.retry_overlay();
        }
        let Some(overlay) = &mut self.overlay else {
            return;
        };
        if let Err(error) = overlay.render(
            &self.device,
            &self.queue,
            &mut self.egui_renderer,
            &meshes,
            textures,
            pixels_per_point,
        ) {
            warn!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[editor] overlay frame failed"
            );
        }
        // The popped-out windows' events have been through their passes, so
        // the next frame starts with none.
        self.clear_popout_events();
    }

    /// Builds the safe-mode panel's frame, used while no shell module is
    /// loaded: the build errors that stopped it, each with its `file:line`,
    /// and a Retry button that asks the host to rebuild the module. The scene
    /// keeps rendering behind the panel, because the editor's own frame is
    /// untouched.
    pub(super) fn fallback_frame(&mut self) -> EguiShellFrame {
        // The fallback uses the same theme the shell does, so a theme change
        // has to reach this posture too: the theme is applied again whenever
        // its generation moves.
        if self.fallback_theme_generation != self.theme_generation {
            self.fallback_theme_generation = self.theme_generation;
            pill_editor_core::style::apply_theme(
                &self.fallback_context,
                &self.theme,
                &self.theme_layers,
                self.theme_generation,
                &self.theme_fonts,
            );
        }
        if !self.reported_fallback {
            self.reported_fallback = true;
            info!(
                target: telemetry_target::EDITOR,
                module = SHELL_MODULE_NAME,
                problems = self.notices.problems.len(),
                "[editor] safe mode"
            );
        }
        let logical_width = self.size.0 as f32 / self.scale_factor;
        let logical_height = self.size.1 as f32 / self.scale_factor;
        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(logical_width, logical_height),
            )),
            ..Default::default()
        };
        let fallback_context = self.fallback_context.clone();
        let mut retry_requested = false;
        let problems: Vec<(String, String, String)> = self
            .notices
            .problems
            .iter()
            .map(|row| {
                let location = match (&row.problem.file, row.problem.line) {
                    (Some(file), Some(line)) => format!("{file}:{line}"),
                    (Some(file), None) => file.clone(),
                    _ => "-".to_string(),
                };
                (
                    row.module.clone(),
                    location,
                    format!("{} {}", row.problem.severity.name(), row.problem.message),
                )
            })
            .collect();
        let output = fallback_context.run_ui(raw_input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.heading("Pill Editor - safe mode");
                ui.label(format!(
                    "the shell module ({SHELL_MODULE_NAME}) is not loaded; the scene \
                     keeps rendering behind this panel"
                ));
                ui.separator();
                if problems.is_empty() {
                    ui.weak("no build error was reported");
                } else {
                    egui::ScrollArea::vertical()
                        .max_height(360.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (module, location, text) in &problems {
                                ui.label(egui::RichText::new(module).strong());
                                ui.label(
                                    egui::RichText::new(format!("  {location}  {text}"))
                                        .monospace(),
                                );
                            }
                        });
                }
                ui.separator();
                if ui
                    .add(egui::Button::new("Retry"))
                    .on_hover_text("rebuild and load the shell module")
                    .clicked()
                {
                    retry_requested = true;
                }
            });
        });
        if retry_requested {
            self.retry_shell_module();
        }
        frame_from_output(&fallback_context, output)
    }

    /// Tries to create the composition overlay again, at most once every 120
    /// frames.
    ///
    /// The window-surface posture is never retried: either
    /// `PILL_EDITOR_NO_OVERLAY` asked for it, or it was already taken as the
    /// fallback. The composition overlay is the posture that failed, so it is
    /// the one worth another attempt - and it only exists where
    /// DirectComposition does, which is Windows.
    pub(super) fn retry_overlay(&mut self) {
        if self.no_overlay {
            return;
        }
        #[cfg(windows)]
        self.retry_composition_overlay();
    }

    /// One attempt at the composition overlay, on the retry schedule.
    #[cfg(windows)]
    fn retry_composition_overlay(&mut self) {
        if self.frame_index < self.overlay_retry_frame + 120 {
            return;
        }
        self.overlay_retry_frame = self.frame_index;
        match OverlaySurface::new(
            &self.instance,
            &self.adapter,
            &self.device,
            Arc::clone(&self.window),
            EDITOR_FORMAT,
            self.size,
        ) {
            Ok(overlay) => {
                info!(
                    target: telemetry_target::EDITOR,
                    "[editor] composition overlay created"
                );
                self.overlay = Some(overlay);
            }
            Err(error) => {
                warn!(
                    target: telemetry_target::EDITOR,
                    error = %error,
                    "[editor] composition overlay still unavailable"
                );
            }
        }
    }

    /// Asks the host to rebuild the shell module and logs the outcome.
    ///
    /// `retry_extension` runs the same load a first start would, or asks a
    /// loaded generation to rebuild as if its sources had been saved, so a fix
    /// made while the editor is in safe mode takes effect without a restart.
    pub(super) fn retry_shell_module(&mut self) {
        let retry_outcome = self.host.host_mut().retry_extension(SHELL_MODULE_NAME);
        match retry_outcome {
            Ok(()) => {
                info!(
                    target: telemetry_target::EDITOR,
                    module = SHELL_MODULE_NAME,
                    "[editor] safe mode retry requested"
                );
                // The next frame looks the shell up again; a generation that
                // loaded in the meantime takes over without another step.
                self.reported_fallback = false;
            }
            Err(error) => warn!(
                target: telemetry_target::EDITOR,
                module = SHELL_MODULE_NAME,
                error = error.as_str(),
                "[editor] safe mode retry failed"
            ),
        }
    }

    /// Sets the cursor shape egui asked for, when it maps onto one of winit's
    /// shapes; any other icon leaves the cursor as it is.
    ///
    /// The resize shapes matter as much as the pointing hand: the dock asks
    /// for one over a splitter, and without it the wider grab area around the
    /// splitter is invisible to the person dragging it.
    pub(super) fn apply_cursor_icon(&mut self, icon: egui::CursorIcon) {
        if icon != self.last_cursor {
            info!(
                target: telemetry_target::EDITOR,
                icon = format!("{icon:?}").as_str(),
                "[editor] cursor icon"
            );
            self.last_cursor = icon;
        }
        let shape = match icon {
            egui::CursorIcon::Default => winit::window::CursorIcon::Default,
            egui::CursorIcon::PointingHand => winit::window::CursorIcon::Pointer,
            egui::CursorIcon::Text => winit::window::CursorIcon::Text,
            egui::CursorIcon::Crosshair => winit::window::CursorIcon::Crosshair,
            egui::CursorIcon::Grab => winit::window::CursorIcon::Grab,
            egui::CursorIcon::Grabbing => winit::window::CursorIcon::Grabbing,
            egui::CursorIcon::ResizeHorizontal => winit::window::CursorIcon::EwResize,
            egui::CursorIcon::ResizeVertical => winit::window::CursorIcon::NsResize,
            egui::CursorIcon::ResizeNeSw => winit::window::CursorIcon::NeswResize,
            egui::CursorIcon::ResizeNwSe => winit::window::CursorIcon::NwseResize,
            egui::CursorIcon::ResizeEast => winit::window::CursorIcon::EResize,
            egui::CursorIcon::ResizeWest => winit::window::CursorIcon::WResize,
            egui::CursorIcon::ResizeNorth => winit::window::CursorIcon::NResize,
            egui::CursorIcon::ResizeSouth => winit::window::CursorIcon::SResize,
            egui::CursorIcon::ResizeColumn => winit::window::CursorIcon::ColResize,
            egui::CursorIcon::ResizeRow => winit::window::CursorIcon::RowResize,
            _ => return,
        };
        self.window.set_cursor(shape);
    }

    /// Applies the shell's IME state to the main window.
    ///
    /// egui decides when a text field wants the input method, and where its
    /// caret is. This executable owns the window, so it is the only place that
    /// can pass those decisions to the platform. The rectangle is in logical
    /// points, the unit the window's IME cursor area takes.
    pub(super) fn apply_ime(
        &mut self,
        allowed: bool,
        rect: Option<pill_editor_core::boundary::ShellRect>,
    ) {
        let window = self.window.clone();
        if allowed != self.ime_allowed {
            window.set_ime_allowed(allowed);
            self.ime_allowed = allowed;
            info!(
                target: telemetry_target::EDITOR,
                allowed,
                "[editor] ime allowed changed"
            );
        }
        if !allowed {
            return;
        }
        if let Some(rect) = rect {
            if self.ime_rect != Some((rect.x, rect.y, rect.width, rect.height)) {
                self.ime_rect = Some((rect.x, rect.y, rect.width, rect.height));
                window.set_ime_cursor_area(
                    winit::dpi::LogicalPosition::new(rect.x as f64, rect.y as f64),
                    winit::dpi::LogicalSize::new(rect.width as f64, rect.height as f64),
                );
                info!(
                    target: telemetry_target::EDITOR,
                    x = rect.x,
                    y = rect.y,
                    width = rect.width,
                    height = rect.height,
                    "[editor] ime cursor area"
                );
            }
        }
    }
}
