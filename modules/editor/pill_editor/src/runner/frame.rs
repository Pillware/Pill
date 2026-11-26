//! Runs one editor frame as a series of named phases.
//!
//! # Responsibilities
//!
//! - Run one editor frame in order: the frame rate sample, input, queued
//!   requests, published resources, the world frame, its answers and the
//!   presentation. Each phase is a method of its own.
//! - Decide when the loop draws next, from the reasons this frame recorded,
//!   and the wake it leaves for the event loop to wait on.
//!
//! # Design
//!
//! Every phase runs on the same borrow of the window, so splitting them into
//! methods organizes the frame but decouples nothing: each method name says
//! which step it performs, and `frame` is the list of steps in order. The
//! publish-before-run and consume-after-run rule the engine relies on is
//! stated once, in `frame`'s doc.

// Current crate
use super::*;

impl EditorWindow {
    /// Runs one editor frame, calling each phase method in order.
    ///
    /// Returns whether the run should end. The event loop is passed in because
    /// a scene view the dock opened this frame creates its child window
    /// through it.
    ///
    /// The order of the phases matters. The resources a pass reads are
    /// published before [`Self::run_world`], the answers that pass produced are
    /// consumed after it, and presentation is last. A later phase never reads
    /// what an earlier one computed in a local variable - it gets a field or a
    /// return value instead.
    ///
    /// # Errors
    ///
    /// Returns [`EditorError::Renderer`] when the world frame fails.
    pub(super) fn frame(&mut self, event_loop: &ActiveEventLoop) -> Result<bool, EditorError> {
        self.frame_index += 1;
        let frame_started = Instant::now();
        self.begin_frame(frame_started);
        self.apply_window_size();
        self.publish_frame_inputs();
        self.service_editor_requests();
        self.publish_frame_resources();
        self.run_world()?;
        self.consume_frame_answers();
        self.present_frame(event_loop);
        self.end_frame(frame_started);
        Ok(self.quit)
    }

    /// Samples this frame's rate and logs the heartbeat line when one is due.
    pub(super) fn begin_frame(&mut self, frame_started: Instant) {
        // Step 1: the frame rate heartbeat, for the person reading the log.
        let now = frame_started;
        if let Some(last_frame_time) = self.last_frame_time {
            let delta_time = now.duration_since(last_frame_time).as_secs_f32();
            if delta_time > 0.0 {
                let instantaneous_fps = 1.0 / delta_time;
                self.fps = if self.fps > 0.0 {
                    self.fps * (1.0 - FPS_SMOOTHING) + instantaneous_fps * FPS_SMOOTHING
                } else {
                    instantaneous_fps
                };
            }
        }
        self.last_frame_time = Some(now);
        if self.frame_index.is_multiple_of(FRAME_HEARTBEAT_FRAMES)
            || self
                .last_heartbeat
                .is_none_or(|at| now.duration_since(at).as_secs_f32() >= FRAME_HEARTBEAT_SECONDS)
        {
            self.last_heartbeat = Some(now);
            let overlay_frame_count = self
                .overlay
                .as_ref()
                .map(OverlaySurface::frames)
                .unwrap_or(0);
            let idle_frame_millis_max = self.idle_frame_millis_max;
            self.idle_frame_millis_max = 0.0;
            debug!(
                target: telemetry_target::EDITOR,
                fps = self.fps,
                frame = self.frame_index,
                overlay_frames = overlay_frame_count,
                idle_frame_millis = idle_frame_millis_max,
                "[editor] frame heartbeat"
            );
        }
    }

    /// Publishes this frame's inputs, and the state the shell's pass reads.
    ///
    /// The window events routed to the game become engine input, and the
    /// cameras and badges the frame draws are published. The shell's own input
    /// record is filled here, before the pass that reads it. The host's notices
    /// and the open scene's title are reported here as well.
    pub(super) fn publish_frame_inputs(&mut self) {
        // A border-band resize in flight follows this frame's pointer first, so
        // everything below reads the size it leaves behind.
        self.step_resize_drag();
        // Step 2: the window events the focus rule routed to the game's window
        // become engine input events. The editor's own windows feed the shell's
        // egui pass instead, so neither side sees the other's keys and clicks.
        for event in self.pending_events.drain(..) {
            self.host.host_mut().engine_mut().push_input_event(event);
        }
        // Step 2a: any camera a navigation moved is published again, so the
        // renderer draws the frame with the view the user just steered.
        self.publish_cameras();
        // Step 2b: a scripted drag in flight takes its next stage - the
        // press, the move and the release, one per frame.
        self.advance_scripted_drag();
        // Step 2c: what the host did since the last frame - a reload, a patch,
        // a failure, a rollback - becomes the notices the shell turns into
        // toasts and Problems rows.
        self.fold_host_notices();
        // Step 2c2: where each visible entity appears in each visible scene
        // view, for the shell's badges.
        self.publish_badges();
        // Step 2d: the document's title is reported when it changes, so the log
        // names the scene the session holds. The window's own title names the
        // project instead.
        self.report_scene_title();
        // Step 2e: the marquee the shell draws over a viewport, read before the
        // world borrow below takes `self` apart.
        let marquee = self.shell_marquee();
        // Step 3: the shell boundary's input resource.
        if let Some(shell_input) = self
            .host
            .host_mut()
            .engine_mut()
            .world_mut()
            .get_resource_mut::<EguiShellInput>()
        {
            // The modifier state is pushed first, so the bridge starts from
            // the keys that are held rather than from nothing.
            let mut events = Vec::with_capacity(self.shell_events.len() + 1);
            events.push(InputEvent::ModifiersChanged {
                modifiers: EngineModifiers {
                    shift: self.modifiers.shift,
                    control: self.modifiers.control,
                    alt: self.modifiers.alt,
                    super_key: false,
                },
            });
            events.append(&mut self.shell_events);
            // A paste chord in this frame's events carries the OS clipboard's
            // text into egui, so a focused text field pastes.
            let paste_text = paste_chord(&events).then(|| {
                crate::clipboard::read()
                    .ok()
                    .flatten()
                    .filter(|text| !text.is_empty())
            });
            *shell_input = EguiShellInput {
                screen_size: pill_core::math::Vector2f::new(
                    self.size.0 as f32 / self.scale_factor,
                    self.size.1 as f32 / self.scale_factor,
                ),
                scale_factor: self.scale_factor,
                focused: self.focused,
                // The strip draws the maximize or the restore glyph from this,
                // and a maximized window registers no resize bands.
                maximized: self.window.is_maximized(),
                // This window has no system frame, so the shell draws the
                // window buttons, the drag zones and the resize bands.
                has_caption: true,
                // The strip names the project where a system title bar would.
                project_name: std::sync::Arc::clone(&self.project_name),
                time: self.started.elapsed().as_secs_f64(),
                frames_per_second: self.fps,
                events,
                cursor: self.shell_cursor,
                paste_text: paste_text.flatten(),
                badges: self.badges.clone(),
                marquee,
                // Only the scripted channel needs the shell's row geometry
                // logged, so a person's editor logs none of it.
                scripted: self.test_channel.is_some(),
            };
        }
        // Step 3a2: refresh each popped-out window's input record with its
        // geometry, focus and frame rate for this frame. The record's events
        // are appended as that window reports them, and cleared once the frame
        // that read them has been presented, so no event is replayed.
        self.publish_popout_geometry();
    }

    /// Applies what the editor asked for since the last frame.
    ///
    /// The queued [`EditorCommand`]s drain here - the frame boundary their
    /// contract names. The watchers, the panels and the clipboard follow, all
    /// before the snapshot, so the tabs draw this frame's state and not a
    /// frame-old one.
    pub(super) fn service_editor_requests(&mut self) {
        // Step 3: publish the viewport state a callback reads, before anything
        // applies, so a hook of a command queued last frame sees this frame's
        // pointer.
        self.publish_viewport_state();
        // Step 3a: bind the callbacks the loaded modules registered, before
        // anything applies, so a module's first edits already reach it.
        self.sync_hooks();
        // Step 3b: apply the engine mutations the shell's assets actions and
        // the scripted channel queued - the frame boundary the
        // `EditorCommand` contract names. A failed command never aborts the
        // rest of the batch.
        self.apply_pending_commands();
        // The notices those callbacks posted take the executable's toast path,
        // so its next republish of the notice list cannot erase them.
        self.apply_hook_notices();
        // Step 3c: write the gizmo's settings back when they changed, then read
        // the live preference, theme and keybinding files. All of it is before
        // the snapshot, so the tabs see the state this frame's edits left. The
        // console history takes the new lines from the log ring buffer and
        // applies the Console tab's filters to them.
        self.persist_gizmo_settings();
        self.persist_gizmo_visibility();
        self.watch_preferences();
        self.watch_theme();
        self.watch_keybindings();
        self.publish_console_lines();
        // Step 3d: apply the recovery document a killed run left behind first,
        // before the dock is read. Save the arrangement and selection again
        // when they change and on a slow cadence. Then register the panels the
        // loaded modules asked for, and apply what the panels drawn last frame
        // asked for. It is all before the snapshot, so a tab's title is never
        // one frame behind.
        self.apply_recovered_state();
        self.autosave_recovery();
        // The commands the loaded modules queued, before the panels whose
        // rows draw them.
        self.sync_commands();
        self.sync_panels();
        self.apply_panel_commands();
        // The quit prompt's answer, and a save it started, after the panel
        // commands so a module's save notices show before the editor quits.
        self.drive_quit_prompt();
        // The clipboard request the shell queued in the last frame: the OS half
        // belongs to this executable, so it is applied here like every other
        // queued request.
        self.apply_clipboard();
    }

    /// Publishes the viewport state a callback reads.
    ///
    /// The active view and the pointer live in this executable, so they reach
    /// the editor API's context through a resource. The pointer is stored in
    /// the scene window's physical pixels and is published in logical points;
    /// the rectangle is the one the shell drew last frame, because the frame's
    /// draw happens after this boundary.
    pub(super) fn publish_viewport_state(&mut self) {
        let active_view = self.active_view;
        let pointer_in_view = self
            .scene_cursor
            .get(active_view)
            .copied()
            .flatten()
            .map(|(x, y)| (x / self.scale_factor, y / self.scale_factor));
        let state = EditorViewportState {
            active_view: Some(active_view),
            pointer_in_view,
            view_rect: self.scene_rects.get(active_view).copied().flatten(),
            view_count: self.views.len(),
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        EditorViewportState::ensure(world);
        if let Some(slot) = world.get_resource_mut::<EditorViewportState>() {
            *slot = state;
        }
    }

    /// Copies the scene cameras when a play session starts and puts them back
    /// when it stops.
    ///
    /// The engine restores the entities and the editor restores its own state;
    /// a camera is neither, because it lives in this executable. A session that
    /// moved a view therefore has to be undone here, or Stop would leave the
    /// person looking from wherever the game left the camera.
    pub(super) fn track_play_cameras(&mut self) {
        let run_mode = self.host.host().engine().run_mode();
        if self.last_run_mode == Some(run_mode) {
            return;
        }
        self.last_run_mode = Some(run_mode);
        match run_mode {
            pill_engine_core::RunMode::Playing => {
                self.play_cameras = Some(self.cameras.clone());
            }
            pill_engine_core::RunMode::Editing => {
                if let Some(cameras) = self.play_cameras.take() {
                    self.cameras = cameras;
                    // The restored poses have to reach the renderer, which
                    // reads them from the surface bindings.
                    self.camera_dirty = vec![true; self.camera_dirty.len()];
                    info!(
                        target: telemetry_target::EDITOR,
                        views = self.cameras.len(),
                        "[editor] play restored the cameras"
                    );
                }
            }
        }
    }

    /// Publishes the resources the frame reads, before it runs.
    pub(super) fn publish_frame_resources(&mut self) {
        // Step 4: the frame's snapshot for the shell's data tabs, when the
        // shell loaded and registered its resources.
        self.publish_snapshot();
        // Step 4a: the cameras follow a play session, before the frame draws.
        self.track_play_cameras();
        // Step 5: the world resources the frame draws and picks with,
        // published before it runs: the shared selection's outline set (one
        // write per change), the same selection as the engine's gizmo pass sees
        // it, the gizmo handles for that selection and the active tool's
        // element handles beside them.
        self.publish_selection_outline();
        self.publish_gizmo_selection();
        self.publish_gizmo_geometry();
        self.publish_element_handles();
    }

    /// Runs the world frame; the shell's egui pass runs inside it.
    ///
    /// # Errors
    ///
    /// Returns [`EditorError::Renderer`] when the host's frame fails.
    pub(super) fn run_world(&mut self) -> Result<(), EditorError> {
        // Step 6: run the world frame; the shell's egui pass runs inside it
        // (a registered post-update system) and leaves the frame resource
        // for presentation.
        self.host.run_frame().map_err(EditorError::Renderer)?;
        Ok(())
    }

    /// Consumes the answers this frame's pass produced.
    pub(super) fn consume_frame_answers(&mut self) {
        // Step 7: answers to scene picks that arrived during the frame
        // move the shared selection; the outline follows on the next
        // publish.
        self.drain_scene_picks();
        // Step 7a: the asset previews the renderer read back, and the next one
        // to ask for.
        self.apply_asset_previews();
        // Step 7b: what the shell pass asked for, taken from the frame it left:
        // undo and redo, a dock tab, a Problems row and a quit. Each goes
        // through the same path the scripted channel uses.
        self.drain_shell_requests();
    }

    /// Draws the shell frame through the overlay; no later phase draws.
    pub(super) fn present_frame(&mut self, event_loop: &ActiveEventLoop) {
        // Step 8: draw the shell frame through the overlay.
        self.present_ui(event_loop);
    }

    /// Runs the scripted channel and the quit deadline, then decides the next
    /// wake.
    pub(super) fn end_frame(&mut self, frame_started: Instant) {
        // Step 9: the scripted channel, then the auto-quit deadline.
        self.poll_test_channel();
        if let Some(seconds) = self.test_seconds {
            if self.started.elapsed().as_secs_f64() >= seconds {
                info!(
                    target: telemetry_target::EDITOR,
                    "[editor] test deadline reached; quitting"
                );
                self.request_quit();
            }
        }
        // Step 10: what the loop does next - draw again at once when anything
        // asked for a frame, or sleep until the idle poll. Either way this
        // frame's demand is cleared now.
        let frame_millis = frame_started.elapsed().as_secs_f32() * 1000.0;
        if self.polling {
            self.idle_frame_millis_max = self.idle_frame_millis_max.max(frame_millis);
        }
        self.decide_next_wake(frame_millis);
        // The redraw is one named Tracy frame of its own. It is a secondary
        // frame, not a primary one: the engine already marks a primary frame
        // per world frame, and the editor runs exactly one of those per redraw,
        // so a second primary mark here would split every redraw into two
        // frames. This one spans the whole redraw, presentation included.
        pill_core::profile_secondary_frame_mark!("UI refresh");
    }

    /// Decides when the loop draws next, from what asked for this frame.
    ///
    /// `frame_millis` is how long this frame took, in milliseconds. The demand
    /// is cleared here rather than at the start of the next frame. An event
    /// that arrives between the two sets it again, and the loop already knows
    /// to draw at once. egui's own repaint request counts as a reason too, so
    /// whatever egui asked to redraw gets its frame.
    ///
    /// A frame that leaves the loop polling logs `[editor] frame idle` once per
    /// transition, with this frame's cost.
    pub(super) fn decide_next_wake(&mut self, frame_millis: f32) {
        let demand = FrameDemand {
            animating: self.animating(),
            repaint_requested: self.repaint_after.is_some(),
            ..self.demand
        };
        self.cadence.record_frame(demand);
        self.last_demand = demand;
        self.next_wake = self.cadence.decide(demand);
        let polling = matches!(self.next_wake, FrameWake::After(_));
        if polling {
            self.cadence.record_wait();
        }
        if polling && !self.polling {
            info!(
                target: telemetry_target::EDITOR,
                poll_millis = pill_editor_core::cadence::IDLE_POLL.as_millis() as u64,
                frame_millis,
                "[editor] frame idle"
            );
        }
        self.polling = polling;
        self.demand = FrameDemand::default();
        self.repaint_after = None;
    }
    /// Whether something is moving, so the next frame is already owed.
    ///
    /// A paused project stops animating, but a camera navigation and a
    /// scripted drag still move something.
    pub(super) fn animating(&self) -> bool {
        let running = !self.host.host().engine().systems_paused();
        running || self.camera_drag.is_some() || self.scripted_drag.is_some()
    }

    /// The wake the last frame left for the loop.
    pub(super) fn next_wake(&self) -> FrameWake {
        self.next_wake
    }
}
