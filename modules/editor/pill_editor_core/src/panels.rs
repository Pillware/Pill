//! Editor panels registered by a module.
//!
//! # Responsibilities
//!
//! - Hold the panels editor crates register: an id, a title, the crate that
//!   registered it, the function that draws it and a liveness marker.
//! - Tell a live panel from one whose module has been unloaded, so the shell
//!   can never call into an unloaded image.
//! - Give each panel a stable slot, which is what a dock tab stores, and keep
//!   the slot across the registering module's reload.
//! - Carry [`PanelContext`], the plain data a panel reads and the commands it
//!   leaves behind.
//!
//! # Design
//!
//! [`PanelRegistry`] is a world resource the executable inserts and the shell
//! reads each frame. A panel's draw function is a plain `fn` pointer into the
//! registering module's image. The registry hands that pointer out only while
//! the source's marker is alive: the marker's strong count says whether the
//! image is still mapped.
//!
//! Registering is per id, not per crate. A module that reloads registers the
//! same id again, and that replaces the entry in place (a new function, marker
//! and epoch) while the slot stays. A module that unloads and never returns
//! leaves its entry with a dead marker, and the tab says so instead of
//! vanishing.

// Standard library
use std::sync::{Arc, Weak};

// External crates
use pill_engine::{Entity, Resource};

/// The liveness marker of one registration source.
///
/// The executable holds the one strong handle, per source, and drops it when
/// that module unloads. The registry stores a [`Weak`] to it, so a strong
/// count of zero means every panel the source registered is dead. A marker
/// struct on purpose: it holds no data, and only its strong count matters.
#[derive(Debug, Default)]
pub struct ModuleLiveness(());

impl ModuleLiveness {
    /// Creates a marker for one registering crate.
    pub fn new() -> Arc<ModuleLiveness> {
        Arc::new(ModuleLiveness(()))
    }
}

/// The draw function a panel registers.
pub type PanelDrawFn = fn(&mut egui::Ui, &mut PanelContext<'_>);

/// What a panel may ask the editor to do.
///
/// A panel queues these into [`PanelContext::commands`] instead of changing
/// the world itself. The editor applies them after the draw, the same contract
/// `EditorCommand` follows, so a panel from a module cannot leave the frame
/// half-applied.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelCommand {
    /// Make this entity the editor's selection.
    SelectEntity(Entity),
    /// Write one component field through the editor's recorded edit path, so
    /// the change is one undo step. The text is parsed against the field's
    /// current value, the same way the Inspector's own rows parse it.
    SetField {
        /// The entity to edit.
        entity: Entity,
        /// The component's registered type name.
        component: String,
        /// The field's name.
        field: String,
        /// The new value, as the Inspector would have typed it.
        value: String,
    },
    /// Write one element of an array field, recorded the same way.
    SetArrayElement {
        /// The entity to edit.
        entity: Entity,
        /// The component's registered type name.
        component: String,
        /// The array field's name.
        field: String,
        /// The element's index.
        index: usize,
        /// The new element value, as the Inspector would have typed it.
        value: String,
    },
    /// Ask the shell to show a window of this panel's.
    OpenWindow(PanelWindowRequest),
    /// Close one of this panel's windows.
    CloseWindow(String),
    /// Write a line the editor's log shows. A module's only way to say what
    /// it did without owning a log of its own.
    Log(String),
}

/// One window a panel asks the shell for.
///
/// The shell owns the window - its position, its lifetime and its closing on
/// the module's unload - so the request is plain data, and the same panel that
/// asked draws the content.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelWindowRequest {
    /// The window's identity within its panel.
    pub id: String,
    /// The title the shell draws in the window's bar.
    pub title: String,
    /// The window's first width in logical points.
    pub width: f32,
    /// The window's first height in logical points.
    pub height: f32,
    /// Whether the window is modal: a modal takes the editor's input until the
    /// person answers it with Enter or Escape, or with its own buttons.
    pub modal: bool,
}

/// One row of the world's entities, as a panel sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelEntityRow {
    /// The entity's handle.
    pub entity: Entity,
    /// The list label the Entities tab shows.
    pub display: String,
    /// The component type names joined into one string. A panel matches on
    /// part of it instead of naming a type from another crate.
    pub archetype: String,
}

/// One component of the selected entity, as a panel sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelComponentValues {
    /// The component's registered type name.
    pub component: String,
    /// The component's fields, in layout order, rendered as text.
    pub fields: Vec<PanelFieldValue>,
}

/// One field value, rendered for a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelFieldValue {
    /// The field's name.
    pub name: String,
    /// The value, in the Inspector's text form.
    pub value: String,
}

/// The selected entity's components, as a panel sees them.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelEntityDetail {
    /// The entity the detail describes.
    pub entity: Entity,
    /// The entity's components, in layout order.
    pub components: Vec<PanelComponentValues>,
}

/// What a panel reads and writes while it draws.
///
/// Borrowed for the call only: a panel must not keep any of it past the frame,
/// and none of it names a type from the panel's own crate, so nothing here can
/// dangle when that module unloads.
pub struct PanelContext<'a> {
    /// The editor's smoothed frame rate.
    pub frames_per_second: f32,
    /// The selected entity, when there is one.
    pub selected_entity: Option<Entity>,
    /// The world's entity rows, copied for the pass that draws the panels.
    pub entities: &'a [PanelEntityRow],
    /// The selected entity's components, when there is a selection.
    pub detail: Option<&'a PanelEntityDetail>,
    /// The window this draw is for: `Some` when the panel is drawing one of
    /// its own windows, `None` when it is drawing its docked tab.
    pub window: Option<&'a str>,
    /// A window id the editor asked this panel to open - the scripted channel's
    /// stand-in for clicking the panel's own button.
    pub requested_window: Option<&'a str>,
    /// The answer a modal window took, for one draw: the panel acts on it and
    /// the shell forgets it afterwards.
    pub answer: Option<&'a PanelAnswer>,
    /// The commands this panel asks for. They are applied after the draw,
    /// never from inside it.
    pub commands: &'a mut Vec<PanelCommand>,
}

/// The answer a modal window took, as the panel that asked reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelAnswer {
    /// The window that was answered.
    pub window: String,
    /// Whether the answer accepted the window: Enter or Apply yes, Escape or
    /// Cancel no.
    pub accepted: bool,
}

/// One registered panel.
#[derive(Clone)]
pub struct PanelRegistration {
    /// The panel's identity: stable across reloads, and what the layout
    /// document stores.
    pub id: String,
    /// The tab's title.
    pub title: String,
    /// The crate that registered it: the log names it, and the panel's
    /// windows are keyed by it.
    pub source: String,
    /// The function that draws the panel's body.
    pub draw: PanelDrawFn,
    /// The marker of the module that registered it: it goes dead when that
    /// module unloads.
    liveness: Weak<ModuleLiveness>,
    /// The number this registration took: every registration takes the next
    /// one, so a panel that registers again gets a new value.
    pub epoch: u64,
}

impl PanelRegistration {
    /// Returns whether the module that registered this panel is still loaded.
    pub fn alive(&self) -> bool {
        self.liveness.strong_count() > 0
    }
}

impl std::fmt::Debug for PanelRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PanelRegistration")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("source", &self.source)
            .field("epoch", &self.epoch)
            .field("alive", &self.alive())
            .finish()
    }
}

/// One panel a module asks the editor to register.
///
/// A module cannot register into the registry itself. Each entry is bound to a
/// liveness marker, and only the executable may hold that marker's strong
/// handle: a marker kept in a world resource would outlive the module and make
/// an unloaded draw function look alive. A module therefore queues its
/// requests here during its `register`, and the executable binds them, so a
/// reload's stale entries go dead with the marker they were bound to.
#[derive(Clone, Copy, Debug)]
pub struct PanelRequest {
    /// The panel's identity, stable across reloads.
    pub id: &'static str,
    /// The tab's title.
    pub title: &'static str,
    /// The crate registering it.
    pub source: &'static str,
    /// The function that draws the panel's body.
    pub draw: PanelDrawFn,
}

/// The panel requests queued since the executable last looked.
///
/// A request is taken once and never read twice, so a request from a module
/// that has since unloaded cannot reach a live entry.
#[derive(Debug, Default)]
pub struct PanelRegistrations {
    /// The requests, oldest first.
    pub pending: Vec<PanelRequest>,
}

impl PanelRegistrations {
    /// Queues one request.
    pub fn push(&mut self, request: PanelRequest) {
        self.pending.push(request);
    }

    /// Takes every queued request and leaves the queue empty.
    pub fn drain(&mut self) -> Vec<PanelRequest> {
        std::mem::take(&mut self.pending)
    }
}

impl Resource for PanelRegistrations {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// Every panel a module registered, live or not.
#[derive(Debug, Default)]
pub struct PanelRegistry {
    registrations: Vec<PanelRegistration>,
    next_epoch: u64,
}

impl PanelRegistry {
    /// Registers a panel and returns its slot, replacing any entry with the
    /// same id.
    ///
    /// The slot is the index the dock tab names. It stays the same across a
    /// reload because the entry is replaced in place, not appended.
    pub fn register(
        &mut self,
        id: &str,
        title: &str,
        source: &str,
        draw: PanelDrawFn,
        liveness: &Arc<ModuleLiveness>,
    ) -> u16 {
        self.next_epoch += 1;
        let epoch = self.next_epoch;
        let registration = PanelRegistration {
            id: id.to_string(),
            title: title.to_string(),
            source: source.to_string(),
            draw,
            liveness: Arc::downgrade(liveness),
            epoch,
        };
        if let Some(slot) = self.slot(id) {
            self.registrations[slot as usize] = registration;
            return slot;
        }
        self.registrations.push(registration);
        (self.registrations.len() - 1) as u16
    }

    /// Returns the slot a panel id occupies, if it is registered.
    pub fn slot(&self, id: &str) -> Option<u16> {
        self.registrations
            .iter()
            .position(|entry| entry.id == id)
            .map(|index| index as u16)
    }

    /// Returns the registration in one slot, if the slot exists.
    pub fn get(&self, slot: u16) -> Option<&PanelRegistration> {
        self.registrations.get(slot as usize)
    }

    /// Returns the draw function of a live panel, or `None` when its module
    /// has unloaded.
    pub fn draw_of(&self, slot: u16) -> Option<PanelDrawFn> {
        self.get(slot)
            .filter(|entry| entry.alive())
            .map(|registration| registration.draw)
    }

    /// Returns the title to show for a slot: the panel's own while it is
    /// alive, its title with " (module not loaded)" added once its module is
    /// gone, and "Module panel" for a slot the registry does not hold.
    pub fn tab_title(&self, slot: u16) -> String {
        match self.get(slot) {
            Some(entry) if entry.alive() => entry.title.clone(),
            Some(entry) => format!("{} (module not loaded)", entry.title),
            None => "Module panel".to_string(),
        }
    }

    /// Returns every registration, in slot order.
    pub fn entries(&self) -> &[PanelRegistration] {
        &self.registrations
    }

    /// Returns how many panels are registered, live or not.
    pub fn len(&self) -> usize {
        self.registrations.len()
    }

    /// Returns whether no panel is registered.
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Returns whether the slot's module is still loaded.
    pub fn alive(&self, slot: u16) -> bool {
        self.get(slot).is_some_and(|entry| entry.alive())
    }

    /// Returns whether a live panel with this id is registered.
    pub fn has_live(&self, id: &str) -> bool {
        self.slot(id).is_some_and(|slot| self.alive(slot))
    }
}

impl Resource for PanelRegistry {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// The commands this frame's panels asked for, applied by the executable at
/// the frame boundary.
///
/// This is a resource rather than a queue the shell owns. The shell is
/// replaced on every reload, and a command queued by a panel of the old
/// generation must still be applied by the executable, which survives the
/// reload.
#[derive(Debug, Default)]
pub struct PanelCommands {
    /// The commands, oldest first.
    pub commands: Vec<PanelCommand>,
}

impl PanelCommands {
    /// Takes every queued command and leaves the queue empty.
    pub fn drain(&mut self) -> Vec<PanelCommand> {
        std::mem::take(&mut self.commands)
    }

    /// Queues one command, as a panel body does through its `PanelContext`.
    pub fn push(&mut self, command: PanelCommand) {
        self.commands.push(command);
    }
}

impl Resource for PanelCommands {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw_panel(ui: &mut egui::Ui, context: &mut PanelContext<'_>) {
        let _ = ui.label("panel");
        let _ = context.frames_per_second;
    }

    #[test]
    fn registering_assigns_slots_in_order() {
        let mut registry = PanelRegistry::default();
        let marker = ModuleLiveness::new();
        assert_eq!(registry.register("a", "A", "shell", draw_panel, &marker), 0);
        assert_eq!(registry.register("b", "B", "shell", draw_panel, &marker), 1);
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.get(1).map(|entry| entry.id.as_str()), Some("b"));
    }

    #[test]
    fn re_registering_replaces_in_place_and_increases_the_epoch() {
        let mut registry = PanelRegistry::default();
        let first_registration_marker = ModuleLiveness::new();
        let slot = registry.register("a", "A", "shell", draw_panel, &first_registration_marker);
        let epoch = registry.get(slot).map(|entry| entry.epoch).unwrap_or(0);
        let second_registration_marker = ModuleLiveness::new();
        assert_eq!(
            registry.register(
                "a",
                "A again",
                "shell",
                draw_panel,
                &second_registration_marker
            ),
            slot
        );
        assert_eq!(registry.len(), 1);
        assert!(registry.get(slot).is_some_and(|entry| entry.epoch > epoch));
        assert_eq!(registry.tab_title(slot), "A again");
    }

    #[test]
    fn a_dropped_marker_makes_the_panel_dead_without_removing_it() {
        let mut registry = PanelRegistry::default();
        let marker = ModuleLiveness::new();
        let slot = registry.register("a", "A", "shell", draw_panel, &marker);
        assert!(registry.alive(slot));
        assert!(registry.draw_of(slot).is_some());
        drop(marker);
        assert!(!registry.alive(slot));
        assert!(registry.draw_of(slot).is_none());
        assert_eq!(registry.tab_title(slot), "A (module not loaded)");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_reload_brings_the_same_slot_back_to_life() {
        let mut registry = PanelRegistry::default();
        let marker = ModuleLiveness::new();
        let slot = registry.register("a", "A", "shell", draw_panel, &marker);
        drop(marker);
        assert!(!registry.alive(slot));
        let marker = ModuleLiveness::new();
        assert_eq!(
            registry.register("a", "A", "shell", draw_panel, &marker),
            slot
        );
        assert!(registry.alive(slot));
    }

    #[test]
    fn an_unknown_slot_has_no_draw_function() {
        let registry = PanelRegistry::default();
        assert!(registry.is_empty());
        assert!(registry.draw_of(9).is_none());
        assert!(!registry.alive(9));
        assert_eq!(registry.tab_title(9), "Module panel");
    }
}
