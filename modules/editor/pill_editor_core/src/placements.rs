//! Where a module's commands appear, and what they look like there.
//!
//! # Responsibilities
//!
//! - Model a placement ([`UiSlot`]): the menu bar, the top toolbar, the
//!   viewport toolbox, one component's Inspector header, the three context
//!   menus, and the asset menu's extension filter.
//! - Model the state a module publishes for a command, and optionally for one
//!   target ([`CommandState`], [`CommandStates`]): whether the row shows, can
//!   act, is checked, and the label, tooltip and dots it draws.
//! - Hold the draw functions a module registered for a free-form place
//!   ([`CommandDraws`]) behind the same liveness markers the panels use.
//! - Give a draw function its context ([`CommandDrawContext`]): the command,
//!   its target, the state published for it, and the invocations it reports.
//!
//! # Design
//!
//! Every place draws a command from plain data by default: the title, the
//! icon, and the state published for the target. A module draw function is the
//! exception, for a place whose look no data describes (the Blender logo). The
//! draw function is a `fn` pointer into the registering image, so it is
//! registered through a request the executable binds to that module's liveness
//! marker and is handed out only while the marker is alive - the panel
//! contract, because an address from an unloaded image must never be called.
//!
//! Nothing here names a type from a registering crate: a placement carries an
//! owned string, a target carries the engine's entity handle, and a state an
//! owned label. That is what lets the values outlive the module that produced
//! them.

// Standard library
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};

// External crates
use pill_engine::Entity;

// Current crate
use crate::commands::CommandInvocation;
use crate::panels::ModuleLiveness;

// =============================================================================
// Placements
// =============================================================================

/// One place a command appears in.
///
/// The menu bar is derived from [`crate::commands::CommandSpec::menu_path`],
/// so a module writes its menu path once and the placement list cannot
/// disagree with it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UiSlot {
    /// A menu row, at this path. The registry adds it for every command with a
    /// menu path.
    MenuBar {
        /// The path a row is drawn at, top segment first.
        path: Vec<String>,
    },
    /// A button in the toolbar beside Play, Pause and Step.
    Toolbar,
    /// A row in the floating viewport toolbox, for the view it is drawn over.
    ViewportToolbox,
    /// A button at the right end of one component's Inspector header.
    InspectorComponentHeader {
        /// The component's registered type name, or its last segment.
        component: String,
    },
    /// A row in the menu a component's Inspector header opens.
    ComponentContextMenu {
        /// The component's registered type name, or its last segment.
        component: String,
    },
    /// A row in the menu an entity row opens.
    EntityContextMenu,
    /// A row in the menu an asset file opens, for assets with this extension.
    AssetContextMenu {
        /// The extension the row is offered for, with its leading dot
        /// (`.scene`).
        extension: String,
    },
    /// The command palette, which offers every registered command and needs
    /// no registration.
    Palette,
}

impl UiSlot {
    /// A toolbar placement.
    pub fn toolbar() -> Self {
        Self::Toolbar
    }

    /// The Inspector header of the component registered as `component`.
    pub fn inspector_component_header(component: &str) -> Self {
        Self::InspectorComponentHeader {
            component: component.to_string(),
        }
    }

    /// The menu of the component registered as `component`.
    pub fn component_context_menu(component: &str) -> Self {
        Self::ComponentContextMenu {
            component: component.to_string(),
        }
    }

    /// An entity row's menu.
    pub fn entity_context_menu() -> Self {
        Self::EntityContextMenu
    }

    /// The menu of asset files with `extension`, written with its leading dot.
    pub fn asset_context_menu(extension: &str) -> Self {
        Self::AssetContextMenu {
            extension: extension.to_lowercase(),
        }
    }

    /// Whether this placement belongs on the header of the component
    /// registered under `type_name`.
    ///
    /// The name matches whole or as its last segment, so a module can name a
    /// component without depending on the crate that defines it.
    pub fn matches_component(&self, type_name: &str) -> bool {
        match self {
            Self::InspectorComponentHeader { component }
            | Self::ComponentContextMenu { component } => matches_component(type_name, component),
            _ => false,
        }
    }

    /// Whether this placement belongs in the menu of an asset with
    /// `extension`, written with its leading dot.
    pub fn matches_extension(&self, extension: &str) -> bool {
        match self {
            Self::AssetContextMenu { extension: wanted } => wanted.eq_ignore_ascii_case(extension),
            _ => false,
        }
    }

    /// The name the log lines and the draw registry spell this placement with.
    pub fn name(&self) -> &'static str {
        match self {
            Self::MenuBar { .. } => "menu_bar",
            Self::Toolbar => "toolbar",
            Self::ViewportToolbox => "viewport_toolbox",
            Self::InspectorComponentHeader { .. } => "inspector_component_header",
            Self::ComponentContextMenu { .. } => "component_context_menu",
            Self::EntityContextMenu => "entity_context_menu",
            Self::AssetContextMenu { .. } => "asset_context_menu",
            Self::Palette => "palette",
        }
    }
}

/// Whether `type_name` names the component `wanted` the placement was
/// registered for.
///
/// The whole name matches, or its end after a `::`, so `ModuleTest` matches
/// `pill_test::ModuleTest` and nothing else.
pub fn matches_component(type_name: &str, wanted: &str) -> bool {
    type_name == wanted
        || type_name
            .strip_suffix(wanted)
            .is_some_and(|prefix| prefix.ends_with("::"))
}

// =============================================================================
// Published state
// =============================================================================

/// What a place shows for one command on one target.
///
/// A module publishes these each frame as plain data; the place reads them
/// while it draws. The default is "visible and usable", because a command drawn
/// from data works before its module says anything about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandState {
    /// Whether the place draws the row at all.
    pub visible: bool,
    /// Whether the row can act. A disabled row draws dimmed and reports no
    /// invocation.
    pub enabled: bool,
    /// `Some(on)` marks a toggle or radio row.
    pub checked: Option<bool>,
    /// The label to draw instead of the command's title.
    pub label: Option<String>,
    /// The hover text to draw instead of the title, one line per entry.
    pub tooltip: Option<Vec<String>>,
    /// A status dot in the button's lower-right corner, as RGBA.
    pub indicator: Option<[u8; 4]>,
    /// An attention dot in the button's upper-right corner, as RGBA.
    pub attention: Option<[u8; 4]>,
}

impl Default for CommandState {
    fn default() -> Self {
        Self {
            visible: true,
            enabled: true,
            checked: None,
            label: None,
            tooltip: None,
            indicator: None,
            attention: None,
        }
    }
}

impl CommandState {
    /// A state that hides the row.
    pub fn hidden() -> Self {
        Self {
            visible: false,
            ..Self::default()
        }
    }

    /// A state that greys the row out.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Sets the label the place draws.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets the hover text, one line per entry.
    pub fn with_tooltip(mut self, lines: Vec<String>) -> Self {
        self.tooltip = Some(lines);
        self
    }

    /// Sets the toggle mark.
    pub fn with_checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// Adds the lower-right status dot.
    pub fn with_indicator(mut self, rgba: [u8; 4]) -> Self {
        self.indicator = Some(rgba);
        self
    }

    /// Adds the upper-right attention dot.
    pub fn with_attention(mut self, rgba: [u8; 4]) -> Self {
        self.attention = Some(rgba);
        self
    }
}

/// The states the modules published for their commands, keyed by command id
/// and then by target.
#[derive(Debug, Default)]
pub struct CommandStates {
    states: HashMap<String, HashMap<CommandTargetKey, CommandState>>,
}

/// One target as the state table keys it, which needs a hashable value.
type CommandTargetKey = crate::commands::CommandTarget;

impl CommandStates {
    /// Replaces every state of `id` with `states`. A module publishes the
    /// whole table each frame, so a target that left drops out.
    pub fn publish(&mut self, id: &str, states: HashMap<CommandTargetKey, CommandState>) {
        self.states.insert(id.to_string(), states);
    }

    /// Returns the state published for `id` on `target`, when there is one.
    pub fn state(&self, id: &str, target: &CommandTargetKey) -> Option<&CommandState> {
        self.states.get(id)?.get(target)
    }

    /// Whether a place draws `id` on `target`: published and visible, or
    /// nothing published at all.
    pub fn is_visible(&self, id: &str, target: &CommandTargetKey) -> bool {
        self.state(id, target).is_none_or(|state| state.visible)
    }

    /// Whether `id` can act on `target`: published and enabled, or nothing
    /// published at all.
    pub fn is_enabled(&self, id: &str, target: &CommandTargetKey) -> bool {
        self.state(id, target).is_none_or(|state| state.enabled)
    }

    /// Inserts the resource into `world` when it holds none.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl pill_engine::Resource for CommandStates {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

// =============================================================================
// Draw functions
// =============================================================================

/// The draw function of one command in one place.
///
/// It draws the command's own widget into the space the place reserved, and
/// calls [`CommandDrawContext::invoke`] when the person clicks it.
pub type CommandDrawFn = fn(&mut egui::Ui, &mut CommandDrawContext<'_>);

/// What a draw function reads and reports while it draws.
///
/// Borrowed for the call only, and it names no type from the registering
/// crate, so nothing here can outlive that module's image.
pub struct CommandDrawContext<'a> {
    /// The command's id.
    pub id: &'a str,
    /// What the command is asked about.
    pub target: &'a CommandTargetKey,
    /// The state the module published for this command on this target.
    pub state: Option<&'a CommandState>,
    /// The invocations the draw reports, applied after the pass.
    pub invocations: &'a mut Vec<CommandInvocation>,
}

impl CommandDrawContext<'_> {
    /// Reports a click, so the command reaches its module on the next update.
    pub fn invoke(&mut self) {
        self.invocations.push(CommandInvocation {
            id: self.id.to_string(),
            target: self.target.clone(),
        });
    }
}

/// One draw function a module asks the editor to use for one place.
///
/// Queued and bound exactly like a panel request: the executable ties it to the
/// registering module's liveness marker, so the draw function of an unloaded
/// module is never called.
#[derive(Clone, Debug)]
pub struct CommandDrawRequest {
    /// The command the draw belongs to.
    pub id: &'static str,
    /// The place it draws the command in.
    pub slot: UiSlot,
    /// The crate registering it.
    pub source: &'static str,
    /// The function that draws the command.
    pub draw: CommandDrawFn,
}

/// One registered draw function.
#[derive(Clone)]
pub struct CommandDrawRegistration {
    /// The command's id.
    pub id: String,
    /// The place the function draws it in.
    pub slot: UiSlot,
    /// The crate that registered it.
    pub source: String,
    /// The function that draws the command.
    pub draw: CommandDrawFn,
    /// The marker of the module that registered it.
    liveness: Weak<ModuleLiveness>,
}

impl CommandDrawRegistration {
    /// Returns whether the module that registered this draw is still loaded.
    pub fn alive(&self) -> bool {
        self.liveness.strong_count() > 0
    }
}

impl std::fmt::Debug for CommandDrawRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandDrawRegistration")
            .field("id", &self.id)
            .field("slot", &self.slot.name())
            .field("source", &self.source)
            .field("alive", &self.alive())
            .finish()
    }
}

/// Every draw function the modules registered, live or not.
#[derive(Debug, Default)]
pub struct CommandDraws {
    entries: Vec<CommandDrawRegistration>,
}

impl CommandDraws {
    /// Registers one draw, replacing any entry for the same command and place
    /// so a reload keeps its position.
    pub fn register(&mut self, request: &CommandDrawRequest, liveness: &Arc<ModuleLiveness>) {
        let registration = CommandDrawRegistration {
            id: request.id.to_string(),
            slot: request.slot.clone(),
            source: request.source.to_string(),
            draw: request.draw,
            liveness: Arc::downgrade(liveness),
        };
        match self
            .entries
            .iter_mut()
            .find(|entry| entry.id == registration.id && entry.slot == registration.slot)
        {
            Some(entry) => *entry = registration,
            None => self.entries.push(registration),
        }
    }

    /// Returns the draw function for `id` in `slot`, when a live module
    /// registered one.
    pub fn draw_for(&self, id: &str, slot: &UiSlot) -> Option<CommandDrawFn> {
        self.entries
            .iter()
            .find(|entry| entry.alive() && entry.id == id && &entry.slot == slot)
            .map(|entry| entry.draw)
    }

    /// Returns every registration, in registration order.
    pub fn entries(&self) -> &[CommandDrawRegistration] {
        &self.entries
    }

    /// Drops the registrations whose module is gone, returning how many.
    pub fn prune(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(CommandDrawRegistration::alive);
        before - self.entries.len()
    }

    /// Returns whether no draw is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Inserts the resource into `world` when it holds none.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl pill_engine::Resource for CommandDraws {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

// =============================================================================
// The places a command appears in
// =============================================================================

/// The commands a set of views places in `slot`, in registration order.
///
/// `slot` matches the placement exactly or, for a per-component or
/// per-extension placement, by its filter.
pub fn commands_in<'a>(
    commands: &'a [crate::commands::CommandView],
    matches: impl Fn(&UiSlot) -> bool + 'a,
) -> impl Iterator<Item = &'a crate::commands::CommandView> + 'a {
    commands
        .iter()
        .filter(move |command| command.placements.iter().any(&matches))
}

/// The distinct placements one command set asks for, for the registry's log
/// and for a place that wants to know what it will draw.
pub fn slots_used(commands: &[crate::commands::CommandView]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for command in commands {
        for slot in &command.placements {
            if seen.insert(slot.name()) {
                names.push(slot.name().to_string());
            }
        }
    }
    names
}

/// Returns the entity a target names, when it names one.
pub fn target_entity(target: &CommandTargetKey) -> Option<Entity> {
    match target {
        CommandTargetKey::Entity { entity, .. } => Some(*entity),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A component name matches whole or as its last segment, never as a
    /// suffix of a different name.
    #[test]
    fn a_component_placement_matches_its_last_segment() {
        let slot = UiSlot::inspector_component_header("MeshRendererComponent");
        assert!(slot.matches_component("MeshRendererComponent"));
        assert!(slot.matches_component("pill_rendering::MeshRendererComponent"));
        assert!(!slot.matches_component("OtherMeshRendererComponent"));
        assert!(!slot.matches_component("MeshRenderer"));
        // An extension filter ignores case and is written with its dot.
        let assets = UiSlot::asset_context_menu(".Scene");
        assert!(assets.matches_extension(".scene"));
        assert!(!assets.matches_extension(".material"));
    }

    /// An unpublished state is visible and usable; a published one decides.
    #[test]
    fn a_command_without_a_state_draws_and_acts() {
        let mut states = CommandStates::default();
        let target = CommandTargetKey::None;
        assert!(states.is_visible("gadget.snap", &target));
        assert!(states.is_enabled("gadget.snap", &target));
        let mut table = HashMap::new();
        table.insert(target.clone(), CommandState::hidden());
        states.publish("gadget.snap", table);
        assert!(!states.is_visible("gadget.snap", &target));
        // Another target keeps the default, which is the point of per-target
        // state.
        let asset = CommandTargetKey::Asset("res/a.scene".to_string());
        assert!(states.is_visible("gadget.snap", &asset));
        assert!(states.is_enabled("gadget.snap", &asset));
    }

    /// A draw function is handed out only while its module's marker is alive,
    /// and a reload replaces the entry in place.
    #[test]
    fn a_draw_function_goes_dead_with_its_module() {
        fn draw(_ui: &mut egui::Ui, _context: &mut CommandDrawContext<'_>) {}
        let mut draws = CommandDraws::default();
        let marker = Arc::new(ModuleLiveness::default());
        let request = CommandDrawRequest {
            id: "gadget.open",
            slot: UiSlot::Toolbar,
            source: "pill_gadget_editor",
            draw,
        };
        draws.register(&request, &marker);
        assert!(draws.draw_for("gadget.open", &UiSlot::Toolbar).is_some());
        // A different place is not what it registered for.
        assert!(draws
            .draw_for("gadget.open", &UiSlot::ViewportToolbox)
            .is_none());
        // The same command and place again keeps one entry.
        draws.register(&request, &marker);
        assert_eq!(draws.entries().len(), 1);
        drop(marker);
        assert!(draws.draw_for("gadget.open", &UiSlot::Toolbar).is_none());
        assert_eq!(draws.prune(), 1);
        assert!(draws.is_empty());
    }
}
