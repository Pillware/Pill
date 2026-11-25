//! The editor's tool layer: the modes a scene view can be in.
//!
//! # Responsibilities
//!
//! - Declare [`EditorTool`], the value a tool is described by: the name a
//!   command and the scripted channel spell, the label the toolbox shows, its
//!   optional shortcut, and the component that makes it offered for an entity.
//! - Hold the registered tools and the active one in [`ToolRegistry`], and
//!   name the three built-ins the editor always offers.
//! - Declare [`ToolExtension`], what a crate adds to a tool another crate owns:
//!   kinds, elements, frames, menu rows, chords and the press answer. The
//!   registry merges an attached extension's additions into what the base tool
//!   reports, and chains the hooks so the base always answers first.
//! - Turn what a tool reports into what the editor draws and picks: the
//!   element handles of one frame ([`element_handles`]) and the selection a
//!   marquee produces ([`marquee_selection`]).
//!
//! # Design
//!
//! A tool is a **mode**, not a widget: exactly one is active at a time, and
//! the editor's draw, pick and drag paths read which one it is. This module is
//! the vocabulary; the editor core applies the modes it already knows (the
//! transform gizmo's translate, rotate and scale), and an extension's editor
//! crate registers its own through the same registry.
//!
//! Everything here is plain data. A registered tool is a record of strings and
//! flags rather than a trait object, because the record outlives the image
//! that declared it: a reload replaces the record instead of dangling a
//! pointer into the image that is about to be unmapped.
//!
//! The registry is a world resource so it survives a shell reload and rides
//! the play session untouched, and it is named for `SetResourceField`-style
//! reading by the scripted channel.

// Standard library
use std::collections::BTreeMap;
use std::fmt;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{Engine, Entity, World};
use pill_renderer_api::GizmoPointShape;

// Current crate
use crate::editor_state::{
    EditorCommand, EditorSelection, ElementRef, ElementSelection, SelectionClick,
};
use crate::gizmo::{
    styled_color, ElementHandleMap, ElementHandleTarget, GizmoSpace, GizmoState, HandleStates,
};

// =============================================================================
// Tools
// =============================================================================

/// The three modes the editor offers with no extension loaded.
///
/// They are global tools: offered whatever the selection holds, because they
/// act on whole entities. A component tool names its own component instead and
/// is offered only for an entity that carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinTool {
    /// Move the selection.
    Translate,
    /// Turn the selection.
    Rotate,
    /// Resize the selection.
    Scale,
}

impl BuiltinTool {
    /// Every built-in, in the order the toolbox lists them.
    pub const ALL: [BuiltinTool; 3] = [
        BuiltinTool::Translate,
        BuiltinTool::Rotate,
        BuiltinTool::Scale,
    ];

    /// The name a command and the scripted channel spell, which is also the
    /// gizmo mode's own name.
    pub fn name(self) -> &'static str {
        match self {
            BuiltinTool::Translate => "translate",
            BuiltinTool::Rotate => "rotate",
            BuiltinTool::Scale => "scale",
        }
    }

    /// The label the toolbox row and the menus show.
    pub fn label(self) -> &'static str {
        match self {
            BuiltinTool::Translate => "Translate",
            BuiltinTool::Rotate => "Rotate",
            BuiltinTool::Scale => "Scale",
        }
    }

    /// The tool a name spells, when it spells one of the built-ins.
    pub fn from_name(name: &str) -> Option<BuiltinTool> {
        BuiltinTool::ALL
            .into_iter()
            .find(|tool| tool.name().eq_ignore_ascii_case(name))
    }

    /// The gizmo mode this tool drives: the three built-ins are the three
    /// modes, under the names tools use.
    pub fn gizmo_mode(self) -> crate::gizmo::GizmoMode {
        match self {
            BuiltinTool::Translate => crate::gizmo::GizmoMode::Translate,
            BuiltinTool::Rotate => crate::gizmo::GizmoMode::Rotate,
            BuiltinTool::Scale => crate::gizmo::GizmoMode::Scale,
        }
    }
}

/// What a tool is described by.
///
/// The built-ins implement it here; an extension's tool implements it in its
/// own editor crate and the editor core copies the answer into a
/// [`RegisteredTool`] record, so nothing holds a pointer into a reloadable
/// image.
pub trait EditorTool {
    /// The name a command and the scripted channel spell.
    fn name(&self) -> &'static str;

    /// The label the toolbox row and the menus show.
    fn label(&self) -> &'static str;

    /// The keyboard shortcut's text, when the tool has one.
    fn shortcut(&self) -> Option<&'static str> {
        None
    }

    /// Whether the tool is offered whatever the selection holds.
    ///
    /// The built-ins are global. A component tool answers `false`, because it
    /// is offered only while its component is selected.
    fn is_global(&self) -> bool {
        true
    }

    /// The component that makes the tool offered for an entity.
    ///
    /// The name is matched against an entity's component names by full name or
    /// by its `::` suffix, so a crate names its own component without spelling
    /// the module path. `None`, the default, means no entity offers the tool:
    /// that is a global tool, which needs no row of its own, and a tool that
    /// declares nothing (a test fixture), which stays out of the toolbox.
    fn requires_component(&self) -> Option<&'static str> {
        None
    }
}

impl EditorTool for BuiltinTool {
    fn name(&self) -> &'static str {
        BuiltinTool::name(*self)
    }

    fn label(&self) -> &'static str {
        BuiltinTool::label(*self)
    }

    fn shortcut(&self) -> Option<&'static str> {
        match self {
            BuiltinTool::Translate => Some("w"),
            BuiltinTool::Rotate => Some("e"),
            BuiltinTool::Scale => Some("r"),
        }
    }
}

/// One registered tool, as the registry keeps it.
///
/// Owned strings and flags only: a record must be readable after the image
/// that registered it is gone, so a reload re-registers instead of dangling.
#[derive(Clone, Debug)]
pub struct RegisteredTool {
    /// The name a command spells.
    pub name: String,
    /// The label the toolbox row shows.
    pub label: String,
    /// The shortcut's text, when the tool has one.
    pub shortcut: Option<String>,
    /// Whether the tool is offered whatever the selection holds.
    pub global: bool,
    /// The component that makes the tool offered for an entity, matched by
    /// full name or by its `::` suffix.
    pub requires_component: Option<String>,
    /// The tool's optional element reporting.
    pub hooks: ToolElementHooks,
}

impl PartialEq for RegisteredTool {
    /// Compares what a reader sees. The hooks are code addresses a reload
    /// replaces, so comparing them would report a difference no reader can
    /// act on.
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.label == other.label
            && self.shortcut == other.shortcut
            && self.global == other.global
            && self.requires_component == other.requires_component
    }
}

impl Eq for RegisteredTool {}

impl RegisteredTool {
    /// Records `tool` as the registry keeps it, with no element reporting.
    pub fn of(tool: &dyn EditorTool) -> RegisteredTool {
        RegisteredTool::with_hooks(tool, ToolElementHooks::default())
    }

    /// Records `tool` as the registry keeps it, with its element reporting.
    pub fn with_hooks(tool: &dyn EditorTool, hooks: ToolElementHooks) -> RegisteredTool {
        RegisteredTool {
            name: tool.name().to_string(),
            label: tool.label().to_string(),
            shortcut: tool.shortcut().map(str::to_string),
            global: tool.is_global(),
            requires_component: tool.requires_component().map(str::to_string),
            hooks,
        }
    }

    /// Whether `entity` carries the component that makes this tool offered.
    ///
    /// A global tool needs no component and a tool that declares none names no
    /// entity's component, so neither is ever offered this way.
    pub fn is_offered_for(&self, entity: Entity, world: &World) -> bool {
        !self.global
            && self
                .requires_component
                .as_deref()
                .is_some_and(|required| has_component_named(world, entity, required))
    }
}

// =============================================================================
// Elements a tool reports
// =============================================================================

/// What one kind of element a tool exposes looks like and allows.
///
/// Plain owned data: the editor keeps what a tool's reporting hook filled in,
/// even after the image that declared the kind is gone.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementKind {
    /// The kind's name, which is what a selection stores and what a refusal
    /// names.
    pub name: String,
    /// The silhouette the editor draws its handles as.
    pub shape: pill_renderer_api::GizmoPointShape,
    /// How wide its handles draw, in pixels, in every view.
    pub size_pixels: f32,
    /// The colour its idle handles draw in.
    pub color: [f32; 4],
    /// Whether a Ctrl/Shift click may extend a selection of this kind.
    pub multi_select: bool,
    /// Whether a marquee over empty space selects the elements it covers.
    pub box_select: bool,
}

impl ElementKind {
    /// A kind with the editor's defaults: a circular handle nine pixels wide,
    /// white, multi-select and box-select allowed.
    pub fn new(name: impl Into<String>) -> ElementKind {
        ElementKind {
            name: name.into(),
            shape: pill_renderer_api::GizmoPointShape::Circle,
            size_pixels: 9.0,
            color: [0.92, 0.92, 0.92, 1.0],
            multi_select: true,
            box_select: true,
        }
    }
}

/// Where one element is in the world, or where a drag moved it to.
///
/// The same three parts the engine's transform component holds, as plain
/// arrays: a tool reads its component into this shape when it reports an
/// element, and writes it back when the editor hands it a moved one. The
/// editor never reads a component's layout itself - the tool owns the mapping
/// between its data and these plain numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElementTransform {
    /// The element's place, in world space.
    pub translation: [f32; 3],
    /// The element's orientation, a unit quaternion in `x y z w` order.
    pub rotation: [f32; 4],
    /// The element's scale.
    pub scale: [f32; 3],
}

impl ElementTransform {
    /// The rotation that turns nothing.
    pub const IDENTITY_ROTATION: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    /// The scale that resizes nothing.
    pub const UNIT_SCALE: [f32; 3] = [1.0, 1.0, 1.0];

    /// An unrotated, unscaled transform at `translation`.
    pub fn at(translation: [f32; 3]) -> ElementTransform {
        ElementTransform {
            translation,
            rotation: ElementTransform::IDENTITY_ROTATION,
            scale: ElementTransform::UNIT_SCALE,
        }
    }
}

/// One element a tool reports for the entity it is editing.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolElement {
    /// The tool's own id for the element, stable while its component is.
    pub id: u64,
    /// The kind's name. The editor keeps one kind per selection, so a set that
    /// mixes kinds is refused by name.
    pub kind: String,
    /// Where it sits, in world space.
    pub transform: ElementTransform,
}

/// One element a drag moved, with the world transform it takes.
///
/// The editor works out where the drag puts each element and hands the whole
/// set to the tool in one call, so a tool never does the frame or the gizmo
/// maths itself.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementMove {
    /// The tool's own id for the element.
    pub element: u64,
    /// The world transform the element ends up with.
    pub transform: ElementTransform,
}

/// One space the active tool offers the gizmo's space toggle, named after
/// what it aligns with.
///
/// A tool returns these for the current selection, so a frame may differ per
/// point: a spline's frame is the curve's tangent and normal at the selected
/// point, not a property of the entity.
#[derive(Clone, Debug, PartialEq)]
pub struct CoordinateFrame {
    /// The name the space row, the menus and the log show.
    pub name: String,
    /// The rotation its axes are turned by, a unit quaternion in `x y z w`
    /// order, about the selection's pivot.
    pub rotation: [f32; 4],
}

/// Fills `out` with the kinds a tool exposes.
pub type ToolKindReportFn = fn(out: &mut Vec<ElementKind>);
/// Fills `out` with the elements `entity` has, as `world` describes it.
pub type ToolElementReportFn = fn(entity: Entity, world: &World, out: &mut Vec<ToolElement>);

/// Writes moved elements back into the tool's component.
///
/// The editor calls this with the world transforms a drag produced; the tool
/// maps them onto whatever it stores, so the frame maths, the snapping and the
/// undo step all stay the editor's.
pub type ToolSetTransformsFn = fn(entity: Entity, moves: &[ElementMove], world: &mut World);

/// Fills `out` with the frames the tool offers for the selection.
///
/// `selected` holds the ids the element selection is on, in selection order,
/// which is what a frame computed at the pivot needs.
pub type ToolFrameReportFn =
    fn(entity: Entity, selected: &[u64], world: &World, out: &mut Vec<CoordinateFrame>);

/// The optional element-reporting half of a tool.
///
/// These are plain function pointers rather than trait methods: a record outlives
/// the image that registered it, and a reload registers the arriving image's
/// pointers in place of the old ones.
#[derive(Clone, Copy, Debug, Default)]
pub struct ToolElementHooks {
    /// The kinds the tool exposes, `None` when it reports no elements.
    pub kinds: Option<ToolKindReportFn>,
    /// The elements the entity has, `None` when the tool reports none.
    pub elements: Option<ToolElementReportFn>,
    /// The write-back a gizmo drag reaches, `None` when the tool's elements
    /// cannot be moved.
    pub set_transforms: Option<ToolSetTransformsFn>,
    /// The extra spaces the gizmo's space toggle offers, `None` when the tool
    /// brings none.
    pub frames: Option<ToolFrameReportFn>,
    /// The rows the tool adds to the element context menu, `None` when it adds
    /// none.
    pub menu: Option<ToolMenuReportFn>,
    /// The action behind those rows, `None` when the tool's menu is empty.
    pub menu_run: Option<ToolMenuRunFn>,
    /// How the tool inserts an element, `None` when it cannot.
    pub insert: Option<ToolInsertElementFn>,
    /// How the tool removes elements, `None` when it cannot.
    pub remove: Option<ToolRemoveElementsFn>,
    /// What a press on one of the tool's elements means, `None` for the
    /// editor's own rule.
    pub press: Option<ToolPressFn>,
    /// What a key means while the tool owns the element selection, `None` for
    /// the editor's own rule.
    pub key: Option<ToolKeyFn>,
    /// The tool's own note of every selection change.
    pub selection_changed: Option<ToolSelectionChangedFn>,
}

impl ToolElementHooks {
    /// Whether the tool reports elements at all.
    pub fn reports_elements(self) -> bool {
        self.kinds.is_some() && self.elements.is_some()
    }

    /// Whether a drag can move the tool's elements.
    pub fn moves_elements(self) -> bool {
        self.set_transforms.is_some()
    }

    /// Whether the tool offers the gizmo a space of its own.
    pub fn reports_frames(self) -> bool {
        self.frames.is_some()
    }

    /// Whether the tool adds rows to the element context menu and can run
    /// them.
    pub fn has_menu(self) -> bool {
        self.menu.is_some() && self.menu_run.is_some()
    }

    /// Whether the tool inserts and removes elements.
    pub fn edits_elements(self) -> bool {
        self.insert.is_some() && self.remove.is_some()
    }
}

// =============================================================================
// What a tool does beside reporting
// =============================================================================

/// One row a tool adds to the element context menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementMenuEntry {
    /// The id the menu spells and `element menu <item>` names.
    pub id: String,
    /// The label the row shows.
    pub label: String,
}

/// Fills `out` with the menu rows the tool adds for the selection.
///
/// `selected` holds the ids the element selection is on, in selection order,
/// so a row can depend on what is selected.
pub type ToolMenuReportFn = fn(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    world: &World,
    out: &mut Vec<ElementMenuEntry>,
);

/// Runs one of the tool's menu rows, answering the commands it asks for.
///
/// The commands are the row's whole effect and run as one recorded batch, so a
/// menu row undoes in one step. A row that changes nothing answers an empty
/// list, and an id the tool does not know answers an error naming it.
pub type ToolMenuRunFn = fn(
    entity: Entity,
    kind: &str,
    selected: &[u64],
    item: &str,
    world: &World,
) -> Result<Vec<EditorCommand>, String>;

/// Inserts one element of the tool's component.
///
/// `id` is the id the caller asks the element to take, which is what an undo
/// restoring a removed element needs; `None` lets the tool choose one. The
/// answer is the id the element has now, or `None` when the tool cannot insert
/// it.
pub type ToolInsertElementFn = fn(
    entity: Entity,
    kind: &str,
    id: Option<u64>,
    transform: ElementTransform,
    world: &mut World,
) -> Option<u64>;

/// Removes elements of the tool's component, answering how many it removed.
pub type ToolRemoveElementsFn =
    fn(entity: Entity, kind: &str, elements: &[u64], world: &mut World) -> usize;

/// What a press on one element does.
///
/// The editor asks before it applies its own rule, so a tool can select what
/// it wants, run recorded commands, or swallow the press entirely.
#[derive(Clone, Debug, PartialEq)]
pub enum PressResponse {
    /// The editor's own select, extend and gizmo rule.
    Default,
    /// Select exactly these elements, of the pressed element's kind.
    Select(Vec<u64>),
    /// Run these commands instead, as one recorded batch.
    Commands(Vec<EditorCommand>),
    /// Do nothing at all: the selection stays as it is and no drag starts.
    Consume,
}

/// Answers one press on one element of the tool's own kind.
///
/// `click` is what the held modifiers mean, so a tool's rule can honour Shift
/// and Ctrl the way the editor's does.
pub type ToolPressFn = fn(
    entity: Entity,
    element: u64,
    click: SelectionClick,
    selected: &[u64],
    world: &World,
) -> PressResponse;

/// What a key does while the tool owns an element selection.
#[derive(Clone, Debug, PartialEq)]
pub enum KeyResponse {
    /// The editor's own rule, then the keybindings.
    Default,
    /// The tool took the key; nothing else sees it.
    Consume,
    /// Run these commands, as one recorded batch.
    Commands(Vec<EditorCommand>),
}

/// Answers one key press while the tool owns an element selection.
///
/// `key` is the key's wire name, lower case: `delete`, `k`. The hook is asked
/// only while an element selection exists and a tool is active, so a key the
/// editor binds keeps working everywhere else.
pub type ToolKeyFn = fn(entity: Entity, key: &str, selected: &[u64], world: &World) -> KeyResponse;

/// Hears every element-selection change, whatever changed it: a click, a
/// marquee, an undo, or a command that cleared the slot.
pub type ToolSelectionChangedFn =
    fn(entity: Entity, previous: &[u64], current: &[u64], world: &World);

// =============================================================================
// Element handles
// =============================================================================

/// The colour a selected element's handle draws in: the orange the scene's
/// selection outline uses, so one selection reads as one selection.
pub const SELECTED_ELEMENT_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 1.0];

/// How far past half an element's pixel size a press still picks it.
///
/// The renderer grows the pick quad of a drawn point by the same slack, so the
/// editor's own hit test and the pick buffer agree about which pixels belong to
/// a handle.
pub const ELEMENT_PICK_SLACK_PIXELS: f32 = 1.0;

/// One element of one frame, as the editor draws and picks it.
///
/// The editor builds these every frame from the active tool's report, so a
/// handle can never outlive the element it names. The fields are what the draw
/// call and the hit test need, so neither has to read the tool again.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementHandle {
    /// The handle id the element is pickable by.
    pub handle: u32,
    /// The entity the element belongs to.
    pub entity: Entity,
    /// The tool's own id for the element.
    pub element: u64,
    /// The kind's name, as the tool reported it.
    pub kind: String,
    /// Where it sits and which way it is turned, in world space: the press
    /// capture a drag restores reads it from here.
    pub transform: ElementTransform,
    /// The silhouette the handle draws as.
    pub shape: GizmoPointShape,
    /// How wide it draws, in pixels, in every view.
    pub size_pixels: f32,
    /// The colour it draws in, with its selection and hover state applied.
    pub color: [f32; 4],
    /// Whether it is part of the current selection.
    pub selected: bool,
}

impl ElementHandle {
    /// What a pick on this handle means.
    pub fn target(&self) -> ElementHandleTarget {
        ElementHandleTarget {
            entity: self.entity,
            element: self.element,
            kind: self.kind.clone(),
        }
    }

    /// Whether a press at `x`/`y`, in the same pixels the handle was
    /// projected to, hits it.
    ///
    /// The test is a square around the handle's centre, half its pixel size
    /// plus [`ELEMENT_PICK_SLACK_PIXELS`] wide on each side: the shape of the
    /// pick quad, not its silhouette. A circle's corners are therefore
    /// pickable too, exactly as they are in the pick buffer.
    pub fn covers_pixel(&self, x: f32, y: f32, centre: (f32, f32)) -> bool {
        let reach = self.size_pixels.max(1.0) * 0.5 + ELEMENT_PICK_SLACK_PIXELS;
        (x - centre.0).abs() <= reach && (y - centre.1).abs() <= reach
    }
}

/// What one frame's elements look like and which handle picks each.
///
/// `reported` is the active tool's report for `entity`, in its own order, so a
/// range click and a marquee both read the order the tool draws in. An element
/// whose kind the tool does not describe is drawn with [`ElementKind::new`]'s
/// defaults: a tool bug must not make an element invisible.
pub fn element_handles(
    reported: &[ToolElement],
    kinds: &[ElementKind],
    entity: Entity,
    selected: &ElementSelection,
    states: HandleStates,
) -> (Vec<ElementHandle>, ElementHandleMap) {
    let mut handles = Vec::with_capacity(reported.len());
    let mut map = ElementHandleMap::new();
    for element in reported {
        let kind = kinds
            .iter()
            .find(|kind| kind.name == element.kind)
            .cloned()
            .unwrap_or_else(|| ElementKind::new(element.kind.clone()));
        let is_selected = selected.kind() == kind.name && selected.contains(entity, element.id);
        let base_color = if is_selected {
            SELECTED_ELEMENT_COLOR
        } else {
            kind.color
        };
        let handle = map.push(ElementHandleTarget {
            entity,
            element: element.id,
            kind: kind.name.clone(),
        });
        handles.push(ElementHandle {
            handle,
            entity,
            element: element.id,
            kind: kind.name,
            transform: element.transform,
            shape: kind.shape,
            size_pixels: kind.size_pixels,
            color: styled_color(base_color, handle, states),
            selected: is_selected,
        });
    }
    (handles, map)
}

// =============================================================================
// Marquee
// =============================================================================

/// Where one reported element of one frame lands on a scene view's surface.
///
/// The editor projects the report itself, because only it knows the view's
/// camera; the marquee rule then needs no camera at all and is a plain
/// rectangle test.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedElement {
    /// The element.
    pub element: ElementRef,
    /// The kind's name, as the tool reported it.
    pub kind: String,
    /// Its place on the surface, in that surface's pixels.
    pub x: f32,
    /// Its place on the surface, in that surface's pixels.
    pub y: f32,
}

/// The rectangle a marquee drag covers, in one view's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarqueeRect {
    /// The left edge.
    pub min_x: f32,
    /// The top edge.
    pub min_y: f32,
    /// The right edge.
    pub max_x: f32,
    /// The bottom edge.
    pub max_y: f32,
}

impl MarqueeRect {
    /// The rectangle two drag corners enclose, in either direction.
    pub fn from_corners(first: (f32, f32), second: (f32, f32)) -> MarqueeRect {
        MarqueeRect {
            min_x: first.0.min(second.0),
            min_y: first.1.min(second.1),
            max_x: first.0.max(second.0),
            max_y: first.1.max(second.1),
        }
    }

    /// Whether `(x, y)` lies inside the rectangle, edges included.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }
}

/// The selection a marquee produces from one frame's projected elements.
///
/// A marquee selects one kind, like every element selection: it takes the
/// first kind, in the tool's own report order, that has an element inside the
/// rectangle, and every element of that kind the rectangle covers. A kind whose
/// [`ElementKind::box_select`] is off is never taken, and no element at all
/// gives an empty selection, which the caller records as a clear.
pub fn marquee_selection(
    reported: &[ProjectedElement],
    rect: MarqueeRect,
    kinds: &[ElementKind],
) -> ElementSelection {
    let mut selection = ElementSelection::default();
    for kind in kinds.iter().filter(|kind| kind.box_select) {
        let covered: Vec<ElementRef> = reported
            .iter()
            .filter(|element| element.kind == kind.name && rect.contains(element.x, element.y))
            .map(|element| element.element)
            .collect();
        if covered.is_empty() {
            continue;
        }
        selection.set(kind.name.clone(), covered);
        return selection;
    }
    selection
}

// =============================================================================
// Extensions
// =============================================================================

/// The optional halves an extension adds to the tool it extends.
///
/// Every field is a plain function pointer, and `None` means the extension
/// does not add that half. The registry records the pointers, so a reload
/// registers the arriving image's in place of the old ones, exactly as
/// [`ToolElementHooks`] does for a tool.
#[derive(Clone, Copy, Debug, Default)]
pub struct ToolExtensionHooks {
    /// The element kinds it adds.
    pub kinds: Option<ToolKindReportFn>,
    /// The elements it reports for the entity, of its own kinds.
    pub elements: Option<ToolElementReportFn>,
    /// How a drag writes its elements back.
    pub set_transforms: Option<ToolSetTransformsFn>,
    /// The extra coordinate frames it offers.
    pub frames: Option<ToolFrameReportFn>,
    /// The rows it adds to the element context menu.
    pub menu: Option<ToolMenuReportFn>,
    /// The action behind those rows.
    pub menu_run: Option<ToolMenuRunFn>,
    /// How it inserts one of its elements.
    pub insert: Option<ToolInsertElementFn>,
    /// How it removes its elements.
    pub remove: Option<ToolRemoveElementsFn>,
    /// What a press on one of its elements means.
    pub press: Option<ToolPressFn>,
    /// What one of its chords means.
    pub key: Option<ToolKeyFn>,
}

impl ToolExtensionHooks {
    /// Whether the extension adds any half at all.
    pub fn is_empty(self) -> bool {
        self.kinds.is_none()
            && self.elements.is_none()
            && self.set_transforms.is_none()
            && self.frames.is_none()
            && self.menu.is_none()
            && self.menu_run.is_none()
            && self.insert.is_none()
            && self.remove.is_none()
            && self.press.is_none()
            && self.key.is_none()
    }
}

/// A crate's addition to a tool another crate owns.
///
/// There is no tool object and no `self` here on purpose. A registry record
/// outlives the image that registered it, so the extension states its
/// declaration and its hooks as associated items: the registry copies them
/// into a [`RegisteredExtension`] and a reload registers the arriving image's
/// hooks in place of the old ones.
///
/// An extension only ever **adds**. It cannot remove or rename what the base
/// tool declares, and nothing it declares is visible while its base tool is
/// not the active one.
pub trait ToolExtension {
    /// The extension's own name: what `dump tools` and the logs show.
    fn name() -> &'static str;

    /// The base tool it extends, by the name a command spells.
    fn extends_tool() -> &'static str;

    /// The kinds of the base tool its additions apply to.
    ///
    /// Empty, the default, means every kind the base tool reports. A non-empty
    /// list keeps its menu rows, presses and keys out of the other kinds, so
    /// an extension's rows never appear for an element they do not describe.
    fn for_kinds() -> &'static [&'static str] {
        &[]
    }

    /// The component an entity must carry for the extension to be attached.
    ///
    /// It is matched against the entity's component names by full name or by
    /// `::` suffix, the way every other component name in the editor is. `None`
    /// attaches the extension to every entity its base tool edits.
    fn requires_component() -> Option<&'static str> {
        None
    }

    /// Where the extension sorts against the others on the same base.
    ///
    /// Lower runs first: its menu rows come earlier and its chord wins a clash.
    /// Equal values keep name order, so the chain never depends on the order
    /// the crates happened to register in.
    fn order() -> i32 {
        0
    }

    /// The chords it answers, as the keys module spells them (`x`, `delete`).
    fn keys() -> &'static [&'static str] {
        &[]
    }

    /// What it adds to the tool. Every hook defaults to nothing.
    fn hooks() -> ToolExtensionHooks {
        ToolExtensionHooks::default()
    }
}

/// One registered extension, as the registry keeps it.
///
/// Owned strings and function pointers only, for the same reason
/// [`RegisteredTool`] is: the record must be readable after the image that
/// registered it is gone.
#[derive(Clone, Debug)]
pub struct RegisteredExtension {
    /// The extension's own name.
    pub name: String,
    /// The base tool's name.
    pub base_tool: String,
    /// The kinds its additions apply to; empty means every kind.
    pub for_kinds: Vec<String>,
    /// The component an entity must carry; `None` attaches to every entity.
    pub requires_component: Option<String>,
    /// Its place in the chain against the other extensions of one base.
    pub order: i32,
    /// The chords it answers.
    pub keys: Vec<String>,
    /// What it adds.
    pub hooks: ToolExtensionHooks,
}

impl PartialEq for RegisteredExtension {
    /// Compares what a reader sees. The hooks are code addresses a reload
    /// replaces, so comparing them would report a difference no reader can act
    /// on.
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.base_tool == other.base_tool
            && self.for_kinds == other.for_kinds
            && self.requires_component == other.requires_component
            && self.order == other.order
            && self.keys == other.keys
    }
}

impl Eq for RegisteredExtension {}

impl RegisteredExtension {
    /// Records `E` as the registry keeps it.
    pub fn of<E: ToolExtension>() -> RegisteredExtension {
        RegisteredExtension {
            name: E::name().to_string(),
            base_tool: E::extends_tool().to_string(),
            for_kinds: E::for_kinds().iter().map(|kind| kind.to_string()).collect(),
            requires_component: E::requires_component().map(str::to_string),
            order: E::order(),
            keys: E::keys().iter().map(|key| key.to_string()).collect(),
            hooks: E::hooks(),
        }
    }

    /// Whether its additions apply to `kind`.
    pub fn applies_to_kind(&self, kind: &str) -> bool {
        self.for_kinds.is_empty()
            || self
                .for_kinds
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(kind))
    }

    /// Whether it answers `chord`.
    pub fn binds_key(&self, chord: &str) -> bool {
        self.keys
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(chord))
    }

    /// Whether `entity` carries the component the extension requires.
    ///
    /// An extension that requires none is always attached.
    pub fn requires_component_of(&self, entity: Entity, world: &World) -> bool {
        let Some(required) = self.requires_component.as_deref() else {
            return true;
        };
        has_component_named(world, entity, required)
    }
}

// =============================================================================
// Registry
// =============================================================================

/// The name [`ToolRegistry`] registers under.
pub const RESOURCE_NAME: &str = "pill_editor_core::ToolRegistry";

/// Whether `entity` carries a component named `required`.
///
/// The name is matched in full or by its `::` suffix, which is the one rule
/// every declaration that names a component follows: a crate names its own
/// component without spelling the module path, and a name that matches nothing
/// is simply absent.
pub fn has_component_named(world: &World, entity: Entity, required: &str) -> bool {
    let suffix = format!("::{required}");
    world.entity_component_names(entity).is_some_and(|names| {
        names
            .iter()
            .any(|name| name == required || name.ends_with(&suffix))
    })
}

/// Every registered tool, and which one is active.
///
/// One tool is active at a time; `None` is the neutral state (no tool), which
/// is what `tool none` and the Esc key produce. The three built-ins are
/// present from the registry's creation, so a fresh editor's `dump tools`
/// lists them without anything having opened a toolbox.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRegistry {
    /// The registered tools, in registration order.
    tools: Vec<RegisteredTool>,
    /// The registered extensions, sorted by `(order, name)`.
    extensions: Vec<RegisteredExtension>,
    /// The active tool's name, or `None` when no tool is active.
    active: Option<String>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        let mut registry = ToolRegistry {
            tools: Vec::new(),
            extensions: Vec::new(),
            active: None,
        };
        for tool in BuiltinTool::ALL {
            registry.register(&tool);
        }
        registry
    }
}

impl ToolRegistry {
    /// Registers `tool`, replacing the record with the same name.
    ///
    /// Replacing in place rather than appending is what a reload relies on:
    /// the arriving image's record must take the old one's slot, so the
    /// toolbox row and any command naming the tool keep working.
    pub fn register(&mut self, tool: &dyn EditorTool) {
        self.register_plugin(tool, ToolElementHooks::default());
    }

    /// Registers `tool` with its element reporting, replacing the record with
    /// the same name.
    ///
    /// A tool that reports elements registers through this call: the hooks are
    /// plain function pointers of the image that declared the tool, so the
    /// reload's re-registration is what keeps them pointing at live code.
    pub fn register_plugin(&mut self, tool: &dyn EditorTool, hooks: ToolElementHooks) {
        let record = RegisteredTool::with_hooks(tool, hooks);
        match self
            .tools
            .iter_mut()
            .find(|registered| registered.name == record.name)
        {
            Some(existing) => *existing = record,
            None => self.tools.push(record),
        }
    }

    /// The active tool's record, or `None` when no tool is active.
    pub fn active_record(&self) -> Option<&RegisteredTool> {
        let active = self.active.as_deref()?;
        self.tools
            .iter()
            .find(|tool| tool.name.eq_ignore_ascii_case(active))
    }

    // -------------------------------------------------------------------------
    // Extensions
    // -------------------------------------------------------------------------

    /// Registers an extension of a tool another crate owns.
    ///
    /// Registration replaces the record with the same name, which is what a
    /// reload relies on, and keeps the list sorted by `(order, name)` so the
    /// chain never depends on the order the crates happened to load in.
    ///
    /// A chord two extensions of one base both bind is resolved by that order.
    /// The clash is logged once, when the extension that creates it is first
    /// registered: re-registering an extension a reload replaced does not log
    /// it again.
    pub fn register_extension<E: ToolExtension>(&mut self) {
        let record = RegisteredExtension::of::<E>();
        let is_new = !self
            .extensions
            .iter()
            .any(|registered| registered.name == record.name);
        match self
            .extensions
            .iter_mut()
            .find(|registered| registered.name == record.name)
        {
            Some(existing) => *existing = record.clone(),
            None => self.extensions.push(record.clone()),
        }
        self.extensions.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.name.cmp(&right.name))
        });
        if is_new {
            self.report_key_conflicts(&record);
        }
    }

    /// Logs every chord `record` binds that another extension of its base binds
    /// too, naming the extension that wins it.
    fn report_key_conflicts(&self, record: &RegisteredExtension) {
        for chord in &record.keys {
            let mut binding_extensions: Vec<&RegisteredExtension> = self
                .extensions
                .iter()
                .filter(|other| {
                    other.base_tool.eq_ignore_ascii_case(&record.base_tool)
                        && other.binds_key(chord)
                })
                .collect();
            if binding_extensions.len() < 2 {
                continue;
            }
            binding_extensions.sort_by(|left, right| {
                left.order
                    .cmp(&right.order)
                    .then_with(|| left.name.cmp(&right.name))
            });
            let names = binding_extensions
                .iter()
                .map(|extension| extension.name.as_str())
                .collect::<Vec<&str>>()
                .join(",");
            pill_core::info!(
                target: telemetry_target::EDITOR,
                base = record.base_tool.as_str(),
                chord = chord.as_str(),
                extensions = names.as_str(),
                winner = binding_extensions[0].name.as_str(),
                "[editor] tool extension key conflict"
            );
        }
    }

    /// Every registered extension, sorted by `(order, name)`.
    pub fn extensions(&self) -> &[RegisteredExtension] {
        &self.extensions
    }

    /// Drops every registered extension.
    ///
    /// A host calls this when the images that declared them are gone - a module
    /// unloaded for good, or a test that wants the plain tool back - so no
    /// record is left pointing at code that is no longer mapped.
    pub fn clear_extensions(&mut self) {
        self.extensions.clear();
    }

    /// The extensions attached to the active tool for `entity`.
    ///
    /// Attached means: a tool is active, its name is the extension's base, and
    /// the entity carries the component the extension requires. Empty when the
    /// registry holds no extension, which is the case for an editor nobody
    /// extended.
    pub fn attached_extensions<'a>(
        &'a self,
        entity: Entity,
        world: &World,
    ) -> Vec<&'a RegisteredExtension> {
        let Some(active) = self.active.as_deref() else {
            return Vec::new();
        };
        self.extensions
            .iter()
            .filter(|extension| {
                extension.base_tool.eq_ignore_ascii_case(active)
                    && extension.requires_component_of(entity, world)
            })
            .collect()
    }

    /// Its name in the log and in `dump tools`.
    pub fn is_extension_attached(&self, name: &str, entity: Entity, world: &World) -> bool {
        self.attached_extensions(entity, world)
            .iter()
            .any(|extension| extension.name == name)
    }

    /// The kinds the active tool and its attached extensions expose, in their
    /// own order.
    ///
    /// The base tool's kinds come first, then each attached extension's, so an
    /// extension's kind is an addition the base has never heard of. A kind name
    /// the base already declares is not replaced: the first declaration wins,
    /// which is what keeps an extension from renaming what it extends.
    ///
    /// Empty when no tool is active or nothing reports elements.
    pub fn active_kinds(&self, entity: Entity, world: &World) -> Vec<ElementKind> {
        let mut kinds = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.kinds) {
            report(&mut kinds);
        }
        for extension in self.attached_extensions(entity, world) {
            let Some(report) = extension.hooks.kinds else {
                continue;
            };
            let mut added = Vec::new();
            report(&mut added);
            for kind in added {
                if kinds.iter().any(|known| known.name == kind.name) {
                    continue;
                }
                kinds.push(kind);
            }
        }
        kinds
    }

    /// One of the active tool's kinds, by name.
    ///
    /// The base tool's own kinds only: a caller that needs a kind an extension
    /// added asks [`Self::active_kinds`], which merges both halves.
    pub fn active_kind(&self, name: &str) -> Option<ElementKind> {
        let mut kinds = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.kinds) {
            report(&mut kinds);
        }
        kinds.into_iter().find(|kind| kind.name == name)
    }

    /// The elements the active tool and its attached extensions report for
    /// `entity`, in their own order.
    ///
    /// Empty when no tool is active or nothing reports elements.
    pub fn active_elements(&self, entity: Entity, world: &World) -> Vec<ToolElement> {
        let mut elements = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.elements) {
            report(entity, world, &mut elements);
        }
        for extension in self.attached_extensions(entity, world) {
            if let Some(report) = extension.hooks.elements {
                report(entity, world, &mut elements);
            }
        }
        elements
    }

    /// Whether anything drawn would come of the active tool for `entity`.
    ///
    /// This is the gate the editor asks before it reports element handles at
    /// all: a tool that reports no elements, with no attached extension that
    /// does, leaves the entity with none.
    pub fn active_reports_elements(&self, entity: Entity, world: &World) -> bool {
        self.active_record()
            .is_some_and(|tool| tool.hooks.reports_elements())
            || self
                .attached_extensions(entity, world)
                .iter()
                .any(|extension| {
                    extension.hooks.kinds.is_some() && extension.hooks.elements.is_some()
                })
    }

    /// Whether the active tool writes element transforms back.
    ///
    /// The editor asks this before it offers an element drag: a tool that
    /// reports elements but takes no moves leaves the drag on the entity. An
    /// attached extension that can move its own elements counts, so a base
    /// tool that only draws them still gets a drag once an extension adds the
    /// write-back.
    pub fn active_moves_elements(&self, entity: Entity, world: &World) -> bool {
        self.active_record()
            .is_some_and(|tool| tool.hooks.moves_elements())
            || self
                .attached_extensions(entity, world)
                .iter()
                .any(|extension| extension.hooks.set_transforms.is_some())
    }

    /// Whether the active tool or an attached extension removes elements.
    ///
    /// Delete falls through to the entity rule when nothing can: the gate the
    /// editor asks before it queues an element removal.
    pub fn active_removes_elements(&self, entity: Entity, world: &World) -> bool {
        self.active_record()
            .is_some_and(|tool| tool.hooks.remove.is_some())
            || self
                .attached_extensions(entity, world)
                .iter()
                .any(|extension| extension.hooks.remove.is_some())
    }

    /// Hands `moves` to the active tool's write-back and to every attached
    /// extension's, and answers whether anything took them.
    ///
    /// Every writer is called: a tool writes its own elements and an extension
    /// its own, so each one ignores the ids it does not know. The editor's gizmo
    /// drag and its undo step both write through this one call, so a tool sees
    /// no difference between a live drag and a restored value.
    pub fn active_set_transforms(
        &self,
        entity: Entity,
        moves: &[ElementMove],
        world: &mut World,
    ) -> bool {
        let mut wrote = false;
        if let Some(write) = self
            .active_record()
            .and_then(|tool| tool.hooks.set_transforms)
        {
            write(entity, moves, world);
            wrote = true;
        }
        let extensions: Vec<ToolSetTransformsFn> = self
            .attached_extensions(entity, world)
            .iter()
            .filter_map(|extension| extension.hooks.set_transforms)
            .collect();
        for write in extensions {
            write(entity, moves, world);
            wrote = true;
        }
        wrote
    }

    /// The extra spaces the active tool and its attached extensions offer for
    /// `selected`, in their own order.
    ///
    /// Empty when no tool is active, nothing brings frames, or they have
    /// nothing to say about this selection.
    pub fn active_frames(
        &self,
        entity: Entity,
        selected: &[u64],
        world: &World,
    ) -> Vec<CoordinateFrame> {
        let mut frames = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.frames) {
            report(entity, selected, world, &mut frames);
        }
        for extension in self.attached_extensions(entity, world) {
            if let Some(report) = extension.hooks.frames {
                report(entity, selected, world, &mut frames);
            }
        }
        frames
    }

    /// The menu rows the active tool and its attached extensions add for
    /// `selected`, in `(order, name)` order.
    ///
    /// The base tool's rows come first, then each extension's, so an
    /// extension's row always reads after what it extends. A row id the base
    /// already uses is skipped: an extension cannot replace a base entry, only
    /// add one beside it.
    ///
    /// Empty when no tool is active, nothing adds rows, or they have nothing to
    /// say about this selection.
    pub fn active_menu(
        &self,
        entity: Entity,
        kind: &str,
        selected: &[u64],
        world: &World,
    ) -> Vec<ElementMenuEntry> {
        let mut menu = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.menu) {
            report(entity, kind, selected, world, &mut menu);
        }
        for extension in self.attached_extensions(entity, world) {
            if !extension.applies_to_kind(kind) {
                continue;
            }
            let Some(report) = extension.hooks.menu else {
                continue;
            };
            let mut added = Vec::new();
            report(entity, kind, selected, world, &mut added);
            for entry in added {
                if menu.iter().any(|known| known.id == entry.id) {
                    continue;
                }
                menu.push(entry);
            }
        }
        menu
    }

    /// Runs one of the active tool's menu rows, answering the commands it asks
    /// for.
    ///
    /// The base tool is asked first; an extension answers only when the base
    /// refuses the row, so an extension can add rows but never take one over.
    ///
    /// # Errors
    ///
    /// Returns a message when no tool is active, when nothing adds a menu, or
    /// when nobody claims `item` - which is how a menu row nobody registered
    /// reads instead of running nothing silently.
    pub fn active_menu_run(
        &self,
        entity: Entity,
        kind: &str,
        selected: &[u64],
        item: &str,
        world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        let Some(tool) = self.active_record() else {
            return Err(format!("no tool is active, so '{item}' has no menu row"));
        };
        let base_outcome = match tool.hooks.menu_run {
            Some(run) => run(entity, kind, selected, item, world),
            None => Err(format!("the tool '{}' adds no menu rows", tool.name)),
        };
        if base_outcome.is_ok() {
            return base_outcome;
        }
        for extension in self.attached_extensions(entity, world) {
            if !extension.applies_to_kind(kind) {
                continue;
            }
            if let Some(run) = extension.hooks.menu_run {
                if let Ok(commands) = run(entity, kind, selected, item, world) {
                    return Ok(commands);
                }
            }
        }
        base_outcome
    }

    /// Inserts one element through the active tool or an attached extension,
    /// answering its id.
    ///
    /// The base tool is asked first, then the extensions in order; an extension
    /// adds an element of its own kind, which the base refuses by not knowing
    /// it. `None` when nothing can insert, which the caller reports as a refused
    /// edit instead of writing nothing.
    pub fn active_insert(
        &self,
        entity: Entity,
        kind: &str,
        id: Option<u64>,
        transform: ElementTransform,
        world: &mut World,
    ) -> Option<u64> {
        if let Some(insert) = self.active_record().and_then(|tool| tool.hooks.insert) {
            if let Some(assigned) = insert(entity, kind, id, transform, world) {
                return Some(assigned);
            }
        }
        let extensions: Vec<ToolInsertElementFn> = self
            .attached_extensions(entity, world)
            .iter()
            .filter(|extension| extension.applies_to_kind(kind))
            .filter_map(|extension| extension.hooks.insert)
            .collect();
        for insert in extensions {
            if let Some(assigned) = insert(entity, kind, id, transform, world) {
                return Some(assigned);
            }
        }
        None
    }

    /// Removes elements through the active tool and every attached extension,
    /// answering how many went.
    ///
    /// `None` when no tool is active or nothing can remove.
    pub fn active_remove(
        &self,
        entity: Entity,
        kind: &str,
        elements: &[u64],
        world: &mut World,
    ) -> Option<usize> {
        let mut removed = None;
        if let Some(remove) = self.active_record().and_then(|tool| tool.hooks.remove) {
            removed = Some(remove(entity, kind, elements, world));
        }
        let extensions: Vec<ToolRemoveElementsFn> = self
            .attached_extensions(entity, world)
            .iter()
            .filter(|extension| extension.applies_to_kind(kind))
            .filter_map(|extension| extension.hooks.remove)
            .collect();
        for remove in extensions {
            let count = remove(entity, kind, elements, world);
            removed = Some(removed.unwrap_or(0) + count);
        }
        removed
    }

    /// What the active tool and its attached extensions make of a press on one
    /// of `kind`'s elements.
    ///
    /// The chain the design states: the base tool is asked first, and an
    /// extension is asked only while the answer so far is
    /// [`PressResponse::Default`]. The first answer that is not `Default` wins.
    pub fn active_press(
        &self,
        entity: Entity,
        element: u64,
        kind: &str,
        click: SelectionClick,
        selected: &[u64],
        world: &World,
    ) -> PressResponse {
        if let Some(press) = self.active_record().and_then(|tool| tool.hooks.press) {
            let response = press(entity, element, click, selected, world);
            if response != PressResponse::Default {
                return response;
            }
        }
        for extension in self.attached_extensions(entity, world) {
            if !extension.applies_to_kind(kind) {
                continue;
            }
            let Some(press) = extension.hooks.press else {
                continue;
            };
            let response = press(entity, element, click, selected, world);
            if response != PressResponse::Default {
                return response;
            }
        }
        PressResponse::Default
    }

    /// What the active tool and its attached extensions make of a key press over
    /// its element selection.
    ///
    /// Same chain as [`Self::active_press`]: the base tool first, then the
    /// extensions attached to `entity` in `(order, name)` order. An extension is
    /// asked only for a chord it declares, which is what makes "bound twice"
    /// meaningful: the lower `order` answers first and wins.
    ///
    /// [`KeyResponse::Default`] when nothing claims the key, so the editor's own
    /// rule and then the keybindings still see it.
    pub fn active_key(
        &self,
        entity: Entity,
        kind: &str,
        key: &str,
        selected: &[u64],
        world: &World,
    ) -> KeyResponse {
        if let Some(handle) = self.active_record().and_then(|tool| tool.hooks.key) {
            let response = handle(entity, key, selected, world);
            if response != KeyResponse::Default {
                return response;
            }
        }
        for extension in self.attached_extensions(entity, world) {
            if !extension.binds_key(key) || !extension.applies_to_kind(kind) {
                continue;
            }
            let Some(handle) = extension.hooks.key else {
                continue;
            };
            let response = handle(entity, key, selected, world);
            if response != KeyResponse::Default {
                return response;
            }
        }
        KeyResponse::Default
    }

    /// Tells the active tool that the element selection changed.
    ///
    /// Called for every change whatever made it, so a tool that mirrors the
    /// selection somewhere hears about an undo the same way it hears about a
    /// click.
    pub fn notify_element_selection_changed(
        &self,
        entity: Entity,
        previous: &[u64],
        current: &[u64],
        world: &World,
    ) {
        if let Some(changed) = self
            .active_record()
            .and_then(|tool| tool.hooks.selection_changed)
        {
            changed(entity, previous, current, world);
        }
    }

    /// The registered tools, in registration order.
    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
    }

    /// The component tools `entity` is offered, in registration order.
    ///
    /// A tool is offered only when it is not global and the entity carries the
    /// component it declares. The toolbox draws one row per entry and names it
    /// by the tool's own name; the built-ins are global, so they never appear
    /// here.
    pub fn offered_component_tools(&self, entity: Entity, world: &World) -> Vec<&RegisteredTool> {
        self.tools
            .iter()
            .filter(|tool| tool.is_offered_for(entity, world))
            .collect()
    }

    /// The active tool's name, or `None` when no tool is active.
    pub fn active(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// Returns whether `name` is a registered tool's name.
    pub fn contains(&self, name: &str) -> bool {
        self.tools
            .iter()
            .any(|tool| tool.name.eq_ignore_ascii_case(name))
    }

    /// Makes the tool `name` active, or clears the active tool with `None`.
    ///
    /// # Errors
    ///
    /// Returns a message naming the tools that exist when `name` is not one of
    /// them, so a scripted line or a row that names a tool nobody registered
    /// says so instead of silently doing nothing.
    pub fn set_active(&mut self, name: Option<&str>) -> Result<(), String> {
        match name {
            None => {
                self.active = None;
                Ok(())
            }
            Some(name) => {
                let Some(tool) = self
                    .tools
                    .iter()
                    .find(|tool| tool.name.eq_ignore_ascii_case(name))
                else {
                    let known: Vec<&str> =
                        self.tools.iter().map(|tool| tool.name.as_str()).collect();
                    return Err(format!(
                        "no tool is named '{name}'; the registered tools are {}",
                        known.join(", ")
                    ));
                };
                self.active = Some(tool.name.clone());
                Ok(())
            }
        }
    }
}

impl pill_engine::Resource for ToolRegistry {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some(RESOURCE_NAME)
    }
}

impl fmt::Display for ToolRegistry {
    /// One line per tool, the active one marked, which is what `dump tools`
    /// logs. An extension follows the tool it extends, indented and marked
    /// with where it sits in the chain.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for tool in &self.tools {
            let active = self.active.as_deref() == Some(tool.name.as_str());
            writeln!(
                formatter,
                "{}{} ({})",
                if active { "* " } else { "  " },
                tool.label,
                tool.name
            )?;
            for extension in self
                .extensions
                .iter()
                .filter(|extension| extension.base_tool.eq_ignore_ascii_case(&tool.name))
            {
                writeln!(
                    formatter,
                    "    + {} (order {})",
                    extension.name, extension.order
                )?;
            }
        }
        Ok(())
    }
}

// =============================================================================
// The space each tool remembers
// =============================================================================

/// The name [`ToolSpaceChoices`] registers under.
pub const SPACE_CHOICES_RESOURCE_NAME: &str = "pill_editor_core::ToolSpaceChoices";

/// The gizmo space one tool was last on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceChoice {
    /// The space setting that was in force.
    pub space: GizmoSpace,
    /// The tool frame it was in, when that was one of the tool's own frames.
    pub frame: Option<u8>,
}

/// The tool frame each tool was last switched to, for the session.
///
/// The gizmo's space is a setting the preferences file keeps, so it can only
/// name World or Local. A tool frame is session state: this resource remembers
/// which frame a tool was on, so choosing another tool and coming back returns
/// to it, while a tool that was never on a frame comes back on the setting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolSpaceChoices {
    by_tool: BTreeMap<String, SpaceChoice>,
}

impl ToolSpaceChoices {
    /// What `tool` was last on, or `None` when it was never switched.
    pub fn choice(&self, tool: &str) -> Option<SpaceChoice> {
        self.by_tool.get(tool).copied()
    }

    /// Records that `tool` is now on `space`, in `frame` when that is one of
    /// the tool's frames.
    pub fn remember(&mut self, tool: &str, space: GizmoSpace, frame: Option<u8>) {
        self.by_tool
            .insert(tool.to_string(), SpaceChoice { space, frame });
    }
}

impl pill_engine::Resource for ToolSpaceChoices {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some(SPACE_CHOICES_RESOURCE_NAME)
    }
}

// =============================================================================
// World access
// =============================================================================

/// Returns the world's tool registry, inserting the built-ins when the world
/// has none.
///
/// Every reader goes through this function so the three built-ins are present
/// even in a world built by a test or by a headless run that never opened a
/// toolbox.
pub fn tool_registry_mut(world: &mut World) -> &mut ToolRegistry {
    if world.get_resource::<ToolRegistry>().is_none() {
        world.insert_resource(ToolRegistry::default());
    }
    world
        .get_resource_mut::<ToolRegistry>()
        .expect("the registry was inserted just above")
}

/// The frames the active tool offers for what the gizmo draws on: the element
/// selection's own entity, or the primary entity when no element is selected.
///
/// Every reader of a tool's frames resolves them here, so the space row, the
/// snapshot and the per-tool memory all name one frame per index.
pub fn selection_frames(world: &World) -> Vec<CoordinateFrame> {
    let Some(registry) = world.get_resource::<ToolRegistry>() else {
        return Vec::new();
    };
    let selector = world.get_resource::<EditorSelection>();
    let (entity, selected) = match selector {
        Some(selection) => (
            selection.elements().owner().or_else(|| selection.primary()),
            selection
                .elements()
                .elements()
                .iter()
                .map(|element| element.id)
                .collect::<Vec<u64>>(),
        ),
        None => (None, Vec::new()),
    };
    match entity {
        Some(entity) => registry.active_frames(entity, &selected, world),
        None => Vec::new(),
    }
}

/// What a tool acts on: the entity the element selection belongs to (or the
/// primary entity when nothing is selected yet), the kind the selection is on,
/// and the selected ids in selection order.
///
/// One resolution serves the menu, the press hook, the key hook and the
/// snapshot, so every hook is told about the same subject.
pub fn element_selection_subject(world: &World) -> Option<(Entity, String, Vec<u64>)> {
    let selection = world.get_resource::<EditorSelection>()?;
    let elements = selection.elements();
    let entity = elements.owner().or_else(|| selection.primary())?;
    let ids = elements
        .elements()
        .iter()
        .map(|element| element.id)
        .collect::<Vec<u64>>();
    Some((entity, elements.kind().to_string(), ids))
}

/// Makes `tool` the active one, and puts the gizmo back on the space that tool
/// was last on.
///
/// Choosing a tool is not a space change on its own: a tool nobody switched
/// leaves the gizmo where it is, and a tool that was on a frame comes back on
/// it, so switching between two tools with their own frames keeps each one's
/// frame.
///
/// # Errors
///
/// Returns the registry's refusal when no tool has that name, and changes
/// nothing then.
pub fn activate_tool(world: &mut World, tool: Option<&str>) -> Result<(), String> {
    tool_registry_mut(world).set_active(tool)?;
    let frames = selection_frames(world);
    if world.get_resource::<GizmoState>().is_none() {
        world.insert_resource(GizmoState::default());
    }
    if world.get_resource::<ToolSpaceChoices>().is_none() {
        world.insert_resource(ToolSpaceChoices::default());
    }
    let choices = world
        .get_resource::<ToolSpaceChoices>()
        .cloned()
        .unwrap_or_default();
    if let Some(state) = world.get_resource_mut::<GizmoState>() {
        crate::gizmo::follow_tool(state, &choices, tool, frames.len());
    }
    Ok(())
}

/// Returns the active tool's name, or `None` when no tool is active.
pub fn active_tool(engine: &Engine) -> Option<String> {
    engine
        .world()
        .get_resource::<ToolRegistry>()
        .and_then(|registry| registry.active().map(str::to_string))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool that is not a built-in, registered to prove the registry
    /// replaces rather than appends.
    struct ProbeTool;

    impl EditorTool for ProbeTool {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn label(&self) -> &'static str {
            "Probe"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    /// A tool with the probe's name and another label: what a reloaded image
    /// reports, which must take the first one's slot.
    struct ReloadedProbeTool;

    impl EditorTool for ReloadedProbeTool {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn label(&self) -> &'static str {
            "Probe reloaded"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_fresh_registry_lists_the_three_builtins_and_no_active_tool() {
        let registry = ToolRegistry::default();
        let names: Vec<&str> = registry
            .tools()
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        assert_eq!(names, ["translate", "rotate", "scale"]);
        assert_eq!(registry.active(), None);
        assert!(registry.tools().iter().all(|tool| tool.global));
    }

    /// A component tool over the engine's transform component, named by its
    /// `::` suffix - the spelling a crate uses without knowing the module path.
    struct TransformTool;

    impl EditorTool for TransformTool {
        fn name(&self) -> &'static str {
            "transform"
        }

        fn label(&self) -> &'static str {
            "Transform"
        }

        fn is_global(&self) -> bool {
            false
        }

        fn requires_component(&self) -> Option<&'static str> {
            Some("TransformComponent")
        }
    }

    /// A world with one entity that carries a transform and one that carries
    /// nothing.
    fn component_tool_world() -> (World, Entity, Entity) {
        let mut world = World::default();
        world.register_component::<pill_engine::TransformComponent>();
        let carrier = world
            .create_entity()
            .with(pill_engine::TransformComponent::default())
            .build()
            .expect("the carrier builds");
        let plain = world
            .create_entity()
            .build()
            .expect("the plain entity builds");
        (world, carrier, plain)
    }

    /// A tool is offered only for an entity that carries the component it
    /// names, matched by the name's `::` suffix.
    #[test]
    fn a_component_tool_is_offered_only_for_an_entity_that_carries_its_component() {
        let (world, carrier, plain) = component_tool_world();
        let mut registry = ToolRegistry::default();
        registry.register(&TransformTool);
        registry.register(&ProbeTool);

        let offered = registry.offered_component_tools(carrier, &world);
        let names: Vec<&str> = offered.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(
            names,
            ["transform"],
            "the suffix spelling matches the carrier's component"
        );
        assert!(
            registry.offered_component_tools(plain, &world).is_empty(),
            "an entity that carries nothing is offered nothing"
        );
    }

    /// The name match reads a component's registered name in full and by its
    /// `::` suffix, and finds nothing for an entity that does not carry it.
    #[test]
    fn a_component_name_matches_in_full_and_by_its_suffix() {
        let (world, carrier, plain) = component_tool_world();
        let registered = world
            .entity_component_names(carrier)
            .expect("the carrier is alive");
        assert_eq!(registered.len(), 1, "one component, registered once");
        let full_name = registered[0].as_str();
        assert!(
            has_component_named(&world, carrier, full_name),
            "{full_name} is the carrier's own name"
        );
        let suffix = full_name.rsplit("::").next().unwrap_or(full_name);
        assert!(
            has_component_named(&world, carrier, suffix),
            "{suffix} matches by suffix"
        );
        assert!(!has_component_named(&world, carrier, "Missing"));
        assert!(!has_component_named(&world, plain, suffix));
    }

    /// A global tool and a tool that names no component are never offered:
    /// the built-ins need no row of their own and a fixture stays out of the
    /// toolbox.
    #[test]
    fn globals_and_component_tools_without_a_component_are_never_offered() {
        let (world, carrier, _) = component_tool_world();
        let registry = ToolRegistry::default();
        assert!(
            registry.offered_component_tools(carrier, &world).is_empty(),
            "the three built-ins are global"
        );
        let mut registry = ToolRegistry::default();
        registry.register(&ProbeTool);
        assert!(
            registry.offered_component_tools(carrier, &world).is_empty(),
            "a non-global tool that names no component is offered nowhere"
        );
    }

    /// The record keeps the declaration through a re-registration, so a
    /// reloaded image's row keeps appearing for the same entity.
    #[test]
    fn a_registered_tool_keeps_its_component_declaration() {
        let record = RegisteredTool::of(&TransformTool);
        assert_eq!(
            record.requires_component.as_deref(),
            Some("TransformComponent")
        );
        let bare = RegisteredTool::of(&ProbeTool);
        assert_eq!(bare.requires_component, None);
    }

    #[test]
    fn registering_a_name_twice_replaces_the_record_in_place() {
        let mut registry = ToolRegistry::default();
        registry.register(&ProbeTool);
        registry.register(&ReloadedProbeTool);
        let probes: Vec<&RegisteredTool> = registry
            .tools()
            .iter()
            .filter(|tool| tool.name == "probe")
            .collect();
        assert_eq!(probes.len(), 1, "a reload must not duplicate a row");
        assert_eq!(probes[0].label, "Probe reloaded");
        assert!(!probes[0].global, "a component tool is not global");
    }

    #[test]
    fn activating_a_tool_refuses_a_name_nobody_registered() {
        let mut registry = ToolRegistry::default();
        registry.set_active(Some("rotate")).expect("rotate exists");
        assert_eq!(registry.active(), Some("rotate"));
        registry.set_active(None).expect("clearing always works");
        assert_eq!(registry.active(), None);

        let refusal = registry
            .set_active(Some("nope"))
            .expect_err("an unregistered name is refused");
        assert!(
            refusal.contains("translate"),
            "the refusal names the tools that exist: {refusal}"
        );
        assert_eq!(
            registry.active(),
            None,
            "a refused activation changes nothing"
        );
    }

    /// A tool that reports one kind and one element, as an extension's editor
    /// crate declares them: plain functions, so a reload swaps them.
    struct ElementProbeTool;

    impl EditorTool for ElementProbeTool {
        fn name(&self) -> &'static str {
            "elements_probe"
        }

        fn label(&self) -> &'static str {
            "Elements probe"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    fn probe_kinds(out: &mut Vec<ElementKind>) {
        let mut kind = ElementKind::new("probe point");
        kind.multi_select = false;
        out.push(kind);
    }

    fn probe_elements(_entity: Entity, _world: &World, out: &mut Vec<ToolElement>) {
        out.push(ToolElement {
            id: 7,
            kind: "probe point".to_string(),
            transform: ElementTransform::at([1.0, 2.0, 3.0]),
        });
    }

    fn element_probe_hooks() -> ToolElementHooks {
        ToolElementHooks {
            kinds: Some(probe_kinds),
            elements: Some(probe_elements),
            ..ToolElementHooks::default()
        }
    }

    /// The reporting half belongs to the active tool: it answers nothing
    /// without one, its kinds are found by name, and re-registering the tool
    /// without hooks drops them.
    #[test]
    fn a_plugin_tool_reports_elements_only_while_active() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let mut registry = ToolRegistry::default();
        registry.register_plugin(&ElementProbeTool, element_probe_hooks());
        assert!(
            registry.active_kinds(entity, &world).is_empty(),
            "no active tool reports nothing"
        );
        assert!(registry.active_elements(entity, &world).is_empty());

        registry
            .set_active(Some("elements_probe"))
            .expect("the probe is registered");
        let kinds = registry.active_kinds(entity, &world);
        assert_eq!(kinds.len(), 1);
        assert_eq!(kinds[0].name, "probe point");
        assert!(!kinds[0].multi_select);
        assert_eq!(
            registry
                .active_kind("probe point")
                .map(|kind| kind.size_pixels),
            Some(9.0)
        );
        assert!(registry.active_kind("missing").is_none());
        let elements = registry.active_elements(entity, &world);
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].id, 7);
        assert_eq!(elements[0].transform.translation, [1.0, 2.0, 3.0]);

        registry.register(&ElementProbeTool);
        assert!(
            registry.active_kinds(entity, &world).is_empty(),
            "a record registered without hooks reports no elements"
        );
    }

    /// The two kinds a frame's test elements belong to.
    fn frame_kinds() -> Vec<ElementKind> {
        let mut point = ElementKind::new("point");
        point.color = [0.2, 0.4, 0.8, 1.0];
        let mut tangent = ElementKind::new("tangent");
        tangent.color = [0.2, 0.8, 0.4, 1.0];
        tangent.box_select = false;
        vec![point, tangent]
    }

    /// Three points on a line and a tangent above them, plus an element of a
    /// kind no kind describes, as a tool reports them for one entity.
    fn frame_report() -> Vec<ToolElement> {
        let point = |id: u64, x: f32| ToolElement {
            id,
            kind: "point".to_string(),
            transform: ElementTransform::at([x, 0.0, 0.0]),
        };
        vec![
            point(0, 0.0),
            point(1, 1.0),
            point(2, 2.0),
            ToolElement {
                id: 10,
                kind: "tangent".to_string(),
                transform: ElementTransform::at([0.0, 1.0, 0.0]),
            },
            ToolElement {
                id: 11,
                kind: "unknown".to_string(),
                transform: ElementTransform::at([0.0, 0.0, 1.0]),
            },
        ]
    }

    /// A frame's handles carry the kind's look, the selection's colour and the
    /// hover style, and every one sits in the element band.
    #[test]
    fn element_handles_style_the_selection_and_the_hover() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let kinds = frame_kinds();
        let report = frame_report();
        let mut selected = ElementSelection::default();
        selected.set("point".to_string(), vec![ElementRef { entity, id: 1 }]);

        // A fresh build with nothing hovered: the second point is the only one
        // drawn as selected, and the rest keep their kind's colour.
        let (idle, map) =
            element_handles(&report, &kinds, entity, &selected, HandleStates::default());
        assert_eq!(idle.len(), report.len());
        assert_eq!(idle[1].color, SELECTED_ELEMENT_COLOR);
        assert!(idle[1].selected);
        assert!(!idle[0].selected);
        assert_eq!(idle[0].color, kinds[0].color);
        assert_eq!(idle[3].color, kinds[1].color);
        // A kind the tool does not describe still draws, with the defaults.
        assert_eq!(idle[4].color, ElementKind::new("unknown").color);
        assert_eq!(idle[4].shape, GizmoPointShape::Circle);

        // Every handle is in the element band, and the map answers what the
        // handle picks.
        for handle in &idle {
            assert!(
                crate::gizmo::element_handle_slot(handle.handle).is_some(),
                "an element handle never reads as a gizmo handle"
            );
        }
        let target = map.target(idle[3].handle).expect("the tangent is mapped");
        assert_eq!(target.entity, entity);
        assert_eq!(target.element, 10);
        assert_eq!(target.kind, "tangent");

        // Hovering the first point brightens it and leaves the selected point
        // alone.
        let states = HandleStates {
            hovered: Some(idle[0].handle),
            active: None,
        };
        let (hovered, _) = element_handles(&report, &kinds, entity, &selected, states);
        assert_ne!(hovered[0].color, idle[0].color);
        assert!(hovered[0].color[0] > idle[0].color[0]);
        assert_eq!(hovered[1].color, SELECTED_ELEMENT_COLOR);
    }

    /// The hit test is the pick quad's square: inside it a press hits, past it
    /// the press misses, however far the pointer is along one axis.
    #[test]
    fn an_element_handle_covers_its_pick_square() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let (handles, _) = element_handles(
            &frame_report(),
            &frame_kinds(),
            entity,
            &ElementSelection::default(),
            HandleStates::default(),
        );
        let point = &handles[0];
        let reach = point.size_pixels * 0.5 + ELEMENT_PICK_SLACK_PIXELS;
        assert!(point.covers_pixel(100.0 + reach, 100.0, (100.0, 100.0)));
        assert!(point.covers_pixel(100.0, 100.0 - reach, (100.0, 100.0)));
        assert!(!point.covers_pixel(100.0 + reach + 0.5, 100.0, (100.0, 100.0)));
        assert!(!point.covers_pixel(100.0, 100.0 + reach + 0.5, (100.0, 100.0)));
    }

    /// A marquee takes the first kind it covers, in the tool's own order, and
    /// leaves every other kind alone even when the rectangle covers it too.
    #[test]
    fn a_marquee_takes_one_kind_in_report_order() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let element = |id: u64, kind: &str, x: f32, y: f32| ProjectedElement {
            element: ElementRef { entity, id },
            kind: kind.to_string(),
            x,
            y,
        };
        let projected = vec![
            element(0, "point", 10.0, 10.0),
            element(1, "point", 20.0, 12.0),
            element(2, "point", 30.0, 14.0),
            element(10, "tangent", 15.0, 11.0),
        ];
        let kinds = frame_kinds();
        let over_the_points = marquee_selection(
            &projected,
            MarqueeRect::from_corners((5.0, 5.0), (35.0, 20.0)),
            &kinds,
        );
        assert_eq!(over_the_points.kind(), "point");
        assert_eq!(over_the_points.elements().len(), 3);
        assert_eq!(
            over_the_points.elements()[2],
            ElementRef { entity, id: 2 },
            "the report's order is the selection's order"
        );

        // A rectangle over the tangent only finds nothing: the tangent's kind
        // does not allow box select, so the marquee never takes it.
        let over_the_tangent = marquee_selection(
            &projected,
            MarqueeRect::from_corners((14.0, 10.0), (16.0, 12.0)),
            &kinds,
        );
        assert!(
            over_the_tangent.is_empty(),
            "a kind without box select is never marquee-selected"
        );

        // A rectangle over nothing is an empty selection, which the caller
        // records as a clear.
        let over_nothing = marquee_selection(
            &projected,
            MarqueeRect::from_corners((90.0, 90.0), (99.0, 99.0)),
            &kinds,
        );
        assert!(over_nothing.is_empty());
    }

    /// A rectangle built from corners in any direction keeps its edges ordered.
    #[test]
    fn marquee_rects_order_their_corners() {
        let rect = MarqueeRect::from_corners((40.0, 60.0), (10.0, 20.0));
        assert_eq!(rect.min_x, 10.0);
        assert_eq!(rect.min_y, 20.0);
        assert_eq!(rect.max_x, 40.0);
        assert_eq!(rect.max_y, 60.0);
        assert!(rect.contains(10.0, 20.0), "the edges are included");
        assert!(!rect.contains(9.0, 20.0));
    }

    /// The moves a probe tool's write-back was handed, so a test can read them
    /// back out of the world.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct ProbeWrites(Vec<ElementMove>);

    impl pill_engine::Resource for ProbeWrites {}

    /// A tool that takes element moves and answers with one frame of its own.
    struct WritingProbeTool;

    impl EditorTool for WritingProbeTool {
        fn name(&self) -> &'static str {
            "writing_probe"
        }

        fn label(&self) -> &'static str {
            "Writing probe"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    fn probe_set_transforms(_entity: Entity, moves: &[ElementMove], world: &mut World) {
        if world.get_resource::<ProbeWrites>().is_none() {
            world.insert_resource(ProbeWrites::default());
        }
        if let Some(writes) = world.get_resource_mut::<ProbeWrites>() {
            writes.0.extend_from_slice(moves);
        }
    }

    fn probe_frames(
        _entity: Entity,
        _selected: &[u64],
        _world: &World,
        out: &mut Vec<CoordinateFrame>,
    ) {
        out.push(CoordinateFrame {
            name: "probe frame".to_string(),
            rotation: [0.0, 0.0, 0.0, 1.0],
        });
    }

    /// The write-back and the frames report belong to the active tool: with no
    /// tool active nothing takes a move and nothing reports a frame.
    #[test]
    fn the_active_tool_takes_moves_and_reports_frames() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let mut registry = ToolRegistry::default();
        registry.register_plugin(
            &WritingProbeTool,
            ToolElementHooks {
                set_transforms: Some(probe_set_transforms),
                frames: Some(probe_frames),
                ..ToolElementHooks::default()
            },
        );
        let moves = vec![ElementMove {
            element: 3,
            transform: ElementTransform::at([4.0, 5.0, 6.0]),
        }];
        assert!(
            !registry.active_set_transforms(entity, &moves, &mut world),
            "no active tool takes a move"
        );
        assert!(registry.active_frames(entity, &[3], &world).is_empty());

        registry
            .set_active(Some("writing_probe"))
            .expect("the probe is registered");
        assert!(registry.active_set_transforms(entity, &moves, &mut world));
        let writes = world
            .get_resource::<ProbeWrites>()
            .expect("the probe took the moves")
            .clone();
        assert_eq!(writes.0, moves);
        let frames = registry.active_frames(entity, &[3], &world);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].name, "probe frame");
    }

    /// A space choice belongs to the tool it was made under: another tool's
    /// choice leaves it alone, and choosing again replaces it.
    #[test]
    fn space_choices_remember_each_tool() {
        let mut choices = ToolSpaceChoices::default();
        assert!(choices.choice("fixture").is_none());
        choices.remember("fixture", GizmoSpace::Local, Some(1));
        choices.remember("other", GizmoSpace::Local, None);
        assert_eq!(
            choices.choice("fixture"),
            Some(SpaceChoice {
                space: GizmoSpace::Local,
                frame: Some(1),
            })
        );
        assert_eq!(
            choices.choice("other"),
            Some(SpaceChoice {
                space: GizmoSpace::Local,
                frame: None,
            })
        );
        choices.remember("fixture", GizmoSpace::World, None);
        assert_eq!(
            choices.choice("fixture"),
            Some(SpaceChoice {
                space: GizmoSpace::World,
                frame: None,
            })
        );
    }

    /// What a probe tool's overrides and edits did, so a test can read them
    /// back out of the world.
    #[derive(Clone, Debug, Default, PartialEq)]
    struct ProbeTrace {
        /// The presses the tool answered, as `(element, extended)`.
        presses: Vec<(u64, bool)>,
        /// The keys it answered.
        keys: Vec<String>,
        /// The selection changes it heard, as `(before, after)` counts.
        changes: Vec<(usize, usize)>,
    }

    impl pill_engine::Resource for ProbeTrace {}

    /// A tool that overrides presses, takes keys, hears selection changes and
    /// edits its element set.
    struct OverridingProbeTool;

    impl EditorTool for OverridingProbeTool {
        fn name(&self) -> &'static str {
            "overriding_probe"
        }

        fn label(&self) -> &'static str {
            "Overriding probe"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    /// Fills the menu with one row per kind, so the test reads the kind and
    /// the selection count back out of the row.
    fn probe_menu(
        _entity: Entity,
        kind: &str,
        selected: &[u64],
        _world: &World,
        out: &mut Vec<ElementMenuEntry>,
    ) {
        out.push(ElementMenuEntry {
            id: format!("{kind} row"),
            label: format!("{kind} row for {}", selected.len()),
        });
    }

    fn probe_menu_run(
        _entity: Entity,
        kind: &str,
        _selected: &[u64],
        item: &str,
        _world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        if item == "refuse" {
            return Err(format!("{kind} refuses {item}"));
        }
        Ok(Vec::new())
    }

    fn probe_insert(
        _entity: Entity,
        _kind: &str,
        id: Option<u64>,
        _transform: ElementTransform,
        _world: &mut World,
    ) -> Option<u64> {
        // An id the caller asks for is honoured, which is what a restored
        // element needs; otherwise the tool chooses.
        Some(id.unwrap_or(99))
    }

    fn probe_remove(_entity: Entity, _kind: &str, elements: &[u64], _world: &mut World) -> usize {
        elements.len()
    }

    fn probe_press(
        _entity: Entity,
        element: u64,
        click: SelectionClick,
        _selected: &[u64],
        world: &World,
    ) -> PressResponse {
        // The probe answers from what it can read, so the test proves the
        // world reached the hook.
        if world.get_resource::<ProbeTrace>().is_some() {
            return PressResponse::Default;
        }
        if click == SelectionClick::Add {
            PressResponse::Consume
        } else {
            PressResponse::Select(vec![element])
        }
    }

    fn probe_key(_entity: Entity, key: &str, _selected: &[u64], _world: &World) -> KeyResponse {
        match key {
            "k" => KeyResponse::Consume,
            "i" => KeyResponse::Commands(Vec::new()),
            _ => KeyResponse::Default,
        }
    }

    fn probe_selection_changed(_entity: Entity, previous: &[u64], current: &[u64], world: &World) {
        let _ = (previous, current, world);
    }

    fn overriding_probe_hooks() -> ToolElementHooks {
        ToolElementHooks {
            menu: Some(probe_menu),
            menu_run: Some(probe_menu_run),
            insert: Some(probe_insert),
            remove: Some(probe_remove),
            press: Some(probe_press),
            key: Some(probe_key),
            selection_changed: Some(probe_selection_changed),
            ..ToolElementHooks::default()
        }
    }

    /// Every override the registry offers answers the active tool's, and a
    /// registry with no tool active answers the editor's own defaults instead.
    #[test]
    fn the_active_tools_overrides_answer_and_leave_the_defaults_without_one() {
        let world = World::default();
        let entity = Entity::default();
        let mut registry = ToolRegistry::default();
        registry.register_plugin(&OverridingProbeTool, overriding_probe_hooks());

        // No tool: the editor's own rules win everywhere.
        assert!(registry
            .active_menu(entity, "point", &[1], &world)
            .is_empty());
        assert!(registry
            .active_menu_run(entity, "point", &[1], "point row", &world)
            .is_err());
        assert_eq!(
            registry.active_press(entity, 1, "point", SelectionClick::Reset, &[1], &world),
            PressResponse::Default
        );
        assert_eq!(
            registry.active_key(entity, "point", "k", &[1], &world),
            KeyResponse::Default
        );

        registry
            .set_active(Some("overriding_probe"))
            .expect("the probe is registered");
        let menu = registry.active_menu(entity, "point", &[1, 2], &world);
        assert_eq!(menu.len(), 1);
        assert_eq!(menu[0].id, "point row");
        assert_eq!(menu[0].label, "point row for 2");
        assert!(registry
            .active_menu_run(entity, "point", &[1], "point row", &world)
            .is_ok());
        assert!(registry
            .active_menu_run(entity, "point", &[1], "refuse", &world)
            .is_err());
        assert_eq!(
            registry.active_press(entity, 3, "point", SelectionClick::Reset, &[], &world),
            PressResponse::Select(vec![3])
        );
        assert_eq!(
            registry.active_press(entity, 3, "point", SelectionClick::Add, &[], &world),
            PressResponse::Consume
        );
        assert_eq!(
            registry.active_key(entity, "point", "k", &[], &world),
            KeyResponse::Consume
        );
        assert_eq!(
            registry.active_key(entity, "point", "i", &[], &world),
            KeyResponse::Commands(Vec::new())
        );
        assert_eq!(
            registry.active_key(entity, "point", "q", &[], &world),
            KeyResponse::Default
        );
    }

    /// The edit hooks belong to the active tool too: inserting honours the id
    /// a caller asks for and removing reports how many went.
    #[test]
    fn the_active_tools_edit_hooks_insert_and_remove() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        let mut registry = ToolRegistry::default();
        assert!(registry
            .active_insert(
                entity,
                "point",
                Some(4),
                ElementTransform::at([0.0; 3]),
                &mut world
            )
            .is_none());
        assert!(registry
            .active_remove(entity, "point", &[4], &mut world)
            .is_none());

        registry.register_plugin(&OverridingProbeTool, overriding_probe_hooks());
        registry
            .set_active(Some("overriding_probe"))
            .expect("the probe is registered");
        assert_eq!(
            registry.active_insert(
                entity,
                "point",
                Some(4),
                ElementTransform::at([0.0; 3]),
                &mut world
            ),
            Some(4),
            "an asked-for id is honoured"
        );
        assert_eq!(
            registry.active_insert(
                entity,
                "point",
                None,
                ElementTransform::at([0.0; 3]),
                &mut world
            ),
            Some(99),
            "a free id is the tool's to choose"
        );
        assert_eq!(
            registry.active_remove(entity, "point", &[4, 5], &mut world),
            Some(2)
        );
        // The trace default is untouched by the edits: they are the tool's own
        // writes, not the press path.
        assert!(world.get_resource::<ProbeTrace>().is_none());
    }

    /// The subject a tool is told about is the element selection's own entity,
    /// kind and ids; with nothing selected in the slot it is the primary
    /// entity.
    #[test]
    fn the_element_subject_names_the_owner_kind_and_ids() {
        let mut world = World::default();
        let entity = world.create_entity().build().expect("entity builds");
        assert!(element_selection_subject(&world).is_none());

        let mut selection = EditorSelection::default();
        selection.set_entities(vec![entity]);
        world.insert_resource(selection);
        let (owner, kind, ids) =
            element_selection_subject(&world).expect("the primary entity is a subject");
        assert_eq!(owner, entity);
        assert_eq!(kind, "");
        assert!(ids.is_empty());

        let mut selection = EditorSelection::default();
        selection.set_entities(vec![entity]);
        assert!(selection.set_elements(
            "point".to_string(),
            vec![ElementRef { entity, id: 2 }, ElementRef { entity, id: 5 },]
        ));
        world.insert_resource(selection);
        let (owner, kind, ids) = element_selection_subject(&world).expect("the slot is a subject");
        assert_eq!(owner, entity);
        assert_eq!(kind, "point");
        assert_eq!(ids, vec![2, 5], "ids keep the selection's own order");
    }

    // -------------------------------------------------------------------------
    // Extensions
    // -------------------------------------------------------------------------

    /// The kind the probe tool reports and the extensions build on.
    const EXTENSION_KIND: &str = "probe point";

    /// The kind the extensions add.
    const EXTENSION_ADDED_KIND: &str = "probe mark";

    /// The row the probe tool itself adds.
    const BASE_ROW: &str = "base";

    /// The chord both key extensions bind.
    const SHARED_CHORD: &str = "n";

    /// The probe tool's kinds: one kind, named [`EXTENSION_KIND`].
    fn extension_base_kinds(out: &mut Vec<ElementKind>) {
        out.push(ElementKind::new(EXTENSION_KIND));
    }

    /// The probe tool's menu: one row, [`BASE_ROW`].
    fn extension_base_menu(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        _world: &World,
        out: &mut Vec<ElementMenuEntry>,
    ) {
        out.push(ElementMenuEntry {
            id: BASE_ROW.to_string(),
            label: "Base row".to_string(),
        });
    }

    /// The probe tool's menu run: it knows [`BASE_ROW`] and nothing else.
    fn extension_base_menu_run(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        item: &str,
        _world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        if item == BASE_ROW {
            return Ok(Vec::new());
        }
        Err(format!("the probe tool adds no '{item}' row"))
    }

    /// The probe tool's press rule: a press on element 1 is taken, every other
    /// press is the editor's.
    fn extension_base_press(
        _entity: Entity,
        element: u64,
        _click: SelectionClick,
        _selected: &[u64],
        _world: &World,
    ) -> PressResponse {
        if element == 1 {
            return PressResponse::Consume;
        }
        PressResponse::Default
    }

    /// The probe tool's key rule: it hears no chord.
    fn extension_base_key(
        _entity: Entity,
        _key: &str,
        _selected: &[u64],
        _world: &World,
    ) -> KeyResponse {
        KeyResponse::Default
    }

    /// The tool every extension below extends.
    struct ExtensionBaseTool;

    impl EditorTool for ExtensionBaseTool {
        fn name(&self) -> &'static str {
            "probe"
        }

        fn label(&self) -> &'static str {
            "Probe"
        }

        fn is_global(&self) -> bool {
            false
        }
    }

    /// The first extension: attached only to an entity carrying a transform,
    /// only for the base kind, first in the chain.
    struct FirstExtension;

    impl ToolExtension for FirstExtension {
        fn name() -> &'static str {
            "first extension"
        }

        fn extends_tool() -> &'static str {
            "probe"
        }

        fn for_kinds() -> &'static [&'static str] {
            &[EXTENSION_KIND]
        }

        fn requires_component() -> Option<&'static str> {
            Some("TransformComponent")
        }

        fn order() -> i32 {
            10
        }

        fn keys() -> &'static [&'static str] {
            &[SHARED_CHORD]
        }

        fn hooks() -> ToolExtensionHooks {
            ToolExtensionHooks {
                kinds: Some(extension_added_kinds),
                elements: Some(extension_elements),
                menu: Some(first_menu),
                menu_run: Some(first_menu_run),
                press: Some(first_press),
                key: Some(first_key),
                ..ToolExtensionHooks::default()
            }
        }
    }

    /// The second extension: attached to every entity, second in the chain, and
    /// binding the same chord as the first.
    struct SecondExtension;

    impl ToolExtension for SecondExtension {
        fn name() -> &'static str {
            "second extension"
        }

        fn extends_tool() -> &'static str {
            "probe"
        }

        fn order() -> i32 {
            20
        }

        fn keys() -> &'static [&'static str] {
            &[SHARED_CHORD]
        }

        fn hooks() -> ToolExtensionHooks {
            ToolExtensionHooks {
                menu: Some(second_menu),
                menu_run: Some(second_menu_run),
                key: Some(second_key),
                ..ToolExtensionHooks::default()
            }
        }
    }

    /// A third extension, last in the chain, that re-declares the base's row id
    /// and re-declares the kind the first one added.
    struct LateExtension;

    impl ToolExtension for LateExtension {
        fn name() -> &'static str {
            "late extension"
        }

        fn extends_tool() -> &'static str {
            "probe"
        }

        fn order() -> i32 {
            30
        }

        fn hooks() -> ToolExtensionHooks {
            ToolExtensionHooks {
                kinds: Some(extension_duplicate_kinds),
                menu: Some(late_menu),
                menu_run: Some(late_menu_run),
                ..ToolExtensionHooks::default()
            }
        }
    }

    /// The kind one extension adds.
    fn extension_added_kinds(out: &mut Vec<ElementKind>) {
        out.push(ElementKind::new(EXTENSION_ADDED_KIND));
    }

    /// The one element the first extension reports, of the kind it adds.
    fn extension_elements(entity: Entity, world: &World, out: &mut Vec<ToolElement>) {
        let translation = world
            .get_component::<pill_engine::TransformComponent>(entity)
            .map_or([0.0; 3], |transform| transform.translation);
        out.push(ToolElement {
            id: 40,
            kind: EXTENSION_ADDED_KIND.to_string(),
            transform: ElementTransform::at(translation),
        });
    }

    /// The same kind name again, with a colour nothing else uses: a reader can
    /// tell which declaration survived by the colour.
    fn extension_duplicate_kinds(out: &mut Vec<ElementKind>) {
        let mut duplicate = ElementKind::new(EXTENSION_ADDED_KIND);
        duplicate.color = [0.0, 1.0, 0.0, 1.0];
        out.push(duplicate);
    }

    /// The first extension's row.
    fn first_menu(
        _entity: Entity,
        kind: &str,
        _selected: &[u64],
        _world: &World,
        out: &mut Vec<ElementMenuEntry>,
    ) {
        if kind != EXTENSION_KIND {
            return;
        }
        out.push(ElementMenuEntry {
            id: "first".to_string(),
            label: "First".to_string(),
        });
    }

    /// Runs the first extension's row.
    fn first_menu_run(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        item: &str,
        _world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        if item == "first" {
            return Ok(Vec::new());
        }
        Err(format!("the first extension adds no '{item}' row"))
    }

    /// The second extension's row, offered for every kind.
    fn second_menu(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        _world: &World,
        out: &mut Vec<ElementMenuEntry>,
    ) {
        out.push(ElementMenuEntry {
            id: "second".to_string(),
            label: "Second".to_string(),
        });
    }

    /// Runs the second extension's row.
    fn second_menu_run(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        item: &str,
        _world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        if item == "second" {
            return Ok(Vec::new());
        }
        Err(format!("the second extension adds no '{item}' row"))
    }

    /// The late extension's rows: one of its own, and one under the base's id,
    /// which must not replace the base's.
    fn late_menu(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        _world: &World,
        out: &mut Vec<ElementMenuEntry>,
    ) {
        out.push(ElementMenuEntry {
            id: "late".to_string(),
            label: "Late".to_string(),
        });
        out.push(ElementMenuEntry {
            id: BASE_ROW.to_string(),
            label: "Base row, replaced".to_string(),
        });
    }

    /// Runs the late extension's row.
    fn late_menu_run(
        _entity: Entity,
        _kind: &str,
        _selected: &[u64],
        item: &str,
        _world: &World,
    ) -> Result<Vec<EditorCommand>, String> {
        if item == "late" {
            return Ok(Vec::new());
        }
        Err(format!("the late extension adds no '{item}' row"))
    }

    /// The first extension's press: it selects element 7.
    fn first_press(
        _entity: Entity,
        element: u64,
        _click: SelectionClick,
        _selected: &[u64],
        _world: &World,
    ) -> PressResponse {
        if element == 7 {
            return PressResponse::Select(vec![7]);
        }
        PressResponse::Default
    }

    /// The first extension's key: it answers the shared chord with a command,
    /// which is what tells its answer from the second extension's.
    fn first_key(_entity: Entity, _key: &str, _selected: &[u64], _world: &World) -> KeyResponse {
        KeyResponse::Commands(Vec::new())
    }

    /// The second extension's key: it consumes the shared chord.
    fn second_key(_entity: Entity, _key: &str, _selected: &[u64], _world: &World) -> KeyResponse {
        KeyResponse::Consume
    }

    /// A world with a probe tool and the three extensions: one entity carries a
    /// transform, the other carries nothing.
    fn extension_world() -> (World, Entity, Entity) {
        let mut world = World::default();
        world.register_component::<pill_engine::TransformComponent>();
        let carrier = world
            .create_entity()
            .with(pill_engine::TransformComponent::default())
            .build()
            .expect("the carrier builds");
        let plain = world
            .create_entity()
            .build()
            .expect("the plain entity builds");
        let mut registry = ToolRegistry::default();
        registry.register_plugin(
            &ExtensionBaseTool,
            ToolElementHooks {
                kinds: Some(extension_base_kinds),
                menu: Some(extension_base_menu),
                menu_run: Some(extension_base_menu_run),
                press: Some(extension_base_press),
                key: Some(extension_base_key),
                ..ToolElementHooks::default()
            },
        );
        registry.register_extension::<FirstExtension>();
        registry.register_extension::<SecondExtension>();
        registry.register_extension::<LateExtension>();
        registry
            .set_active(Some("probe"))
            .expect("the probe exists");
        world.insert_resource(registry);
        (world, carrier, plain)
    }

    /// An extension is attached only while its base tool is active and the
    /// entity carries the component it requires.
    #[test]
    fn an_extension_is_attached_by_its_base_and_the_entitys_component() {
        let (mut world, carrier, plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        let attached: Vec<&str> = registry
            .attached_extensions(carrier, &world)
            .iter()
            .map(|extension| extension.name.as_str())
            .collect();
        assert_eq!(
            attached,
            ["first extension", "second extension", "late extension"],
            "every extension of the base, in order"
        );
        let on_plain: Vec<&str> = registry
            .attached_extensions(plain, &world)
            .iter()
            .map(|extension| extension.name.as_str())
            .collect();
        assert_eq!(
            on_plain,
            ["second extension", "late extension"],
            "the one that requires a component is not attached without it"
        );

        let registry = world
            .get_resource_mut::<ToolRegistry>()
            .expect("registered");
        registry.set_active(None).expect("clearing works");
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        assert!(
            registry.attached_extensions(carrier, &world).is_empty(),
            "no active tool means no extension"
        );
    }

    /// Kinds and elements are additive: the base's come first, an extension's
    /// follow, and a kind name declared twice keeps the first declaration.
    #[test]
    fn extension_kinds_are_added_after_the_base_tools() {
        let (world, carrier, plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        let kinds = registry.active_kinds(carrier, &world);
        let names: Vec<&str> = kinds.iter().map(|kind| kind.name.as_str()).collect();
        assert_eq!(
            names,
            [EXTENSION_KIND, EXTENSION_ADDED_KIND],
            "the base kind, then the kind an extension added"
        );
        assert_eq!(
            kinds[1].color,
            ElementKind::new(EXTENSION_ADDED_KIND).color,
            "the first declaration of a kind name is the one that survives"
        );
        // On the entity without the component the first extension is not
        // attached, so the late one's declaration of the same kind is the first
        // one in, and its colour is the one a reader sees.
        let plain_kinds = registry.active_kinds(plain, &world);
        assert_eq!(
            plain_kinds
                .iter()
                .map(|kind| kind.name.as_str())
                .collect::<Vec<&str>>(),
            [EXTENSION_KIND, EXTENSION_ADDED_KIND],
            "a kind an attached extension adds is still added"
        );
        assert_eq!(plain_kinds[1].color, [0.0, 1.0, 0.0, 1.0]);
        assert!(
            registry.active_reports_elements(carrier, &world),
            "an extension's kinds and elements count as a report"
        );
        assert_eq!(
            registry
                .active_elements(carrier, &world)
                .iter()
                .map(|element| element.id)
                .collect::<Vec<u64>>(),
            [40],
            "the base reports no elements, so the extension's are all there are"
        );
    }

    /// The menu keeps every base row, adds the extensions' rows after it in
    /// `order`, and never lets an extension take a base row's id.
    #[test]
    fn the_merged_menu_keeps_the_base_rows_and_orders_the_extensions() {
        let (world, carrier, plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        let rows = registry.active_menu(carrier, EXTENSION_KIND, &[], &world);
        let ids: Vec<&str> = rows.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(
            ids,
            [BASE_ROW, "first", "second", "late"],
            "the base's row first, then each extension's in order"
        );
        let base_label = rows
            .iter()
            .find(|entry| entry.id == BASE_ROW)
            .map(|entry| entry.label.as_str());
        assert_eq!(
            base_label,
            Some("Base row"),
            "the late extension's row under the base id is skipped"
        );

        let on_plain: Vec<String> = registry
            .active_menu(plain, EXTENSION_KIND, &[], &world)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            on_plain,
            [BASE_ROW, "second", "late"],
            "an extension attached by a component adds nothing to an entity without it"
        );

        let other_kind: Vec<String> = registry
            .active_menu(carrier, "another kind", &[], &world)
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(
            other_kind,
            [BASE_ROW, "second", "late"],
            "a `for_kinds` extension stays out of the kinds it does not name"
        );
    }

    /// A row an extension added runs through the extension, and the base still
    /// answers its own.
    #[test]
    fn the_menu_run_reaches_an_extensions_row() {
        let (world, carrier, _plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        assert!(registry
            .active_menu_run(carrier, EXTENSION_KIND, &[], BASE_ROW, &world)
            .is_ok());
        assert!(registry
            .active_menu_run(carrier, EXTENSION_KIND, &[], "first", &world)
            .is_ok());
        assert!(registry
            .active_menu_run(carrier, EXTENSION_KIND, &[], "late", &world)
            .is_ok());
        let refusal = registry
            .active_menu_run(carrier, EXTENSION_KIND, &[], "nobody", &world)
            .expect_err("a row nobody adds is refused");
        assert!(
            refusal.contains("the probe tool adds no"),
            "the refusal is the base tool's, which names what it adds: {refusal}"
        );
    }

    /// The press chain: the base answers first, and an extension only while the
    /// answer so far is the editor's own rule.
    #[test]
    fn a_press_chains_from_the_base_to_the_extensions() {
        let (world, carrier, _plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        assert_eq!(
            registry.active_press(
                carrier,
                7,
                EXTENSION_KIND,
                SelectionClick::Reset,
                &[],
                &world
            ),
            PressResponse::Select(vec![7]),
            "the base let it through, so the extension's answer wins"
        );
        assert_eq!(
            registry.active_press(
                carrier,
                1,
                EXTENSION_KIND,
                SelectionClick::Reset,
                &[],
                &world
            ),
            PressResponse::Consume,
            "a press the base claims never reaches an extension"
        );
    }

    /// A chord two extensions bind is answered by the lower `order`, and an
    /// extension hears only the chords it declares.
    #[test]
    fn a_key_bound_twice_is_answered_by_the_lower_order() {
        let (world, carrier, _plain) = extension_world();
        let registry = world.get_resource::<ToolRegistry>().expect("registered");
        assert_eq!(
            registry.active_key(carrier, EXTENSION_KIND, SHARED_CHORD, &[], &world),
            KeyResponse::Commands(Vec::new()),
            "the first extension (order 10) answers, not the second (order 20)"
        );
        assert_eq!(
            registry.active_key(carrier, EXTENSION_KIND, "z", &[], &world),
            KeyResponse::Default,
            "a chord nobody declares falls through to the editor"
        );
        assert_eq!(
            registry.active_key(carrier, "another kind", SHARED_CHORD, &[], &world),
            KeyResponse::Consume,
            "the first extension's `for_kinds` keeps it out, so the second answers"
        );
    }

    /// Registering an extension twice replaces its record, which is what a
    /// reload does, and the chain stays sorted by `(order, name)`.
    #[test]
    fn re_registering_an_extension_keeps_one_record_in_order() {
        let mut registry = ToolRegistry::default();
        registry.register_extension::<LateExtension>();
        registry.register_extension::<FirstExtension>();
        registry.register_extension::<SecondExtension>();
        registry.register_extension::<FirstExtension>();
        let names: Vec<&str> = registry
            .extensions()
            .iter()
            .map(|extension| extension.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["first extension", "second extension", "late extension"]
        );
        registry.clear_extensions();
        assert!(registry.extensions().is_empty());
    }
}
