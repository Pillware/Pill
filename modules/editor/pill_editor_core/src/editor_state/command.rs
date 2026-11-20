//! The editor's command vocabulary and the metadata the history reads.
//!
//! # Responsibilities
//!
//! - List every mutation the editor can queue ([`EditorCommand`]). This is the
//!   one owned type the shell queues and the frame boundary applies.
//! - Answer what the history asks of a command: whether it records, which
//!   component and field identifiers it addresses, and which entity handle it
//!   names ([`EditorCommand::recordable`],
//!   [`EditorCommand::touched_identifiers`], [`EditorCommand::remap_entity`]).
//! - Sort each command into the family whose applier module applies it
//!   ([`CommandFamily`], [`EditorCommand::family`]).
//!
//! # Design
//!
//! The command enum stays in one piece because it is the format of the queue
//! the shell fills and the executable drains, so its values cross the reload
//! boundary between the two. Only the applier is split, one module per
//! [`CommandFamily`].

// External crates
use pill_engine::Entity;

// Current crate
use super::*;

// =============================================================================
// Types
// =============================================================================

/// One mutation the editor can queue, applied at the frame boundary.
///
/// The values are plain data: the shell queues them into a world resource and
/// the executable applies them. The methods below answer what the recording
/// layer and the applier ask of a command.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorCommand {
    /// Create an entity with the given components.
    ///
    /// An empty `components` list creates an entity with no components.
    CreateEntity {
        /// Components to attach; each must have a registered field layout.
        components: Vec<ComponentSeed>,
    },
    /// Destroy one entity.
    DestroyEntity {
        /// The entity to destroy.
        entity: Entity,
    },
    /// Add a zero-initialised component to a live entity.
    AddComponent {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
    },
    /// Remove a component from a live entity. Removing the entity's last
    /// component destroys the entity.
    RemoveComponent {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
    },
    /// Re-create a destroyed entity from the component values captured when
    /// it was destroyed.
    ///
    /// The world assigns a fresh handle. The recording layer reports the
    /// `(previous, assigned)` pair so the history can move its older entries
    /// to the new handle. A component without a field layout cannot be
    /// rebuilt, so it is left out and the entity comes back without it.
    RestoreEntity {
        /// The handle the entity had before it was destroyed.
        previous: Entity,
        /// The components to put back, with their type names and values.
        components: Vec<ComponentSeed>,
    },
    /// Re-attach one component with captured field values, the inverse of
    /// removing it.
    RestoreComponent {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Field values to write over the zero-initialised component.
        fields: Vec<(String, FieldValue)>,
    },
    /// Write one field of a component.
    SetField {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Field name.
        field: String,
        /// New value.
        value: FieldValue,
    },
    /// Point an asset reference field at the asset `reference` names, or clear
    /// the field with `None`.
    ///
    /// `reference` is a path relative to `res`, a 32-digit guid, or the name an
    /// asset was added to the store under. The apply resolves it at the frame
    /// boundary and loads the asset first when it is not loaded yet. The write
    /// then behaves like [`Self::SetField`] and records the same kind of
    /// inverse.
    SetFieldAsset {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Field name.
        field: String,
        /// The asset's path, guid or store name, or `None` to clear the
        /// reference.
        reference: Option<String>,
    },
    /// Point an entity reference field at `reference`, or clear the field with
    /// `None`.
    ///
    /// The handle is checked against the world when the command is applied,
    /// so a reference to an entity that is gone by then is refused instead of
    /// written. A write records the [`Self::SetField`] that puts back what the
    /// field held.
    SetFieldEntity {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Field name.
        field: String,
        /// The entity to reference, or `None` to clear the reference.
        reference: Option<Entity>,
    },
    /// Wait for the next scene click to name an entity reference field.
    ///
    /// The Inspector row's target button arms it, and the scene click answers
    /// it. Neither step is an edit, so nothing is recorded until the click
    /// names an entity.
    ArmEntityPick {
        /// The entity the field belongs to.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Field name.
        field: String,
    },
    /// Write one element of a fixed-size array field.
    SetArrayElement {
        /// The target entity.
        entity: Entity,
        /// Registered component type name.
        component: String,
        /// Array field name.
        field: String,
        /// Element index.
        index: usize,
        /// New element value.
        value: FieldValue,
    },
    /// Enable or disable a system by registration index.
    SetSystemEnabled {
        /// Registration index from the systems snapshot.
        index: usize,
        /// The state the system should be in.
        enabled: bool,
    },
    /// Pause or resume the engine's system phase.
    SetPaused {
        /// `true` pauses the system phase and `false` resumes it.
        paused: bool,
    },
    /// Set the transform gizmo's tool, its space or its snapping. A field left
    /// as `None` keeps its current value.
    SetGizmoTool {
        /// The mode to select.
        mode: Option<crate::gizmo::GizmoMode>,
        /// The space to select.
        space: Option<crate::gizmo::GizmoSpace>,
        /// The snapping toggle's new state.
        snap: Option<bool>,
    },
    /// Turn the scene grid on or off. The grid is the infinite ground plane
    /// that the editor's view-bound surfaces draw.
    SetGrid {
        /// The grid's new state.
        enabled: bool,
    },
    /// Turn the engine's component gizmos on or off. Every component that
    /// declares a gizmo stops drawing while this is off, which is the toolbox
    /// row's switch.
    SetGizmos {
        /// The gizmos' new state.
        enabled: bool,
    },
    /// Make one registered tool the active one, or clear the active tool with
    /// `None`.
    ///
    /// This is how a component tool is entered from the UI: its toolbox row
    /// queues this command. The built-in tool rows reach the registry through
    /// `SetGizmoTool`, which also owns the gizmo's mode; a component tool has
    /// no mode of its own, so its activation is this command alone.
    SetTool {
        /// The tool's registered name, as a command and the scripted channel
        /// spell it.
        tool: Option<String>,
    },
    /// Show or hide one component type's gizmo, which is a row in the
    /// Preferences tab. The choice is written to the preferences file, so it
    /// survives a reload.
    SetGizmoTypeVisible {
        /// The component's registered type name, as the gizmo registry holds
        /// it.
        type_name: String,
        /// Whether its gizmo draws.
        visible: bool,
    },
    /// Switch the gizmo's space to the next one available to it.
    ///
    /// The cycle is World, Local, then each frame the active tool offers, then
    /// World again. It lives here rather than in the row that queues it, so the
    /// toolbox, the View menu and the space chord all walk one order - and a
    /// tool with no frames leaves the two settings' spaces cycling into each
    /// other.
    CycleGizmoSpace,
    /// Move elements through the active tool: the world transforms a gizmo
    /// drag produced.
    ///
    /// The tool writes its own component, so this command is the undo step:
    /// applying it first captures the transforms the tool reports, then hands
    /// the moves to the tool's write-back. The inverse is the same command
    /// carrying those captured transforms, which is what makes a whole drag
    /// one step.
    SetElementTransforms {
        /// The entity whose elements moved.
        entity: Entity,
        /// The kind the moved elements belong to.
        kind: String,
        /// The moved elements, each with the world transform it takes.
        moves: Vec<crate::tools::ElementMove>,
    },
    /// Insert elements through the active tool.
    ///
    /// Each element names the id it asks for and the transform it starts with;
    /// the tool answers the id the element ended up under, which is what the
    /// inverse removes. An insert a tool refuses puts back the ones this
    /// command already added, so a refused edit changes nothing.
    InsertElements {
        /// The entity whose elements grow.
        entity: Entity,
        /// The kind the new elements belong to.
        kind: String,
        /// The elements to add, with the transform each takes.
        elements: Vec<crate::tools::ElementMove>,
    },
    /// Remove elements through the active tool.
    ///
    /// Applying it first reads the transforms the tool reports for those
    /// elements, so the inverse is an insert that puts the same elements back
    /// where they were, under the same ids.
    RemoveElements {
        /// The entity whose elements shrink.
        entity: Entity,
        /// The kind the removed elements belong to.
        kind: String,
        /// The tool's own ids for the elements to remove.
        elements: Vec<u64>,
    },
    /// Run one row of the active tool's element context menu.
    ///
    /// The tool decides what the row does, so the command is only a request:
    /// the frame boundary asks the tool for the commands it means and applies
    /// those in the batch's place, which makes a menu row one undo step.
    RunElementMenu {
        /// The row's id, as the tool reported it.
        item: String,
    },
    /// Run exactly one system phase on the next frame, then pause again.
    StepOnce,
    /// Start a play session: the edit world is copied aside and the game runs.
    ///
    /// Nothing is recorded, and Stop is the only way back. A session that cannot
    /// start reports every type it could not copy.
    EnterPlay,
    /// Stop the session, restore the edit world and put the editor's own state
    /// back.
    ExitPlay,
    /// Copy files from outside the project into the assets directory, which is
    /// what a drag-and-drop import queues. Undo removes the copies. A file with
    /// the same name is overwritten.
    ImportFiles {
        /// Absolute source paths, as the OS reported them.
        sources: Vec<String>,
        /// The destination directory (the project's asset root).
        destination: String,
    },
    /// Delete the copies an import created. This is the import's inverse and a
    /// redoable step of its own; it also carries the source paths, so a redo
    /// can copy the files again.
    RemoveFiles {
        /// The created copies, absolute paths.
        paths: Vec<String>,
        /// The original source paths, the redo's sources.
        sources: Vec<String>,
        /// Where the copies live (the redo's destination).
        destination: String,
    },
    /// Create a standalone asset of `type_name` called `name` in `folder`,
    /// where `folder` is relative to the asset directory.
    ///
    /// The file holds the type's default document. An existing file is never
    /// replaced unless the dialog asked for that. The new file is imported at
    /// once, and the inverse deletes it.
    CreateStandaloneAsset {
        /// The registered standalone type's name.
        type_name: String,
        /// The destination folder, `/`-separated and relative; empty for the
        /// asset directory's own root.
        folder: String,
        /// The name the user typed; the type's naming rules add the
        /// extension.
        name: String,
        /// What to do when the file is already there. `None` refuses the
        /// create, which is what the dialog sends before it has asked.
        /// `Some(true)` replaces the file and keeps its guid when the store or
        /// the old file holds one. `Some(false)` replaces it with a fresh guid.
        overwrite: Option<bool>,
    },
    /// Delete the asset file at `path`, which is relative to the asset
    /// directory, together with its `.meta` sidecar.
    ///
    /// The apply reads both files' bytes before it deletes them. The inverse is
    /// the restore that puts them back, the sidecar's guid included, so the
    /// asset keeps its identity through an undo.
    DeleteAssetFile {
        /// The asset's path relative to the asset directory.
        path: String,
    },
    /// Restore the asset files that a delete captured: the file's bytes and,
    /// when it had one, the sidecar's. The inverse is the delete, so the pair
    /// round-trips. Like the other restore commands, it never records as an
    /// edit of its own.
    RestoreAssetFile {
        /// The asset's path relative to the asset directory.
        path: String,
        /// The file's bytes, exactly as they were.
        bytes: Vec<u8>,
        /// The `.meta` sidecar's bytes, when one existed.
        meta: Option<Vec<u8>>,
    },
    /// Move or rename the asset at `from` to `to`, both relative to the
    /// asset directory.
    ///
    /// The `.meta` sidecar travels with the file and the asset watcher
    /// follows the move, so the loaded asset keeps its handle and guid; the
    /// inverse moves it back.
    MoveAssetFile {
        /// The asset's current path relative to the asset directory.
        from: String,
        /// Where it goes, relative to the asset directory.
        to: String,
    },
    /// Write the `.meta` sidecar that the sourced asset at `path` is missing,
    /// with the type's defaults and a fresh guid. A sidecar that already
    /// carries a guid is left alone, so nothing is recorded for it. A sidecar
    /// that is empty or unreadable counts as missing and is replaced. The
    /// inverse deletes the sidecar that was written.
    WriteAssetMeta {
        /// The asset's path relative to the asset directory.
        path: String,
    },
    /// Replace the import settings of the asset at `path` - or a standalone
    /// asset's own document - with `settings`. The inverse
    /// carries the previous value, which the apply reads before writing.
    SetAssetSettings {
        /// The asset's path relative to the asset directory.
        path: String,
        /// The new settings (or document) body.
        settings: serde_json::Value,
    },
    /// Set one field of one editor-editable resource.
    ///
    /// The engine keeps no resource reflection, so the resources this can name
    /// are listed by hand in [`crate::resources::EDITABLE_RESOURCES`], and
    /// their fields are read and written one at a time through [`crate::gizmo`].
    /// The inverse carries the previous value.
    SetResourceField {
        /// The resource's stable name (its type path).
        resource: String,
        /// The field's name within the resource.
        field: String,
        /// The new value.
        value: FieldValue,
    },
    /// Set the shared selection. Selection changes that arrive back to back
    /// merge into one history step through a coalesce key. The inverse carries
    /// the previous entities and asset, so undo goes back one selection at a
    /// time.
    SetSelection {
        /// The selected entities, in selection order (the last is primary);
        /// empty selects none.
        entities: Vec<Entity>,
        /// The selected asset's path, when an asset is selected.
        asset: Option<String>,
    },
    /// Select the elements of one entity, all of one kind.
    ///
    /// The editor's tool layer records this; the ids are the tool's own and
    /// mean nothing to the core. The entity slot is left alone, because the
    /// elements belong to whichever entity is selected. The inverse puts the
    /// previous elements back.
    SetElementSelection {
        /// The kind every element belongs to, as the tool named it.
        kind: String,
        /// The elements to select, in selection order; empty clears the slot.
        elements: Vec<ElementRef>,
    },
    /// Select one world resource for the Inspector.
    ///
    /// This is its own command rather than a third field of
    /// [`EditorCommand::SetSelection`], because picking a resource also clears
    /// the entity and asset slots, and undoing the pick has to bring all three
    /// back ([`Self::RestoreSelection`]).
    SelectResource {
        /// The resource's stable name, `None` to clear the selection.
        resource: Option<String>,
    },
    /// Restore all four selection slots as one recorded step.
    ///
    /// This is the inverse that undoing a resource pick applies: picking a
    /// resource clears the entity, asset and element slots, so the undo has to
    /// bring them back instead of leaving the Inspector pointing at nothing.
    RestoreSelection {
        /// The entity slots to restore.
        entities: Vec<Entity>,
        /// The asset slot to restore.
        asset: Option<String>,
        /// The resource slot to restore.
        resource: Option<String>,
        /// The element slot to restore.
        elements: ElementSelection,
    },
    /// Write values to the preferences file: one `(key, text)` pair per edited
    /// row. The apply reads the document, changes only those keys and writes it
    /// back, so keys other writers own survive. A pair whose text is blank
    /// writes nothing. The file's live reader picks the new values up (history
    /// limits, snapping steps) the next time it looks.
    SetPreferences {
        /// `(key, text)` pairs. The keys are the file's own, and dots nest.
        values: Vec<(String, String)>,
    },
    /// Start an empty scene: every entity is removed, no file is open, and the
    /// selection and history are cleared.
    ///
    /// This is a document action rather than an edit, so it is not recorded.
    /// The history it replaces named entities that the old document owned, and
    /// those handles mean nothing in the new world.
    SceneNew,
    /// Open the scene at `path`, replacing the world's entities.
    SceneOpen {
        /// The scene's path below the asset directory.
        path: String,
    },
    /// Write the world to the open scene, keeping its guid.
    SceneSave,
    /// Write the world to `path`, taking over that file's guid when it exists
    /// and a fresh one otherwise.
    SceneSaveAs {
        /// The scene's path below the asset directory.
        path: String,
    },
}

impl EditorCommand {
    /// Whether this command is an undoable edit.
    ///
    /// Most engine controls are not edits: pause, a single step, the gizmo tool
    /// and the grid never enter the history, while a system toggle does.
    /// Arming an entity pick is not an edit either - it waits for the next
    /// scene click, and the click queues the recorded field write. Restoring
    /// an entity, a component or an asset file replays an earlier step instead
    /// of being one, so it does not record; the selection restore does, and is
    /// a step of its own.
    ///
    /// Two checks in [`Self::apply_recorded`] rely on this answer: a batch
    /// becomes one history step only when every command records, and a command
    /// that applied marks the document unsaved only when it records. A command
    /// that must mark the document unsaved without being undoable needs its
    /// own predicate, not another arm here.
    pub fn recordable(&self) -> bool {
        matches!(
            self,
            EditorCommand::CreateEntity { .. }
                | EditorCommand::DestroyEntity { .. }
                | EditorCommand::AddComponent { .. }
                | EditorCommand::RemoveComponent { .. }
                | EditorCommand::SetField { .. }
                | EditorCommand::SetFieldAsset { .. }
                | EditorCommand::SetFieldEntity { .. }
                | EditorCommand::SetArrayElement { .. }
                | EditorCommand::SetSystemEnabled { .. }
                | EditorCommand::ImportFiles { .. }
                | EditorCommand::RemoveFiles { .. }
                | EditorCommand::CreateStandaloneAsset { .. }
                | EditorCommand::DeleteAssetFile { .. }
                | EditorCommand::MoveAssetFile { .. }
                | EditorCommand::WriteAssetMeta { .. }
                | EditorCommand::SetAssetSettings { .. }
                | EditorCommand::SetResourceField { .. }
                | EditorCommand::SetElementTransforms { .. }
                | EditorCommand::InsertElements { .. }
                | EditorCommand::RemoveElements { .. }
                | EditorCommand::SetSelection { .. }
                | EditorCommand::SetElementSelection { .. }
                | EditorCommand::SelectResource { .. }
                | EditorCommand::RestoreSelection { .. }
        )
    }

    /// The component and field identifiers this command addresses.
    ///
    /// A field identifier is written `component.field`. The history keeps the
    /// identifiers with each entry, so a reload can drop the entries whose
    /// component or field is gone.
    pub fn touched_identifiers(&self) -> Vec<String> {
        let mut identifiers = Vec::new();
        match self {
            EditorCommand::SetField {
                component, field, ..
            }
            | EditorCommand::SetFieldAsset {
                component, field, ..
            }
            | EditorCommand::SetFieldEntity {
                component, field, ..
            }
            | EditorCommand::ArmEntityPick {
                component, field, ..
            }
            | EditorCommand::SetArrayElement {
                component, field, ..
            } => {
                identifiers.push(component.clone());
                identifiers.push(format!("{component}.{field}"));
            }
            EditorCommand::AddComponent { component, .. }
            | EditorCommand::RemoveComponent { component, .. }
            | EditorCommand::RestoreComponent { component, .. } => {
                identifiers.push(component.clone());
            }
            EditorCommand::CreateEntity { components }
            | EditorCommand::RestoreEntity { components, .. } => {
                for seed in components {
                    identifiers.push(seed.type_name.clone());
                    for (field, _) in &seed.fields {
                        identifiers.push(format!("{}.{}", seed.type_name, field));
                    }
                }
            }
            EditorCommand::DestroyEntity { .. }
            | EditorCommand::SetSystemEnabled { .. }
            | EditorCommand::SetPaused { .. }
            | EditorCommand::EnterPlay
            | EditorCommand::ExitPlay
            | EditorCommand::SetGizmoTool { .. }
            | EditorCommand::SetGrid { .. }
            | EditorCommand::SetGizmos { .. }
            | EditorCommand::SetTool { .. }
            | EditorCommand::SetGizmoTypeVisible { .. }
            | EditorCommand::CycleGizmoSpace
            | EditorCommand::StepOnce
            | EditorCommand::SetPreferences { .. }
            | EditorCommand::ImportFiles { .. }
            | EditorCommand::RemoveFiles { .. }
            | EditorCommand::CreateStandaloneAsset { .. }
            | EditorCommand::DeleteAssetFile { .. }
            | EditorCommand::RestoreAssetFile { .. }
            | EditorCommand::MoveAssetFile { .. }
            | EditorCommand::WriteAssetMeta { .. }
            | EditorCommand::SetAssetSettings { .. }
            | EditorCommand::SetResourceField { .. }
            | EditorCommand::SetSelection { .. }
            | EditorCommand::SetElementSelection { .. }
            | EditorCommand::SelectResource { .. }
            | EditorCommand::RestoreSelection { .. }
            | EditorCommand::SetElementTransforms { .. }
            | EditorCommand::InsertElements { .. }
            | EditorCommand::RemoveElements { .. }
            | EditorCommand::RunElementMenu { .. }
            | EditorCommand::SceneNew
            | EditorCommand::SceneOpen { .. }
            | EditorCommand::SceneSave
            | EditorCommand::SceneSaveAs { .. } => {}
        }
        identifiers
    }

    /// Replaces one entity handle with another wherever this command names it.
    ///
    /// The drain calls this after a restore re-created a destroyed entity under
    /// a fresh handle: every remaining history entry has to follow the entity
    /// to its new handle, or a later undo would target the dead one.
    pub fn remap_entity(&mut self, previous: Entity, assigned: Entity) {
        match self {
            EditorCommand::DestroyEntity { entity }
            | EditorCommand::AddComponent { entity, .. }
            | EditorCommand::RemoveComponent { entity, .. }
            | EditorCommand::RestoreComponent { entity, .. }
            | EditorCommand::SetField { entity, .. }
            | EditorCommand::SetFieldAsset { entity, .. }
            | EditorCommand::SetFieldEntity { entity, .. }
            | EditorCommand::ArmEntityPick { entity, .. }
            | EditorCommand::SetElementTransforms { entity, .. }
            | EditorCommand::InsertElements { entity, .. }
            | EditorCommand::RemoveElements { entity, .. }
            | EditorCommand::SetArrayElement { entity, .. } => {
                if *entity == previous {
                    *entity = assigned;
                }
            }
            EditorCommand::RestoreEntity {
                previous: carried_previous,
                ..
            } => {
                if *carried_previous == previous {
                    *carried_previous = assigned;
                }
            }
            EditorCommand::SetSelection { entities, .. } => {
                for entity in entities.iter_mut() {
                    if *entity == previous {
                        *entity = assigned;
                    }
                }
            }
            EditorCommand::SetElementSelection { elements, .. } => {
                for element in elements.iter_mut() {
                    if element.entity == previous {
                        element.entity = assigned;
                    }
                }
            }
            EditorCommand::RestoreSelection {
                elements, entities, ..
            } => {
                // Both slots of a restore follow the entity: a re-created
                // entity carries its elements with it.
                for entity in entities.iter_mut() {
                    if *entity == previous {
                        *entity = assigned;
                    }
                }
                for element in elements.elements_mut() {
                    if element.entity == previous {
                        element.entity = assigned;
                    }
                }
            }
            EditorCommand::CreateEntity { .. }
            | EditorCommand::SetSystemEnabled { .. }
            | EditorCommand::SetPaused { .. }
            | EditorCommand::EnterPlay
            | EditorCommand::ExitPlay
            | EditorCommand::SetGizmoTool { .. }
            | EditorCommand::SetGrid { .. }
            | EditorCommand::SetGizmos { .. }
            | EditorCommand::SetTool { .. }
            | EditorCommand::SetGizmoTypeVisible { .. }
            | EditorCommand::CycleGizmoSpace
            | EditorCommand::RunElementMenu { .. }
            | EditorCommand::StepOnce
            | EditorCommand::SetPreferences { .. }
            | EditorCommand::ImportFiles { .. }
            | EditorCommand::RemoveFiles { .. }
            | EditorCommand::CreateStandaloneAsset { .. }
            | EditorCommand::DeleteAssetFile { .. }
            | EditorCommand::RestoreAssetFile { .. }
            | EditorCommand::MoveAssetFile { .. }
            | EditorCommand::WriteAssetMeta { .. }
            | EditorCommand::SetAssetSettings { .. }
            | EditorCommand::SetResourceField { .. }
            | EditorCommand::SelectResource { .. }
            | EditorCommand::SceneNew
            | EditorCommand::SceneOpen { .. }
            | EditorCommand::SceneSave
            | EditorCommand::SceneSaveAs { .. } => {}
        }
    }

    /// The applier module that applies this command.
    ///
    /// The applier sends each command to the module this names, so a new
    /// variant is classified here once instead of gaining an arm in every
    /// family. This is deliberately not the source of [`Self::recordable`] or
    /// [`Self::touched_identifiers`]: those answer per variant, and one family
    /// can disagree with itself - a system toggle records while pause and step
    /// do not.
    pub fn family(&self) -> CommandFamily {
        match self {
            EditorCommand::CreateEntity { .. }
            | EditorCommand::DestroyEntity { .. }
            | EditorCommand::AddComponent { .. }
            | EditorCommand::RemoveComponent { .. }
            | EditorCommand::RestoreEntity { .. }
            | EditorCommand::RestoreComponent { .. }
            | EditorCommand::SetField { .. }
            | EditorCommand::SetFieldAsset { .. }
            | EditorCommand::SetFieldEntity { .. }
            | EditorCommand::SetArrayElement { .. }
            | EditorCommand::SetResourceField { .. }
            | EditorCommand::SetElementTransforms { .. }
            | EditorCommand::InsertElements { .. }
            | EditorCommand::RemoveElements { .. } => CommandFamily::World,
            EditorCommand::SetSystemEnabled { .. }
            | EditorCommand::SetPaused { .. }
            | EditorCommand::SetGizmoTool { .. }
            | EditorCommand::SetGrid { .. }
            | EditorCommand::SetGizmos { .. }
            | EditorCommand::SetTool { .. }
            | EditorCommand::SetGizmoTypeVisible { .. }
            | EditorCommand::CycleGizmoSpace
            | EditorCommand::RunElementMenu { .. }
            | EditorCommand::StepOnce
            | EditorCommand::EnterPlay
            | EditorCommand::ExitPlay => CommandFamily::Engine,
            EditorCommand::SetSelection { .. }
            | EditorCommand::SetElementSelection { .. }
            | EditorCommand::SelectResource { .. }
            | EditorCommand::RestoreSelection { .. }
            | EditorCommand::ArmEntityPick { .. } => CommandFamily::Selection,
            EditorCommand::SetPreferences { .. }
            | EditorCommand::SceneNew
            | EditorCommand::SceneOpen { .. }
            | EditorCommand::SceneSave
            | EditorCommand::SceneSaveAs { .. } => CommandFamily::Document,
            EditorCommand::ImportFiles { .. }
            | EditorCommand::RemoveFiles { .. }
            | EditorCommand::CreateStandaloneAsset { .. }
            | EditorCommand::DeleteAssetFile { .. }
            | EditorCommand::RestoreAssetFile { .. }
            | EditorCommand::MoveAssetFile { .. }
            | EditorCommand::WriteAssetMeta { .. }
            | EditorCommand::SetAssetSettings { .. } => CommandFamily::Asset,
        }
    }
}

/// What one applied command hands back to the recording layer.
pub(super) struct CommandOutcome {
    /// The command that reverses this one, when one exists.
    pub(super) inverse: Option<EditorCommand>,
    /// A `(previous, assigned)` handle pair, when the command re-created an
    /// entity. The history moves its older entries to the new handle.
    pub(super) remap: Option<(Entity, Entity)>,
}

impl CommandOutcome {
    /// A successful command that produced an inverse.
    pub(super) fn inverse(inverse: EditorCommand) -> Self {
        Self {
            inverse: Some(inverse),
            remap: None,
        }
    }

    /// A successful command that has no inverse, such as an engine control or
    /// a restore.
    pub(super) fn plain() -> Self {
        Self {
            inverse: None,
            remap: None,
        }
    }
}

/// The result of [`EditorCommand::apply_recorded`].
#[derive(Debug, Default)]
pub struct RecordedApply {
    /// One `(index, message)` per failed command, where `index` is the
    /// command's position in the batch.
    pub failures: Vec<(usize, String)>,
    /// The inverse of every successful edit, in the order undo applies them
    /// (the reverse of the order they ran in).
    pub inverses: Vec<EditorCommand>,
    /// True when every command is a recorded edit that succeeded and produced
    /// an inverse. Only then may the batch become one undo step.
    pub fully_invertible: bool,
    /// The `(previous, assigned)` handle pairs that entity restores produced.
    pub remaps: Vec<(Entity, Entity)>,
}

/// The family a command belongs to, which is also the applier module that
/// applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandFamily {
    /// World edits: component fields, array elements, the entity and component
    /// lifecycle, and resource fields.
    World,
    /// Engine controls: pause, step, the gizmo tool, the grid, a system toggle.
    Engine,
    /// Selection changes and the request that arms the next scene click.
    Selection,
    /// Scene documents and preferences.
    Document,
    /// Asset-file reads and writes, and their rollbacks.
    Asset,
}
