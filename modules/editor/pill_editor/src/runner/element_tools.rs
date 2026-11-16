//! The active tool's elements: drawing them, picking them and selecting them.
//!
//! # Responsibilities
//!
//! - Publish what the active tool reports for the selected entity as pickable
//!   point handles, styled for selection and hover, and log where they land.
//! - Answer which handle a pixel of one scene view hits, and apply an element
//!   click to the shared selection as one recorded step.
//! - Run the marquee: the drag a press in empty space starts while the active
//!   tool offers a kind that allows box select.
//!
//! # Design
//!
//! The editor owns every generic part; a tool only reports where its elements
//! are, of which kind, and what a kind looks like
//! ([`pill_editor_core::tools`]). This module turns that report into the
//! frame's drawn handles, the handle-to-target map and the screen projections,
//! so a click, an extend or a marquee needs no tool code at all - and a tool
//! that reports nothing costs nothing.
//!
//! An element is picked twice over. The CPU test here answers the hover and
//! the press in the frame they happen in, against the exact pixels the handle
//! was projected to; the renderer's pick buffer answers the same handles to a
//! queued pick, because a point's pick quad carries the handle's element-band
//! id. Both answers land in [`EditorWindow::apply_element_click`], so the two
//! paths cannot disagree about what a click selects.
//!
//! The handles are rebuilt from the tool every frame and never cached across
//! one: a component edit can move or remove an element, and a handle that
//! outlived its element would select something that is no longer there.

// Current crate
use super::*;

// =============================================================================
// Free Functions
// =============================================================================

/// Queues one recorded element-selection step.
///
/// No coalesce key: every click, extend, tool answer and clear is its own undo
/// step, which is the gesture rule the transform gizmo follows.
pub(super) fn queue_element_selection(
    world: &mut pill_engine_core::World,
    kind: String,
    elements: Vec<ElementRef>,
) {
    if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
        pending.push_recorded(
            vec![EditorCommand::SetElementSelection { kind, elements }],
            "select elements",
            None,
        );
    }
}

/// Queues the commands a tool answered a press or a key with, as one recorded
/// step.
pub(super) fn queue_tool_commands(
    world: &mut pill_engine_core::World,
    commands: Vec<EditorCommand>,
) {
    if commands.is_empty() {
        return;
    }
    if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
        pending.push_recorded(commands, "tool edit", None);
    }
}

// =============================================================================
// Constants
// =============================================================================

/// How far a press must travel, in pixels, before its drag counts as a
/// marquee instead of a click. A press and a release inside one pixel are the
/// click that still asks the scene what is under the pointer.
pub(super) const MARQUEE_DRAG_PIXELS: f32 = 3.0;

/// What a scene pixel hits among the editor's own handles.
///
/// The two bands never overlap (`gizmo::element_handle_slot` asserts it), so a
/// handle id says which kind of answer it is; this type carries that decision
/// to the caller that acts on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SceneHandle {
    /// One of the active tool's element handles.
    Element(u32),
    /// One of the transform gizmo's handles.
    Gizmo(u32),
}

impl EditorWindow {
    /// Publishes the active tool's elements as this frame's handles.
    ///
    /// The handles are drawn as the debug drawing's point primitive, so one
    /// report reaches every scene view at its own pixel size; the same frame
    /// keeps their world positions, kinds and handle ids for the press, the
    /// hover and the marquee. A tool that reports nothing clears all of it, so
    /// nothing from an earlier tool can be pressed.
    pub(super) fn publish_element_handles(&mut self) {
        // Step 1: read the frame's inputs and let go of the world before any
        // camera helper borrows `self`. The subject is the entity the element
        // selection belongs to, or the primary when nothing is selected yet.
        let frame = {
            let world = self.host.host_mut().engine_mut().world_mut();
            let registry = world.get_resource::<tools::ToolRegistry>();
            let selection = world.get_resource::<EditorSelection>();
            match (registry, selection) {
                (Some(registry), Some(selection))
                    if registry
                        .active_record()
                        .is_some_and(|tool| tool.hooks.reports_elements()) =>
                {
                    let selected = selection.elements().clone();
                    selection
                        .elements()
                        .owner()
                        .or_else(|| selection.primary())
                        .map(|entity| {
                            let kinds = registry.active_kinds();
                            let reported = registry.active_elements(entity, world);
                            (entity, reported, kinds, selected)
                        })
                }
                _ => None,
            }
        };
        let Some((entity, reported, kinds, selected)) = frame else {
            self.element_handles.clear();
            self.element_kinds.clear();
            self.element_map = gizmo::ElementHandleMap::new();
            self.element_logged = None;
            // A hover that pointed at an element handle belongs to handles
            // that are no longer drawn, so it is dropped with them: leaving it
            // set would report an interaction nothing on screen owns.
            if self
                .hovered_handle
                .is_some_and(|handle| gizmo::element_handle_slot(handle).is_some())
            {
                self.hovered_handle = None;
            }
            return;
        };
        // Step 2: build the handles and hand them to the debug drawing the
        // renderer reads, beside the transform gizmo's own handles.
        let states = HandleStates {
            hovered: self.hovered_handle,
            active: None,
        };
        let (handles, map) = tools::element_handles(&reported, &kinds, entity, &selected, states);
        if !handles.is_empty() {
            let world = self.host.host_mut().engine_mut().world_mut();
            if world.get_resource::<DebugDraw>().is_none() {
                world.insert_resource(DebugDraw::new(true));
            }
            if let Some(debug_draw) = world.get_resource_mut::<DebugDraw>() {
                for element in &handles {
                    debug_draw.gizmo_point(
                        element.transform.translation,
                        element.size_pixels,
                        element.shape,
                        element.color,
                        element.handle,
                    );
                }
            }
        }
        // Step 3: log where the handles land in the active view, on a change
        // or on the heartbeat. A suite reads the pixels from these lines to
        // hover, click and drag them.
        let view = self.active_view;
        let landed: Vec<(u64, i32, i32, bool)> = handles
            .iter()
            .map(|element| {
                let (x, y) = self
                    .project_to_screen_in(view, element.transform.translation)
                    .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    .unwrap_or((-1, -1));
                (element.element, x, y, element.selected)
            })
            .collect();
        let signature = (view, landed.clone());
        if self.element_logged.as_ref() != Some(&signature)
            || self.frame_index.is_multiple_of(GIZMO_HEARTBEAT_FRAMES)
        {
            self.element_logged = Some(signature);
            info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                kind_count = kinds.len(),
                handles = handles.len(),
                "[editor] element handles"
            );
            for (element, (_, x, y, selected)) in handles.iter().zip(&landed) {
                info!(
                    target: telemetry_target::EDITOR,
                    view,
                    kind = element.kind.as_str(),
                    element = element.element,
                    handle = element.handle,
                    x,
                    y,
                    selected,
                    "[editor] element handle"
                );
            }
        }
        self.element_handles = handles;
        self.element_kinds = kinds;
        self.element_map = map;
        // A hover on an element that this frame no longer reports is dropped
        // with it, so the hover state names something that is on screen.
        if let Some(handle) = self.hovered_handle {
            if gizmo::element_handle_slot(handle).is_some()
                && !self
                    .element_handles
                    .iter()
                    .any(|element| element.handle == handle)
            {
                self.hovered_handle = None;
            }
        }
    }

    /// The element handle under one pixel of one scene view.
    ///
    /// The last drawn handle wins a tie, which is the order the renderer's own
    /// pick buffer resolves, and the pixel test is the pick quad's square: a
    /// press that lights a handle up is a press that selects it.
    pub(super) fn element_handle_at(&self, view: usize, x: f32, y: f32) -> Option<u32> {
        self.element_handles
            .iter()
            .rev()
            .find(|element| {
                self.project_to_screen_in(view, element.transform.translation)
                    .is_some_and(|centre| element.covers_pixel(x, y, centre))
            })
            .map(|element| element.handle)
    }

    /// What one pixel of one scene view hits: an element handle first, because
    /// the points draw on top of the gizmo, then a gizmo handle.
    pub(super) fn scene_handle_at(&self, view: usize, x: f32, y: f32) -> Option<SceneHandle> {
        if let Some(handle) = self.element_handle_at(view, x, y) {
            return Some(SceneHandle::Element(handle));
        }
        self.handle_at(view, x, y).map(SceneHandle::Gizmo)
    }

    /// Whether the active tool offers a kind a marquee may select, which is
    /// what makes a press in empty space start one.
    pub(super) fn active_tool_boxes_select(&self) -> bool {
        self.element_kinds.iter().any(|kind| kind.box_select)
    }

    /// Applies one element click to the shared selection as one recorded step,
    /// using this frame's report order for a range.
    pub(super) fn apply_element_click(
        &mut self,
        target: gizmo::ElementHandleTarget,
        click: SelectionClick,
    ) {
        let report = self.element_report();
        self.apply_element_click_with(target, click, &report);
    }

    /// Applies one element click against an explicit report order.
    ///
    /// The pointer path passes this frame's handles; the scripted channel
    /// passes the report it just read, so neither has to depend on the other's
    /// frame. The active tool is asked first: it may select what it likes, run
    /// recorded commands, or swallow the press. Only an answer of
    /// [`tools::PressResponse::Default`] reaches the editor's own rule, which
    /// is what lets a component tool override a click without the editor
    /// knowing anything about it. A refusal changes nothing and logs why.
    pub(super) fn apply_element_click_with(
        &mut self,
        target: gizmo::ElementHandleTarget,
        click: SelectionClick,
        report: &[ElementRef],
    ) {
        let reference = ElementRef {
            entity: target.entity,
            id: target.element,
        };
        let world = self.host.host_mut().engine_mut().world_mut();
        // Step 1: offer the press to the tool, with what is selected now. The
        // answer is taken before the editor's own rule runs, so a tool can
        // select what it likes, queue recorded commands, or swallow the press.
        let selected: Vec<u64> = world
            .get_resource::<EditorSelection>()
            .map(|selection| {
                selection
                    .elements()
                    .elements()
                    .iter()
                    .map(|element| element.id)
                    .collect()
            })
            .unwrap_or_default();
        let registry = tools::tool_registry_mut(world).clone();
        let response =
            registry.active_press(target.entity, target.element, click, &selected, world);
        match response {
            tools::PressResponse::Default => {}
            tools::PressResponse::Consume => {
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %target.entity,
                    element = target.element,
                    "[editor] element press consumed"
                );
                return;
            }
            tools::PressResponse::Select(ids) => {
                let elements: Vec<ElementRef> = ids
                    .into_iter()
                    .map(|id| ElementRef {
                        entity: target.entity,
                        id,
                    })
                    .collect();
                let count = elements.len();
                queue_element_selection(world, target.kind.clone(), elements);
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %target.entity,
                    element = target.element,
                    count,
                    "[editor] element press selected"
                );
                return;
            }
            tools::PressResponse::Commands(commands) => {
                let count = commands.len();
                queue_tool_commands(world, commands);
                info!(
                    target: telemetry_target::EDITOR,
                    entity = %target.entity,
                    element = target.element,
                    commands = count,
                    "[editor] element press commands queued"
                );
                return;
            }
        }
        // Step 2: the editor's own rule, which is the select / extend / range
        // behaviour every click has when no tool overrides it.
        let Some(selection) = world.get_resource::<EditorSelection>() else {
            return;
        };
        let next = match selection.clicked_elements(click, reference, &target.kind, report) {
            Ok(next) => next,
            Err(refusal) => {
                warn!(
                    target: telemetry_target::EDITOR,
                    entity = %target.entity,
                    kind = target.kind.as_str(),
                    reason = %refusal,
                    "[editor] element selection refused"
                );
                return;
            }
        };
        let kind = next.kind().to_string();
        let elements = next.elements().to_vec();
        let count = elements.len();
        queue_element_selection(world, kind, elements);
        info!(
            target: telemetry_target::EDITOR,
            entity = %target.entity,
            kind = target.kind.as_str(),
            element = target.element,
            count,
            shift = click == SelectionClick::Range,
            add = click == SelectionClick::Add,
            "[editor] element clicked"
        );
    }

    /// The elements this frame's handles stand for, in the tool's report
    /// order.
    pub(super) fn element_report(&self) -> Vec<ElementRef> {
        self.element_handles
            .iter()
            .map(|element| ElementRef {
                entity: element.entity,
                id: element.element,
            })
            .collect()
    }

    /// Starts a marquee at a press in empty space. The drag only becomes a
    /// marquee once the pointer moves past [`MARQUEE_DRAG_PIXELS`]; until then
    /// it is still the click that asks what is under the pixel.
    pub(super) fn begin_marquee(&mut self, view: usize, point: (f32, f32)) {
        self.marquee = Some(MarqueeDrag {
            view,
            from: point,
            to: point,
            live: false,
        });
    }

    /// Feeds one move to a marquee, making it live on the move that passes the
    /// drag threshold.
    pub(super) fn update_marquee(&mut self, view: usize, point: (f32, f32)) {
        let Some(drag) = self.marquee.as_mut().filter(|drag| drag.view == view) else {
            return;
        };
        drag.to = point;
        if drag.live {
            return;
        }
        if (point.0 - drag.from.0).abs() < MARQUEE_DRAG_PIXELS
            && (point.1 - drag.from.1).abs() < MARQUEE_DRAG_PIXELS
        {
            return;
        }
        drag.live = true;
        let (from, to) = (drag.from, drag.to);
        info!(
            target: telemetry_target::EDITOR,
            view,
            from_x = from.0,
            from_y = from.1,
            to_x = to.0,
            to_y = to.1,
            state = "started",
            "[editor] element marquee"
        );
    }

    /// Takes the marquee open for `view`, if any, so the release can decide
    /// what it was.
    pub(super) fn take_marquee(&mut self, view: usize) -> Option<MarqueeDrag> {
        self.marquee.take_if(|drag| drag.view == view)
    }

    /// Applies a live marquee: the elements of one kind its rectangle covers
    /// become the selection, in one recorded step.
    pub(super) fn apply_marquee(&mut self, drag: MarqueeDrag) {
        let mut projected = Vec::new();
        for element in &self.element_handles {
            let Some((x, y)) = self.project_to_screen_in(drag.view, element.transform.translation)
            else {
                continue;
            };
            projected.push(tools::ProjectedElement {
                element: ElementRef {
                    entity: element.entity,
                    id: element.element,
                },
                kind: element.kind.clone(),
                x,
                y,
            });
        }
        let rect = tools::MarqueeRect::from_corners(drag.from, drag.to);
        let selection = tools::marquee_selection(&projected, rect, &self.element_kinds);
        let kind = selection.kind().to_string();
        let elements = selection.elements().to_vec();
        let count = elements.len();
        let world = self.host.host_mut().engine_mut().world_mut();
        if let Some(pending) = world.get_resource_mut::<PendingEditorCommands>() {
            // An empty selection is recorded too: a marquee over nothing is
            // the clear a person expects, in one undoable step.
            pending.push_recorded(
                vec![EditorCommand::SetElementSelection { kind, elements }],
                "box select elements",
                None,
            );
        }
        info!(
            target: telemetry_target::EDITOR,
            view = drag.view,
            count,
            state = "released",
            "[editor] element marquee"
        );
    }

    /// Drops the marquee in flight because no release is coming.
    pub(super) fn cancel_marquee(&mut self, reason: &str) {
        if let Some(drag) = self.marquee.take() {
            info!(
                target: telemetry_target::EDITOR,
                view = drag.view,
                reason,
                state = "canceled",
                "[editor] element marquee"
            );
        }
    }

    /// The marquee the shell draws over a viewport, in the main window's
    /// logical points like its badges, or `None` when no live drag is open.
    pub(super) fn shell_marquee(&self) -> Option<ShellMarquee> {
        let drag = self.marquee.as_ref().filter(|drag| drag.live)?;
        let (origin_x, origin_y) = self.view_origin(drag.view);
        let rect = tools::MarqueeRect::from_corners(drag.from, drag.to);
        Some(ShellMarquee {
            view: drag.view,
            rect: ShellRect {
                x: (origin_x + rect.min_x) / self.scale_factor,
                y: (origin_y + rect.min_y) / self.scale_factor,
                width: (rect.max_x - rect.min_x) / self.scale_factor,
                height: (rect.max_y - rect.min_y) / self.scale_factor,
            },
        })
    }
}
