//! The editor's tool layer: the modes a scene view can be in.
//!
//! # Responsibilities
//!
//! - Declare [`EditorTool`], the value a tool is described by: the name a
//!   command and the scripted channel spell, the label the toolbox shows and
//!   its optional shortcut.
//! - Hold the registered tools and the active one in [`ToolRegistry`], and
//!   name the three built-ins the editor always offers.
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
//! crate registers its own through the same registry once a stage gives tools
//! something to do.
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
use std::fmt;

// External crates
use pill_engine::{Engine, Entity, GizmoPointShape, World};

// Current crate
use crate::editor_state::{ElementRef, ElementSelection};
use crate::gizmo::{styled_color, ElementHandleMap, ElementHandleTarget, HandleStates};

// =============================================================================
// Tools
// =============================================================================

/// The three modes the editor offers with no extension loaded.
///
/// They are global tools: offered whatever the selection holds, because they
/// act on whole entities. An extension's tool names its own component and is
/// offered only while that component is selected (a later stage).
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
            hooks,
        }
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
    pub shape: pill_engine::GizmoPointShape,
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
            shape: pill_engine::GizmoPointShape::Circle,
            size_pixels: 9.0,
            color: [0.92, 0.92, 0.92, 1.0],
            multi_select: true,
            box_select: true,
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
    pub position: [f32; 3],
}

/// Fills `out` with the kinds a tool exposes.
pub type ToolKindReportFn = fn(out: &mut Vec<ElementKind>);

/// Fills `out` with the elements `entity` has, as `world` describes it.
pub type ToolElementReportFn = fn(entity: Entity, world: &World, out: &mut Vec<ToolElement>);

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
}

impl ToolElementHooks {
    /// Whether the tool reports elements at all.
    pub fn reports_elements(self) -> bool {
        self.kinds.is_some() && self.elements.is_some()
    }
}

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
    /// Where it sits, in world space.
    pub position: [f32; 3],
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
            position: element.position,
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
// Registry
// =============================================================================

/// The name [`ToolRegistry`] registers under.
pub const RESOURCE_NAME: &str = "pill_editor_core::tools::ToolRegistry";

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
    /// The active tool's name, or `None` when no tool is active.
    active: Option<String>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        let mut registry = ToolRegistry {
            tools: Vec::new(),
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

    /// The kinds the active tool exposes, in its own order.
    ///
    /// Empty when no tool is active or the tool reports no elements.
    pub fn active_kinds(&self) -> Vec<ElementKind> {
        let mut kinds = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.kinds) {
            report(&mut kinds);
        }
        kinds
    }

    /// One of the active tool's kinds, by name.
    pub fn active_kind(&self, name: &str) -> Option<ElementKind> {
        self.active_kinds()
            .into_iter()
            .find(|kind| kind.name == name)
    }

    /// The elements the active tool reports for `entity`, in its own order.
    ///
    /// Empty when no tool is active or the tool reports no elements.
    pub fn active_elements(&self, entity: Entity, world: &World) -> Vec<ToolElement> {
        let mut elements = Vec::new();
        if let Some(report) = self.active_record().and_then(|tool| tool.hooks.elements) {
            report(entity, world, &mut elements);
        }
        elements
    }

    /// The registered tools, in registration order.
    pub fn tools(&self) -> &[RegisteredTool] {
        &self.tools
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
    /// logs.
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
        }
        Ok(())
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
            position: [1.0, 2.0, 3.0],
        });
    }

    fn element_probe_hooks() -> ToolElementHooks {
        ToolElementHooks {
            kinds: Some(probe_kinds),
            elements: Some(probe_elements),
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
            registry.active_kinds().is_empty(),
            "no active tool reports nothing"
        );
        assert!(registry.active_elements(entity, &world).is_empty());

        registry
            .set_active(Some("elements_probe"))
            .expect("the probe is registered");
        let kinds = registry.active_kinds();
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
        assert_eq!(elements[0].position, [1.0, 2.0, 3.0]);

        registry.register(&ElementProbeTool);
        assert!(
            registry.active_kinds().is_empty(),
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

    /// Three points on a line and one tangent above them, as a tool reports
    /// them for one entity.
    fn frame_report() -> Vec<ToolElement> {
        vec![
            ToolElement {
                id: 0,
                kind: "point".to_string(),
                position: [0.0, 0.0, 0.0],
            },
            ToolElement {
                id: 1,
                kind: "point".to_string(),
                position: [1.0, 0.0, 0.0],
            },
            ToolElement {
                id: 2,
                kind: "point".to_string(),
                position: [2.0, 0.0, 0.0],
            },
            ToolElement {
                id: 10,
                kind: "tangent".to_string(),
                position: [0.0, 1.0, 0.0],
            },
            ToolElement {
                id: 11,
                kind: "unknown".to_string(),
                position: [0.0, 0.0, 1.0],
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
}
