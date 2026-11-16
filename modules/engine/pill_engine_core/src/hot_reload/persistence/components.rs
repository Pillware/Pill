//! Component-side persistence: what one persistable component type registers,
//! and what a reload compares to decide which types to migrate.
//!
//! # Responsibilities
//!
//! - Defines [`PersistTypeMetadata`] and [`PersistTypeManifestEntry`], which
//!   the reload transaction compares before and after the swap to decide which
//!   component types need migrating.
//! - Registers the per-type serialize, deserialize and insert functions that
//!   selective migration reads and writes rows through.
//!
//! # Design
//!
//! Deserializers and schema hashes are keyed by type name, not by `TypeId`: a
//! rebuilt image gives every type a fresh `TypeId` for the same name, and the
//! name is the only thing two generations agree on. Serializers and inserters
//! are keyed by `ComponentId`, which the migration resolves from the name
//! first. Registration removes a superseded generation's entries, so a name
//! always resolves to one live registration.

// External crates
use pill_core::telemetry::telemetry_target;

use super::*;

/// One persistable component type's registration, as the reload sees it.
///
/// The reload captures this before the swap, so it can compare the old schema
/// with the new one and migrate only the component types that changed.
#[derive(Clone, Copy)]
pub struct PersistTypeMetadata {
    /// Runtime component identifier used by archetype columns.
    pub component_id: ComponentId,
    /// Schema hash of the type: its name, its size, the field kinds of a
    /// default instance and, when the type declared one, its field layout.
    pub schema_hash: u64,
    /// Serializer of the retiring generation, the only code that can read
    /// rows still stored in the old layout.
    pub serializer: SerializeComponentFn,
}

/// Lightweight manifest entry for the persistable component types registered
/// right now.
///
/// [`World::persist_type_manifest`] builds one entry per registered type, so
/// the host can compare the registrations before and after a reload and decide
/// which types need migration.
#[derive(Clone)]
pub struct PersistTypeManifestEntry {
    /// Name the type is registered under: a shared component's declared name,
    /// otherwise the path of the Rust type.
    pub type_name: String,
    /// Runtime component identifier used by archetype columns.
    pub component_id: ComponentId,
    /// Schema hash the registration stored for this type name.
    pub schema_hash: u64,
}

/// One component type the selective pass could not migrate.
#[derive(Debug, Clone)]
pub struct SkippedType {
    /// Registered type name the pass skipped.
    pub type_name: String,
    /// Why the pass skipped it, as the migration error text says. The
    /// development host logs the whole list once at warning level, so the
    /// normal log answers "why" without a re-run at a lower level.
    pub reason: String,
}

/// What selective migration did.
///
/// [`World::migrate_changed_persistable_components`] returns one of these: the
/// number of component types it migrated, the number of rows, and the types it
/// had to skip.
#[derive(Debug, Default)]
pub struct SelectiveMigrationReport {
    /// Number of component types that were migrated.
    pub migrated_type_count: usize,
    /// Number of rows migrated across all changed types. An entity whose two
    /// component types both changed is counted twice.
    pub migrated_entity_count: usize,
    /// Types that could not be migrated selectively, with the reason.
    pub skipped_types: Vec<SkippedType>,
}

// =============================================================================
// World — Persistable Component Registration
// =============================================================================

impl World {
    /// Register a component type that supports persistence and schema migration.
    ///
    /// Besides the normal component registration (bit index, storage factory),
    /// this stores the type's serialize, deserialize and insert functions, so
    /// the engine can migrate the type's rows during a hot reload.
    ///
    /// `T` must implement `serde::Serialize + serde::DeserializeOwned` so JSON
    /// can round-trip its data. When the struct shape changes between reloads,
    /// serde matches fields by **name**: a new field gets the value from
    /// `Default::default()`, and a removed field is silently ignored.
    ///
    /// This registration's schema hash covers the type name, the type's size
    /// and the kind of every field in a default instance. A type registered
    /// through the derive uses
    /// [`Self::register_persistable_component_with_layout`] instead, whose hash
    /// also covers the declared field layout. Two shapes that serialize
    /// identical defaults but hold different containers (`Vec<f32>` against
    /// `Vec<String>`) can therefore never compare equal.
    pub fn register_persistable_component<T>(&mut self)
    where
        T: Component + Serialize + DeserializeOwned + Default + 'static,
    {
        self.register_persistable_component_inner::<T>(&[]);
    }

    /// Shared body of the two registration entry points.
    ///
    /// `fields` is the compile-time field layout when the caller has one, and
    /// the schema hash includes it, so a changed container kind or element
    /// type forces a migration instead of taking the unchanged-schema fast
    /// path. This does not record the layout for the editor; the
    /// layout-carrying entry point does that after the call.
    pub(super) fn register_persistable_component_inner<T>(
        &mut self,
        fields: &'static [crate::component_registry::ComponentFieldDescriptor],
    ) where
        T: Component + Serialize + DeserializeOwned + Default + 'static,
    {
        let component_id = ComponentId::of::<T>();
        // The name-keyed persist maps resolve against the name the registry
        // recorded, so both sides must use the same one: a shared component's
        // declared name, an ordinary component's Rust path. The id and the name
        // are both known before anything is registered, which is what lets the
        // collision guard run first.
        let type_name = crate::component::ComponentRegistry::registered_name::<T>();
        // The collision guard below uses up a superseded announcement, so the
        // layout guard remembers it first.
        let superseding = self.superseded_persist_names.contains(&type_name);

        // Step 1: Refuse to re-lay out a component this binary is already
        // stale for. Such a binary is a rollback build, made against a layout
        // another reload has since replaced. When its hash differs from the
        // registered one, the whole registration is skipped: the layout, the
        // persist entries and the column stay with the current subjects. The
        // binary stays stale until it is rebuilt, with its queries refused and
        // its systems suspended by the host.
        if self.stale_superseded_persist_names.contains(&type_name) {
            let incoming_hash = calculate_schema_hash::<T>(fields);
            if let Some(registered_hash) = self.persist_schema_hashes.get(&type_name).copied() {
                if registered_hash != incoming_hash {
                    self.superseded_persist_names.remove(&type_name);
                    warn!(
                        target: telemetry_target::ECS,
                        type_name = %type_name,
                        "kept the registered layout: this binary was built against a previous layout of the component and stays stale until it is rebuilt"
                    );
                    return;
                }
            }
        }

        // Step 2: Refuse a name collision before anything registers. A guard
        // placed after `register_component_inner` would return with the
        // registration half done. A fresh bit, a name entry, a storage factory
        // pointing into the image about to be discarded, and a registration-log
        // entry would all survive. The name would then resolve to two ids, and
        // every retry would use one more bit toward the 128-type ceiling.
        if let Some((existing_component_id, live_row_count)) =
            self.live_component_with_name(&type_name, component_id)
        {
            // An announced name is the predecessor, not a peer: the reload is
            // replacing that generation and moves its rows afterwards. Live
            // rows alone cannot tell the two apart, because a generation being
            // retired still holds them - which is why the host announces the
            // names, and why the announcement is used up here.
            if !self.superseded_persist_names.remove(&type_name) {
                self.record_registration_error(WorldError::ComponentNameCollision {
                    type_name: type_name.clone(),
                    existing_id: existing_component_id,
                    incoming_id: component_id,
                    live_rows: live_row_count,
                });
                warn!(
                    target: telemetry_target::ECS,
                    type_name = %type_name,
                    live_rows = live_row_count,
                    "{}",
                    COMPONENT_NAME_COLLISION_REFUSAL
                );
                return;
            }
        }

        // Step 3: Refuse a layout the predecessor's columns cannot hold.
        //
        // The migration reads rows out of the old column with the incoming
        // type. Those rows sit `existing_layout.size` bytes apart in a buffer
        // that was sized and aligned for the old layout, so each address must
        // be aligned for the incoming type: the incoming alignment may not
        // exceed the buffer's, and the old stride must be a multiple of it.
        // Adding an `f32` field keeps the alignment and still migrates;
        // widening a field to `f64` does not, and letting it through makes the
        // migration's typed read of the old column panic on the layout
        // mismatch.
        //
        // Every same-name registration is checked, not just one: older
        // generations can still be listed under the name and the migration
        // picks the predecessor by name, so any column that cannot hold the
        // incoming rows is a reason to refuse.
        if superseding {
            let incoming_size = std::mem::size_of::<T>();
            let incoming_align = std::mem::align_of::<T>();
            for existing_component_id in self.component_ids_with_name(&type_name) {
                if let Some(existing_layout) =
                    self.component_registry.get_layout(&existing_component_id)
                {
                    if incoming_align > existing_layout.align
                        || existing_layout.size % incoming_align != 0
                    {
                        self.record_registration_error(WorldError::ComponentLayoutChanged {
                            type_name: type_name.clone(),
                            existing_size: existing_layout.size,
                            existing_align: existing_layout.align,
                            incoming_size,
                            incoming_align,
                        });
                        warn!(
                            target: telemetry_target::ECS,
                            type_name = %type_name,
                            existing_size = existing_layout.size,
                            existing_align = existing_layout.align,
                            incoming_size,
                            incoming_align,
                            "{}",
                            COMPONENT_LAYOUT_CHANGED_REFUSAL
                        );
                        return;
                    }
                }
            }
        }

        // Step 4: Do the normal component registration (bit index, storage
        // factory), passing the field layout so the registry can check a
        // repeat registration against the first one. A superseding
        // registration is allowed to re-lay out a shared component; the reload
        // migrates its rows once `init` returns.
        self.register_component_inner_superseding::<T>(fields, superseding);

        // Step 5: Remove persist entries left by earlier registrations of the
        // same name, so a name never resolves to two columns. A type that
        // changed and then changed back can reuse its `TypeId`, and the entries
        // recorded for the shapes in between must go with the generation that
        // recorded them.
        //
        // This is also what keeps name resolution unambiguous later, so it may
        // only remove a *superseded* generation. A same-name entry whose column
        // still holds rows is not superseded: it is a concurrent peer,
        // registered by another binary that linked the same type and so got a
        // `TypeId` of its own. Removing that entry would drop its inserter, and
        // every row it owns would be silently thrown away at the next reload.
        // That case is reported as a collision instead.
        let stale_component_ids: Vec<ComponentId> = self
            .component_registry
            .registered_components()
            .filter(|(_, _, name)| *name == type_name)
            .map(|(id, _, _)| id)
            .filter(|id| *id != component_id)
            .collect();
        for stale_component_id in &stale_component_ids {
            self.persist_serializers.remove(stale_component_id);
            self.persist_inserters.remove(stale_component_id);
        }
        // Also clear this name's deserializer; the new one is inserted below.
        self.persist_deserializers.remove(&type_name);

        // Step 6: Store the new monomorphized serialize, deserialize and
        // insert functions, and the schema hash of the new shape.
        self.persist_serializers.insert(
            component_id,
            serialize_component::<T> as SerializeComponentFn,
        );

        self.persist_deserializers.insert(
            type_name.clone(),
            deserialize_component::<T> as DeserializeComponentFn,
        );

        self.persist_inserters.insert(
            component_id,
            insert_boxed_component::<T> as InsertComponentFn,
        );

        let schema_hash = calculate_schema_hash::<T>(fields);
        self.persist_schema_hashes
            .insert(type_name.clone(), schema_hash);

        // Record the registration in order, so the host can list exactly which
        // types one module's `init` registered. That is how a component type
        // dropped from a reloaded module is found.
        self.persist_registration_log
            .push((type_name, self.persist_registration_sequence));
        self.persist_registration_sequence += 1;
    }

    /// Announce the persistable type names a retiring generation registered, so
    /// the init pass that follows may replace their persist entries instead of
    /// being refused as a concurrent peer.
    ///
    /// A reloaded project registers every type its previous `init` registered
    /// again, and the rebuilt image gives each of them a fresh `TypeId` under
    /// the same name. At the registration site that looks exactly like another
    /// binary that linked the same type. Only the host can tell the two apart,
    /// because it captured these names from the generation it is retiring, so
    /// it announces them here, just before init.
    ///
    /// An announcement covers one init pass. Each mark is used up by the
    /// registration it applies to, and
    /// [`Self::clear_superseded_persist_registrations`] drops the rest: a name
    /// the new generation no longer declares was forgotten, not superseded, and
    /// must not let a later peer through.
    #[cfg(feature = "development")]
    pub fn supersede_persist_registrations(&mut self, type_names: &[String]) {
        self.superseded_persist_names
            .extend(type_names.iter().cloned());
    }

    /// Announce which superseded names the retiring generation was already
    /// stale for, because it was built against a layout of a shared component
    /// that another subject's reload has replaced since.
    ///
    /// Only a subject that was current for a component may re-lay it out. A
    /// stale one registering the old layout again - the rollback of a module
    /// whose rebuild failed - would otherwise take the component back to a
    /// layout every current subject has left. That registration is skipped, and
    /// the registered layout is kept.
    #[cfg(feature = "development")]
    pub fn mark_stale_superseded_persist_registrations(&mut self, type_names: &[String]) {
        self.stale_superseded_persist_names
            .extend(type_names.iter().cloned());
    }

    /// Drop the marks left by [`Self::supersede_persist_registrations`] and
    /// [`Self::mark_stale_superseded_persist_registrations`].
    pub fn clear_superseded_persist_registrations(&mut self) {
        self.superseded_persist_names.clear();
        self.stale_superseded_persist_names.clear();
    }

    /// Register a persistable component together with its compile-time field
    /// layout, so the C# mirror codegen can emit a typed struct and the schema
    /// hash can account for container fields. See
    /// [`Self::register_component_with_layout`].
    ///
    /// The layout separates two shapes whose defaults look identical to serde:
    /// `Vec<f32>` and `Vec<String>` both serialize a default instance as `[]`,
    /// but their type tags differ, so changing the element type changes the
    /// hash. The reload then migrates the stored values instead of reading the
    /// old buffer as the new layout.
    pub fn register_persistable_component_with_layout<T>(
        &mut self,
        fields: &'static [crate::component_registry::ComponentFieldDescriptor],
    ) where
        T: Component + Serialize + DeserializeOwned + Default + 'static,
    {
        self.register_persistable_component_inner::<T>(fields);
        self.record_component_field_layout(ComponentId::of::<T>(), fields);
    }
}

// =============================================================================
// Per-Type Monomorphized Functions
// =============================================================================

/// Serialize the component at `index` from storage into JSON bytes.
///
/// Returns `None` when the type's own `Serialize` implementation refuses the
/// value. The caller then reports the type as skipped and migrates the others:
/// a reload must not stop because one type cannot be written, and the signature
/// should not hide that choice.
pub(super) fn serialize_component<T>(storage: &ComponentColumns, index: usize) -> Option<Vec<u8>>
where
    T: Component + Serialize,
{
    let typed_storage = storage.column_of::<T>();
    let component: &T = typed_storage.get::<T>(index);
    serde_json::to_vec(component).ok()
}

/// Deserialize JSON bytes into a heap-allocated component.
///
/// If the schema changed, for example when a field was added, missing fields
/// take their value from `T::default()`: the default JSON is merged with the
/// snapshot JSON before deserializing. A field that no longer exists in the
/// struct is ignored.
///
/// Returns `None` when the bytes or the default instance cannot be turned into
/// JSON, or when the merged value does not deserialize into `T` - for example
/// when a field's type changed.
pub(super) fn deserialize_component<T>(bytes: &[u8]) -> Option<Box<dyn Component>>
where
    T: Component + Serialize + DeserializeOwned + Default + 'static,
{
    // Step 1: Deserialize the snapshot bytes into a generic JSON Value.
    let snapshot_json: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to parse JSON for '{}': {}",
                std::any::type_name::<T>(),
                error
            );
        })
        .ok()?;

    // Step 2: Serialize a default instance to JSON as the schema baseline
    // for fields missing from the snapshot.
    let default_instance = T::default();
    let default_json: serde_json::Value = serde_json::to_value(&default_instance)
        .map_err(|error| {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to serialize default for '{}': {}",
                std::any::type_name::<T>(),
                error
            );
        })
        .ok()?;

    // Step 3: Merge the defaults with the snapshot. Where both have a value,
    // the snapshot wins.
    let merged_json = merge_json(default_json, snapshot_json);

    match serde_json::from_value::<T>(merged_json) {
        Ok(value) => Some(Box::new(value)),
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to deserialize '{}': {}; the component data is skipped",
                std::any::type_name::<T>(),
                error
            );
            None
        }
    }
}

/// Downcast and push a boxed component into the concrete column.
pub(super) fn insert_boxed_component<T>(
    storage: &mut ComponentColumns,
    component: Box<dyn Component>,
) where
    T: Component + 'static,
{
    let component_pointer = Box::into_raw(component);
    // SAFETY: `component_pointer` comes from `Box::into_raw` just above, so it
    // is valid, aligned and uniquely owned, and `Box::from_raw` takes that
    // ownership back exactly once. The cast to `*mut T` is sound because the
    // boxed value really is a `T`: `migrate_single_component_type` looks this
    // inserter up through the id it resolved from the type name, and the
    // deserializer that built the box was registered for the same name.
    // Moving `*typed_component` out ends the box's ownership without a double
    // free.
    let typed_component: Box<T> = unsafe { Box::from_raw(component_pointer as *mut T) };
    storage.column_of_mut::<T>().push::<T>(*typed_component);
}
