//! The transform gizmo: its handles' geometry, its drags and their history.
//!
//! # Responsibilities
//!
//! - Publish the handles' geometry and their screen positions in pixels for
//!   the active tool, so the hover test can name the handle under a pixel.
//! - Run a drag: the press's ray, the per-move write, and the release that
//!   records one undoable step.
//! - Draw the guides an active drag shows.
//!
//! # Design
//!
//! The drag's math lives in `pill_editor_core::gizmo`. This module reads the
//! world and the view cameras, writes the transform fields and queues the
//! recorded history step. Every move is computed from the transform at the
//! press, not from the previous move, so a snapped drag cannot add up
//! rounding errors. The release writes the press's value back before it
//! queues that step, because the recorded command takes its inverse from the
//! world's current value. Without the write, the inverse would be the final
//! value and the first undo would change nothing.
//!
//! A drag over an element selection takes the other path: the same measurement
//! turns into one world transform per element, and the active tool writes them
//! into whatever component it owns. The gizmo therefore never learns a tool's
//! component, and a tool never does frame maths - it is handed the finished
//! transforms.

// Current crate
use super::*;

// =============================================================================
// Free Functions
// =============================================================================

/// The four numbers a drag's last measurement reports, for its end log.
fn drag_value_components(value: gizmo::DragValue) -> [f32; 4] {
    match value {
        gizmo::DragValue::Distance(distance) => [distance, 0.0, 0.0, 0.0],
        gizmo::DragValue::Angle(radians) => [radians, 0.0, 0.0, 0.0],
        gizmo::DragValue::Plane(first, second) => [first, second, 0.0, 0.0],
    }
}

// =============================================================================
// Editor Window
// =============================================================================

impl EditorWindow {
    /// Publishes the gizmo handles for the selection: their hover and drag
    /// states, the translation planes, the dynamic scale handles and the drag
    /// guides.
    ///
    /// The handles are the renderer contract's own primitives, with handle
    /// ids set. The geometry that is drawn is also the geometry the hover
    /// test reads, so a handle a drag grabs is always one that was on
    /// screen. Each handle keeps a constant size on screen: its length in the
    /// world follows the active view's camera distance. The debug drawing
    /// holds only one frame's worth of gizmo geometry, so nothing has to be
    /// cleared here.
    pub(super) fn publish_gizmo_geometry(&mut self) {
        // Step 1: read the frame's inputs. The borrow of the world must end
        // before the camera helpers run, because those borrow `self`.
        let (gizmo_settings, entity, transform, basis, frame_name, element_press) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let gizmo_state = world
                .get_resource::<GizmoState>()
                .copied()
                .unwrap_or_default();
            let selected_entity = world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.primary());
            match selected_entity.and_then(|entity| {
                read_transform(world, entity).map(|transform| (entity, transform))
            }) {
                Some((entity, transform)) => {
                    // The handles are drawn in the frame a press would drag
                    // in, so what is on screen and what a drag measures are
                    // the same axes.
                    let basis = gizmo_frame_basis(world, entity, gizmo_state, &transform);
                    let frame_name = active_tool_frame(world, entity, gizmo_state.tool_frame)
                        .map(|frame| frame.name);
                    let element_press =
                        element_drag_elements(world, entity).map(|(_, press)| press);
                    (
                        gizmo_state.settings,
                        entity,
                        transform,
                        basis,
                        frame_name,
                        element_press,
                    )
                }
                None => {
                    // No handles on screen. The next selection logs its
                    // anchor at once instead of waiting for the heartbeat.
                    self.gizmo_logged = None;
                    self.gizmo_geometry = GizmoGeometry::default();
                    self.gizmo_reference = 0.0;
                    self.hovered_handle = None;
                    return;
                }
            }
        };
        // Step 2: build the handles and hand them to the debug drawing the
        // renderer reads. A drag over an element selection pivots on the
        // elements, so a turn and a scale carry all of them.
        let gizmo_origin = match &element_press {
            Some(press) => element_pivot(press, transform.translation),
            None => transform.translation,
        };
        let view = self.active_view;
        let handle_length = self.gizmo_length(view, gizmo_origin);
        let handle_states = HandleStates {
            hovered: self.hovered_handle,
            active: self.gizmo_drag.as_ref().map(|drag| drag.handle),
        };
        // A scale drag grows or shrinks the dragged axis' handle with the
        // factor it is writing. The length is always computed from the scale
        // at the press, never from the previous frame's, so it cannot drift.
        let handle_lengths = match self.gizmo_drag.as_ref() {
            Some(drag) => match drag.kind {
                HandleKind::Axis {
                    mode: GizmoMode::Scale,
                    axis,
                } => gizmo::scale_handle_lengths(
                    drag.reference,
                    drag.original.scale,
                    transform.scale,
                    axis,
                ),
                _ => [handle_length; 3],
            },
            None => [handle_length; 3],
        };
        let geometry = gizmo::build_geometry_styled(
            gizmo_settings.mode,
            gizmo_origin,
            basis,
            handle_lengths,
            handle_states,
        );
        let screen_anchor = self.project_to_screen(gizmo_origin);
        let guides = self.gizmo_drag.as_ref().map(|drag| {
            (
                drag.kind,
                drag.basis,
                drag.start_pivot,
                drag.current_pivot,
                drag.reference,
            )
        });
        let world = self.host.host_mut().engine_mut().world_mut();
        if world.get_resource::<DebugDraw>().is_none() {
            world.insert_resource(DebugDraw::new(true));
        }
        if let Some(debug_draw) = world.get_resource_mut::<DebugDraw>() {
            for (start, end, color, handle) in &geometry.lines {
                debug_draw.gizmo_line(*start, *end, *color, *handle);
            }
            for (corners, color, handle) in &geometry.triangles {
                debug_draw.gizmo_triangle(*corners, *color, *handle);
            }
            if let Some((kind, drag_basis, start_pivot, current_pivot, reference)) = guides {
                draw_gizmo_guides(
                    debug_draw,
                    kind,
                    drag_basis,
                    start_pivot,
                    current_pivot,
                    reference,
                );
            }
        }
        let guide_kind = self.gizmo_drag.as_ref().map(|drag| drag.kind);
        let dotted = guide_kind.is_some_and(|kind| gizmo_drag_mode(kind) == GizmoMode::Translate);
        let dragged_axis = guide_kind.and_then(HandleKind::axis).map(|(_, axis)| axis);
        if self.gizmo_guides != Some(guide_kind.is_some()) {
            self.gizmo_guides = Some(guide_kind.is_some());
            match dragged_axis {
                Some(axis) => info!(
                    target: telemetry_target::EDITOR,
                    axis = gizmo_axis_name(axis),
                    dotted,
                    "[editor] gizmo guides"
                ),
                None => info!(
                    target: telemetry_target::EDITOR,
                    guides = guide_kind.is_some(),
                    dotted,
                    "[editor] gizmo guides"
                ),
            }
        }
        self.gizmo_geometry = geometry;
        self.gizmo_reference = handle_length;
        // Step 3: log the anchor on a change or on the periodic cadence. It
        // tells a developer, and the suite, where the handles are.
        if let Some((x, y)) = screen_anchor {
            let anchor_key = (
                gizmo_settings.mode,
                gizmo_settings.space,
                frame_name.clone(),
                x as i32,
                y as i32,
            );
            if self.gizmo_logged.as_ref() != Some(&anchor_key)
                || self.frame_index.is_multiple_of(GIZMO_HEARTBEAT_FRAMES)
            {
                self.gizmo_logged = Some(anchor_key);
                let geometry = &self.gizmo_geometry;
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %entity,
                    mode = gizmo_mode_name(gizmo_settings.mode),
                    space = gizmo_space_name(gizmo_settings.space),
                    frame = frame_name.as_deref().unwrap_or("-"),
                    x,
                    y,
                    handles = geometry.lines.len() + geometry.triangles.len(),
                    snapped = gizmo_settings.snap,
                    "[editor] gizmo anchor"
                );
            }
        }
    }

    /// Runs the command the active keymap binds to a pressed key; the tool
    /// commands switch the gizmo's mode.
    ///
    /// The command registry owns which key means what, so a preset or a
    /// keybindings override changes the editor without a restart. A key with
    /// no binding does nothing here, so the rest of the editor and the game
    /// still receive it.
    pub(super) fn gizmo_shortcut(&mut self, event: &winit::event::KeyEvent) {
        if event.state != winit::event::ElementState::Pressed || event.repeat {
            return;
        }
        let winit::keyboard::PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let Some(key) = pill_input::winit_events::key_code(code) else {
            return;
        };
        let held_modifiers = EngineModifiers {
            shift: self.modifiers.shift,
            control: self.modifiers.control,
            alt: self.modifiers.alt,
            super_key: false,
        };
        self.run_bound_command(key, held_modifiers);
    }

    /// Switches the gizmo's mode; the keyboard shortcuts and the scripted
    /// `gizmo` verb call this.
    pub(super) fn set_gizmo_tool(&mut self, mode: GizmoMode) {
        let world = self.host.host_mut().engine_mut().world_mut();
        if world.get_resource::<GizmoState>().is_none() {
            world.insert_resource(GizmoState::default());
        }
        let Some(gizmo_state) = world.get_resource_mut::<GizmoState>() else {
            return;
        };
        if gizmo_state.settings.mode == mode {
            return;
        }
        gizmo_state.settings.mode = mode;
        info!(
            target: telemetry_target::EDITOR,
            mode = gizmo_mode_name(mode),
            "[editor] gizmo tool"
        );
    }

    /// Returns the handle length, in world units, at `gizmo_origin` in
    /// `view`: a constant fraction of that view's camera distance, clamped so
    /// the handles neither become too small to grab nor cover the scene.
    pub(super) fn gizmo_length(&self, view: usize, gizmo_origin: [f32; 3]) -> f32 {
        (gizmo::distance(self.cameras[view].eye(), gizmo_origin) * GIZMO_SCREEN_FRACTION)
            .clamp(0.05, 50.0)
    }

    /// Returns one scene view's surface size in physical pixels, or `None`
    /// when that view has no window.
    pub(super) fn scene_pixel_size(&self, view: usize) -> Option<(f32, f32)> {
        let scene_view = self.views.get(view)?.as_ref()?;
        let (width, height) = scene_view.window.size();
        Some((width as f32, height as f32))
    }

    /// Returns the cursor ray through one physical pixel of one scene view's
    /// surface, or `None` when the view has no window or the ray has no
    /// length.
    ///
    /// The ray is built from the editor camera's basis and the vertical field
    /// of view of the camera in the renderer contract. Those are the same
    /// numbers the renderer projects with, so a pick and a drag agree about
    /// what a pixel looks at.
    pub(super) fn scene_ray(&self, view: usize, x: f32, y: f32) -> Option<([f32; 3], [f32; 3])> {
        let (width, height) = self.scene_pixel_size(view)?;
        let (tangent, aspect) = projection_terms(width, height);
        let camera = &self.cameras[view];
        let forward = camera.forward();
        let right = camera.right();
        let up = gizmo::cross(right, forward);
        let ndc_x = x / width * 2.0 - 1.0;
        let ndc_y = 1.0 - y / height * 2.0;
        let direction = gizmo::add(
            forward,
            gizmo::add(
                gizmo::scale(right, ndc_x * tangent * aspect),
                gizmo::scale(up, ndc_y * tangent),
            ),
        );
        gizmo::normalize(direction).map(|direction| (camera.eye(), direction))
    }

    /// Returns where a world point appears on the active scene view's
    /// surface, in physical pixels. Returns `None` when the view has no
    /// window or the point is at or behind the camera.
    pub(super) fn project_to_screen(&self, point: [f32; 3]) -> Option<(f32, f32)> {
        self.project_to_screen_in(self.active_view, point)
    }

    /// Returns where a world point appears on one scene view's surface, in
    /// physical pixels. Returns `None` when the view has no window or the
    /// point is at or behind the camera.
    pub(super) fn project_to_screen_in(&self, view: usize, point: [f32; 3]) -> Option<(f32, f32)> {
        self.screen_projection(view)?.project(point)
    }

    /// The camera and surface terms that place a world point on one scene
    /// view, captured once so a caller projecting many points does not
    /// recompute them per point. Returns `None` when the view has no window.
    pub(super) fn screen_projection(&self, view: usize) -> Option<ScreenProjection> {
        let (width, height) = self.scene_pixel_size(view)?;
        let (tangent, aspect) = projection_terms(width, height);
        let camera = &self.cameras[view];
        let forward = camera.forward();
        let right = camera.right();
        Some(ScreenProjection {
            eye: camera.eye(),
            forward,
            right,
            up: gizmo::cross(right, forward),
            tangent,
            aspect,
            width,
            height,
        })
    }

    /// Begins a drag from a handle pick. The press pixel's ray records the
    /// parameter or angle that every later move is measured against.
    pub(super) fn begin_gizmo_drag(
        &mut self,
        view: usize,
        handle: u32,
        press_x: f32,
        press_y: f32,
    ) {
        let Some(kind) = gizmo::handle_kind(handle) else {
            warn!(
                target: telemetry_target::EDITOR,
                handle,
                "[editor] gizmo drag refused: unknown handle"
            );
            return;
        };
        // Step 1: read the press's inputs. The basis is resolved here, so a
        // tool frame the handles were drawn in is the frame the drag measures
        // its deltas in, and the subject decides what a move writes.
        let (entity, original, basis, subject) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let gizmo_state = world
                .get_resource::<GizmoState>()
                .copied()
                .unwrap_or_default();
            let selected_entity = world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.primary());
            let Some(entity) = selected_entity else {
                return;
            };
            let Some(original) = read_transform(world, entity) else {
                return;
            };
            let basis = gizmo_frame_basis(world, entity, gizmo_state, &original);
            let subject = match element_drag_elements(world, entity) {
                Some((kind, press)) => DragSubject::Elements {
                    kind,
                    press,
                    moves: None,
                },
                None => DragSubject::Entity,
            };
            (entity, original, basis, subject)
        };
        let Some((ray_origin, ray_direction)) = self.scene_ray(view, press_x, press_y) else {
            return;
        };
        // Step 2: an element drag pivots on the selection's average position,
        // so a turn or a scale carries every element around it; an entity drag
        // pivots on the object's own origin.
        let gizmo_origin = match &subject {
            DragSubject::Elements { press, .. } => element_pivot(press, original.translation),
            DragSubject::Entity => original.translation,
        };
        let (start_value, start_plane) = match kind {
            HandleKind::Axis {
                mode: GizmoMode::Rotate,
                axis,
            } => {
                let Some(point) =
                    gizmo::ray_plane_point(gizmo_origin, basis[axis], ray_origin, ray_direction)
                else {
                    warn!(
                        target: telemetry_target::EDITOR,
                        "[editor] gizmo drag refused: the ray is parallel to the ring"
                    );
                    return;
                };
                (gizmo::plane_angle(gizmo_origin, basis[axis], point), None)
            }
            HandleKind::Axis { axis, .. } => {
                let Some(value) =
                    gizmo::closest_axis_point(gizmo_origin, basis[axis], ray_origin, ray_direction)
                else {
                    warn!(
                        target: telemetry_target::EDITOR,
                        "[editor] gizmo drag refused: the ray is parallel to the axis"
                    );
                    return;
                };
                (value, None)
            }
            HandleKind::Plane { first, second } => {
                // The plane is the one the pair spans. Its normal is the
                // remaining axis, in world space and local space alike.
                let normal = basis[3 - first - second];
                let Some(point) =
                    gizmo::ray_plane_point(gizmo_origin, normal, ray_origin, ray_direction)
                else {
                    warn!(
                        target: telemetry_target::EDITOR,
                        "[editor] gizmo drag refused: the ray is parallel to the plane"
                    );
                    return;
                };
                let (along_first, along_second) =
                    gizmo::plane_coordinates(gizmo_origin, basis[first], basis[second], point);
                (0.0, Some((along_first, along_second)))
            }
        };
        let reference = self.gizmo_length(view, gizmo_origin);
        self.gizmo_drag = Some(GizmoDrag {
            view,
            handle,
            kind,
            entity,
            original,
            basis,
            start_value,
            start_plane,
            start_pivot: gizmo_origin,
            current_pivot: gizmo_origin,
            reference,
            last_value: None,
            written: None,
            subject,
        });
        // The grabbed handle is the active one from here on, so it draws
        // dimmed for as long as the drag runs.
        self.set_hovered_handle(view, Some(handle));
        match kind.plane() {
            Some((first, second)) => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(GizmoMode::Translate),
                plane = gizmo_plane_name(first, second),
                entity = %entity,
                handle,
                "[editor] gizmo drag started"
            ),
            None => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(gizmo::handle_axis(handle).map(|(mode, _)| mode).unwrap_or(GizmoMode::Translate)),
                axis = gizmo_axis_name(kind.axis().map(|(_, axis)| axis).unwrap_or(0)),
                entity = %entity,
                handle,
                "[editor] gizmo drag started"
            ),
        }
    }

    /// Applies one move of the drag in flight. It reads the ray again, takes
    /// the delta against the press, snaps it when the tool snaps, and writes
    /// the value straight into the world. A move records no undo entry.
    ///
    /// Every write is computed from the transform at the press, never from
    /// the previous frame's value, so a snapped or clamped drag cannot add up
    /// rounding errors. The drag also keeps the basis its handles were built
    /// in, so a local-space drag follows the object's own axes.
    pub(super) fn update_gizmo_drag(&mut self, cursor_x: f32, cursor_y: f32) {
        let Some(drag) = self.gizmo_drag.clone() else {
            return;
        };
        let (view, kind, entity) = (drag.view, drag.kind, drag.entity);
        let (original, basis) = (drag.original, drag.basis);
        let (start_value, start_plane) = (drag.start_value, drag.start_plane);
        let reference = drag.reference;
        let Some((ray_origin, ray_direction)) = self.scene_ray(view, cursor_x, cursor_y) else {
            return;
        };
        let gizmo_settings = self.gizmo_settings();
        let gizmo_origin = drag.start_pivot;
        // An element drag measures the same way and then hands the finished
        // world transforms to the tool, so the tool never does frame maths.
        if matches!(drag.subject, DragSubject::Elements { .. }) {
            self.update_element_drag(&drag, ray_origin, ray_direction);
            return;
        }
        let (written, pivot) = match kind {
            HandleKind::Axis {
                mode: GizmoMode::Rotate,
                axis,
            } => {
                let Some(point) =
                    gizmo::ray_plane_point(gizmo_origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let delta = gizmo::wrap_angle(
                    gizmo::plane_angle(gizmo_origin, basis[axis], point) - start_value,
                );
                let degrees = if gizmo_settings.snap {
                    gizmo::snap_value(delta.to_degrees(), gizmo_settings.rotate_step_degrees)
                } else {
                    delta.to_degrees()
                };
                // The axis comes from the drag's own basis, so a local-mode
                // drag turns around the object's axis and a world-mode drag
                // around the world's.
                let turn_rotation = gizmo::axis_angle_quaternion(basis[axis], degrees.to_radians());
                let combined_rotation = rotate_by_turn(turn_rotation, original.rotation);
                let written = self.write_rotation(entity, combined_rotation);
                (written.then_some(combined_rotation), gizmo_origin)
            }
            HandleKind::Axis {
                mode: GizmoMode::Translate,
                axis,
            } => {
                let Some(value) =
                    gizmo::closest_axis_point(gizmo_origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let mut delta = value - start_value;
                if gizmo_settings.snap {
                    delta = gizmo::snap_value(delta, gizmo_settings.translate_step);
                }
                // The displacement is along the handle's axis, not one world
                // component. In local space the two differ as soon as the
                // object is rotated.
                let translation =
                    gizmo::add(original.translation, gizmo::scale(basis[axis], delta));
                let written = self.write_translation(entity, translation);
                (
                    written.then_some([translation[0], translation[1], translation[2], 1.0]),
                    translation,
                )
            }
            HandleKind::Axis {
                mode: GizmoMode::Scale,
                axis,
            } => {
                let Some(value) =
                    gizmo::closest_axis_point(gizmo_origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let scale_factor = 1.0 + (value - start_value) / reference.max(1e-4);
                let mut axis_scale = original.scale[axis] * scale_factor;
                if gizmo_settings.snap {
                    axis_scale = gizmo::snap_value(axis_scale, gizmo_settings.scale_step);
                }
                // A zero or negative scale would make the mesh degenerate, so
                // the drag stops at the smallest positive value instead.
                let axis_scale = axis_scale.max(0.01);
                let mut updated_scale = original.scale;
                updated_scale[axis] = axis_scale;
                let written = self.write_scale(entity, updated_scale);
                (written.then_some([axis_scale, 0.0, 0.0, 0.0]), gizmo_origin)
            }
            HandleKind::Plane { first, second } => {
                let Some((start_first, start_second)) = start_plane else {
                    return;
                };
                let normal = basis[3 - first - second];
                let Some(point) =
                    gizmo::ray_plane_point(gizmo_origin, normal, ray_origin, ray_direction)
                else {
                    return;
                };
                let (along_first, along_second) =
                    gizmo::plane_coordinates(gizmo_origin, basis[first], basis[second], point);
                let (mut delta_first, mut delta_second) =
                    (along_first - start_first, along_second - start_second);
                if gizmo_settings.snap {
                    delta_first = gizmo::snap_value(delta_first, gizmo_settings.translate_step);
                    delta_second = gizmo::snap_value(delta_second, gizmo_settings.translate_step);
                }
                let translation = gizmo::add(
                    gizmo::add(
                        original.translation,
                        gizmo::scale(basis[first], delta_first),
                    ),
                    gizmo::scale(basis[second], delta_second),
                );
                let written = self.write_translation(entity, translation);
                (
                    written.then_some([delta_first, delta_second, 0.0, 0.0]),
                    translation,
                )
            }
        };
        if let Some(drag) = self.gizmo_drag.as_mut() {
            if let Some(written) = written {
                drag.last_value = Some(written);
            }
            drag.current_pivot = pivot;
        }
    }

    /// Applies one move of an element drag: measures this position against the
    /// press the way the entity drag does, turns the measurement into world
    /// transforms for every dragged element, and hands them to the active
    /// tool's write-back.
    ///
    /// The tool owns the component the elements live in, so nothing here names
    /// a field and a move records nothing. The release queues the one recorded
    /// step, through the same write-back an undo replays.
    fn update_element_drag(
        &mut self,
        drag: &GizmoDrag,
        ray_origin: [f32; 3],
        ray_direction: [f32; 3],
    ) {
        let DragSubject::Elements { kind, press, .. } = &drag.subject else {
            return;
        };
        let gizmo_settings = self.gizmo_settings();
        let (origin, basis, reference) = (drag.start_pivot, drag.basis, drag.reference);
        let value = match drag.kind {
            HandleKind::Axis {
                mode: GizmoMode::Rotate,
                axis,
            } => {
                let Some(point) =
                    gizmo::ray_plane_point(origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let delta = gizmo::wrap_angle(
                    gizmo::plane_angle(origin, basis[axis], point) - drag.start_value,
                );
                let degrees = if gizmo_settings.snap {
                    gizmo::snap_value(delta.to_degrees(), gizmo_settings.rotate_step_degrees)
                } else {
                    delta.to_degrees()
                };
                gizmo::DragValue::Angle(degrees.to_radians())
            }
            HandleKind::Axis {
                mode: GizmoMode::Translate,
                axis,
            } => {
                let Some(value) =
                    gizmo::closest_axis_point(origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let mut delta = value - drag.start_value;
                if gizmo_settings.snap {
                    delta = gizmo::snap_value(delta, gizmo_settings.translate_step);
                }
                gizmo::DragValue::Distance(delta)
            }
            HandleKind::Axis {
                mode: GizmoMode::Scale,
                axis,
            } => {
                let Some(value) =
                    gizmo::closest_axis_point(origin, basis[axis], ray_origin, ray_direction)
                else {
                    return;
                };
                let mut factor = 1.0 + (value - drag.start_value) / reference.max(1e-4);
                // Snapping rounds the factor, not each element's own scale, so
                // every element grows by the same amount.
                if gizmo_settings.snap {
                    factor = gizmo::snap_value(factor, gizmo_settings.scale_step);
                }
                gizmo::DragValue::Distance((factor.max(0.01) - 1.0) * reference)
            }
            HandleKind::Plane { first, second } => {
                let Some((start_first, start_second)) = drag.start_plane else {
                    return;
                };
                let normal = basis[3 - first - second];
                let Some(point) = gizmo::ray_plane_point(origin, normal, ray_origin, ray_direction)
                else {
                    return;
                };
                let (along_first, along_second) =
                    gizmo::plane_coordinates(origin, basis[first], basis[second], point);
                let (mut delta_first, mut delta_second) =
                    (along_first - start_first, along_second - start_second);
                if gizmo_settings.snap {
                    delta_first = gizmo::snap_value(delta_first, gizmo_settings.translate_step);
                    delta_second = gizmo::snap_value(delta_second, gizmo_settings.translate_step);
                }
                gizmo::DragValue::Plane(delta_first, delta_second)
            }
        };
        let moves = gizmo::moved_elements(drag.kind, origin, basis, reference, press, value);
        if moves.is_empty() {
            return;
        }
        // Step 2: the tool writes the world transforms, live and unrecorded.
        // A copy of the registry keeps the report and the write apart, since
        // the write needs the world mutably.
        let written = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let registry = tools::tool_registry_mut(world).clone();
            registry.active_set_transforms(drag.entity, &moves, world)
        };
        let moved: Vec<(u64, tools::ElementTransform)> = moves
            .iter()
            .map(|movement| (movement.element, movement.transform))
            .collect();
        let pivot = element_pivot(&moved, origin);
        if let Some(active_drag) = self.gizmo_drag.as_mut() {
            if written {
                active_drag.subject = DragSubject::Elements {
                    kind: kind.clone(),
                    press: press.clone(),
                    moves: Some(moves),
                };
                active_drag.last_value = Some(drag_value_components(value));
            }
            active_drag.current_pivot = pivot;
        }
    }

    /// Writes a translation straight into the world. Returns false when a
    /// component refused the write.
    pub(super) fn write_translation(&mut self, entity: Entity, translation: [f32; 3]) -> bool {
        let written = self.write_transform_field(entity, "translation", &translation);
        if written {
            self.note_gizmo_write("translation", &translation);
        }
        written
    }

    /// Writes a rotation straight into the world. Returns false when a
    /// component refused the write.
    pub(super) fn write_rotation(&mut self, entity: Entity, rotation: [f32; 4]) -> bool {
        let written = self.write_transform_field(entity, "rotation", &rotation);
        if written {
            self.note_gizmo_write("rotation", &rotation);
        }
        written
    }

    /// Writes a scale straight into the world. Returns false when a component
    /// refused the write.
    pub(super) fn write_scale(&mut self, entity: Entity, scale: [f32; 3]) -> bool {
        let written = self.write_transform_field(entity, "scale", &scale);
        if written {
            self.note_gizmo_write("scale", &scale);
        }
        written
    }

    /// Notes what the drag in flight last wrote, so its release can record
    /// one step for it. A restore has no drag in flight and notes nothing.
    pub(super) fn note_gizmo_write(
        &mut self,
        transform_field: &'static str,
        component_values: &[f32],
    ) {
        if let Some(drag) = self.gizmo_drag.as_mut() {
            drag.written = Some((transform_field, component_values.to_vec()));
        }
    }

    /// Writes one transform field's components, in order, and returns false
    /// when any of them was refused.
    ///
    /// A refusal is logged here, where the entity, the field and the element
    /// index are still known. Callers only see a `false`, which becomes
    /// `ended without a move` and reads the same as a drag that never moved.
    pub(super) fn write_transform_field(
        &mut self,
        entity: Entity,
        transform_field: &str,
        component_values: &[f32],
    ) -> bool {
        let world = self.host.host_mut().engine_mut().world_mut();
        let mut written = true;
        for (index, value) in component_values.iter().enumerate() {
            if let Err(error) = world.write_component_array_element(
                entity,
                TRANSFORM_SHARED_NAME,
                transform_field,
                index,
                FieldValue::F32(*value),
            ) {
                warn!(
                    target: telemetry_target::EDITOR,
                    entity = %entity,
                    field = %transform_field,
                    index,
                    error = %error,
                    "[editor] gizmo write refused"
                );
                written = false;
            }
        }
        written
    }

    /// Writes the whole transform from before the press back, so a cancelled
    /// drag leaves the object exactly where it started. Returns false when
    /// any of the three writes was refused, so the cancel can say so instead
    /// of reporting a clean restore.
    pub(super) fn restore_transform(
        &mut self,
        entity: Entity,
        transform: &TransformComponent,
    ) -> bool {
        let mut restored = self.write_translation(entity, transform.translation);
        restored &= self.write_rotation(entity, transform.rotation);
        restored &= self.write_scale(entity, transform.scale);
        restored
    }

    /// Turns the drag that just ended into one recorded step, so `Ctrl+Z`
    /// after a drag undoes it the way it undoes every other edit.
    ///
    /// The press's value is written back first. That write is required: the
    /// recorded command takes its inverse from the world's current value,
    /// which the drag has already overwritten with the final one. Without the
    /// write, the step would record the final value as its starting point,
    /// and the first undo would use up a history entry and change nothing.
    pub(super) fn record_gizmo_drag(&mut self, drag: &GizmoDrag, mode: GizmoMode) {
        // An element drag records the moves the tool already took: the command
        // carries the world transforms, and the applier reads the press's own
        // transforms off the tool for its inverse.
        if let DragSubject::Elements { kind, press, moves } = &drag.subject {
            let Some(moves) = moves else {
                return;
            };
            let restored: Vec<tools::ElementMove> = press
                .iter()
                .map(|(element, transform)| tools::ElementMove {
                    element: *element,
                    transform: *transform,
                })
                .collect();
            let world = self.host.host_mut().engine_mut().world_mut();
            // The same reason the entity path writes its press value back: the
            // recorded command takes its inverse from the world's current
            // value, which the drag has already overwritten.
            let registry = tools::tool_registry_mut(world).clone();
            registry.active_set_transforms(drag.entity, &restored, world);
            let label = format!("gizmo {}", gizmo_mode_name(mode));
            let command = EditorCommand::SetElementTransforms {
                entity: drag.entity,
                kind: kind.clone(),
                moves: moves.clone(),
            };
            if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
                pending.push_recorded(vec![command], label, None);
            }
            info!(
                target: telemetry_target::EDITOR,
                entity = %drag.entity,
                kind = kind.as_str(),
                elements = moves.len(),
                "[editor] element drag recorded"
            );
            return;
        }
        let Some((transform_field, component_values)) = drag.written.as_ref() else {
            return;
        };
        let original = match *transform_field {
            "rotation" => drag.original.rotation.to_vec(),
            "scale" => drag.original.scale.to_vec(),
            _ => drag.original.translation.to_vec(),
        };
        let Some(edits) = gizmo::release_edits(&original, component_values) else {
            return;
        };
        self.write_transform_field(drag.entity, transform_field, &original);
        let recorded_commands = edits
            .into_iter()
            .map(|(index, value)| {
                inspector::element_edit(
                    drag.entity,
                    TRANSFORM_SHARED_NAME,
                    transform_field,
                    index,
                    FieldValue::F32(value),
                )
                .0
            })
            .collect();
        let label = format!("gizmo {}", gizmo_mode_name(mode));
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            pending.push_recorded(recorded_commands, label, None);
        }
        info!(
            target: telemetry_target::EDITOR,
            entity = %drag.entity,
            field = %transform_field,
            "[editor] gizmo drag recorded"
        );
    }

    /// Ends the drag in flight and reports what it wrote. The guides and the
    /// active highlight go with it, so the handles are drawn in their idle or
    /// hovered look on the next frame.
    pub(super) fn end_gizmo_drag(&mut self) {
        let Some(drag) = self.gizmo_drag.take() else {
            return;
        };
        let gizmo_settings = self.gizmo_settings();
        let mode = gizmo_drag_mode(drag.kind);
        match (drag.kind.plane(), drag.last_value) {
            (Some((first, second)), Some(value)) => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(mode),
                plane = gizmo_plane_name(first, second),
                entity = %drag.entity,
                value = ?value,
                snapped = gizmo_settings.snap,
                "[editor] gizmo drag ended"
            ),
            (Some((first, second)), None) => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(mode),
                plane = gizmo_plane_name(first, second),
                entity = %drag.entity,
                "[editor] gizmo drag ended without a move"
            ),
            (None, Some(value)) => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(mode),
                axis = gizmo_axis_name(drag.kind.axis().map(|(_, axis)| axis).unwrap_or(0)),
                entity = %drag.entity,
                value = ?value,
                snapped = gizmo_settings.snap,
                "[editor] gizmo drag ended"
            ),
            (None, None) => info!(
                target: telemetry_target::EDITOR,
                mode = gizmo_mode_name(mode),
                axis = gizmo_axis_name(drag.kind.axis().map(|(_, axis)| axis).unwrap_or(0)),
                entity = %drag.entity,
                "[editor] gizmo drag ended without a move"
            ),
        }
        self.record_gizmo_drag(&drag, mode);
        self.set_hovered_handle(drag.view, None);
    }

    /// Cancels the drag in flight. The transform from before the press is
    /// written back and the drag is dropped, so losing the focus leaves no
    /// half-applied move (and no guides) behind.
    pub(super) fn cancel_gizmo_drag(&mut self) {
        let Some(drag) = self.gizmo_drag.take() else {
            return;
        };
        // An element drag is undone through the tool that wrote it, so the
        // press's transforms come back the same way every other element write
        // goes out.
        let restored = match &drag.subject {
            DragSubject::Elements { press, .. } => {
                let press: Vec<tools::ElementMove> = press
                    .iter()
                    .map(|(element, transform)| tools::ElementMove {
                        element: *element,
                        transform: *transform,
                    })
                    .collect();
                let world = self.host.host_mut().engine_mut().world_mut();
                let registry = tools::tool_registry_mut(world).clone();
                registry.active_set_transforms(drag.entity, &press, world)
            }
            DragSubject::Entity => self.restore_transform(drag.entity, &drag.original),
        };
        if !restored {
            warn!(
                target: telemetry_target::EDITOR,
                entity = %drag.entity,
                "[editor] gizmo drag restore failed"
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            mode = gizmo_mode_name(gizmo_drag_mode(drag.kind)),
            entity = %drag.entity,
            "[editor] gizmo drag canceled"
        );
        self.set_hovered_handle(drag.view, None);
    }

    /// Drops every interaction in flight. The focus was lost, so no press or
    /// release is coming to finish them.
    pub(super) fn cancel_interactions(&mut self, reason: &str) {
        if self.camera_drag.take().is_some() {
            info!(
                target: telemetry_target::EDITOR,
                reason,
                "[editor] camera navigation canceled"
            );
        }
        self.cancel_gizmo_drag();
        self.cancel_marquee(reason);
        self.press_pick = None;
        self.left_down.fill(false);
        self.set_hovered_handle(self.active_view, None);
    }

    /// Presses one handle of the active tool for the scripted channel. The
    /// press pixel is the handle's grab point projected, so the pick that
    /// answers it proves the same geometry the renderer draws.
    pub(super) fn start_synthetic_gizmo_drag(&mut self, handle: u32, delta: Option<(f32, f32)>) {
        // Step 1: refuse the request when it cannot run. That covers an
        // unknown handle, another drag in flight, no visible view, and no
        // selected entity with a transform.
        let Some(kind) = gizmo::handle_kind(handle) else {
            warn!(
                target: telemetry_target::EDITOR,
                handle,
                "[editor] gizmo drag refused: unknown handle"
            );
            return;
        };
        if self.gizmo_drag.is_some() {
            debug!(
                target: telemetry_target::EDITOR,
                "[editor] gizmo drag refused: one is already in flight"
            );
            return;
        }
        let Some(view) = self.visible_scene_view() else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] gizmo drag refused: no scene view is visible"
            );
            return;
        };
        let (origin, basis) = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let gizmo_state = world
                .get_resource::<GizmoState>()
                .copied()
                .unwrap_or_default();
            let selected_entity = world
                .get_resource::<EditorSelection>()
                .and_then(|selection| selection.primary());
            let Some(entity) = selected_entity else {
                warn!(
                    target: telemetry_target::EDITOR,
                    "[editor] gizmo drag refused: nothing is selected"
                );
                return;
            };
            let Some(transform) = read_transform(world, entity) else {
                warn!(
                    target: telemetry_target::EDITOR,
                    entity = %entity,
                    "[editor] gizmo drag refused: the entity has no transform"
                );
                return;
            };
            let basis = gizmo_frame_basis(world, entity, gizmo_state, &transform);
            let origin = match element_drag_elements(world, entity) {
                Some((_, press)) => element_pivot(&press, transform.translation),
                None => transform.translation,
            };
            (origin, basis)
        };
        // Step 2: project the handle's own grab point into the view's pixels.
        // Each handle is grabbed where its geometry carries it. A rotation
        // ring is grabbed at its radius on the diagonal between two axes,
        // clear of the other two rings. A plane square is grabbed at its
        // centre, and a straight handle just short of its tip.
        let handle_length = self.gizmo_length(view, origin);
        let grab_point = match kind {
            HandleKind::Axis { axis, .. } => match gizmo_drag_mode(kind) {
                GizmoMode::Rotate => gizmo::add(
                    origin,
                    gizmo::scale(
                        gizmo::normalize(gizmo::add(basis[(axis + 1) % 3], basis[(axis + 2) % 3]))
                            .unwrap_or(basis[axis]),
                        handle_length,
                    ),
                ),
                _ => gizmo::add(
                    origin,
                    gizmo::scale(basis[axis], handle_length * GIZMO_PRESS_FRACTION),
                ),
            },
            HandleKind::Plane { first, second } => gizmo::add(
                origin,
                gizmo::add(
                    gizmo::scale(basis[first], handle_length * gizmo::PLANE_OFFSET),
                    gizmo::scale(basis[second], handle_length * gizmo::PLANE_OFFSET),
                ),
            ),
        };
        let Some((x, y)) = self.project_to_screen_in(view, grab_point) else {
            warn!(
                target: telemetry_target::EDITOR,
                "[editor] gizmo drag refused: the handle is behind the camera"
            );
            return;
        };
        // Step 3: record the scripted press and queue its picks.
        self.synthetic_gizmo_pending = delta;
        self.synthetic_gizmo_hold = delta.is_none();
        self.synthetic_gizmo_press = None;
        self.synthetic_gizmo_view = Some(view);
        match kind.plane() {
            Some((first, second)) => info!(
                target: telemetry_target::EDITOR,
                plane = gizmo_plane_name(first, second),
                x,
                y,
                "[editor] gizmo drag press"
            ),
            None => info!(
                target: telemetry_target::EDITOR,
                axis = gizmo_axis_name(kind.axis().map(|(_, axis)| axis).unwrap_or(0)),
                x,
                y,
                "[editor] gizmo drag press"
            ),
        }
        // The press pixel projects onto the handle's own geometry, but a
        // one-pixel line straddles pixel rows. The press therefore asks for
        // the exact pixel and its four neighbours, and the first answer that
        // names a handle wins.
        let (centre_x, centre_y) = (x.round() as i64, y.round() as i64);
        self.synthetic_press_ids.clear();
        for (pixel_x, pixel_y) in [
            (centre_x, centre_y),
            (centre_x - 1, centre_y),
            (centre_x + 1, centre_y),
            (centre_x, centre_y - 1),
            (centre_x, centre_y + 1),
        ] {
            if let Some(id) =
                self.queue_scene_pick_at(view, pixel_x.max(0) as u32, pixel_y.max(0) as u32)
            {
                self.synthetic_press_ids.push(id);
            }
        }
    }

    /// Returns the gizmo settings from the world, or the defaults when no
    /// state exists.
    pub(super) fn gizmo_settings(&mut self) -> GizmoSettings {
        let world = self.host.host_mut().engine_mut().world_mut();
        world
            .get_resource::<GizmoState>()
            .map(|state| state.settings)
            .unwrap_or_default()
    }
}

/// One scene view's camera basis and projection terms, in the surface's
/// physical pixels.
///
/// Built by [`EditorWindow::screen_projection`]. Every point a frame projects
/// into the same view shares these terms, so they are captured once rather
/// than recomputed for each point.
#[derive(Clone, Copy, Debug)]
pub(super) struct ScreenProjection {
    eye: [f32; 3],
    forward: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    tangent: f32,
    aspect: f32,
    width: f32,
    height: f32,
}

impl ScreenProjection {
    /// Where `point` appears on the view's surface, in physical pixels, or
    /// `None` when it is at or behind the camera.
    pub(super) fn project(&self, point: [f32; 3]) -> Option<(f32, f32)> {
        let eye_to_point = gizmo::sub(point, self.eye);
        let depth = gizmo::dot(eye_to_point, self.forward);
        if depth <= 1e-4 {
            return None;
        }
        let ndc_x = gizmo::dot(eye_to_point, self.right) / (depth * self.tangent * self.aspect);
        let ndc_y = gizmo::dot(eye_to_point, self.up) / (depth * self.tangent);
        Some((
            (ndc_x + 1.0) * 0.5 * self.width,
            (1.0 - ndc_y) * 0.5 * self.height,
        ))
    }

    /// The surface's size, in physical pixels.
    pub(super) fn size(&self) -> (f32, f32) {
        (self.width, self.height)
    }
}
