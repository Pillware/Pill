//! The editor's own view of the engine's state, and the commands that edit it.
//!
//! This module uses `pill_engine` directly. `pill_development_host` only hands
//! the engine over, through `DevHost::engine()` and `DevHost::engine_mut()`.
//!
//! # Responsibilities
//!
//! - Own the tiered [`EditorSnapshot`] types rebuilt from a live `&Engine`.
//! - Own the [`EditorCommand`] types and the applier that runs one batch.
//! - Own the shared [`EditorSelection`] and the armed [`EntityPickRequest`],
//!   the two world resources the editor reads and writes.
//!
//! # Design
//!
//! - **Reads** are tiered snapshots ([`EditorSnapshot`]) rebuilt from a live
//!   `&Engine` at the frame boundary. A capture reads everything it shows from
//!   the live engine instead of from a cache, so a hot reload cannot leave
//!   stale rows behind. Components are addressed by name and looked up again
//!   on every capture.
//! - **Mutations** are queued [`EditorCommand`]s applied at the frame
//!   boundary, before the frame runs. Each command's engine change is all or
//!   nothing, and a failure comes back as an `(index, message)` pair for the
//!   Console panel. The asset-file commands are the exception, because a
//!   filesystem is not a transaction. They copy, write and remove files, try a
//!   rollback when a later step fails, and name every file the rollback could
//!   not take back.

// Standard library
use std::fmt;
use std::path::{Path, PathBuf};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::component_registry::ComponentFieldDescriptor;
use pill_engine::{Engine, Entity, FieldValue, SystemOwner};

// Current crate
use command::CommandOutcome;
pub use command::{CommandFamily, EditorCommand, RecordedApply};

mod apply;
mod command;

// =============================================================================
// Batch observers
// =============================================================================

/// Receives each command a batch applied, for the editor API's callbacks.
///
/// The drain implements this with its hook session; every other caller uses
/// [`NoopObserver`], so their recording path is unchanged.
pub(crate) trait CommandObserver {
    /// Called after one command applied successfully.
    fn after_command(
        &mut self,
        engine: &mut Engine,
        index: usize,
        command: &EditorCommand,
        inverse: Option<&EditorCommand>,
    );

    /// Takes the commands a callback queued for the command just reported.
    fn take_followups(&mut self) -> Vec<EditorCommand>;
}

/// The observer that does nothing: the form [`EditorCommand::apply_recorded`]
/// uses.
pub(crate) struct NoopObserver;

impl CommandObserver for NoopObserver {
    fn after_command(
        &mut self,
        _engine: &mut Engine,
        _index: usize,
        _command: &EditorCommand,
        _inverse: Option<&EditorCommand>,
    ) {
    }

    fn take_followups(&mut self) -> Vec<EditorCommand> {
        Vec::new()
    }
}

// =============================================================================
// Constants
// =============================================================================

/// How many selected entities the snapshot captures a detail for.
///
/// A larger selection still edits the primary entity and the entities it has
/// details for. The cap keeps a Ctrl+A over a large scene from rebuilding
/// hundreds of component views every frame.
pub const SELECTED_DETAILS_MAX: usize = 64;

/// The Assets panel's second root: the folder that holds every extension's
/// read-only resources. It sits beside the project's own tree, not inside it.
pub const EXTENSIONS_ROOT: &str = "extensions";

/// The listing label a folder row carries. Every other label is a file's type.
pub const FOLDER_KIND: &str = "folder";

// =============================================================================
// Types
// =============================================================================

/// One line of the Hierarchy list. It is cheap to build and is rebuilt on
/// every refresh.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityListEntry {
    /// The generation-tagged handle, which is also how the selection names an
    /// entity.
    pub entity: Entity,
    /// `"{id}v{generation}"` display label.
    pub display: String,
    /// Number of components attached.
    pub component_count: usize,
    /// Joined component type names, for the list subtitle.
    pub archetype_label: String,
}

impl EntityListEntry {
    /// The label one entity is listed and referenced by.
    ///
    /// The Hierarchy list, an entity reference row and the picker all name an
    /// entity this way, so a reference reads the same wherever it is shown.
    pub fn display_of(entity: Entity) -> String {
        format!("{}v{}", entity.id(), entity.generation())
    }
}

/// One field of a component's layout, as the editor's own snapshot keeps it.
///
/// The registry's [`ComponentFieldDescriptor`] borrows its name and its
/// decorators from the artifact that declared them. That artifact can be
/// unloaded between two captures, for example a project module replaced by a
/// reload. This type owns both, so nothing reachable from [`EditorSnapshot`]
/// keeps a module image alive.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentFieldView {
    /// The field's registered name.
    pub name: String,
    /// The presentation decorators the field declared, in declaration order.
    pub decorators: Vec<pill_engine::component_registry::FieldDecorator>,
}

impl ComponentFieldView {
    /// Builds the editor's own copy of one registry descriptor.
    ///
    /// It copies the two things the editor reads from the descriptor, so the
    /// snapshot does not borrow from the registry.
    fn from_descriptor(descriptor: &ComponentFieldDescriptor) -> Self {
        Self {
            name: descriptor.name.to_string(),
            decorators: descriptor.decorators.to_vec(),
        }
    }
}

/// One attached component of the selected entity, with its current values.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentView {
    /// Registered component type name (the stable cross-reload key).
    pub type_name: String,
    /// Whether the component is persistable (schema-migrated across reloads).
    pub persistable: bool,
    /// Whether the component has a non-empty field layout and is editable.
    pub editable: bool,
    /// The field layout, owned by this view. The image that declared it may
    /// be unloaded before the next capture.
    pub fields: Vec<ComponentFieldView>,
    /// One current value per field, parallel to `fields`.
    pub values: Vec<FieldValue>,
    /// What each asset reference field names, parallel to `values`.
    ///
    /// It holds `None` for a field that is not an asset reference. The labels
    /// are resolved when the view is captured, because the shell cannot read
    /// the asset store.
    pub asset_labels: Vec<Option<AssetReferenceLabel>>,
    /// What each entity reference field names, parallel to `values`.
    ///
    /// It holds `None` for a field that is not an entity reference. The labels
    /// are resolved when the view is captured, for the same reason.
    pub entity_labels: Vec<Option<EntityReferenceLabel>>,
}

/// One component to place on a newly created entity.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentSeed {
    /// Registered component type name.
    pub type_name: String,
    /// Field values to write over the zero-initialised image. Fields left out
    /// stay zero.
    pub fields: Vec<(String, FieldValue)>,
}

/// One entry of the Inspector's "add component" picker.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredComponent {
    /// Registered type name (the stable cross-reload key).
    pub type_name: String,
    /// True when some generation of this type has a non-empty field layout,
    /// so the editor can zero-initialise an image of it.
    pub addable: bool,
}

/// What an asset reference field currently names, as the Inspector shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetReferenceLabel {
    /// The field is cleared and holds the invalid handle.
    Unset,
    /// The field addresses a live asset, named by its path (or the name its
    /// creator gave it).
    Named(String),
    /// The field addresses a live asset that has no name.
    Unnamed,
    /// The field addresses nothing live, because the asset was removed or the
    /// handle is stale.
    Missing,
}

impl AssetReferenceLabel {
    /// Returns the label of an asset value, read through the world's import
    /// registry.
    pub fn of(world: &pill_engine::World, value: &FieldValue) -> Option<Self> {
        let FieldValue::Asset {
            asset_type,
            index,
            generation,
        } = value
        else {
            return None;
        };
        if (*index, *generation) == (u32::MAX, u32::MAX) {
            return Some(Self::Unset);
        }
        let identity = asset_type.as_deref().and_then(|asset_type| {
            let registry = world.get_resource::<pill_engine::ImportRegistry>()?;
            let assets = world.get_resource::<pill_engine::AssetManager>()?;
            registry.asset_identity(assets, asset_type, *index, *generation)
        });
        Some(match identity {
            Some(identity) => match identity.name {
                Some(name) => Self::Named(name),
                None => Self::Unnamed,
            },
            None => Self::Missing,
        })
    }

    /// Returns the text the Inspector draws for this asset label.
    pub fn text(&self) -> &str {
        match self {
            Self::Unset => "None",
            Self::Named(name) => name,
            Self::Unnamed => "(unnamed)",
            Self::Missing => "(missing)",
        }
    }
}

/// What an entity reference field currently names, as the Inspector shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityReferenceLabel {
    /// The field is cleared and holds the null handle.
    Unset,
    /// The field addresses a live entity, shown by its Hierarchy label.
    Live(String),
    /// The field addresses nothing live. The entity was destroyed, or the
    /// handle names a slot whose occupant has since been replaced.
    Missing,
}

impl EntityReferenceLabel {
    /// Returns the label of an entity value, checked against the world.
    pub fn of(world: &pill_engine::World, value: &FieldValue) -> Option<Self> {
        let FieldValue::Entity { id, generation } = value else {
            return None;
        };
        let entity = Entity::from_raw(*id, *generation);
        if entity.is_null() {
            return Some(Self::Unset);
        }
        Some(if world.is_entity_valid(entity) {
            Self::Live(EntityListEntry::display_of(entity))
        } else {
            Self::Missing
        })
    }

    /// The text the Inspector draws for this label.
    pub fn text(&self) -> &str {
        match self {
            Self::Unset => "None",
            Self::Live(display) => display,
            Self::Missing => "(missing)",
        }
    }
}

/// The field an armed entity reference row waits for a scene click to name,
/// together with the entity it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityPickTarget {
    /// The entity the field belongs to.
    pub entity: Entity,
    /// Registered component type name.
    pub component: String,
    /// Field name.
    pub field: String,
}

/// The world resource that carries an armed entity pick.
///
/// The Inspector row arms it through [`EditorCommand::ArmEntityPick`], and the
/// executable that answers scene picks reads it. The shell draws the rows and
/// has no world, while the picking code lives in the executable, so a world
/// resource is the one place both can reach.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntityPickRequest {
    /// The field the next scene click should name, if a row is waiting.
    pub target: Option<EntityPickTarget>,
}

impl EntityPickRequest {
    /// Takes the armed target and leaves the request clear.
    ///
    /// The click that answers the request is the only caller that consumes it.
    pub fn take(&mut self) -> Option<EntityPickTarget> {
        self.target.take()
    }
}

impl pill_engine::Resource for EntityPickRequest {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// The expensive tier of the snapshot, built only for the selected entities.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityDetail {
    /// The entity this detail describes.
    pub entity: Entity,
    /// Every attached component with its current field values.
    pub components: Vec<ComponentView>,
}

/// One row of the Systems tab.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemSummary {
    /// Registration index, which is the unambiguous key for a toggle.
    pub index: usize,
    /// Registration name.
    pub name: String,
    /// Human label of the owning module: `"project"`, `"engine"`,
    /// `"renderer"`, a module's name, or `"extension <n>"` when the host does
    /// not name it.
    pub owner_label: String,
    /// Whether the system participates in frame execution.
    pub enabled: bool,
    /// Whether a hot-patch dispatch slot exists (host built with the engine's
    /// `development` feature).
    pub hot_patchable: bool,
    /// Whether another system shares this name.
    pub name_is_ambiguous: bool,
}

/// What a listing row is: a folder, or the type the host's listing reported
/// for the file.
///
/// The kind is parsed once, where the snapshot is built, and every branch that
/// cares asks this type. A misspelt label therefore cannot quietly reclassify
/// a row, which would let Rename and Delete queue a move against a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// A directory: the tree expands it, the grid opens it, and no toolbar
    /// action writes to it.
    Folder,
    /// A file's type, as the host reported it: `Texture`, `file`, or a
    /// registered path like `pill_master_renderer::assets::Material`.
    Type(String),
}

impl AssetKind {
    /// Parses a listing label into the kind it names.
    ///
    /// [`FOLDER_KIND`] is the folder; any other label spells a type path.
    pub fn parse(label: &str) -> Self {
        if label == FOLDER_KIND {
            Self::Folder
        } else {
            Self::Type(label.to_string())
        }
    }

    /// Returns whether the row is a directory.
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder)
    }

    /// Returns the label a dump, a scripted channel or a panel row prints:
    /// [`FOLDER_KIND`] or the type path.
    pub fn text(&self) -> &str {
        match self {
            Self::Folder => FOLDER_KIND,
            Self::Type(path) => path,
        }
    }

    /// Returns the type name's last `::` segment, which is what a preview and
    /// a picker compare. It is `None` for a folder, which has no type.
    pub fn type_leaf(&self) -> Option<&str> {
        match self {
            Self::Folder => None,
            Self::Type(path) => Some(path.rsplit("::").next().unwrap_or(path)),
        }
    }
}

/// One row of the Assets panel: a file or folder the browser knows about.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetListing {
    /// The root the row came from. It is empty for the project's own `res`,
    /// and otherwise the name of the module whose read-only resources the row
    /// belongs to. A root's own row has an empty [`Self::path`].
    pub root: String,
    /// The path within its root, forward slashes.
    pub path: String,
    /// Whether the row is one of a module's resources: listed and inspected,
    /// never written.
    pub read_only: bool,
    /// What the row is: a folder, or the file's type.
    pub kind: AssetKind,
    /// Whether the entry is a standalone asset (it holds its own header).
    pub standalone: bool,
    /// Whether a `.meta` sidecar, or a standalone header, carries a guid.
    /// A sourced file without one can get a written sidecar.
    pub has_meta: bool,
    /// The asset's guid when the file names one, in its sidecar or its own
    /// header. A settings document's reference writes this value, and it is
    /// the key that names the asset behind such a reference.
    pub guid: Option<String>,
}

impl AssetListing {
    /// Returns the row as the tree and the grid name it.
    ///
    /// A project row is its [`Self::path`]. An extension's row is
    /// `extensions/<root>/<path>`, or just `extensions/<root>` for the root's
    /// own row.
    ///
    /// Project rows keep their plain path, so a selection, a command and a
    /// scripted verb name a project asset by the path it carries. Extension
    /// rows sit under [`EXTENSIONS_ROOT`], which is a root of the panel in its
    /// own right and never a folder inside the project's tree.
    pub fn key(&self) -> String {
        if self.root.is_empty() {
            self.path.clone()
        } else if self.path.is_empty() {
            format!("{EXTENSIONS_ROOT}/{}", self.root)
        } else {
            format!("{EXTENSIONS_ROOT}/{}/{}", self.root, self.path)
        }
    }
}

/// An asset the store holds that has no file of the project's behind it.
/// Examples are a chain's built-in passes, the renderer's default material, and
/// a pass a module builds in code.
///
/// A picker offers these beside the listing's rows and badges them as the
/// engine's. No directory walk can find them, and a document that references
/// one names its guid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineAsset {
    /// The registered type name a picker matches on, as a listing row's
    /// [`AssetListing::kind`].
    pub kind: String,
    /// The name the store holds it under (`pill.pbr.opaque`).
    pub name: String,
    /// Its guid: what a picker writes, because no path names it.
    pub guid: Option<String>,
}

/// One standalone asset type that a new file can be created for. It is a row
/// of the create dialog, built straight from the loaded modules'
/// registrations.
#[derive(Clone, Debug, PartialEq)]
pub struct StandaloneTypeInfo {
    /// The registered type name, written to the file's `asset_type`.
    pub type_name: String,
    /// The last `::` segment of the type name, for the dialog row.
    pub display_name: String,
    /// The file extension, without the dot.
    pub extension: String,
}

/// The selected asset's detail for the Inspector: its row data
/// and the import settings document the editable rows build on.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetDetail {
    /// The listing row the detail was captured from. Its
    /// [`AssetListing::key`] is the selection identity the Inspector matches
    /// on, so the detail carries the row itself rather than copies of its
    /// fields.
    pub row: AssetListing,
    /// The import settings document: a standalone asset's body, or the
    /// sidecar's `settings`. It is `None` for folders and unknown extensions.
    pub settings: Option<serde_json::Value>,
    /// The reference fields the asset's type declares, expanded: one entry per
    /// picker the Inspector draws, whether or not the document binds it yet.
    pub references: Vec<AssetReferenceField>,
    /// How the asset's document fields draw, as the type declared them: a range
    /// draws as a slider, a tooltip as the label's hover text. Empty for a
    /// type that declares none.
    pub decorators: Vec<AssetFieldDecorator>,
}

/// One field decorator an asset type declares, as the Inspector reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetFieldDecorator {
    /// The document key the decorator applies to.
    pub key: String,
    /// How the row draws.
    pub decorator: pill_engine::FieldDecorator,
}

impl AssetFieldDecorator {
    /// The decorator declared for `key`, if the type declared one.
    pub fn find<'a>(decorators: &'a [Self], key: &str) -> Option<&'a pill_engine::FieldDecorator> {
        decorators
            .iter()
            .find_map(|entry| (entry.key == key).then_some(&entry.decorator))
    }
}

/// One reference field of an asset's settings, as the Inspector draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetReferenceField {
    /// The settings key the edit writes: a field name such as `shader`,
    /// `textures.<slot>`, or `passes[<index>]`.
    pub key: String,
    /// The registered type name its picker offers, as a listing row's kind.
    pub target: String,
    /// The guid the document holds now, when it holds one.
    pub guid: Option<String>,
    /// Whether the document writes `{ "guid": … }` (a named reference) rather
    /// than a bare guid string (a texture slot).
    pub nested: bool,
    /// What the row reads for the asset the guid names: its path when the
    /// listing holds it. Otherwise it is the name the store loaded the asset
    /// under, because a chain's built-in passes have no file of their own. It
    /// is `None` when neither knows the guid.
    pub label: Option<String>,
}

impl AssetDetail {
    /// Returns the asset as the tree, the grid and the selection name it: the
    /// row's own key, so a detail and the row it came from cannot disagree.
    pub fn key(&self) -> String {
        self.row.key()
    }
}

/// Why an element click could not become a selection.
///
/// Both variants are refusals the editor logs by name: the click changes
/// nothing, and the person is told which kinds or entities it would have
/// mixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElementClickRefusal {
    /// An extend or range click named a kind the selection does not hold.
    KindMismatch {
        /// The kind the selection holds.
        selected: String,
        /// The kind the clicked element belongs to.
        clicked: String,
    },
    /// The click would have put two entities in one element selection.
    OtherEntity,
}

impl fmt::Display for ElementClickRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ElementClickRefusal::KindMismatch { selected, clicked } => write!(
                formatter,
                "the selection holds {selected}s; a click cannot extend it with a {clicked}"
            ),
            ElementClickRefusal::OtherEntity => {
                write!(formatter, "an element selection belongs to one entity")
            }
        }
    }
}

/// What a click on an entity row means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionClick {
    /// Replace the selection with the clicked entity.
    Reset,
    /// Add the clicked entity to the selection (Ctrl).
    Add,
    /// Select every row between the primary and the clicked one (Shift).
    Range,
}

impl SelectionClick {
    /// What a click means with these modifiers held: Shift selects a range,
    /// Control adds to the selection, and a plain click resets it.
    ///
    /// Shift wins over Control because a range needs an anchor, and a set that
    /// both extends and ranges has no meaning. The scene's element presses and
    /// the shell's row clicks take their meaning from this one rule.
    pub fn from_modifiers(shift: bool, control: bool) -> SelectionClick {
        if shift {
            SelectionClick::Range
        } else if control {
            SelectionClick::Add
        } else {
            SelectionClick::Reset
        }
    }
}

/// The editor's shared selection, read by every data panel and by the scene.
///
/// It holds one entity set, one asset slot and one resource slot. The
/// Hierarchy and the viewports select entities, the Assets panel selects
/// files, and the Inspector follows whichever slot holds a selection. A
/// resource selection excludes the other two.
///
/// The entity slot is a **set**. The last entity in it is the primary: the
/// one the gizmo acts on, the one the scene outlines most brightly and the one
/// the Inspector edits first. Ctrl extends the set, Shift extends it over the
/// Entities list's range, and a plain click replaces it.
///
/// The element slot is the sub-entity counterpart a tool fills, and it stands
/// apart from the other three on purpose: it names one entity's elements of
/// one kind (see [`ElementSelection`]), and an entity or resource pick clears
/// it because a new entity means a new set of elements.
#[derive(Clone, Default)]
pub struct EditorSelection {
    /// The selected entities, in the order they were selected. The last one is
    /// the primary.
    entities: Vec<Entity>,
    /// The selected asset's project-relative path, when one is selected.
    pub asset: Option<String>,
    /// The selected world resource's stable name, when one is selected. A
    /// resource selection excludes the other two, because the Inspector shows
    /// one thing at a time.
    pub resource: Option<String>,
    /// The selected elements of one entity, when a tool has them selected.
    elements: ElementSelection,
}

/// One element of one entity: what a tool reports and the selection stores.
///
/// The id is the tool's own and stays valid while the component it belongs to
/// is unchanged; the entity is part of the key because element ids are only
/// unique within one entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ElementRef {
    /// The entity the element belongs to.
    pub entity: Entity,
    /// The tool's own id for the element.
    pub id: u64,
}

/// The editor's element slot: the selected elements of one entity, all of one
/// kind.
///
/// The kind lives beside the elements rather than inside each one, so "one kind
/// per selection" is a property of the value: a caller that tries to mix two
/// kinds is refused by [`EditorSelection::set_elements`], which is the only way
/// to fill the slot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementSelection {
    /// The kind every element in [`Self::elements`] belongs to. Empty when
    /// nothing is selected.
    kind: String,
    /// The selected elements, in the order they were selected.
    elements: Vec<ElementRef>,
}

impl ElementSelection {
    /// The kind the selection is limited to, or `""` when nothing is selected.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The selected elements, in the order they were selected.
    pub fn elements(&self) -> &[ElementRef] {
        &self.elements
    }

    /// The selected elements, so a restore can follow them to a fresh entity
    /// handle.
    pub fn elements_mut(&mut self) -> &mut [ElementRef] {
        &mut self.elements
    }

    /// Whether no element is selected.
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// The entity every selected element belongs to, when anything is selected.
    pub fn owner(&self) -> Option<Entity> {
        self.elements.first().map(|element| element.entity)
    }

    /// Whether `id` on `entity` is selected.
    pub fn contains(&self, entity: Entity, id: u64) -> bool {
        self.elements
            .iter()
            .any(|element| element.entity == entity && element.id == id)
    }

    /// Replaces the slot with `elements` of one kind.
    ///
    /// Returns `false` and changes nothing when the elements name more than one
    /// entity: element ids are only unique within one entity, so a mixed set
    /// could not be read back. The caller reports the refusal.
    pub fn set(&mut self, kind: String, elements: Vec<ElementRef>) -> bool {
        let mut owners = elements.iter().map(|element| element.entity);
        if let Some(first) = owners.next() {
            if owners.any(|entity| entity != first) {
                return false;
            }
        }
        self.kind = kind;
        self.elements = elements;
        true
    }

    /// Drops every selected element and the kind with them.
    pub fn clear(&mut self) {
        self.kind.clear();
        self.elements.clear();
    }
}

impl EditorSelection {
    /// Returns the entity set a click produces, given the list's own row
    /// order.
    ///
    /// The function is pure, so the shell (which reads the modifiers) and the
    /// scripted channel (which names the mode) share one rule for what a click
    /// means. A range click needs the primary as its anchor and both rows
    /// present in `rows`. Without them it behaves like a plain click.
    pub fn clicked(&self, click: SelectionClick, entity: Entity, rows: &[Entity]) -> Vec<Entity> {
        match click {
            SelectionClick::Reset => vec![entity],
            SelectionClick::Add => {
                let mut entities: Vec<Entity> = self
                    .entities
                    .iter()
                    .copied()
                    .filter(|selected| *selected != entity)
                    .collect();
                entities.push(entity);
                entities
            }
            SelectionClick::Range => {
                let anchor = self.primary();
                let indices = anchor
                    .and_then(|anchor| rows.iter().position(|row| *row == anchor))
                    .zip(rows.iter().position(|row| *row == entity));
                match indices {
                    Some((anchor_index, entity_index)) => {
                        let (first, last) = if anchor_index <= entity_index {
                            (anchor_index, entity_index)
                        } else {
                            (entity_index, anchor_index)
                        };
                        rows[first..=last].to_vec()
                    }
                    // A range needs an anchor that the list shows. Without one
                    // the click is a plain one.
                    None => vec![entity],
                }
            }
        }
    }

    /// Returns the entity the gizmo acts on and the Inspector edits first,
    /// which is the one selected last.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }

    /// Returns the element set a click produces, given the tool's report
    /// order.
    ///
    /// The rule is the entity click's rule with the kind restriction added: a
    /// plain click replaces the selection whatever kind it held, while an
    /// extend or a range click only works inside the selection's kind - that
    /// refusal is how "control points and tangents never select together" is
    /// enforced. A range needs the last selected element and the clicked one
    /// in `reported`, and falls back to a plain click without them, exactly as
    /// [`Self::clicked`] does for rows.
    ///
    /// # Errors
    ///
    /// Returns [`ElementClickRefusal::KindMismatch`] when an extend or range
    /// click names a kind the selection does not hold, and
    /// [`ElementClickRefusal::OtherEntity`] when it would mix two entities.
    pub fn clicked_elements(
        &self,
        click: SelectionClick,
        target: ElementRef,
        kind: &str,
        reported: &[ElementRef],
    ) -> Result<ElementSelection, ElementClickRefusal> {
        let current = self.elements();
        if click != SelectionClick::Reset && !current.kind().is_empty() {
            if current.kind() != kind {
                return Err(ElementClickRefusal::KindMismatch {
                    selected: current.kind().to_string(),
                    clicked: kind.to_string(),
                });
            }
            if current
                .elements()
                .iter()
                .any(|element| element.entity != target.entity)
            {
                return Err(ElementClickRefusal::OtherEntity);
            }
        }
        let mut next = ElementSelection::default();
        let elements = match click {
            SelectionClick::Reset => vec![target],
            SelectionClick::Add => {
                let mut elements: Vec<ElementRef> = current
                    .elements()
                    .iter()
                    .filter(|element| element.id != target.id)
                    .copied()
                    .collect();
                elements.push(target);
                elements
            }
            SelectionClick::Range => {
                let anchor = current.elements().last().map(|element| element.id);
                let span = anchor
                    .and_then(|anchor| {
                        reported
                            .iter()
                            .position(|element| element.id == anchor)
                            .zip(reported.iter().position(|element| element.id == target.id))
                    })
                    .map(|(anchor_index, target_index)| {
                        let (first, last) = if anchor_index <= target_index {
                            (anchor_index, target_index)
                        } else {
                            (target_index, anchor_index)
                        };
                        reported[first..=last].to_vec()
                    });
                span.unwrap_or_else(|| vec![target])
            }
        };
        if !next.set(kind.to_string(), elements) {
            return Err(ElementClickRefusal::OtherEntity);
        }
        Ok(next)
    }

    /// Returns every selected entity, in selection order.
    pub fn entities(&self) -> &[Entity] {
        &self.entities
    }

    /// Returns whether `entity` is selected.
    pub fn contains(&self, entity: Entity) -> bool {
        self.entities.contains(&entity)
    }

    /// Replaces the whole entity selection.
    pub fn set_entities(&mut self, entities: Vec<Entity>) {
        self.entities = entities;
    }

    /// Selects exactly one entity.
    pub fn select_only(&mut self, entity: Entity) {
        self.entities.clear();
        self.entities.push(entity);
    }

    /// Adds one entity to the selection and makes it the primary.
    ///
    /// Adding an entity that is already selected only makes it primary, so a
    /// Ctrl-click on a selected row does not lose the rest of the set.
    pub fn add_entity(&mut self, entity: Entity) {
        self.entities.retain(|selected| *selected != entity);
        self.entities.push(entity);
    }

    /// Deselects one entity without disturbing the others.
    pub fn remove_entity(&mut self, entity: Entity) {
        self.entities.retain(|selected| *selected != entity);
    }

    /// Drops the entity selection.
    pub fn clear_entities(&mut self) {
        self.entities.clear();
    }

    /// The selected elements of one entity, when a tool has them selected.
    pub fn elements(&self) -> &ElementSelection {
        &self.elements
    }

    /// Replaces the element slot, refusing a set that names more than one
    /// entity.
    ///
    /// Returns `false` and leaves the slot alone on a refusal, which the caller
    /// reports: a tool that reports elements of two entities at once is a tool
    /// bug, not a selection the editor can store.
    pub fn set_elements(&mut self, kind: String, elements: Vec<ElementRef>) -> bool {
        self.elements.set(kind, elements)
    }

    /// Drops the element selection. The entity slot is left alone: the elements
    /// belong to an entity that is usually still selected.
    pub fn clear_elements(&mut self) {
        self.elements.clear();
    }
}

impl pill_engine::Resource for EditorSelection {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// The complete per-refresh view the editor renders.
///
/// It is captured from a live engine on every frame and published as a world
/// resource. The shell and the executable's dumps read the captured copy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditorSnapshot {
    /// The editor's frame index at capture time.
    pub revision: u64,
    /// One row per live entity. This is the tier whose cost grows with the
    /// world.
    pub entities: Vec<EntityListEntry>,
    /// Detail of the selected entity, if the selection is still alive.
    pub detail: Option<EntityDetail>,
    /// The detail of every selected entity, in selection order. The Inspector
    /// draws the primary's rows and reads this to find which other selected
    /// entities a component edit can reach, and which rows the selection
    /// disagrees on. Bounded by [`SELECTED_DETAILS_MAX`].
    pub selected_details: Vec<EntityDetail>,
    /// Registered systems with owner labels.
    pub systems: Vec<SystemSummary>,
    /// Every registered component type, and whether the add picker can offer
    /// it.
    pub components: Vec<RegisteredComponent>,
    /// The asset browser's rows (the Assets panel).
    pub assets: Vec<AssetListing>,
    /// The store's own assets that no file backs. A picker offers them beside
    /// [`Self::assets`], badged as the engine's.
    pub engine_assets: Vec<EngineAsset>,
    /// The live standalone asset types: what a create dialog may
    /// offer.
    pub standalone_types: Vec<StandaloneTypeInfo>,
    /// The selected asset's detail: the Inspector's asset
    /// section.
    pub asset_detail: Option<AssetDetail>,
    /// Whether the engine's system phase is paused.
    pub paused: bool,
    /// Whether Play brings the Game tab forward, from the project's
    /// `editor_settings.yaml`. The executable fills it in; a snapshot captured
    /// without one assumes the switch is on.
    pub focus_game_on_play: bool,
    /// Whether the frame is edited or played. While playing, the menu bar and
    /// the dock's frame are tinted and Play has become Stop.
    pub run_mode: pill_engine::RunMode,
    /// The gizmo tool's settings: mode, space and snapping. The Scene tab's
    /// context menu checks its items against them.
    pub gizmo: crate::gizmo::GizmoSettings,
    /// The name of the tool frame the gizmo is in, when the space row has
    /// reached one of the active tool's frames. The toolbox's space row shows
    /// the name.
    pub gizmo_frame: Option<String>,
    /// The rows the active tool adds to the element context menu for the
    /// current selection, in the tool's own order. The Scene tab's context
    /// menu draws them and a click queues the row's `RunElementMenu`.
    pub element_menu: Vec<crate::tools::ElementMenuEntry>,
    /// Whether the scene grid is on. The toolbox's grid row and the Scene tab's
    /// menu check it against the renderer's switch.
    pub grid_enabled: bool,
    /// Whether the engine's component gizmos draw. The toolbox's gizmos row
    /// checks it against the registry's switch.
    pub gizmos_enabled: bool,
    /// One row per component tool the primary selection's entity is offered,
    /// in registration order. The floating toolbox draws one button per row.
    pub component_tools: Vec<ComponentToolRow>,
    /// One row per registered component gizmo: the type, the package that
    /// declared it and whether an editor hid it. The Preferences tab draws a
    /// switch for each of them.
    pub gizmo_types: Vec<GizmoTypeView>,
    /// The preferences file's values and the current project's remembered
    /// entry. The Preferences tab's rows read them.
    pub preferences: crate::preferences::PreferencesView,
    /// The theme the editor's UI paints with: the code
    /// default merged with the user's and the project's layers.
    pub theme: pill_ui_theme::Theme,
    /// The generation the executable last resolved the theme at. It moves
    /// whenever a layer, an override or a font changes. The shell installs
    /// [`Self::theme`] when this differs from the generation it applied.
    pub theme_generation: u64,
    /// The layers [`Self::theme`] was merged from, in apply order. The code
    /// default is always first.
    pub theme_layers: Vec<crate::style::ThemeLayerInfo>,
    /// The font bytes the theme's layers resolved, installed
    /// with the theme.
    pub theme_fonts: crate::kit::style::ThemeFonts,
    /// How many steps the undo history holds. The menu bar's Undo row greys
    /// out at zero.
    pub undo_depth: usize,
    /// How many steps the redo history holds.
    pub redo_depth: usize,
    /// What the title bar shows for the open scene: the file name, or
    /// `untitled`, with a trailing `*` while it has unsaved edits.
    pub scene_title: String,
    /// Whether a scene file is open. Save greys out when nothing is.
    pub scene_open: bool,
    /// Where Save As writes by default: the open scene's path, or
    /// `scenes/untitled.scene` when no scene file is open.
    pub scene_save_path: String,
    /// Every registered command in registration order. The menu bar draws a
    /// row per command whose menu path names the menu, and a spec that is not
    /// here has no row.
    pub commands: Vec<crate::commands::CommandView>,
    /// The Console tab's filters and newest rows.
    pub console: crate::console::ConsoleView,
    /// Build errors and warnings, one row per diagnostic with the module that
    /// reported it.
    pub problems: Vec<crate::notices::ProblemRow>,
    /// Command failures handed to the capture. Both executables pass an empty
    /// list and report failures in the Console instead.
    pub errors: Vec<String>,
    /// The editable world resources and their field values.
    pub resources: Vec<ResourceView>,
}

/// One editable resource, as the Inspector's resource section shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceView {
    /// The resource's stable name, the key a write addresses.
    pub name: String,
    /// The section's label.
    pub label: String,
    /// The editable fields, in row order.
    pub fields: Vec<ResourceFieldView>,
}

/// One component tool the selection's entity is offered, as the floating
/// toolbox draws it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentToolRow {
    /// The tool's registered name, which is what a click names and what the
    /// row is logged under.
    pub name: String,
    /// The label the row shows.
    pub label: String,
    /// Whether this tool is the active one.
    pub active: bool,
}

/// One registered component gizmo, as the Preferences tab lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GizmoTypeView {
    /// The component's registered type name, the key a visibility write
    /// addresses.
    pub type_name: String,
    /// The package that declared the component, which the row groups by.
    pub owner: String,
    /// Whether the type's gizmo draws.
    pub visible: bool,
}

/// One editable field of a resource.
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceFieldView {
    /// The field's name, the key a write addresses.
    pub field: String,
    /// The row's label.
    pub label: String,
    /// The value, as the row's widget shows it.
    pub value: FieldValue,
}

impl pill_engine::Resource for EditorSnapshot {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl EditorSnapshot {
    /// Builds the tiers this crate can read from a live engine. It never
    /// mutates the world.
    ///
    /// This is a partial build, and the name says so. It fills the tiers the
    /// model crate can read for itself: `entities`, `systems`, `components`,
    /// the command registry, the console, the problems, the resources, the
    /// scene's document state, and the paused, gizmo and grid settings. It
    /// leaves the rest for the frame's caller to supply before publishing:
    ///
    /// - `assets`, `engine_assets`, `standalone_types` and `asset_detail` (the
    ///   listing the executable walks),
    /// - `detail` and `selected_details` (the selected entities' rows),
    /// - `preferences` (the reader's view of the preferences document),
    /// - the four theme fields (the executable resolves the layers),
    /// - `undo_depth` and `redo_depth` (the executable owns the history).
    ///
    /// The shell only ever reads the published value, so a tier added here
    /// still has to be filled, or published, by the executable.
    pub fn capture_engine_tiers(
        engine: &Engine,
        revision: u64,
        extension_names: &[String],
        errors: Vec<String>,
    ) -> Self {
        Self {
            revision,
            entities: Self::capture_entities(engine),
            detail: None,
            selected_details: Vec::new(),
            systems: Self::capture_systems(engine, extension_names),
            components: Self::capture_components(engine),
            assets: Vec::new(),
            engine_assets: Vec::new(),
            standalone_types: Vec::new(),
            asset_detail: None,
            paused: engine.systems_paused(),
            focus_game_on_play: true,
            run_mode: engine.run_mode(),
            gizmo: engine
                .world()
                .get_resource::<crate::gizmo::GizmoState>()
                .map(|state| state.settings)
                .unwrap_or_default(),
            gizmo_frame: Self::capture_gizmo_frame(engine),
            element_menu: Self::capture_element_menu(engine),
            grid_enabled: engine
                .world()
                .get_resource::<pill_renderer_api::SurfaceCameras>()
                .map(|cameras| cameras.grid.enabled)
                .unwrap_or(false),
            gizmos_enabled: engine
                .world()
                .get_resource::<pill_engine::gizmos::GizmoRegistry>()
                .map(pill_engine::gizmos::GizmoRegistry::is_enabled)
                .unwrap_or(false),
            component_tools: Self::capture_component_tools(engine),
            gizmo_types: engine
                .world()
                .get_resource::<pill_engine::gizmos::GizmoRegistry>()
                .map(|registry| {
                    registry
                        .entries()
                        .iter()
                        .map(|entry| GizmoTypeView {
                            type_name: entry.type_name.clone(),
                            owner: entry.owner.clone(),
                            visible: registry.is_type_visible(&entry.type_name),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            preferences: crate::preferences::PreferencesView::default(),
            // The code default, until the executable resolves its layers. This
            // way every posture's first frame paints the editor's own look
            // rather than the crate's neutral fallback.
            theme: crate::style::default_theme(),
            theme_generation: 1,
            theme_layers: vec![crate::style::ThemeLayerInfo::code()],
            theme_fonts: crate::kit::style::ThemeFonts::default(),
            undo_depth: 0,
            redo_depth: 0,
            scene_title: engine
                .world()
                .get_resource::<crate::scene_workflow::SceneSession>()
                .map(crate::scene_workflow::SceneSession::title)
                .unwrap_or_else(|| "untitled".to_string()),
            scene_open: engine
                .world()
                .get_resource::<crate::scene_workflow::SceneSession>()
                .is_some_and(|session| session.open.is_some()),
            scene_save_path: engine
                .world()
                .get_resource::<crate::scene_workflow::SceneSession>()
                .map(crate::scene_workflow::default_save_path)
                .unwrap_or_else(|| "scenes/untitled.scene".to_string()),
            commands: engine
                .world()
                .get_resource::<crate::commands::CommandRegistry>()
                .map(crate::commands::CommandRegistry::views)
                .unwrap_or_default(),
            console: engine
                .world()
                .get_resource::<crate::console::ConsoleState>()
                .map(crate::console::ConsoleState::view)
                .unwrap_or_default(),
            problems: engine
                .world()
                .get_resource::<crate::notices::EditorNotices>()
                .map(|notices| notices.problems.clone())
                .unwrap_or_default(),
            errors,
            resources: Self::capture_resources(engine),
        }
    }

    /// The name of the tool frame the gizmo is in, when the space row has
    /// reached one of the active tool's frames.
    ///
    /// The frames belong to the element selection's own entity, so a frame
    /// index is resolved against the tool's report for that selection instead
    /// of being trusted: a tool whose frames changed under it (a reload, a new
    /// selection) simply stops naming one, and the settings' own space applies.
    fn capture_gizmo_frame(engine: &Engine) -> Option<String> {
        let world = engine.world();
        let index = world
            .get_resource::<crate::gizmo::GizmoState>()?
            .tool_frame? as usize;
        crate::tools::selection_frames(world)
            .into_iter()
            .nth(index)
            .map(|frame| frame.name)
    }

    /// The menu rows the active tool adds for the current element selection.
    ///
    /// Empty when no tool is active or the tool adds none, which is what makes
    /// an editor without a component tool's menu look exactly as it did.
    fn capture_element_menu(engine: &Engine) -> Vec<crate::tools::ElementMenuEntry> {
        let world = engine.world();
        let Some((entity, kind, selected)) = crate::tools::element_selection_subject(world) else {
            return Vec::new();
        };
        let Some(registry) = world.get_resource::<crate::tools::ToolRegistry>() else {
            return Vec::new();
        };
        registry.active_menu(entity, &kind, &selected, world)
    }

    /// The component tools the Inspector's entity is offered, in registration
    /// order.
    ///
    /// The subject is the selection's primary entity, which is what the
    /// Inspector shows. An empty selection, or an entity that carries no tool's
    /// component, offers nothing.
    fn capture_component_tools(engine: &Engine) -> Vec<ComponentToolRow> {
        let world = engine.world();
        let Some(entity) = world
            .get_resource::<EditorSelection>()
            .and_then(EditorSelection::primary)
        else {
            return Vec::new();
        };
        let Some(registry) = world.get_resource::<crate::tools::ToolRegistry>() else {
            return Vec::new();
        };
        let active = registry.active();
        registry
            .offered_component_tools(entity, world)
            .into_iter()
            .map(|tool| ComponentToolRow {
                name: tool.name.clone(),
                label: tool.label.clone(),
                active: active == Some(tool.name.as_str()),
            })
            .collect()
    }

    /// Returns the snapshot fields that differ from `previous`, comma-joined.
    ///
    /// The redraw cadence treats any change as a reason to draw, so this
    /// answers the question that follows: which fields moved. A field that
    /// changes on its own, such as a timer, a counter or an unordered
    /// collection, would keep the editor drawing forever. Naming it in the log
    /// is how that is caught rather than suspected.
    ///
    /// This is also the change test itself, so an empty answer means the two
    /// captures draw the same data. Four fields are deliberately not compared,
    /// so a capture that only added log lines is not a reason to draw: the
    /// frame index, `theme_fonts` (a font change always moves the theme
    /// generation, which is compared) and the two tiers a frame feeds itself,
    /// the console and the problems. The test lives here, beside the fields it
    /// walks, so a new field is one line away from the decision that has to
    /// include it.
    #[must_use]
    pub fn changed_fields(&self, current: &Self) -> String {
        let previous = self;
        let mut changed: Vec<&str> = Vec::new();
        if previous.entities != current.entities {
            changed.push("entities");
        }
        if previous.detail != current.detail {
            changed.push("detail");
        }
        if previous.selected_details != current.selected_details {
            changed.push("selected_details");
        }
        if previous.systems != current.systems {
            changed.push("systems");
        }
        if previous.components != current.components {
            changed.push("components");
        }
        if previous.assets != current.assets {
            changed.push("assets");
        }
        if previous.engine_assets != current.engine_assets {
            changed.push("engine_assets");
        }
        if previous.standalone_types != current.standalone_types {
            changed.push("standalone_types");
        }
        if previous.asset_detail != current.asset_detail {
            changed.push("asset_detail");
        }
        if previous.paused != current.paused {
            changed.push("paused");
        }
        if previous.run_mode != current.run_mode {
            changed.push("run_mode");
        }
        if previous.gizmo != current.gizmo {
            changed.push("gizmo");
        }
        if previous.gizmo_frame != current.gizmo_frame {
            changed.push("gizmo_frame");
        }
        if previous.element_menu != current.element_menu {
            changed.push("element_menu");
        }
        if previous.grid_enabled != current.grid_enabled {
            changed.push("grid_enabled");
        }
        if previous.gizmos_enabled != current.gizmos_enabled {
            changed.push("gizmos_enabled");
        }
        if previous.component_tools != current.component_tools {
            changed.push("component_tools");
        }
        if previous.gizmo_types != current.gizmo_types {
            changed.push("gizmo_types");
        }
        if previous.preferences != current.preferences {
            changed.push("preferences");
        }
        if previous.theme_generation != current.theme_generation
            || previous.theme_layers != current.theme_layers
        {
            changed.push("theme");
        }
        if previous.undo_depth != current.undo_depth || previous.redo_depth != current.redo_depth {
            changed.push("history");
        }
        if previous.scene_title != current.scene_title
            || previous.scene_open != current.scene_open
            || previous.scene_save_path != current.scene_save_path
        {
            changed.push("scene");
        }
        if previous.commands != current.commands {
            changed.push("commands");
        }
        if previous.errors != current.errors {
            changed.push("errors");
        }
        if previous.resources != current.resources {
            changed.push("resources");
        }
        changed.join(",")
    }

    /// Reads the editable resources' values through their curated tables.
    ///
    /// The engine keeps no resource reflection, so a resource that is not in
    /// [`crate::resources::EDITABLE_RESOURCES`] is not captured at all, because
    /// there is nothing to read it through. Every row of a table reads, because
    /// the table is the same one the write path goes through.
    fn capture_resources(engine: &Engine) -> Vec<ResourceView> {
        let world = engine.world();
        let Some(gizmo_settings) = world
            .get_resource::<crate::gizmo::GizmoState>()
            .map(|state| state.settings)
        else {
            return Vec::new();
        };
        crate::resources::EDITABLE_RESOURCES
            .iter()
            .map(|entry| ResourceView {
                name: entry.name.to_string(),
                label: entry.label.to_string(),
                fields: entry
                    .fields
                    .iter()
                    .map(|field| ResourceFieldView {
                        field: field.name.to_string(),
                        label: field.label.to_string(),
                        value: (field.read)(&gizmo_settings),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Builds the entity tier: one row per live entity, each with owned label
    /// strings. This is the tier whose cost grows with the world, and a
    /// 200k-entity world spends real time here in debug builds.
    ///
    /// The editor captures it every frame because the rows must follow a
    /// system that spawns, despawns or recomposes an entity. Caching would need
    /// a change signal the world does not expose: `entity_count` alone misses
    /// a spawn and a despawn in the same frame.
    pub fn capture_entities(engine: &Engine) -> Vec<EntityListEntry> {
        engine
            .world()
            .entity_rows()
            .into_iter()
            .map(|row| {
                let display_label = EntityListEntry::display_of(row.entity);
                let archetype_label = row.components.join(", ");
                let component_count = row.components.len();
                EntityListEntry {
                    entity: row.entity,
                    display: display_label,
                    component_count,
                    archetype_label,
                }
            })
            .collect()
    }

    /// Builds the system tier: one summary per registered system. It is cheap.
    pub fn capture_systems(engine: &Engine, extension_names: &[String]) -> Vec<SystemSummary> {
        engine
            .system_snapshots()
            .into_iter()
            .map(|system| SystemSummary {
                index: system.index,
                owner_label: system_owner_label(system.owner, extension_names),
                name: system.name,
                enabled: system.enabled,
                hot_patchable: system.hot_patchable,
                name_is_ambiguous: system.name_is_ambiguous,
            })
            .collect()
    }

    /// Builds the component tier: every registered component type and whether
    /// it can be added. It is cheap.
    pub fn capture_components(engine: &Engine) -> Vec<RegisteredComponent> {
        registered_components(engine.world())
    }

    /// Rebuilds the expensive detail tier for one selected entity.
    pub fn capture_detail(engine: &Engine, entity: Entity) -> Option<EntityDetail> {
        let world = engine.world();
        if !world.is_entity_valid(entity) {
            return None;
        }
        let component_names = world.entity_component_names(entity)?;
        let components = component_names
            .into_iter()
            .filter_map(|type_name| {
                let component_id = world.resolve_entity_component_id(entity, &type_name)?;
                let persistable = world.component_is_persistable(component_id);
                let (fields, editable) = match world.component_field_layout(component_id) {
                    // The registry's descriptors borrow from the artifact that
                    // declared them. The snapshot keeps its own copy, so a
                    // module reloaded between two captures cannot dangle under
                    // the shell.
                    Some(layout) if !layout.is_empty() => (
                        layout
                            .iter()
                            .map(ComponentFieldView::from_descriptor)
                            .collect(),
                        true,
                    ),
                    _ => (Vec::new(), false),
                };
                // Values can only be decoded when a layout exists. A component
                // with no layout still appears, read-only.
                let field_values = if editable {
                    world
                        .read_component_fields(entity, &type_name)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let asset_labels = field_values
                    .iter()
                    .map(|value| AssetReferenceLabel::of(world, value))
                    .collect();
                let entity_labels = field_values
                    .iter()
                    .map(|value| EntityReferenceLabel::of(world, value))
                    .collect();
                Some(ComponentView {
                    type_name,
                    persistable,
                    editable,
                    fields,
                    values: field_values,
                    asset_labels,
                    entity_labels,
                })
            })
            .collect();
        Some(EntityDetail { entity, components })
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Resolves the asset value `asset_reference` names, and loads the asset when
/// it is not loaded yet.
///
/// The reference is a path relative to `res`, a guid the project's files
/// carry, or a name the store already holds. The returned value carries the
/// type the asset loaded as, so the field write that follows refuses an asset
/// of a different type than the field references.
fn resolve_asset_reference(
    engine: &mut Engine,
    asset_reference: &str,
) -> Result<FieldValue, String> {
    // Step 1: work out what the reference names - a path to load, or an asset
    // the store already holds.
    let world = engine.world_mut();
    let registry = world
        .get_resource::<pill_engine::ImportRegistry>()
        .cloned()
        .ok_or("no asset types are registered")?;
    let asset_path = match pill_engine::AssetGuid::parse(asset_reference) {
        Some(guid) => match world
            .get_resource::<pill_engine::AssetDatabase>()
            .and_then(|database| database.by_guid(guid))
            .map(|record| record.path.clone())
        {
            Some(path) => path,
            None => {
                // No file names the guid, so the asset was built in code and
                // the store already holds it. Its handle is the whole answer.
                let assets = world
                    .get_resource::<pill_engine::AssetManager>()
                    .ok_or("the world has no asset store")?;
                let (asset_type, _name, index, generation) = registry
                    .loaded_by_guid(assets, guid)
                    .ok_or_else(|| format!("no asset in the project has the guid {guid}"))?;
                return Ok(FieldValue::Asset {
                    asset_type: Some(asset_type),
                    index,
                    generation,
                });
            }
        },
        None => {
            // A name the store holds is an asset with no file to load, such as
            // a chain's built-in pass or the renderer's default material. It is
            // addressed by the name it was added under.
            let held_asset = world
                .get_resource::<pill_engine::AssetManager>()
                .and_then(|assets| registry.loaded_by_name(assets, asset_reference));
            if let Some((asset_type, index, generation)) = held_asset {
                return Ok(FieldValue::Asset {
                    asset_type: Some(asset_type),
                    index,
                    generation,
                });
            }
            asset_reference.to_owned()
        }
    };
    // Step 2: load the file the reference names, and hand back the handle it
    // loaded as.
    let assets = world
        .get_resource_mut::<pill_engine::AssetManager>()
        .ok_or("the world has no asset store")?;
    let outcome = registry
        .load_by_path(
            assets,
            std::path::Path::new(&asset_path),
            pill_engine::MetadataPolicy::CreateIfMissing,
        )
        .map_err(|error| error.to_string())?;
    Ok(FieldValue::Asset {
        asset_type: Some(outcome.type_name),
        index: outcome.index,
        generation: outcome.generation,
    })
}

/// Labels a system owner using the host's extension names.
///
/// The owner's index is decoded by [`SystemOwner::extension_index`]. An
/// extension the host does not name falls back to the owner's own rendering.
fn system_owner_label(system_owner: SystemOwner, extension_names: &[String]) -> String {
    match system_owner.extension_index() {
        None => system_owner.to_string(),
        Some(index) => extension_names
            .get(index)
            .cloned()
            .unwrap_or_else(|| system_owner.to_string()),
    }
}

/// Returns the registered component types for the add picker, one entry per
/// type name.
///
/// The registry keeps one entry per reload generation of a type name, which is
/// expected. A type is offered once and counts as addable when any of its
/// generations carries a field layout.
pub fn registered_components(world: &pill_engine::World) -> Vec<RegisteredComponent> {
    use std::collections::HashMap;
    let mut addable_by_type_name: HashMap<String, bool> = HashMap::new();
    for (type_name, component_id) in world.registered_components() {
        let addable = world
            .component_field_layout(component_id)
            .is_some_and(|layout| !layout.is_empty());
        addable_by_type_name
            .entry(type_name)
            .and_modify(|is_addable| *is_addable |= addable)
            .or_insert(addable);
    }
    let mut addable_components = addable_by_type_name
        .into_iter()
        .map(|(type_name, addable)| RegisteredComponent { type_name, addable })
        .collect::<Vec<_>>();
    addable_components.sort_by(|left, right| left.type_name.cmp(&right.type_name));
    addable_components
}

/// Returns the mounted asset directory that the asset commands resolve their
/// relative paths against.
fn asset_authoring_root() -> Result<PathBuf, String> {
    pill_engine::asset_store::mounted_directory().ok_or("no asset directory is mounted".to_owned())
}

/// Returns an error when `relative_path` names one of a module's resources.
///
/// A module's resources are listed and inspected, never written. The check sits
/// in every mutation arm, so the widget, the context menu and the scripted
/// channel all reach it. The [`EXTENSIONS_ROOT`] itself is refused too: no
/// asset of the project's lives there, so a command carrying a key that deep is
/// a bug rather than an edit.
fn refuse_module_resource(command: &EditorCommand, relative_path: &Path) -> Result<(), String> {
    let under_extensions_root = relative_path
        .components()
        .next()
        .is_some_and(|first| first.as_os_str() == EXTENSIONS_ROOT);
    if under_extensions_root || pill_engine::asset_store::is_module_asset(relative_path) {
        return Err(format!(
            "{command:?}: `{}` is a module's resource and cannot be modified",
            asset_slash(relative_path)
        ));
    }
    Ok(())
}

/// Returns `path` as a `/`-separated string, the form asset commands carry.
fn asset_slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Returns the guid a standalone asset's own header holds, when its bytes are a
/// document and the guid parses.
fn document_guid(bytes: &[u8]) -> Option<pill_engine::AssetGuid> {
    let document = pill_engine::asset_document::parse(bytes).ok()?;
    pill_engine::AssetGuid::parse(document.get("guid")?.as_str()?)
}

/// Writes `bytes` to `file_path` through a temporary file beside it, so a
/// reader never sees a half-written file. The parent directory is created when
/// it is missing.
fn write_asset_bytes(file_path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create `{}`: {error}", parent.display()))?;
    }
    let file_name = file_path
        .file_name()
        .ok_or_else(|| format!("`{}` has no file name", file_path.display()))?;
    let temporary_path = file_path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::write(&temporary_path, bytes)
        .map_err(|error| format!("cannot write `{}`: {error}", temporary_path.display()))?;
    std::fs::rename(&temporary_path, file_path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary_path);
        format!("cannot write `{}`: {error}", file_path.display())
    })
}

/// Removes the files a command created and returns the ones it could not
/// remove.
///
/// A failed asset-file command takes its own work back, so a half-applied
/// change is not left for the asset watcher to import as an unrecorded edit.
/// The removal can fail as well, from a locked file, a permission or a disk
/// error, and those cases are what this reports: the paths still on disk.
fn roll_back_files(paths: &[String]) -> Vec<String> {
    roll_back_files_with(paths, &mut |path| std::fs::remove_file(path))
}

/// Same as [`roll_back_files`] with a file remover the caller supplies, so the
/// failing branch can be tested without a permission change.
fn roll_back_files_with(
    paths: &[String],
    remove_file: &mut dyn FnMut(&str) -> std::io::Result<()>,
) -> Vec<String> {
    let mut left_behind_paths = Vec::new();
    for path in paths {
        if remove_file(path).is_err() && Path::new(path).exists() {
            left_behind_paths.push(path.clone());
        }
    }
    left_behind_paths
}

/// Logs the files a command meant to take back and could not.
fn warn_left_behind(left_behind_paths: &[String]) {
    if left_behind_paths.is_empty() {
        return;
    }
    pill_core::warn!(
        target: telemetry_target::EDITOR,
        paths = left_behind_paths.join(","),
        "[editor] rollback left files behind"
    );
}

/// Extends a failed command's message with the paths its rollback left on
/// disk. The Console has to say what is still there, not only what failed.
fn with_rollback_paths(message: String, left_behind_paths: &[String]) -> String {
    if left_behind_paths.is_empty() {
        return message;
    }
    warn_left_behind(left_behind_paths);
    format!(
        "{message} (rollback incomplete: {})",
        left_behind_paths.join(", ")
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A click replaces the selection, Ctrl adds to it, and Shift covers the
    /// rows between the anchor and the clicked row.
    #[test]
    fn selection_clicks_reset_add_and_range() {
        let rows: Vec<Entity> = (1..=4).map(|id| Entity::from_raw(id, 0)).collect();
        let (first, second, third, fourth) = (rows[0], rows[1], rows[2], rows[3]);

        let selection = EditorSelection::default();
        assert_eq!(
            selection.clicked(SelectionClick::Reset, second, &rows),
            vec![second]
        );

        // Ctrl adds, and re-adding an entity already selected only makes it
        // primary rather than duplicating it.
        let mut selection = EditorSelection::default();
        selection.set_entities(vec![first]);
        let added = selection.clicked(SelectionClick::Add, third, &rows);
        assert_eq!(added, vec![first, third]);
        selection.set_entities(added);
        assert_eq!(
            selection.clicked(SelectionClick::Add, first, &rows),
            vec![third, first]
        );

        // Shift selects over the list's own order, in either direction.
        let mut selection = EditorSelection::default();
        selection.set_entities(vec![first, second]);
        assert_eq!(
            selection.clicked(SelectionClick::Range, fourth, &rows),
            vec![second, third, fourth]
        );
        assert_eq!(
            selection.clicked(SelectionClick::Range, second, &rows),
            vec![second]
        );

        // A range with no anchor the list shows is a plain click.
        let empty = EditorSelection::default();
        assert_eq!(
            empty.clicked(SelectionClick::Range, third, &rows),
            vec![third]
        );
    }

    /// A held key means one thing everywhere: Shift ranges, Control adds, and
    /// Shift wins when both are held.
    #[test]
    fn click_modifiers_map_to_one_rule() {
        assert_eq!(
            SelectionClick::from_modifiers(false, false),
            SelectionClick::Reset
        );
        assert_eq!(
            SelectionClick::from_modifiers(false, true),
            SelectionClick::Add
        );
        assert_eq!(
            SelectionClick::from_modifiers(true, false),
            SelectionClick::Range
        );
        assert_eq!(
            SelectionClick::from_modifiers(true, true),
            SelectionClick::Range
        );
    }

    /// An element click replaces across kinds, extends only within the
    /// selection's kind (refusing and naming both kinds otherwise), and takes
    /// a range over the tool's report order.
    #[test]
    fn element_clicks_replace_extend_and_range_within_one_kind() {
        let owner = Entity::from_raw(1, 0);
        let other = Entity::from_raw(2, 0);
        let reported: Vec<ElementRef> = (0..4).map(|id| ElementRef { entity: owner, id }).collect();
        let target = reported[2];

        // A plain click replaces whatever kind the selection held.
        let mut selection = EditorSelection::default();
        assert!(selection.set_elements(
            "tangent".to_string(),
            vec![ElementRef {
                entity: owner,
                id: 9
            }]
        ));
        let next = selection
            .clicked_elements(SelectionClick::Reset, target, "point", &reported)
            .expect("a plain click always applies");
        assert_eq!(next.kind(), "point");
        assert_eq!(next.elements(), &[target]);

        // Ctrl-clicking the same kind extends, and re-clicking an element
        // already selected only makes it last.
        selection.set_elements("point".to_string(), vec![reported[0]]);
        let next = selection
            .clicked_elements(SelectionClick::Add, target, "point", &reported)
            .expect("the kinds agree");
        assert_eq!(next.elements(), &[reported[0], target]);
        selection.set_elements("point".to_string(), vec![reported[0], target]);
        let next = selection
            .clicked_elements(SelectionClick::Add, reported[0], "point", &reported)
            .expect("the kinds agree");
        assert_eq!(
            next.elements(),
            &[target, reported[0]],
            "an element already selected moves to the end, it is not duplicated"
        );

        // A Ctrl-click naming another kind is refused, and the refusal names
        // both kinds.
        let refusal = selection
            .clicked_elements(SelectionClick::Add, target, "tangent", &reported)
            .expect_err("a tangent cannot extend a point selection");
        assert_eq!(
            refusal,
            ElementClickRefusal::KindMismatch {
                selected: "point".to_string(),
                clicked: "tangent".to_string(),
            }
        );
        assert!(refusal.to_string().contains("points"), "{refusal}");
        assert!(selection
            .clicked_elements(SelectionClick::Range, target, "tangent", &reported)
            .is_err());

        // Shift covers the report order between the anchor and the click.
        selection.set_elements("point".to_string(), vec![reported[0]]);
        let next = selection
            .clicked_elements(SelectionClick::Range, target, "point", &reported)
            .expect("the kinds agree");
        assert_eq!(next.elements(), &reported[0..=2]);

        // A range whose anchor the report no longer holds is a plain click.
        selection.set_elements(
            "point".to_string(),
            vec![ElementRef {
                entity: owner,
                id: 77,
            }],
        );
        let next = selection
            .clicked_elements(SelectionClick::Range, target, "point", &reported)
            .expect("the kinds agree");
        assert_eq!(next.elements(), &[target]);

        // An extend that would mix two entities is refused by name.
        selection.set_elements(
            "point".to_string(),
            vec![ElementRef {
                entity: other,
                id: 0,
            }],
        );
        assert_eq!(
            selection
                .clicked_elements(SelectionClick::Add, target, "point", &reported)
                .expect_err("two entities cannot share one element selection"),
            ElementClickRefusal::OtherEntity
        );
    }

    /// A plain component with a field layout, so entity create and field
    /// writes have something to target.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct DemoComponent {
        strength: f32,
        enabled: bool,
    }
    impl pill_engine::Component for DemoComponent {}

    /// Hand-written layout mirroring what `#[derive(PillComponent)]` emits.
    static DEMO_COMPONENT_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "strength",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "enabled",
            type_tag: "bool",
            offset: 4,
            size: 1,
            align: 1,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    fn demo_component_name() -> String {
        std::any::type_name::<DemoComponent>().to_string()
    }

    fn demo_engine() -> Engine {
        let mut engine = Engine::new();
        engine
            .world_mut()
            .register_component_with_layout::<DemoComponent>(DEMO_COMPONENT_FIELDS);
        engine
    }

    #[test]
    fn snapshot_list_reflects_live_entities() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 2.0,
                enabled: true,
            })
            .build()
            .expect("entity builds");

        let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
        assert_eq!(snapshot.entities.len(), 1);
        assert_eq!(snapshot.entities[0].entity, entity);
        assert_eq!(snapshot.entities[0].component_count, 1);

        // Detail capture reads the component values.
        let detail = EditorSnapshot::capture_detail(&engine, entity).expect("detail");
        assert_eq!(detail.components.len(), 1);
        assert!(detail.components[0].type_name.contains("DemoComponent"));
        assert_eq!(
            detail.components[0].values,
            vec![FieldValue::F32(2.0), FieldValue::Bool(true)]
        );
    }

    /// A component tool over the test module's own component.
    struct DemoComponentTool;

    impl crate::tools::EditorTool for DemoComponentTool {
        fn name(&self) -> &'static str {
            "demo component"
        }

        fn label(&self) -> &'static str {
            "Demo Component"
        }

        fn is_global(&self) -> bool {
            false
        }

        fn requires_component(&self) -> Option<&'static str> {
            Some("DemoComponent")
        }
    }

    #[test]
    fn the_snapshot_carries_the_component_tools_the_selection_is_offered() {
        let mut engine = demo_engine();
        let carrier = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 1.0,
                enabled: true,
            })
            .build()
            .expect("the carrier builds");
        let plain = engine
            .world_mut()
            .create_entity()
            .build()
            .expect("the plain entity builds");
        crate::tools::tool_registry_mut(engine.world_mut()).register(&DemoComponentTool);

        // Nothing selected: no row.
        let empty = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
        assert!(empty.component_tools.is_empty());

        // The carrier offers its row, inactive until the tool is chosen.
        let mut selection = EditorSelection::default();
        selection.select_only(carrier);
        engine.world_mut().insert_resource(selection);
        let offered = EditorSnapshot::capture_engine_tiers(&engine, 1, &[], Vec::new());
        assert_eq!(
            offered.component_tools,
            vec![ComponentToolRow {
                name: "demo component".to_string(),
                label: "Demo Component".to_string(),
                active: false,
            }]
        );
        let changed = empty.changed_fields(&offered);
        assert!(
            changed.split(',').any(|field| field == "component_tools"),
            "a row appearing is a change the frame has to draw: {changed}"
        );

        // Choosing the tool turns its row active.
        crate::tools::activate_tool(engine.world_mut(), Some("demo component"))
            .expect("the demo tool is registered");
        let chosen = EditorSnapshot::capture_engine_tiers(&engine, 2, &[], Vec::new());
        assert_eq!(chosen.component_tools.len(), 1);
        assert!(chosen.component_tools[0].active);

        // An entity that carries nothing is offered nothing.
        let mut selection = EditorSelection::default();
        selection.select_only(plain);
        engine.world_mut().insert_resource(selection);
        let none = EditorSnapshot::capture_engine_tiers(&engine, 3, &[], Vec::new());
        assert!(none.component_tools.is_empty());
    }

    #[test]
    fn apply_changes_real_state_and_reports_failures() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 1.0,
                enabled: false,
            })
            .build()
            .expect("entity builds");
        let component = demo_component_name();

        let failures = EditorCommand::apply(
            &mut engine,
            &[
                EditorCommand::SetField {
                    entity,
                    component: component.clone(),
                    field: "strength".to_string(),
                    value: FieldValue::F32(5.0),
                },
                EditorCommand::SetField {
                    entity,
                    component: component.clone(),
                    field: "strength".to_string(),
                    value: FieldValue::U32(3),
                },
            ],
        );
        // One bad value type; the good write still applied.
        assert_eq!(failures.len(), 1);
        let field_value = engine
            .world()
            .read_component_field(entity, &component, "strength")
            .expect("read");
        assert_eq!(field_value, FieldValue::F32(5.0));
    }

    /// A component with a fixed-size scalar array, for element writes.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct ArrayComponent {
        values: [i16; 3],
    }
    impl pill_engine::Component for ArrayComponent {}

    static ARRAY_COMPONENT_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
        name: "values",
        type_tag: "array:i16",
        offset: 0,
        size: 6,
        align: 2,
        element_count: 3,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];

    fn array_component_name() -> String {
        std::any::type_name::<ArrayComponent>().to_string()
    }

    #[test]
    fn create_entity_command_builds_seeded_components() {
        let mut engine = demo_engine();
        let component = demo_component_name();
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::CreateEntity {
                components: vec![ComponentSeed {
                    type_name: component.clone(),
                    fields: vec![
                        ("strength".to_string(), FieldValue::F32(9.0)),
                        ("enabled".to_string(), FieldValue::Bool(true)),
                    ],
                }],
            }],
        );
        assert!(failures.is_empty(), "create failed: {failures:?}");

        let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
        assert_eq!(snapshot.entities.len(), 1);
        let entity = snapshot.entities[0].entity;
        let detail = EditorSnapshot::capture_detail(&engine, entity).expect("detail");
        assert_eq!(
            detail.components[0].values,
            vec![FieldValue::F32(9.0), FieldValue::Bool(true)]
        );
    }

    #[test]
    fn destroy_entity_command_removes_row_and_queued_edits_error() {
        let mut engine = demo_engine();
        let component = demo_component_name();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 1.0,
                enabled: true,
            })
            .build()
            .expect("entity builds");

        let failures =
            EditorCommand::apply(&mut engine, &[EditorCommand::DestroyEntity { entity }]);
        assert!(failures.is_empty(), "destroy failed: {failures:?}");
        let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
        assert!(snapshot.entities.is_empty());

        // Editing a dead entity surfaces an error instead of panicking.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetField {
                entity,
                component: component.clone(),
                field: "strength".to_string(),
                value: FieldValue::F32(2.0),
            }],
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].1.contains("is not alive"));
    }

    /// The editor's generic field write reaches a component's registered
    /// field layout, which is the path the Inspector uses to edit values live.
    #[test]
    fn set_field_updates_a_registered_layout() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 1.0,
                enabled: true,
            })
            .build()
            .unwrap();
        let component_type_name = demo_component_name();
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetField {
                entity,
                component: component_type_name.clone(),
                field: "strength".into(),
                value: FieldValue::F32(0.8),
            }],
        );
        assert!(failures.is_empty(), "{failures:?}");
        assert!(engine
            .world()
            .read_component_fields(entity, &component_type_name)
            .unwrap()
            .contains(&FieldValue::F32(0.8)));
    }

    #[test]
    fn add_and_remove_component_commands_round_trip() {
        let mut engine = demo_engine();
        let array_component = array_component_name();
        engine
            .world_mut()
            .register_component_with_layout::<ArrayComponent>(ARRAY_COMPONENT_FIELDS);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 0.0,
                enabled: false,
            })
            .build()
            .expect("entity builds");

        // AddComponent zero-initialises a component with a field layout.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::AddComponent {
                entity,
                component: array_component.clone(),
            }],
        );
        assert!(failures.is_empty(), "add failed: {failures:?}");
        let detail = EditorSnapshot::capture_detail(&engine, entity).expect("detail");
        assert_eq!(detail.components.len(), 2);
        let added = detail
            .components
            .iter()
            .find(|view| view.type_name == array_component)
            .expect("added component visible");
        assert_eq!(
            added.values,
            vec![FieldValue::Array(vec![
                FieldValue::I16(0),
                FieldValue::I16(0),
                FieldValue::I16(0),
            ])]
        );

        // Adding the same component twice is refused.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::AddComponent {
                entity,
                component: array_component.clone(),
            }],
        );
        assert_eq!(failures.len(), 1);

        // RemoveComponent detaches it again, leaving the other component.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::RemoveComponent {
                entity,
                component: array_component.clone(),
            }],
        );
        assert!(failures.is_empty(), "remove failed: {failures:?}");
        let detail = EditorSnapshot::capture_detail(&engine, entity).expect("detail");
        assert_eq!(detail.components.len(), 1);
        assert!(detail.components[0].type_name.contains("DemoComponent"));
    }

    #[test]
    fn set_array_element_command_writes_one_element() {
        let mut engine = demo_engine();
        let component = array_component_name();
        engine
            .world_mut()
            .register_component_with_layout::<ArrayComponent>(ARRAY_COMPONENT_FIELDS);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(ArrayComponent { values: [1, 2, 3] })
            .build()
            .expect("entity builds");

        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetArrayElement {
                entity,
                component: component.clone(),
                field: "values".to_string(),
                index: 1,
                value: FieldValue::I16(9),
            }],
        );
        assert!(failures.is_empty(), "array write failed: {failures:?}");
        let field_value = engine
            .world()
            .read_component_field(entity, &component, "values")
            .expect("read");
        assert_eq!(
            field_value,
            FieldValue::Array(vec![
                FieldValue::I16(1),
                FieldValue::I16(9),
                FieldValue::I16(3)
            ])
        );
    }

    #[test]
    fn pause_step_and_system_toggle_commands_drive_the_engine() {
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&runs);
        let mut engine = Engine::new();
        engine.register_system("duplicate", move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        engine.begin_module_registration(SystemOwner::extension(0));
        engine.register_system("duplicate", || {});
        engine.end_module_registration();

        // Pause via command; frames stop running systems.
        let failures =
            EditorCommand::apply(&mut engine, &[EditorCommand::SetPaused { paused: true }]);
        assert!(failures.is_empty());
        engine.process_frame().unwrap();
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 0);

        // One step command runs exactly one phase.
        let failures = EditorCommand::apply(&mut engine, &[EditorCommand::StepOnce]);
        assert!(failures.is_empty());
        engine.process_frame().unwrap();
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        engine.process_frame().unwrap();
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Index-based toggle disables exactly one of the two same-named
        // systems.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetSystemEnabled {
                index: 0,
                enabled: false,
            }],
        );
        assert!(failures.is_empty());
        let after = engine.system_snapshots();
        assert!(!after[0].enabled);
        assert!(after[1].enabled);
    }

    /// Applies a recorded batch, undoes it, and checks a field's value.
    fn round_trip_field(
        engine: &mut Engine,
        entity: Entity,
        component_type_name: &str,
        new_value: FieldValue,
    ) {
        let before = engine
            .world()
            .read_component_field(entity, component_type_name, "strength")
            .expect("read before");
        let command = EditorCommand::SetField {
            entity,
            component: component_type_name.to_string(),
            field: "strength".to_string(),
            value: new_value.clone(),
        };
        let recorded = EditorCommand::apply_recorded(engine, std::slice::from_ref(&command));
        assert!(recorded.failures.is_empty());
        assert!(recorded.fully_invertible);
        assert_eq!(recorded.inverses.len(), 1);

        // Undo restores the previous value; redo applies the edit again.
        let undo = EditorCommand::apply_recorded(engine, &recorded.inverses);
        assert!(undo.failures.is_empty());
        let restored = engine
            .world()
            .read_component_field(entity, component_type_name, "strength")
            .expect("read restored");
        assert_eq!(restored, before);
        let redo = EditorCommand::apply_recorded(engine, std::slice::from_ref(&command));
        assert!(redo.failures.is_empty());
        let reapplied = engine
            .world()
            .read_component_field(entity, component_type_name, "strength")
            .expect("read reapplied");
        assert_eq!(reapplied, new_value);
    }

    #[test]
    fn recorded_apply_inverts_field_edits() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 2.0,
                enabled: true,
            })
            .build()
            .expect("entity builds");
        let component = demo_component_name();
        round_trip_field(&mut engine, entity, &component, FieldValue::F32(7.5));
    }

    #[test]
    fn recorded_apply_inverts_array_element_edits() {
        let mut engine = demo_engine();
        engine
            .world_mut()
            .register_component_with_layout::<ArrayComponent>(ARRAY_COMPONENT_FIELDS);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(ArrayComponent {
                values: [10, 20, 30],
            })
            .build()
            .expect("entity builds");
        let component = array_component_name();

        let edit = EditorCommand::SetArrayElement {
            entity,
            component: component.clone(),
            field: "values".to_string(),
            index: 1,
            value: FieldValue::I16(99),
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&edit));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(recorded.fully_invertible);

        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty());
        let values = engine
            .world()
            .read_component_field(entity, &component, "values")
            .expect("read array");
        assert_eq!(
            values,
            FieldValue::Array(vec![
                FieldValue::I16(10),
                FieldValue::I16(20),
                FieldValue::I16(30)
            ])
        );

        // An element outside the array is never applied, so it is never
        // recorded either.
        let bad = EditorCommand::SetArrayElement {
            entity,
            component,
            field: "values".to_string(),
            index: 9,
            value: FieldValue::I16(1),
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&bad));
        assert_eq!(recorded.failures.len(), 1);
        assert!(!recorded.fully_invertible);
        assert!(recorded.inverses.is_empty());
    }

    #[test]
    fn recorded_apply_inverts_create_and_destroy_with_restore() {
        let mut engine = demo_engine();
        let component = demo_component_name();

        // Create: the inverse destroys the very handle the creation took.
        let create = EditorCommand::CreateEntity {
            components: vec![ComponentSeed {
                type_name: component.clone(),
                fields: vec![
                    ("strength".to_string(), FieldValue::F32(4.5)),
                    ("enabled".to_string(), FieldValue::Bool(true)),
                ],
            }],
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&create));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        let [EditorCommand::DestroyEntity { entity: created }] = recorded.inverses.as_slice()
        else {
            panic!(
                "create must invert to a destroy, got {:?}",
                recorded.inverses
            );
        };
        let created = *created;
        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty());
        assert!(!engine.world().is_entity_valid(created));

        // Destroy: the inverse restores the entity, with its values, under a
        // fresh handle reported as a remap.
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 6.5,
                enabled: false,
            })
            .build()
            .expect("entity builds");
        let destroy = EditorCommand::DestroyEntity { entity };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&destroy));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        let [EditorCommand::RestoreEntity { previous, .. }] = recorded.inverses.as_slice() else {
            panic!(
                "destroy must invert to a restore, got {:?}",
                recorded.inverses
            );
        };
        assert_eq!(*previous, entity);
        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        let [(dead_handle, restored_handle)] = undo.remaps.as_slice() else {
            panic!("restore must report the remap, got {:?}", undo.remaps);
        };
        assert_eq!(*dead_handle, entity);
        let strength = engine
            .world()
            .read_component_field(*restored_handle, &component, "strength")
            .expect("restored value");
        assert_eq!(strength, FieldValue::F32(6.5));

        // The remap rewrites a command naming the dead handle.
        let mut queued_after_destroy = destroy.clone();
        queued_after_destroy.remap_entity(entity, *restored_handle);
        assert_eq!(
            queued_after_destroy,
            EditorCommand::DestroyEntity {
                entity: *restored_handle
            }
        );
    }

    #[test]
    fn recorded_apply_inverts_component_add_and_remove_with_values() {
        let mut engine = demo_engine();
        engine
            .world_mut()
            .register_component_with_layout::<ArrayComponent>(ARRAY_COMPONENT_FIELDS);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 3.0,
                enabled: true,
            })
            .build()
            .expect("entity builds");
        let component = array_component_name();

        // Add inverts to remove.
        let add = EditorCommand::AddComponent {
            entity,
            component: component.clone(),
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&add));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(matches!(
            recorded.inverses.as_slice(),
            [EditorCommand::RemoveComponent { .. }]
        ));
        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty());

        // Remove inverts to a restore that brings the values back.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::AddComponent {
                entity,
                component: component.clone(),
            }],
        );
        assert!(failures.is_empty());
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetArrayElement {
                entity,
                component: component.clone(),
                field: "values".to_string(),
                index: 2,
                value: FieldValue::I16(42),
            }],
        );
        assert!(failures.is_empty());

        let remove = EditorCommand::RemoveComponent {
            entity,
            component: component.clone(),
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&remove));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        let values = engine
            .world()
            .read_component_field(entity, &component, "values")
            .expect("values restored");
        assert_eq!(
            values,
            FieldValue::Array(vec![
                FieldValue::I16(0),
                FieldValue::I16(0),
                FieldValue::I16(42)
            ])
        );
    }

    #[test]
    fn recorded_apply_marks_engine_controls_unrecordable() {
        let mut engine = demo_engine();
        let recorded = EditorCommand::apply_recorded(
            &mut engine,
            &[
                EditorCommand::SetPaused { paused: true },
                EditorCommand::StepOnce,
            ],
        );
        assert!(recorded.failures.is_empty());
        // Controls never enter the history as steps.
        assert!(!recorded.fully_invertible);
        assert!(recorded.inverses.is_empty());
    }

    /// Choosing a tool is a control: the registry's active name moves and no
    /// undo step is recorded.
    #[test]
    fn choosing_a_tool_activates_it_without_recording_a_step() {
        let mut engine = demo_engine();
        let recorded = EditorCommand::apply_recorded(
            &mut engine,
            &[EditorCommand::SetTool {
                tool: Some("rotate".to_string()),
            }],
        );
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(!recorded.fully_invertible);
        assert!(recorded.inverses.is_empty());
        let active = crate::tools::active_tool(&engine);
        assert_eq!(active.as_deref(), Some("rotate"));
    }

    /// A name nobody registered fails the command with the registry's list of
    /// the tools that exist, so a stale row says what is wrong.
    #[test]
    fn choosing_an_unknown_tool_reports_the_registered_names() {
        let mut engine = demo_engine();
        let recorded = EditorCommand::apply_recorded(
            &mut engine,
            &[EditorCommand::SetTool {
                tool: Some("spline".to_string()),
            }],
        );
        assert_eq!(recorded.failures.len(), 1, "one command, one failure");
        let (_, reason) = &recorded.failures[0];
        assert!(
            reason.contains("spline") && reason.contains("translate"),
            "the failure names the missing tool and the ones that exist: {reason}"
        );
        assert_eq!(crate::tools::active_tool(&engine), None);
    }

    #[test]
    fn an_element_selection_holds_one_entity_and_one_kind() {
        let mut selection = EditorSelection::default();
        let first = Entity::from_raw(1, 0);
        let second = Entity::from_raw(2, 0);

        // Two elements of one entity are a legal selection.
        assert!(selection.set_elements(
            "control point".to_string(),
            vec![
                ElementRef {
                    entity: first,
                    id: 0,
                },
                ElementRef {
                    entity: first,
                    id: 2,
                },
            ],
        ));
        assert_eq!(selection.elements().kind(), "control point");
        assert_eq!(selection.elements().owner(), Some(first));
        assert!(selection.elements().contains(first, 2));

        // A set that names two entities is refused and changes nothing.
        let before = selection.elements().clone();
        assert!(!selection.set_elements(
            "control point".to_string(),
            vec![
                ElementRef {
                    entity: first,
                    id: 0,
                },
                ElementRef {
                    entity: second,
                    id: 0,
                },
            ],
        ));
        assert_eq!(
            selection.elements(),
            &before,
            "a refused set leaves the selection as it was"
        );

        selection.clear_elements();
        assert!(selection.elements().is_empty());
        assert_eq!(selection.elements().kind(), "");
    }

    #[test]
    fn an_entity_pick_clears_the_elements_and_its_undo_brings_them_back() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 1.0,
                enabled: true,
            })
            .build()
            .expect("entity builds");

        // A tool had two elements of that entity selected.
        let recorded = EditorCommand::apply_recorded(
            &mut engine,
            &[EditorCommand::SetElementSelection {
                kind: "control point".to_string(),
                elements: vec![ElementRef { entity, id: 0 }, ElementRef { entity, id: 1 }],
            }],
        );
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(
            recorded.fully_invertible,
            "an element pick is one undo step"
        );

        // Picking the entity again replaces the entity slot and drops the
        // elements, which belonged to the selection it replaced.
        let pick = EditorCommand::apply_recorded(
            &mut engine,
            &[EditorCommand::SetSelection {
                entities: vec![entity],
                asset: None,
            }],
        );
        assert!(pick.failures.is_empty(), "{:?}", pick.failures);
        let selection = engine
            .world()
            .get_resource::<EditorSelection>()
            .expect("the pick wrote the selection");
        assert!(
            selection.elements().is_empty(),
            "an entity pick clears the element slot"
        );

        // Its undo puts the elements back, and leaves the entity slot as the
        // pick found it: nothing was selected there before the pick.
        let undo = EditorCommand::apply_recorded(&mut engine, &pick.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        let selection = engine
            .world()
            .get_resource::<EditorSelection>()
            .expect("the undo wrote the selection");
        assert_eq!(selection.elements().elements().len(), 2);
        assert_eq!(selection.elements().kind(), "control point");
        assert_eq!(
            selection.primary(),
            None,
            "the entity slot held nothing before the pick, so the undo clears it"
        );
    }

    #[test]
    fn recorded_delete_group_restores_the_entity_and_the_selection() {
        let mut engine = demo_engine();
        let entity = engine
            .world_mut()
            .create_entity()
            .with(DemoComponent {
                strength: 6.5,
                enabled: true,
            })
            .build()
            .expect("entity builds");
        // The user had this entity (and a file) selected.
        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::SetSelection {
                entities: vec![entity],
                asset: Some("textures/x.png".to_string()),
            }],
        );
        assert!(failures.is_empty(), "{failures:?}");

        // The delete the menu records clears the selection, then destroys.
        let (commands, label) =
            crate::entities::entity_delete(entity, Some("textures/x.png".to_string()));
        assert_eq!(label, "delete entity");
        let recorded = EditorCommand::apply_recorded(&mut engine, &commands);
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(recorded.fully_invertible);
        assert!(!engine.world().is_entity_valid(entity));
        let selection = engine
            .world()
            .get_resource::<EditorSelection>()
            .expect("the delete wrote the selection resource");
        assert_eq!(selection.primary(), None, "the delete clears the selection");

        // The undo restores the entity under a fresh handle, and the batch's
        // own selection command follows it there.
        let undo = EditorCommand::apply_recorded(&mut engine, &recorded.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        let [(previous, restored)] = undo.remaps.as_slice() else {
            panic!(
                "the delete's undo must report the restore's remap, got {:?}",
                undo.remaps
            );
        };
        assert_eq!(*previous, entity);
        let selection = engine
            .world()
            .get_resource::<EditorSelection>()
            .expect("selection resource");
        assert_eq!(
            selection.primary(),
            Some(*restored),
            "the selection must follow the restored entity to its fresh handle"
        );
        assert_eq!(selection.asset.as_deref(), Some("textures/x.png"));
        let strength = engine
            .world()
            .read_component_field(*restored, &demo_component_name(), "strength")
            .expect("restored value");
        assert_eq!(strength, FieldValue::F32(6.5));
    }

    /// A note file as an asset, for the asset reference tests.
    #[derive(Debug)]
    struct Note;
    impl pill_engine::asset::Asset for Note {}
    trait_type_map::impl_trait_accessible!(dyn pill_engine::asset::Asset; Note);

    impl pill_engine::asset_metadata::ImportedAsset for Note {
        type ImportSettings = NoteSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["note"];

        fn metadata_type_name() -> &'static str {
            "pill_editor_core::tests::Note"
        }

        fn import(
            _name: &str,
            _bytes: &[u8],
            _settings: &NoteSettings,
        ) -> pill_engine::AssetLoadResult<Self> {
            Ok(Self)
        }
    }

    /// Another imported type, so a wrong-type pick has something to name.
    #[derive(Debug)]
    struct Card;
    impl pill_engine::asset::Asset for Card {}
    trait_type_map::impl_trait_accessible!(dyn pill_engine::asset::Asset; Card);

    impl pill_engine::asset_metadata::ImportedAsset for Card {
        type ImportSettings = NoteSettings;
        const SOURCE_EXTENSIONS: &'static [&'static str] = &["card"];

        fn metadata_type_name() -> &'static str {
            "pill_editor_core::tests::Card"
        }

        fn import(
            _name: &str,
            _bytes: &[u8],
            _settings: &NoteSettings,
        ) -> pill_engine::AssetLoadResult<Self> {
            Ok(Self)
        }
    }

    /// The test assets' (empty) import settings.
    #[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
    struct NoteSettings {}

    /// A component with one note reference, laid out as the derive does.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct NoteHolder {
        note: pill_engine::Handle<Note>,
    }
    impl pill_engine::Component for NoteHolder {}

    static NOTE_HOLDER_FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
        name: "note",
        type_tag: "struct:Handle<Note>",
        offset: 0,
        size: 8,
        align: 4,
        element_count: 0,
        decorators: std::borrow::Cow::Borrowed(&[]),
    }];

    /// One `.meta` sidecar's text: the header naming `asset_type` and `guid`.
    fn sidecar(asset_type: &str, guid: &str) -> String {
        let document = serde_json::json!({
            "format_version": 1,
            "asset_type": asset_type,
            "guid": guid,
            "settings": {},
        });
        String::from_utf8(
            pill_engine::asset_document::render(&document).expect("the sidecar renders"),
        )
        .expect("the sidecar is UTF-8")
    }

    /// Builds an engine with both test asset types registered and a mounted
    /// scratch `res` that holds `a.note`, `b.note` and `c.card`, each with a
    /// `.meta`. The asset database is built over that folder, and one entity's
    /// note field references nothing.
    fn reference_engine(scenario_name: &str) -> (Engine, Entity, String, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "pill-editor-asset-reference-{scenario_name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (file, guid, asset_type) in [
            (
                "a.note",
                "000000000000000000000000000000a1",
                "pill_editor_core::tests::Note",
            ),
            (
                "b.note",
                "000000000000000000000000000000b1",
                "pill_editor_core::tests::Note",
            ),
            (
                "c.card",
                "000000000000000000000000000000c1",
                "pill_editor_core::tests::Card",
            ),
        ] {
            std::fs::write(root.join(file), "x").unwrap();
            std::fs::write(root.join(format!("{file}.meta")), sidecar(asset_type, guid)).unwrap();
        }
        pill_engine::asset_store::mount_directory(&root);
        let mut engine = Engine::new();
        let world = engine.world_mut();
        world.register_imported_asset::<Note>();
        world.register_imported_asset::<Card>();
        world.register_component_with_layout::<NoteHolder>(NOTE_HOLDER_FIELDS);
        world.record_asset_field_type(
            pill_engine::ComponentId::of::<NoteHolder>(),
            "note",
            "pill_editor_core::tests::Note",
        );
        let registry = world
            .get_resource::<pill_engine::ImportRegistry>()
            .unwrap()
            .clone();
        world.insert_resource(pill_engine::AssetDatabase::from_directory(&root, &registry));
        let entity = world
            .create_entity()
            .with(NoteHolder {
                note: pill_engine::Handle::INVALID,
            })
            .build()
            .unwrap();
        let component_type_name =
            pill_engine::component::ComponentRegistry::registered_name::<NoteHolder>();
        (engine, entity, component_type_name, root)
    }

    /// Returns the note field's label as the Inspector would show it.
    fn note_label(engine: &Engine, entity: Entity) -> AssetReferenceLabel {
        let detail = EditorSnapshot::capture_detail(engine, entity).unwrap();
        let view = detail
            .components
            .iter()
            .find(|view| view.type_name.contains("NoteHolder"))
            .unwrap();
        view.asset_labels[0]
            .clone()
            .expect("an asset field has a label")
    }

    /// One `SetFieldAsset` for the note field.
    fn pick(entity: Entity, component_type_name: &str, reference: Option<&str>) -> EditorCommand {
        EditorCommand::SetFieldAsset {
            entity,
            component: component_type_name.to_owned(),
            field: "note".to_owned(),
            reference: reference.map(str::to_owned),
        }
    }

    /// Picking an unloaded asset by path loads it and points the field at it.
    /// A guid names an asset just as well as its path, the recorded inverse
    /// restores the previous reference, and `None` clears the field.
    #[test]
    fn asset_picks_load_assign_undo_clear_and_refuse() {
        let _mount = crate::test_support::asset_mount_lock();
        let (mut engine, entity, component_type_name, root) = reference_engine("pick");
        assert_eq!(note_label(&engine, entity), AssetReferenceLabel::Unset);

        let applied = EditorCommand::apply_recorded(
            &mut engine,
            &[pick(entity, &component_type_name, Some("a.note"))],
        );
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(
            note_label(&engine, entity),
            AssetReferenceLabel::Named("a.note".to_owned())
        );

        let by_guid = EditorCommand::apply_recorded(
            &mut engine,
            &[pick(
                entity,
                &component_type_name,
                Some("000000000000000000000000000000b1"),
            )],
        );
        assert!(by_guid.failures.is_empty(), "{:?}", by_guid.failures);
        assert_eq!(
            note_label(&engine, entity),
            AssetReferenceLabel::Named("b.note".to_owned())
        );

        // Undo: the inverse is a plain field write of the previous handle.
        let undo = EditorCommand::apply_recorded(&mut engine, &by_guid.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        assert_eq!(
            note_label(&engine, entity),
            AssetReferenceLabel::Named("a.note".to_owned())
        );

        // Another asset type, a path the project lacks and an unknown guid
        // are refused, and the field keeps what it had.
        for reference in ["c.card", "missing.note", "000000000000000000000000000000ff"] {
            let refused = EditorCommand::apply_recorded(
                &mut engine,
                &[pick(entity, &component_type_name, Some(reference))],
            );
            assert_eq!(refused.failures.len(), 1, "{reference} is refused");
            assert_eq!(
                note_label(&engine, entity),
                AssetReferenceLabel::Named("a.note".to_owned())
            );
        }

        let cleared =
            EditorCommand::apply_recorded(&mut engine, &[pick(entity, &component_type_name, None)]);
        assert!(cleared.failures.is_empty(), "{:?}", cleared.failures);
        assert_eq!(note_label(&engine, entity), AssetReferenceLabel::Unset);
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A pick may name a guid the store holds with no file behind it: an asset
    /// built in code then resolves to its own handle instead of failing as an
    /// unknown project guid. A guid nothing holds is still refused by name.
    #[test]
    fn a_pick_takes_a_guid_the_store_holds_without_a_file() {
        let mut engine = Engine::new();
        pill_ui_theme::register(engine.world_mut());
        let guid = pill_engine::AssetGuid::random().expect("a fresh guid");
        {
            let world = engine.world_mut();
            let assets = world
                .get_resource_mut::<pill_engine::AssetManager>()
                .expect("the engine owns an asset store");
            assets
                .add_named_with_guid(
                    "built.theme",
                    guid,
                    pill_ui_theme::ThemeOverrides::default(),
                )
                .expect("the name is free");
        }
        let field_value = resolve_asset_reference(&mut engine, &guid.to_string())
            .expect("a stored guid resolves");
        match field_value {
            FieldValue::Asset {
                asset_type,
                index,
                generation,
            } => {
                assert_eq!(asset_type.as_deref(), Some("pill_ui_theme::ThemeOverrides"));
                let handle = engine
                    .world()
                    .get_resource::<pill_engine::AssetManager>()
                    .and_then(|assets| {
                        assets.handle_by_name::<pill_ui_theme::ThemeOverrides>("built.theme")
                    })
                    .expect("the asset stayed loaded");
                assert_eq!((index, generation), (handle.index(), handle.generation()));
            }
            other => panic!("expected an asset value, got {other:?}"),
        }

        let unknown = pill_engine::AssetGuid::random().expect("a fresh guid");
        let error = resolve_asset_reference(&mut engine, &unknown.to_string()).unwrap_err();
        assert!(error.contains(&unknown.to_string()), "{error}");
    }

    /// A pick may name a store asset by the name it was added under, which is
    /// the only identity an asset built in code without a guid has.
    #[test]
    fn a_pick_takes_the_name_the_store_holds_an_asset_under() {
        let mut engine = Engine::new();
        pill_ui_theme::register(engine.world_mut());
        {
            let world = engine.world_mut();
            let assets = world
                .get_resource_mut::<pill_engine::AssetManager>()
                .expect("the engine owns an asset store");
            assets
                .add_named(
                    "pill.theme.default",
                    pill_ui_theme::ThemeOverrides::default(),
                )
                .expect("the name is free");
        }
        let field_value = resolve_asset_reference(&mut engine, "pill.theme.default")
            .expect("a stored name resolves");
        match field_value {
            FieldValue::Asset {
                asset_type,
                index,
                generation,
            } => {
                assert_eq!(asset_type.as_deref(), Some("pill_ui_theme::ThemeOverrides"));
                let handle = engine
                    .world()
                    .get_resource::<pill_engine::AssetManager>()
                    .and_then(|assets| {
                        assets.handle_by_name::<pill_ui_theme::ThemeOverrides>("pill.theme.default")
                    })
                    .expect("the asset stayed loaded");
                assert_eq!((index, generation), (handle.index(), handle.generation()));
            }
            other => panic!("expected an asset value, got {other:?}"),
        }
    }

    /// A component holding a reference to another entity, as the derive lays
    /// one out.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    struct TargetHolder {
        target: Entity,
        weight: f32,
    }
    impl pill_engine::Component for TargetHolder {}

    static TARGET_HOLDER_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "target",
            type_tag: "entity",
            offset: 0,
            size: std::mem::size_of::<Entity>(),
            align: 8,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "weight",
            type_tag: "f32",
            offset: 16,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    /// An engine with the holder registered, one holder (referencing nothing)
    /// and two entities it can be pointed at.
    fn holder_engine() -> (Engine, Entity, Entity, Entity) {
        let mut engine = Engine::new();
        let world = engine.world_mut();
        world.register_component_with_layout::<TargetHolder>(TARGET_HOLDER_FIELDS);
        let first = world.create_entity().build().unwrap();
        let second = world.create_entity().build().unwrap();
        let holder = world
            .create_entity()
            .with(TargetHolder {
                target: Entity::NULL,
                weight: 0.5,
            })
            .build()
            .unwrap();
        (engine, holder, first, second)
    }

    /// Returns the holder's reference label as the Inspector would show it.
    fn target_label(engine: &Engine, entity: Entity) -> EntityReferenceLabel {
        let detail = EditorSnapshot::capture_detail(engine, entity).unwrap();
        let view = detail
            .components
            .iter()
            .find(|view| view.type_name.contains("TargetHolder"))
            .unwrap();
        view.entity_labels[0]
            .clone()
            .expect("an entity field has a label")
    }

    /// One `SetFieldEntity` for the target field.
    fn retarget(
        entity: Entity,
        component_type_name: &str,
        reference: Option<Entity>,
    ) -> EditorCommand {
        EditorCommand::SetFieldEntity {
            entity,
            component: component_type_name.to_owned(),
            field: "target".to_owned(),
            reference,
        }
    }

    /// Arming a row records the field a scene press names. The request the
    /// runner takes is what the press turns into one recorded step, the same
    /// `entity_edit` the row's own picker and drop queue.
    #[test]
    fn an_armed_row_answers_the_scene_press_with_one_recorded_step() {
        let (mut engine, holder, first, second) = holder_engine();
        crate::commands::ensure_command_resources(engine.world_mut());
        let component_type_name =
            pill_engine::component::ComponentRegistry::registered_name::<TargetHolder>();
        let armed = EditorCommand::ArmEntityPick {
            entity: holder,
            component: component_type_name.clone(),
            field: "target".to_owned(),
        };
        // Arming is not an edit, so it neither records a step nor dirties the
        // document. Clicking a reference row's target button therefore does not
        // make the title claim unsaved changes.
        assert!(!armed.recordable(), "arming an entity pick is not an edit");
        let outcome = EditorCommand::apply_recorded(&mut engine, &[armed]);
        assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
        assert!(
            !engine
                .world_mut()
                .get_resource_mut::<crate::scene_workflow::SceneSession>()
                .map(|session| session.dirty)
                .unwrap_or(false),
            "arming an entity pick marked the scene dirty"
        );
        let target = engine
            .world_mut()
            .get_resource_mut::<EntityPickRequest>()
            .and_then(|request| request.take())
            .expect("the arm left a target for the press");
        assert_eq!(target.entity, holder);
        assert_eq!(target.field, "target");
        assert!(
            engine
                .world_mut()
                .get_resource_mut::<EntityPickRequest>()
                .and_then(|request| request.take())
                .is_none(),
            "the press takes the request once"
        );

        // The runner turns the answer into one recorded step, which undoes back
        // to the reference the field held.
        let (edit, label) = crate::inspector::entity_edit(
            target.entity,
            &target.component,
            &target.field,
            Some(second),
        );
        assert_eq!(label, "set TargetHolder.target");
        let applied = EditorCommand::apply_recorded(&mut engine, &[edit]);
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(
            target_label(&engine, holder),
            EntityReferenceLabel::Live(EntityListEntry::display_of(second))
        );
        let undone = EditorCommand::apply_recorded(&mut engine, &applied.inverses);
        assert!(undone.failures.is_empty(), "{:?}", undone.failures);
        assert_eq!(target_label(&engine, holder), EntityReferenceLabel::Unset);
        assert_ne!(first, second);
    }

    /// A reference reads as unset, names its entity by the Hierarchy label,
    /// writes, undoes and clears. A dangling reference reads as missing, and a
    /// reference to an entity that is gone is refused rather than written.
    #[test]
    fn entity_references_read_write_undo_clear_and_go_missing() {
        let (mut engine, holder, first, second) = holder_engine();
        let component_type_name =
            pill_engine::component::ComponentRegistry::registered_name::<TargetHolder>();
        assert_eq!(target_label(&engine, holder), EntityReferenceLabel::Unset);

        let applied = EditorCommand::apply_recorded(
            &mut engine,
            &[retarget(holder, &component_type_name, Some(first))],
        );
        assert!(applied.failures.is_empty(), "{:?}", applied.failures);
        assert_eq!(
            target_label(&engine, holder),
            EntityReferenceLabel::Live(EntityListEntry::display_of(first))
        );
        assert_eq!(
            engine
                .world()
                .get_component::<TargetHolder>(holder)
                .unwrap()
                .target,
            first
        );

        let rewritten = EditorCommand::apply_recorded(
            &mut engine,
            &[retarget(holder, &component_type_name, Some(second))],
        );
        assert!(rewritten.failures.is_empty(), "{:?}", rewritten.failures);
        assert_eq!(
            target_label(&engine, holder),
            EntityReferenceLabel::Live(EntityListEntry::display_of(second))
        );

        // Undo: the inverse is a plain field write of the previous handle.
        let undo = EditorCommand::apply_recorded(&mut engine, &rewritten.inverses);
        assert!(undo.failures.is_empty(), "{:?}", undo.failures);
        assert_eq!(
            target_label(&engine, holder),
            EntityReferenceLabel::Live(EntityListEntry::display_of(first))
        );

        // The target dies: the row shows the reference as missing, because the
        // field still holds the handle it was given.
        let _ = engine.world_mut().destroy_entity(second);
        let pointed_at_dead = EditorCommand::apply_recorded(
            &mut engine,
            &[retarget(holder, &component_type_name, Some(second))],
        );
        assert_eq!(
            pointed_at_dead.failures.len(),
            1,
            "a dead handle is refused"
        );

        let _ = engine.world_mut().destroy_entity(first);
        assert_eq!(target_label(&engine, holder), EntityReferenceLabel::Missing);

        let cleared = EditorCommand::apply_recorded(
            &mut engine,
            &[retarget(holder, &component_type_name, None)],
        );
        assert!(cleared.failures.is_empty(), "{:?}", cleared.failures);
        assert_eq!(target_label(&engine, holder), EntityReferenceLabel::Unset);
    }

    /// A standalone type that refuses every document, so a create writes its
    /// file and then fails to import it.
    #[derive(Debug)]
    struct Refused;
    impl pill_engine::asset::Asset for Refused {}
    trait_type_map::impl_trait_accessible!(dyn pill_engine::asset::Asset; Refused);

    impl pill_engine::asset_standalone::StandaloneAsset for Refused {
        type Document = RefusedDocument;
        const FILE_EXTENSION: &'static str = "refused";

        fn metadata_type_name() -> &'static str {
            "pill_editor_core::tests::Refused"
        }

        fn from_document(
            _name: &str,
            _document: RefusedDocument,
            _assets: &pill_engine::AssetManager,
        ) -> pill_engine::AssetLoadResult<Self> {
            Err(pill_engine::AssetLoadError::Decode {
                label: "refused".to_owned(),
                detail: "the test type refuses every document".to_owned(),
            })
        }
    }

    /// The refusing type's (empty) document.
    #[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
    struct RefusedDocument {}

    /// A rollback that cannot take a file back names it, in the command's
    /// message and in the log. A failure that changed the disk has to say so.
    #[test]
    fn a_rollback_that_cannot_finish_names_the_file_it_left() {
        let path = std::env::temp_dir().join(format!(
            "pill-editor-rollback-left-{}.tmp",
            std::process::id()
        ));
        std::fs::write(&path, b"kept").unwrap();
        let path_text = path.display().to_string();

        let left_behind_paths = roll_back_files_with(std::slice::from_ref(&path_text), &mut |_| {
            Err(std::io::Error::other("a handle holds it"))
        });
        assert_eq!(
            left_behind_paths,
            vec![path_text.clone()],
            "the file is still there"
        );
        let message = with_rollback_paths("import failed".to_string(), &left_behind_paths);
        assert!(message.starts_with("import failed"), "{message}");
        assert!(
            message.contains(&path_text),
            "the message must name what the rollback left: {message}"
        );

        // A rollback that finishes adds nothing to the message and leaves no
        // file behind.
        let cleaned = roll_back_files(std::slice::from_ref(&path_text));
        assert!(cleaned.is_empty());
        assert!(!path.exists());
        assert_eq!(
            with_rollback_paths("import failed".to_string(), &cleaned),
            "import failed"
        );
    }

    /// A create whose import refuses the file it just wrote leaves no file,
    /// and a delete that cannot remove the sidecar puts the asset back.
    #[test]
    fn a_failed_create_or_delete_leaves_the_directory_as_it_found_it() {
        let _mount = crate::test_support::asset_mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-editor-asset-rollback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        pill_engine::asset_store::mount_directory(&root);

        let mut engine = Engine::new();
        engine.world_mut().register_standalone_asset::<Refused>();

        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::CreateStandaloneAsset {
                type_name: "pill_editor_core::tests::Refused".to_owned(),
                folder: String::new(),
                name: "broken".to_owned(),
                overwrite: None,
            }],
        );
        assert_eq!(failures.len(), 1, "the refusing import fails the create");
        assert!(
            failures[0].1.contains("refuses every document"),
            "{:?}",
            failures[0].1
        );
        assert!(
            !root.join("broken.refused").exists(),
            "a create whose import refused the file must not leave it behind"
        );

        // A delete removes the asset file and then its sidecar. The one state
        // where the second step fails after the first one succeeded is a sidecar
        // that another handle holds open without delete sharing. A platform that
        // cannot set that up has no way to reach the rollback.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_SHARE_READ: u32 = 0x0000_0001;
            let asset = root.join("kept.refused");
            std::fs::write(&asset, "{\"asset_type\": \"refused\"}").unwrap();
            let meta = root.join("kept.refused.meta");
            std::fs::write(
                &meta,
                sidecar(
                    "pill_editor_core::tests::Refused",
                    "000000000000000000000000000000d1",
                ),
            )
            .unwrap();
            let held_sidecar = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&meta)
                .unwrap();
            assert!(
                std::fs::remove_file(&meta).is_err(),
                "the sidecar must be unremovable for this test"
            );

            let failures = EditorCommand::apply(
                &mut engine,
                &[EditorCommand::DeleteAssetFile {
                    path: "kept.refused".to_owned(),
                }],
            );
            assert_eq!(
                failures.len(),
                1,
                "the unremovable sidecar fails the delete"
            );
            assert!(
                asset.exists(),
                "a delete that could not finish must put the asset back: {}",
                failures[0].1
            );
            drop(held_sidecar);
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A created asset that an undo deleted comes back when the step is redone.
    /// The delete unloads the asset together with its file, so the redo's create
    /// can import the fresh file instead of colliding with a guid the store
    /// still holds.
    #[test]
    fn a_redone_create_writes_the_file_again() {
        let _mount = crate::test_support::asset_mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-editor-asset-redo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        pill_engine::asset_store::mount_directory(&root);

        let mut engine = Engine::new();
        pill_ui_theme::register(engine.world_mut());
        engine
            .world_mut()
            .insert_resource(crate::history::CommandHistory::default());
        engine
            .world_mut()
            .insert_resource(crate::commands::PendingEditorCommands::default());

        // What the store holds for a path: whether it is loaded, and under
        // which guid.
        let asset_path = std::path::Path::new("probe.theme");
        let store_state = |engine: &Engine| {
            let registry = engine
                .world()
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .expect("the theme registration registers its types");
            let assets = engine
                .world()
                .get_resource::<pill_engine::AssetManager>()
                .expect("the engine has an AssetManager");
            (
                registry.is_loaded(assets, asset_path),
                registry.guid_by_path(assets, asset_path),
            )
        };

        let create = EditorCommand::CreateStandaloneAsset {
            type_name: "pill_ui_theme::ThemeOverrides".to_owned(),
            folder: String::new(),
            name: "probe".to_owned(),
            overwrite: None,
        };
        let recorded = EditorCommand::apply_recorded(&mut engine, std::slice::from_ref(&create));
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        let path = root.join("probe.theme");
        assert!(path.exists(), "the create writes the file");
        {
            let world = engine.world_mut();
            let history = world
                .get_resource_mut::<crate::history::CommandHistory>()
                .unwrap();
            history.push(
                crate::history::HistoryEntry::engine(
                    "create asset probe".to_owned(),
                    vec![create],
                    recorded.inverses,
                ),
                0,
                crate::history::CommandHistory::DEFAULT_COALESCE_WINDOW,
            );
        }

        let undo = crate::drain::step_history(&mut engine, crate::drain::HistoryDirection::Undo);
        assert!(
            matches!(undo, crate::drain::HistoryOutcome::Queued { .. }),
            "{undo:?}"
        );
        crate::drain::apply_pending(&mut engine, 1);
        assert!(!path.exists(), "the undo deletes the file");
        assert_eq!(
            store_state(&engine),
            (false, None),
            "the undo's delete unloads the asset with its file"
        );

        let redo = crate::drain::step_history(&mut engine, crate::drain::HistoryDirection::Redo);
        assert!(
            matches!(redo, crate::drain::HistoryOutcome::Queued { .. }),
            "{redo:?}"
        );
        let replay = {
            let world = engine.world_mut();
            let pending = world
                .get_resource_mut::<crate::commands::PendingEditorCommands>()
                .unwrap();
            std::mem::take(&mut pending.batches)
        };
        assert_eq!(replay.len(), 1, "the redo queues one batch");
        let recorded = EditorCommand::apply_recorded(&mut engine, &replay[0].commands);
        assert!(recorded.failures.is_empty(), "{:?}", recorded.failures);
        assert!(path.exists(), "the redo writes the file again");
        assert!(
            store_state(&engine).0,
            "the redo's create loads the asset again"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A replace keeps the identity the store already holds at the path, even
    /// when the file's own header disagrees with it. The value reloads, because
    /// the handle stays what it was.
    #[test]
    fn a_replace_keeps_the_guid_the_store_holds_and_reloads_the_value() {
        let _mount = crate::test_support::asset_mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-editor-asset-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        pill_engine::asset_store::mount_directory(&root);

        let mut engine = Engine::new();
        pill_ui_theme::register(engine.world_mut());

        let create = |overwrite: Option<bool>| EditorCommand::CreateStandaloneAsset {
            type_name: "pill_ui_theme::ThemeOverrides".to_owned(),
            folder: String::new(),
            name: "probe".to_owned(),
            overwrite,
        };
        let path = root.join("probe.theme");
        let failures = EditorCommand::apply(&mut engine, &[create(None)]);
        assert!(failures.is_empty(), "{:?}", failures);

        // The store's identity, and the version of the value it holds.
        let (loaded_guid, loaded_version) = {
            let world = engine.world();
            let registry = world
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .unwrap();
            let assets = world.get_resource::<pill_engine::AssetManager>().unwrap();
            let guid = registry
                .guid_by_path(assets, std::path::Path::new("probe.theme"))
                .expect("the create loaded the file");
            let version = assets
                .handle_by_name::<pill_ui_theme::ThemeOverrides>("probe.theme")
                .and_then(|handle| assets.content_version(handle));
            (guid, version)
        };

        // A file whose header carries another guid. The store cannot import it
        // at that path, so the replace has to keep the guid the store knows
        // instead of the one the file claims.
        let other_guid = pill_engine::AssetGuid::random().unwrap();
        std::fs::write(&path, theme_document(other_guid)).unwrap();

        let failures = EditorCommand::apply(&mut engine, &[create(Some(true))]);
        assert!(failures.is_empty(), "{:?}", failures);
        assert_eq!(
            document_guid(&std::fs::read(&path).unwrap()),
            Some(loaded_guid),
            "the replace keeps the guid the store holds"
        );
        let (after_guid, after_version) = {
            let world = engine.world();
            let registry = world
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .unwrap();
            let assets = world.get_resource::<pill_engine::AssetManager>().unwrap();
            let guid = registry.guid_by_path(assets, std::path::Path::new("probe.theme"));
            let version = assets
                .handle_by_name::<pill_ui_theme::ThemeOverrides>("probe.theme")
                .and_then(|handle| assets.content_version(handle));
            (guid, version)
        };
        assert_eq!(after_guid, Some(loaded_guid), "the identity survives");
        assert_ne!(after_version, loaded_version, "the value reloaded");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path the store still holds with no file behind it binds the fresh guid
    /// a create writes. The stale entry goes instead of refusing the write the
    /// person asked for.
    #[test]
    fn a_create_over_a_stale_loaded_path_binds_the_fresh_guid() {
        let _mount = crate::test_support::asset_mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-editor-asset-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        pill_engine::asset_store::mount_directory(&root);

        let mut engine = Engine::new();
        pill_ui_theme::register(engine.world_mut());
        let create = |overwrite: Option<bool>| EditorCommand::CreateStandaloneAsset {
            type_name: "pill_ui_theme::ThemeOverrides".to_owned(),
            folder: String::new(),
            name: "ghost".to_owned(),
            overwrite,
        };
        let path = root.join("ghost.theme");
        let failures = EditorCommand::apply(&mut engine, &[create(None)]);
        assert!(failures.is_empty(), "{:?}", failures);
        let stale_guid = document_guid(&std::fs::read(&path).unwrap()).unwrap();

        // The file is gone while the store keeps the path loaded, because
        // nothing tells the editor the asset left.
        std::fs::remove_file(&path).unwrap();

        let failures = EditorCommand::apply(&mut engine, &[create(None)]);
        assert!(failures.is_empty(), "{:?}", failures);
        let fresh_guid = document_guid(&std::fs::read(&path).unwrap()).unwrap();
        assert_ne!(fresh_guid, stale_guid, "the create binds a fresh guid");
        let bound = {
            let world = engine.world();
            let registry = world
                .get_resource::<pill_engine::ImportRegistry>()
                .cloned()
                .unwrap();
            let assets = world.get_resource::<pill_engine::AssetManager>().unwrap();
            registry.guid_by_path(assets, std::path::Path::new("ghost.theme"))
        };
        assert_eq!(bound, Some(fresh_guid), "the store holds the new identity");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A replace whose import refuses the file puts the bytes it replaced
    /// back, rather than deleting what was there.
    #[test]
    fn a_refused_replace_restores_the_previous_file() {
        let _mount = crate::test_support::asset_mount_lock();
        let root =
            std::env::temp_dir().join(format!("pill-editor-asset-restore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        pill_engine::asset_store::mount_directory(&root);

        let mut engine = Engine::new();
        engine.world_mut().register_standalone_asset::<Refused>();
        let path = root.join("kept.refused");
        let previous_document = serde_json::json!({
            "format_version": 1,
            "asset_type": "pill_editor_core::tests::Refused",
            "guid": "000000000000000000000000000000e1",
            "asset": {},
        });
        let previous_bytes =
            pill_engine::asset_document::render(&previous_document).expect("the document renders");
        std::fs::write(&path, &previous_bytes).unwrap();

        let failures = EditorCommand::apply(
            &mut engine,
            &[EditorCommand::CreateStandaloneAsset {
                type_name: "pill_editor_core::tests::Refused".to_owned(),
                folder: String::new(),
                name: "kept".to_owned(),
                overwrite: Some(true),
            }],
        );
        assert_eq!(failures.len(), 1, "the refusing import fails the replace");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            previous_bytes,
            "a replace that could not load leaves the file it replaced"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A theme overrides document with `guid` in its header, which is what a
    /// standalone asset's own file holds.
    fn theme_document(guid: pill_engine::AssetGuid) -> Vec<u8> {
        let document = serde_json::json!({
            "format_version": pill_engine::asset_metadata::METADATA_FORMAT_VERSION,
            "asset_type": "pill_ui_theme::ThemeOverrides",
            "guid": guid.to_string(),
            "asset": {},
        });
        pill_engine::asset_document::render(&document).expect("the document renders")
    }
}
