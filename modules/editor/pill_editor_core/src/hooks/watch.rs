//! The last reported selection and tool state, for the diff-driven events.
//!
//! # Responsibilities
//!
//! - Hold the selection, active tool and element slot as they were when the
//!   last drain reported them ([`HookWatch`]).
//! - Compare the world against them and answer the changes
//!   ([`HookWatch::apply`]).
//! - Build the snapshots from the editor's own state ([`snapshot`],
//!   [`element_watch`], [`active_tool`]).
//!
//! # Design
//!
//! Selection and tool changes happen in many places - the shell's rows and
//! tiles, the scene picks, the scripted channel, an undo - so they cannot be
//! reported from one command. The drain compares the live state with this
//! record at the frame boundary instead: every source is caught, and a click
//! stays a click rather than becoming a history step. The record is a `Keep`
//! resource, so a play session leaves it alone.

// External crates
use pill_engine::{Entity, Resource, World};

// Current crate
use crate::editor_state::EditorSelection;
use crate::hooks::events::{
    ElementSelectionChanged, SelectionChanged, SelectionSnapshot, ToolActivatedEdit,
};
use crate::tools::ToolRegistry;

// =============================================================================
// Types
// =============================================================================

/// The element slot as the last drain reported it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ElementWatch {
    /// The entity the elements belong to.
    pub(crate) entity: Option<Entity>,
    /// The elements' kind.
    pub(crate) kind: String,
    /// The selected element ids.
    pub(crate) ids: Vec<u64>,
}

/// The changes one comparison found.
#[derive(Debug, Default)]
pub(crate) struct WatchChanges {
    /// A selection change, when one happened.
    pub(crate) selection: Option<SelectionChanged>,
    /// An active-tool change, when one happened.
    pub(crate) tool: Option<ToolActivatedEdit>,
    /// An element-selection change, when one happened.
    pub(crate) elements: Option<ElementSelectionChanged>,
}

/// The selection and tool state the last drain reported.
#[derive(Debug, Default)]
pub struct HookWatch {
    /// The selection, once a drain has seen one.
    selection: Option<SelectionSnapshot>,
    /// The active tool, once a drain has seen one.
    active_tool: Option<Option<String>>,
    /// The element slot, once a drain has seen one.
    elements: Option<ElementWatch>,
}

impl HookWatch {
    /// Compares the live state with the recorded one, records it, and answers
    /// what changed.
    ///
    /// The first observation records silently: a session that starts with an
    /// entity selected must not report a change nobody made.
    pub(crate) fn apply(
        &mut self,
        selection: SelectionSnapshot,
        active_tool: Option<String>,
        elements: ElementWatch,
    ) -> WatchChanges {
        let changes = WatchChanges {
            selection: self
                .selection
                .as_ref()
                .filter(|previous| **previous != selection)
                .map(|previous| SelectionChanged {
                    previous: previous.clone(),
                    current: selection.clone(),
                }),
            tool: self
                .active_tool
                .as_ref()
                .filter(|previous| **previous != active_tool)
                .map(|previous| ToolActivatedEdit {
                    previous: previous.clone(),
                    current: active_tool.clone(),
                }),
            elements: self
                .elements
                .as_ref()
                .filter(|previous| **previous != elements)
                .map(|previous| ElementSelectionChanged {
                    entity: elements.entity,
                    kind: elements.kind.clone(),
                    previous: previous.ids.clone(),
                    current: elements.ids.clone(),
                }),
        };
        self.selection = Some(selection);
        self.active_tool = Some(active_tool);
        self.elements = Some(elements);
        changes
    }

    /// Registers the resource, inserting a default only when the world has
    /// none.
    pub fn ensure(world: &mut World) {
        world.register_resource::<Self>();
        if world.get_resource::<Self>().is_none() {
            world.insert_resource(Self::default());
        }
    }
}

impl Resource for HookWatch {
    /// Infrastructure, not game state: Play leaves this value alone and Stop
    /// does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_editor_core::HookWatch")
    }
}

// =============================================================================
// Snapshots
// =============================================================================

/// The selection's four slots as one comparable value.
pub(crate) fn snapshot(selection: &EditorSelection) -> SelectionSnapshot {
    SelectionSnapshot {
        entities: selection.entities().to_vec(),
        asset: selection.asset.clone(),
        resource: selection.resource.clone(),
        element_kind: selection.elements().kind().to_string(),
        element_owner: selection.elements().owner(),
        elements: selection
            .elements()
            .elements()
            .iter()
            .map(|element| element.id)
            .collect(),
    }
}

/// The element slot as one comparable value.
pub(crate) fn element_watch(selection: &EditorSelection) -> ElementWatch {
    ElementWatch {
        entity: selection.elements().owner(),
        kind: selection.elements().kind().to_string(),
        ids: selection
            .elements()
            .elements()
            .iter()
            .map(|element| element.id)
            .collect(),
    }
}

/// The active tool's name, when a registry and an active tool exist.
pub(crate) fn active_tool(world: &World) -> Option<String> {
    world
        .get_resource::<ToolRegistry>()
        .and_then(|registry| registry.active().map(str::to_string))
}
