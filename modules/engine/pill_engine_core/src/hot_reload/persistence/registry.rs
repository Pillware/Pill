//! Registration bookkeeping: what each generation registers, and how a type
//! that no generation registers any more is dropped and forgotten.
//!
//! # Responsibilities
//!
//! - Answers what this generation registers: the persistable type manifest
//!   and the sequence markers the host captures before a module's `init`.
//! - Enumerates what one `init` registered afterwards, by name and by id, so
//!   a type a reloaded module dropped is detected instead of orphaned.
//! - Forgets registrations whose owner is gone: drops their columns and
//!   releases their bits, keeping name-keyed entries only while a surviving
//!   registration still claims the name.
//!
//! # Design
//!
//! After several reloads the registry can hold one entry per generation for
//! the same type name, each with its own `TypeId`, so the forget paths find
//! entries by name and retire every id they find. A bit is released only when
//! no archetype still maps it: an archetype's mask is also its id, so a bit
//! reused while live rows still reference it would make two component sets
//! share one column.

// External crates
use pill_core::telemetry::telemetry_target;

use super::*;

// =============================================================================
// World — Registration Bookkeeping
// =============================================================================

impl World {
    /// Returns the single [`ComponentId`] that a persistable component type
    /// name resolves to.
    ///
    /// After several hot reloads the component registry can hold one entry per
    /// reload for the same type name, each with a different `TypeId`.
    /// Registering a new generation removes every same-name entry from
    /// `persist_inserters` (see [`Self::register_persistable_component`]), so
    /// the filter below normally leaves exactly one candidate.
    ///
    /// The bit index is deliberately not used to break a tie, because it does
    /// not order registrations in time: `ComponentRegistry::allocate_bit`
    /// hands out bits reclaimed by `remove` before it advances `next_bit`, so
    /// a later registration can receive a lower bit than an earlier one. More
    /// than one surviving candidate therefore means eviction did not do its
    /// job, which is a bug rather than something to break a tie on.
    ///
    /// [`persist_registration_sequence`](Self::persist_registration_sequence)
    /// is the real chronological ordering, should one ever be needed.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ComponentNameAmbiguous`] when more than one
    /// registration survives for the name.
    pub(super) fn resolve_component_id_by_name(
        &self,
        type_name: &str,
    ) -> Result<Option<ComponentId>, WorldError> {
        let mut candidates = self
            .component_registry
            .registered_components()
            .filter(|(_, _, name)| *name == type_name)
            .filter(|(id, _, _)| self.persist_inserters.contains_key(id))
            .map(|(id, _, _)| id);

        let Some(first) = candidates.next() else {
            return Ok(None);
        };
        // A second candidate means eviction did not collapse the set. Picking
        // either one would bind half the rows to the wrong column without any
        // error.
        let extra_candidate_count = candidates.count();
        if extra_candidate_count > 0 {
            return Err(WorldError::ComponentNameAmbiguous {
                type_name: type_name.to_string(),
                count: extra_candidate_count + 1,
            });
        }
        Ok(Some(first))
    }

    /// [`Self::resolve_component_id_by_name`] for callers that cannot return
    /// an error, treating an ambiguous name as unresolved.
    ///
    /// The manifest builder is the only caller: it leaves an unresolved type
    /// out of the manifest. Reporting the ambiguity and taking that path is
    /// better than a tiebreak that silently picks one candidate and binds rows
    /// to the wrong column.
    pub(super) fn resolve_component_id_by_name_logged(
        &self,
        type_name: &str,
    ) -> Option<ComponentId> {
        match self.resolve_component_id_by_name(type_name) {
            Ok(component_id) => component_id,
            Err(error) => {
                error!(
                    target: telemetry_target::HOT_RELOAD,
                    type_name = %type_name,
                    error = %error,
                    "component type name is ambiguous; treating it as unresolved"
                );
                None
            }
        }
    }

    /// Returns the schema hash currently recorded for one persistable
    /// component name.
    ///
    /// The host stores the hash each subject registered and compares it with
    /// this one later. If another subject's reload replaced the type's layout
    /// in between, the comparison identifies the binaries still built against
    /// the old layout, without asking them anything.
    pub fn persist_schema_hash(&self, type_name: &str) -> Option<u64> {
        self.persist_schema_hashes.get(type_name).copied()
    }

    /// Returns one entry per persistable component that still resolves to a
    /// registered id, sorted by type name.
    ///
    /// A type whose name resolves to no id, or to more than one, is left out
    /// of the manifest.
    pub fn persist_type_manifest(&self) -> Vec<PersistTypeManifestEntry> {
        let mut entries: Vec<PersistTypeManifestEntry> = self
            .persist_schema_hashes
            .iter()
            .filter_map(|(type_name, schema_hash)| {
                self.resolve_component_id_by_name_logged(type_name)
                    .map(|component_id| PersistTypeManifestEntry {
                        type_name: type_name.clone(),
                        component_id,
                        schema_hash: *schema_hash,
                    })
            })
            .collect();

        entries.sort_by(|left, right| left.type_name.cmp(&right.type_name));
        entries
    }

    /// Returns the counter that a module's persistable registrations are
    /// recorded against.
    ///
    /// The host reads it before a module's `init`, so afterwards it can list
    /// exactly the persistable types that `init` registered.
    pub fn persist_registration_sequence(&self) -> u64 {
        self.persist_registration_sequence
    }

    /// Returns the persistable component names registered at or after
    /// `sequence`.
    ///
    /// The host compares this list with the one the previous generation
    /// registered, so a component type that was accidentally dropped from the
    /// module is reported instead of silently orphaned.
    ///
    /// The test is `>=`, not `>`: a registration stores the current counter
    /// value and then adds one, so a type registered right after the capture
    /// carries exactly the captured value.
    pub fn persist_type_names_registered_since(&self, sequence: u64) -> Vec<String> {
        self.persist_registration_log
            .iter()
            .filter(|(_, registration_sequence)| *registration_sequence >= sequence)
            .map(|(type_name, _)| type_name.clone())
            .collect()
    }

    /// Returns the counter that every component registration is recorded
    /// against, plain or persistable.
    ///
    /// The host reads it before a module's `init`, so afterwards it can list
    /// exactly the component types that `init` registered.
    pub fn component_registration_sequence(&self) -> u64 {
        self.component_registration_sequence
    }

    /// Returns the names of all component types registered at or after
    /// `sequence`, plain and persistable alike.
    ///
    /// Unlike [`Self::persist_type_names_registered_since`], this covers plain
    /// components too. That lets the host tell a type a reloaded module
    /// dropped entirely apart from one that was only downgraded from
    /// persistable to plain, whose data is still live.
    pub fn registered_component_names_since(&self, sequence: u64) -> Vec<String> {
        self.component_registration_log
            .iter()
            .filter(|(_, _, registration_sequence)| *registration_sequence >= sequence)
            .map(|(type_name, _, _)| type_name.clone())
            .collect()
    }

    /// Returns the ids of all component types registered at or after
    /// `sequence`.
    ///
    /// The id-level twin of [`Self::registered_component_names_since`]. A
    /// rebuilt image gives every name it declares a fresh `TypeId`, so only
    /// the id tells a failed generation's entries apart from a rollback
    /// generation's re-registration of the same names.
    pub fn registered_component_ids_since(&self, sequence: u64) -> Vec<ComponentId> {
        self.component_registration_log
            .iter()
            .filter(|(_, _, registration_sequence)| *registration_sequence >= sequence)
            .map(|(_, component_id, _)| *component_id)
            .collect()
    }

    /// Removes the component from every entity for each type that no module
    /// or project generation registers any more, then forgets the type.
    /// Returns the number of entities whose data was removed.
    ///
    /// The host calls this after a reloaded module or project stopped
    /// registering a persistable type entirely, so the type is not even a
    /// plain component and its data is truly orphaned. Removing the rows now,
    /// while the generation that last registered the type is still loaded,
    /// means the drop never calls into an unloaded DLL.
    ///
    /// Every native id sharing the name loses its rows, not only the one a
    /// name resolver would return. An ambiguous name resolves to nothing, and
    /// a superseded generation's rows would otherwise survive the registration
    /// purge that `forget_component_type` runs for all of them.
    pub fn drop_forgotten_components(&mut self, type_names: &[String]) -> usize {
        // Step 1: Keep the native registrations of each name. A managed
        // (foreign-language) component has a type-erased column, which this
        // path does not cover, while a shared component is native and is
        // covered. If no id is native, the name belongs to a live peer rather
        // than to a forgotten type, and the whole name is skipped, name-keyed
        // entries included.
        let native_ids_by_name: Vec<(&String, Vec<ComponentId>)> = type_names
            .iter()
            .map(|type_name| {
                let native_ids: Vec<ComponentId> = self
                    .component_registry
                    .registered_components()
                    .filter(|(_, _, name)| *name == type_name.as_str())
                    .map(|(component_id, _, _)| component_id)
                    .filter(|component_id| component_id.is_native_storage())
                    .collect();
                (type_name, native_ids)
            })
            .filter(|(_, native_ids)| !native_ids.is_empty())
            .collect();

        // Step 2: Remove each forgotten component from every entity that
        // carries it, in the live storage and in the held one.
        let mut dropped_entities = 0;
        self.run_on_every_entity_storage(|world| {
            for (_, native_ids) in &native_ids_by_name {
                for component_id in native_ids {
                    dropped_entities += world.remove_component_from_every_entity(*component_id);
                }
            }
        });

        // Step 3: Forget each name that named a native type: retire every
        // registration under it and drop its name-keyed entries.
        for (type_name, _) in &native_ids_by_name {
            self.forget_component_type(type_name);
        }
        dropped_entities
    }

    /// Removes `component_id` from every entity carrying it, returning how
    /// many entities lost a row.
    ///
    /// The entities are collected first, because removing one needs a mutable
    /// world while the search holds shared borrows.
    fn remove_component_from_every_entity(&mut self, component_id: ComponentId) -> usize {
        let entities: Vec<Entity> = self
            .storage
            .entity_locations
            .iter()
            .filter(|(_, location)| {
                self.storage
                    .archetypes
                    .get(&location.archetype_id)
                    .is_some_and(|archetype| archetype.component_types.contains(&component_id))
            })
            .map(|(entity, _)| *entity)
            .collect();
        let mut removed = 0;
        for entity in entities {
            if self.remove_component_by_id(entity, component_id).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Removes the given components from every entity that carries them and
    /// retires their registrations. Returns how many entities lost data.
    ///
    /// This sweep works by id, for the host to use when a failed generation's
    /// registrations have to go: the ids are the only handle that tells those
    /// registrations apart from a rollback generation's re-registration of the
    /// same names. For each id, every entity carrying it loses that one
    /// component, so rows the failed generation created do not leak into the
    /// image that is about to be retired. The registration is then retired
    /// through [`Self::retire_registration`], and the name-keyed entries
    /// (deserializer and schema hash) are dropped only when no surviving
    /// registration still claims the name.
    ///
    /// Unlike the name-based [`Self::drop_forgotten_components`], this covers
    /// managed components: one has no Rust type to name it by, so the host
    /// reaches it through the id its binding held. The host also uses this
    /// when a reloaded manifest stops naming a managed component.
    pub fn retire_component_storage(&mut self, component_ids: &[ComponentId]) -> usize {
        let mut dropped_entities = 0;
        for &component_id in component_ids {
            // Step 1: Find the name the id was registered under, if it is
            // still registered.
            let type_name = self
                .component_registry
                .registered_components()
                .find(|(id, _, _)| *id == component_id)
                .map(|(_, _, name)| name.to_string());

            // Step 2: Remove the component from every entity that carries it.
            // The entities are collected first, because removing one needs a
            // mutable world while the search holds shared borrows.
            let entities: Vec<Entity> = self
                .storage
                .entity_locations
                .iter()
                .filter(|(_, location)| {
                    self.storage
                        .archetypes
                        .get(&location.archetype_id)
                        .is_some_and(|archetype| archetype.component_types.contains(&component_id))
                })
                .map(|(entity, _)| *entity)
                .collect();

            for entity in entities {
                if self.remove_component_by_id(entity, component_id).is_ok() {
                    dropped_entities += 1;
                }
            }

            // Step 3: Retire the registration, then drop the name-keyed
            // entries once no registration still claims the name.
            self.retire_registration(component_id);
            if let Some(name) = type_name {
                let still_claimed = self
                    .component_registry
                    .registered_components()
                    .any(|(_, _, other)| other == name.as_str());
                if !still_claimed {
                    self.persist_deserializers.remove(&name);
                    self.persist_schema_hashes.remove(&name);
                }
            }
        }
        dropped_entities
    }

    /// Retires one registration, releasing its bit only when no archetype that
    /// still holds rows maps that bit.
    ///
    /// An archetype's mask is also its id, so a bit that returned to the pool
    /// while an archetype still referenced it would make two component sets
    /// share one id. The next registration that reused the bit would then
    /// reach the stale archetype through `get_or_create_archetype` and be served
    /// its columns. Every caller removes the entities holding the type first,
    /// so the archetypes carrying the bit are empty and are dropped here. If
    /// one still holds rows, the registration is removed without releasing the
    /// bit, because a leaked bit is recoverable while a shared archetype is
    /// not.
    pub(crate) fn retire_registration(&mut self, component_id: ComponentId) {
        let bit = self.component_registry.get_bit(&component_id);
        let rows_still_reference_bit = bit.is_some_and(|bit| {
            self.storage.archetypes.values().any(|archetype| {
                archetype.component_mask.has_bit(bit) && !archetype.entities.is_empty()
            })
        });
        debug_assert!(
            !rows_still_reference_bit,
            "an archetype with live rows references the component bit being released"
        );
        if !rows_still_reference_bit {
            if let Some(bit) = bit {
                self.storage
                    .archetypes
                    .retain(|_, archetype| !archetype.component_mask.has_bit(bit));
            }
            self.component_registry.remove(&component_id);
        }
        self.storage_factories.remove(&component_id);
        self.retired_native_storage_ops.remove(&component_id);
        self.persist_serializers.remove(&component_id);
        self.persist_inserters.remove(&component_id);
    }

    /// Removes every registration and name-keyed persist entry of one
    /// forgotten component type, so the type stops appearing in the manifest
    /// and in the registry.
    pub(super) fn forget_component_type(&mut self, type_name: &str) {
        // The registry can hold one entry per generation for the same name,
        // each with its own `TypeId`, so every entry sharing the name goes.
        let stale_ids: Vec<ComponentId> = self
            .component_registry
            .registered_components()
            .filter(|(_, _, name)| *name == type_name)
            .map(|(id, _, _)| id)
            .collect();
        for stale_id in &stale_ids {
            self.retire_registration(*stale_id);
        }
        self.persist_deserializers.remove(type_name);
        self.persist_schema_hashes.remove(type_name);
    }

    /// Returns the serializer and schema hash of every persistable component
    /// currently registered, keyed by type name.
    ///
    /// The host captures this before a reload replaces the registrations.
    /// Migration then reads the retiring generation's rows through these
    /// serializers, the only code that understands the old layout.
    #[cfg(feature = "development")]
    pub fn capture_persist_type_metadata(&self) -> HashMap<String, PersistTypeMetadata> {
        let mut metadata_by_name: HashMap<String, PersistTypeMetadata> = HashMap::new();

        for manifest_entry in self.persist_type_manifest() {
            if let Some(&serializer) = self.persist_serializers.get(&manifest_entry.component_id) {
                metadata_by_name.insert(
                    manifest_entry.type_name,
                    PersistTypeMetadata {
                        component_id: manifest_entry.component_id,
                        schema_hash: manifest_entry.schema_hash,
                        serializer,
                    },
                );
            }
        }

        metadata_by_name
    }

    /// Returns the id of every entity that is currently alive.
    ///
    /// Migration runs after the incoming generation's `init`, so archetypes
    /// may already hold entities that `init` spawned, written with the current
    /// schema. This set lets the migration tell those entities apart from the
    /// ones that need converting. See
    /// [`Self::migrate_changed_persistable_components`].
    #[cfg(feature = "development")]
    pub fn capture_live_entities(&self) -> HashSet<Entity> {
        let mut entities: HashSet<Entity> = HashSet::new();
        for archetype in self.storage.archetypes.values() {
            entities.extend(archetype.entities.iter().copied());
        }
        entities
    }
}

// =============================================================================
// Function Pointer Type Aliases
// =============================================================================

/// Serializes the component at `index` in the given storage map into JSON
/// bytes, or `None` when the value cannot be serialized.
///
/// The signature could not be infallible: `serde_json::to_vec` can fail for a
/// user type. Migration reports such a failure as a skipped type instead of
/// aborting the reload.
pub(crate) type SerializeComponentFn =
    fn(storage: &ComponentColumns, index: usize) -> Option<Vec<u8>>;

/// Rebuilds a heap-allocated component from JSON bytes.
///
/// Returns `None` when the schema changed incompatibly, for example when a
/// field's type changed from `f32` to `String`. A field that was added takes
/// its default value, and a field that was removed is ignored: serde matches
/// fields by name.
pub(crate) type DeserializeComponentFn = fn(bytes: &[u8]) -> Option<Box<dyn Component>>;

/// Downcasts a `Box<dyn Component>` to its concrete type and pushes the value
/// into that type's [`ComponentColumn`](crate::archetype::ComponentColumn) in
/// the storage map.
pub(crate) type InsertComponentFn =
    fn(storage: &mut ComponentColumns, component: Box<dyn Component>);

/// Serializes one registered resource out of the world into JSON bytes.
///
/// Returns `None` when the world holds no value for the resource, which is
/// different from an empty value: a resource that was never inserted has
/// nothing to record. The function takes the whole world rather than a value
/// because a resource is found by its id, and the `T` it is monomorphized for
/// supplies that id.
pub type SerializeResourceFn = fn(world: &World) -> Option<Vec<u8>>;

/// Rebuilds one resource from snapshot bytes and inserts it into the world.
///
/// Returns whether the payload could be read. A value that no longer parses
/// under the current shape is logged and skipped instead of being inserted
/// half-formed, the same way an unreadable component row is handled.
pub type RestoreResourceFn = fn(world: &mut World, bytes: &[u8]) -> bool;

// =============================================================================
// Schema Shape and Hash Helpers
// =============================================================================

/// Deep-merges two JSON values: `base_json` provides the defaults and
/// `override_json` the stored data. Fields present in `override_json` win;
/// fields only in `base_json` keep their default value.
pub(super) fn merge_json(
    mut base_json: serde_json::Value,
    override_json: serde_json::Value,
) -> serde_json::Value {
    match (&mut base_json, override_json) {
        (serde_json::Value::Object(base_map), serde_json::Value::Object(override_map)) => {
            for (key, value) in override_map {
                match base_map.get_mut(&key) {
                    Some(base_value) => {
                        // Recursively merge nested objects.
                        let merged = merge_json(base_value.clone(), value);
                        *base_value = merged;
                    }
                    None => {
                        // The snapshot has a field the default does not: the
                        // struct no longer declares it. Keep it; serde ignores
                        // unknown fields while deserializing.
                        base_map.insert(key, value);
                    }
                }
            }
            serde_json::Value::Object(base_map.clone())
        }
        // For non-object values (primitives, arrays), override wins.
        (_, override_value) => override_value,
    }
}

/// Builds a copy of a JSON tree in which every value is replaced by a marker
/// for its kind, such as `"number"` or `"string"`.
///
/// Hashing this copy instead of the original means that changing a default
/// value does not change the schema hash, while changing a field's kind does.
pub(super) fn normalize_schema_shape(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Null => serde_json::Value::String("null".to_string()),
        serde_json::Value::Bool(_) => serde_json::Value::String("bool".to_string()),
        serde_json::Value::Number(_) => serde_json::Value::String("number".to_string()),
        serde_json::Value::String(_) => serde_json::Value::String("string".to_string()),
        serde_json::Value::Array(values) => {
            let normalized_values: Vec<serde_json::Value> =
                values.iter().map(normalize_schema_shape).collect();
            serde_json::Value::Array(normalized_values)
        }
        serde_json::Value::Object(map) => {
            let mut normalized_map = serde_json::Map::new();
            let mut sorted_keys: Vec<&String> = map.keys().collect();
            sorted_keys.sort();

            for key in sorted_keys {
                if let Some(entry_value) = map.get(key) {
                    normalized_map.insert(key.clone(), normalize_schema_shape(entry_value));
                }
            }

            serde_json::Value::Object(normalized_map)
        }
    }
}

/// Computes the schema hash of one persistable component type.
///
/// `fields` is the declared field layout when the derive registered the type,
/// and empty for a hand-registered one. The layout must be part of the hash:
/// the normalized default value of a `Vec<f32>` and of a `Vec<String>` are
/// both an empty array, so without the field tags a reload could decide the
/// schema was unchanged and then read one as the other.
pub(super) fn calculate_schema_hash<T>(
    fields: &[crate::component_registry::ComponentFieldDescriptor],
) -> u64
where
    T: Component + Serialize + Default + 'static,
{
    // Step 1: Serialize a default instance and keep only the kind of each
    // value, so changing a default value does not change the hash.
    let default_value = T::default();
    let default_json = serde_json::to_value(default_value).unwrap_or(serde_json::Value::Null);
    let normalized_schema = normalize_schema_shape(&default_json);
    let normalized_schema_string = serde_json::to_string(&normalized_schema)
        .unwrap_or_else(|_| "<schema-serialization-failed>".to_string());

    // Step 2: Hash the type name, the type's size and the normalized schema
    // into one value that can be compared across reloads.
    let mut hasher = DefaultHasher::new();
    crate::type_names::stable_type_name::<T>().hash(&mut hasher);
    std::mem::size_of::<T>().hash(&mut hasher);
    normalized_schema_string.hash(&mut hasher);

    // Step 3: Add the declared field layout. The type tag says which container
    // a field holds (`vec:f32`, `string`), the offset catches a reordered or
    // padded shape, and the element count separates a fixed array from a
    // resized one. A registration without a layout adds nothing here.
    for field in fields {
        field.name.hash(&mut hasher);
        field.type_tag.hash(&mut hasher);
        field.offset.hash(&mut hasher);
        field.size.hash(&mut hasher);
        field.align.hash(&mut hasher);
        field.element_count.hash(&mut hasher);
    }
    hasher.finish()
}
