//! The editor's tool layer: the modes a scene view can be in.
//!
//! # Responsibilities
//!
//! - Declare [`EditorTool`], the value a tool is described by: the name a
//!   command and the scripted channel spell, the label the toolbox shows and
//!   its optional shortcut.
//! - Hold the registered tools and the active one in [`ToolRegistry`], and
//!   name the three built-ins the editor always offers.
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
use pill_engine::{Engine, Entity, World};

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
}
