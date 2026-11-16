//! Resource-side persistence: the snapshot shape and the per-type manifest.
//!
//! # Responsibilities
//!
//! - Defines [`ResourceSnapshot`], the flat `(name, payload)` capture of
//!   resource values, and [`PersistResourceManifestEntry`], the registration
//!   view the reload transaction compares across the swap.
//! - Owns the resource restore path: a Rust resource's payload goes back
//!   through the restore function registered for its name, and a foreign
//!   resource's bytes are stored as they are.
//! - Runs the resource half of the reload migration: reads a reshaped value
//!   through the retiring generation's serializer and writes it back through
//!   the arriving one.
//!
//! # Design
//!
//! A resource is a singleton, so unlike components there is no entity to group
//! by, and the snapshot is a flat list sorted by name. Sorting makes a snapshot
//! reproducible: hash-map order is not, and a snapshot that differs between two
//! identical runs cannot be diffed, compared or used as a test fixture.

// External crates
use pill_core::telemetry::telemetry_target;

use super::*;

// =============================================================================
// ResourceSnapshot
// =============================================================================

/// Resource values captured at one point in time.
///
/// Each value is keyed by name, like every other persistence payload. A
/// [`ResourceId::Native`] is a `TypeId`, and no other process, and no later
/// build of this one, produces the same `TypeId`; the name is the part that can
/// outlive the process.
///
/// The list is flat rather than nested: a resource is a singleton, so there is
/// no entity to group by.
///
/// # Examples
///
/// ```
/// use pill_engine::ResourceSnapshot;
///
/// let snapshot = ResourceSnapshot {
///     entries: vec![("project::Score".to_string(), b"{\"points\":7}".to_vec())],
/// };
/// assert_eq!(snapshot.resource_count(), 1);
/// ```
#[derive(Debug, Default, Clone)]
pub struct ResourceSnapshot {
    /// One `(registered name, JSON payload)` pair per resource that held a
    /// value, sorted by name so a snapshot is reproducible.
    pub entries: Vec<(String, Vec<u8>)>,
}

impl ResourceSnapshot {
    /// Number of resource values stored.
    #[must_use]
    pub fn resource_count(&self) -> usize {
        self.entries.len()
    }

    /// The payload recorded for one resource name, if the snapshot has it.
    #[must_use]
    pub fn payload(&self, type_name: &str) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(name, _)| name == type_name)
            .map(|(_, payload)| payload.as_slice())
    }
}

/// One persistable resource registration, as the reload transaction sees it.
///
/// Captured before the swap by [`World::persist_resource_manifest`] so the pass
/// afterwards can compare schema hashes and read a reshaped value through the
/// serializer that wrote it. The component side splits the same information
/// across [`PersistTypeMetadata`], which keeps the retiring serializer, and
/// [`PersistTypeManifestEntry`], which keeps the name and the id.
#[derive(Clone)]
pub struct PersistResourceManifestEntry {
    /// The id this resource was registered under when the entry was captured.
    pub resource_id: ResourceId,
    /// The name the resource is recorded under; a snapshot and a reload match
    /// on it.
    pub type_name: String,
    /// Structural hash of the shape this generation declared.
    pub schema_hash: u64,
    /// The serializer of the retiring generation, the only code that can read
    /// its stored value.
    pub serializer: SerializeResourceFn,
}

// =============================================================================
// World — Persistable Resource Registration
// =============================================================================
//
// A resource is a singleton, so there is no archetype to walk and no row to
// update for each entity. A snapshot is one payload per registered resource,
// keyed by the name both generations agree on.
//
// Migration is what makes this safe and not merely convenient.
// `rehome_resources` gives a live resource a fresh function table across a
// reload but leaves its bytes alone, so a resource whose shape changed would be
// read as the new type unless something migrates it.
// `migrate_changed_persistable_resources` is that step, the resource twin of the
// component migration the reload transaction runs.

impl World {
    /// Register a resource that survives snapshot, restore and reload.
    ///
    /// This is the resource counterpart of
    /// [`Self::register_persistable_component`]. It is a registration call
    /// rather than a derive attribute because resources have no derive: a
    /// resource is declared with `impl Resource` and registered by hand, so
    /// turning persistence on is one more call at the same place.
    ///
    /// Registering is what opts a resource in. A resource registered with
    /// [`Self::register_resource`] alone never appears in a snapshot, just as a
    /// component registered with [`Self::register_component`] alone never does.
    ///
    /// `T` must implement `Serialize + DeserializeOwned + Default`, for the same
    /// reason a persistable component must: the value crosses between two shapes
    /// of the type as JSON, and `Default` supplies the fields a snapshot written
    /// before those fields existed cannot carry.
    pub fn register_persistable_resource<T>(&mut self)
    where
        T: Resource + Serialize + DeserializeOwned + Default,
    {
        // A shared resource is known everywhere by its declared name; an
        // ordinary one falls back to its Rust path. Both builds of one type
        // derive the same string, which is what makes a snapshot portable
        // across a reload and across a process.
        let type_name = T::shared_name()
            .map(str::to_owned)
            .unwrap_or_else(|| crate::type_names::stable_type_name::<T>().to_owned());
        self.register_persistable_resource_as::<T>(&type_name);
    }

    /// Register a persistable resource under a persistence name you choose.
    ///
    /// The name is the only thing a snapshot and a reload match on, so pinning
    /// it separates save compatibility from the Rust path: moving a resource to
    /// another module, or renaming the crate it lives in, then costs nothing.
    /// The foreign registration takes a name for the same reason: a resource
    /// declared by another language has no Rust type path to record.
    ///
    /// Two different Rust types may share one persistence name. That is the
    /// reload case, not a mistake: the arriving build of a type is a different
    /// type as far as this process is concerned. The later registration replaces
    /// the earlier one's entries, and the value the earlier one owns is carried
    /// across by [`Self::migrate_changed_persistable_resources`].
    pub fn register_persistable_resource_as<T>(&mut self, type_name: &str)
    where
        T: Resource + Serialize + DeserializeOwned + Default,
    {
        // Step 1: Register the type itself first, because that call decides
        // whether the claim is accepted. When another type already owns the
        // name, the entries below must not be written: a refused generation
        // that kept a serializer and a restorer could still read and overwrite
        // the live value through its own code.
        let id = ResourceId::of::<T>();
        if !self.register_resource_claiming::<T>() {
            return;
        }
        let type_name = type_name.to_owned();

        // Step 2: Drop any earlier registration under the same name instead of
        // keeping both. Its serializer points into an image that is being
        // retired, and after a rebuild the same type can land on a different
        // id. Left in place, the old entry would make a snapshot read through
        // code that is about to be unmapped and write the name twice.
        let stale_ids: Vec<ResourceId> = self
            .persist_resource_names
            .iter()
            .filter(|(existing_id, existing_name)| {
                **existing_name == type_name && **existing_id != id
            })
            .map(|(existing_id, _)| *existing_id)
            .collect();
        for stale_id in stale_ids {
            self.persist_resource_names.remove(&stale_id);
            self.persist_resource_serializers.remove(&stale_id);
        }

        // Step 3: Record the serializer, the restorer and the schema hash for
        // this name.
        self.persist_resource_serializers
            .insert(id, serialize_resource::<T> as SerializeResourceFn);
        self.persist_resource_restorers.insert(
            type_name.clone(),
            restore_resource::<T> as RestoreResourceFn,
        );
        self.persist_resource_schema_hashes
            .insert(type_name.clone(), calculate_resource_schema_hash::<T>());
        self.persist_resource_names.insert(id, type_name);
    }

    /// Record the persistence name of a resource declared by another language.
    ///
    /// A foreign resource has no Rust type, so no serializer can be generated
    /// for it. It needs none: its payload is plain bytes and the world already
    /// knows their size, so the bytes are the value. Recording the name is all
    /// it takes to make the resource visible to a snapshot, which is why this
    /// takes a name instead of a type parameter.
    ///
    /// Returns whether the name belongs to a registered foreign resource. A
    /// caller that has just registered one can ignore the result.
    pub fn register_persistable_foreign_resource(&mut self, type_name: &str) -> bool {
        let id = ResourceId::Shared(crate::component::shared_component_identity(type_name));
        if !self
            .resource_factories
            .get(&id)
            .is_some_and(|ops| ops.foreign)
        {
            return false;
        }
        self.persist_resource_names.insert(id, type_name.to_owned());
        true
    }

    /// Whether the resource `T` is registered for persistence.
    #[must_use]
    pub fn is_resource_persistable<T: Resource>(&self) -> bool {
        self.persist_resource_names
            .contains_key(&ResourceId::of::<T>())
    }

    /// Capture the value of every persistable resource.
    ///
    /// A resource with no value stored yet is absent rather than written as
    /// null: a snapshot records what the world held, and "never inserted" is
    /// not a value. Restoring such a snapshot leaves the resource as whatever
    /// the loading generation inserts for it.
    #[must_use]
    pub fn snapshot_resources(&self) -> ResourceSnapshot {
        let mut entries = Vec::new();
        for (id, resource) in &self.resources {
            let Some(type_name) = self.persist_resource_names.get(id) else {
                continue;
            };
            // A foreign payload is captured byte for byte. There is no Rust
            // type to ask and nothing to interpret, so the bytes go in as a
            // JSON array of numbers. That is the same rule the descriptor
            // component codec applies to an opaque field: keep what you cannot
            // read.
            if resource.is_foreign() {
                entries.push((type_name.clone(), serialize_foreign_resource(resource)));
                continue;
            }
            let Some(serialize) = self.persist_resource_serializers.get(id) else {
                continue;
            };
            if let Some(bytes) = serialize(self) {
                entries.push((type_name.clone(), bytes));
            }
        }
        // Sort the entries so a snapshot is reproducible: hash-map order is
        // not, and a snapshot that differs between two identical runs cannot be
        // diffed, compared or used as a test fixture.
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        info!(
            target: telemetry_target::HOT_RELOAD,
            "[persistence] Snapshotting {} resource(s)",
            entries.len(),
        );
        ResourceSnapshot { entries }
    }

    /// Restore resource values captured by [`Self::snapshot_resources`].
    ///
    /// A resource the loading generation does not declare is skipped, just as a
    /// component type the project stopped declaring is skipped: a snapshot
    /// records what was there, it does not ask for it to be recreated.
    pub fn restore_resources(&mut self, snapshot: &ResourceSnapshot) {
        let mut restored_count = 0usize;
        let mut skipped_count = 0usize;
        for (type_name, payload) in &snapshot.entries {
            let accepted = match self.persist_resource_restorers.get(type_name).copied() {
                Some(restore) => restore(self, payload),
                // Without a Rust restorer the entry is either a foreign
                // resource, whose bytes go back unchanged, or a name this
                // generation no longer declares.
                None => self.restore_foreign_resource(type_name, payload),
            };
            if accepted {
                restored_count += 1;
            } else {
                skipped_count += 1;
            }
        }
        info!(
            target: telemetry_target::HOT_RELOAD,
            "[persistence] Restored {} resource(s), {} skipped",
            restored_count,
            skipped_count,
        );
    }

    /// Write a foreign resource's bytes back, when this generation declares it.
    ///
    /// Returns whether the payload was accepted. A payload whose size disagrees
    /// with the current declaration is refused, not padded: foreign bytes carry
    /// no field names, so a shorter or longer payload cannot be fitted to the
    /// live layout without a migration, and only the declaring language can
    /// describe one.
    pub(super) fn restore_foreign_resource(&mut self, type_name: &str, payload: &[u8]) -> bool {
        let id = ResourceId::Shared(crate::component::shared_component_identity(type_name));
        let Some(ops) = self.resource_factories.get(&id).copied() else {
            return false;
        };
        if !ops.foreign {
            return false;
        }
        let Some(bytes) = decode_foreign_resource_payload(payload, ops.size) else {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                resource = %type_name,
                expected_size = ops.size,
                "[persistence] Foreign resource payload does not match the declared size; skipped"
            );
            return false;
        };
        self.insert_foreign_resource_bytes(id, &bytes).is_ok()
    }

    /// The persistable Rust resources this generation declares.
    ///
    /// There is one entry per resource with a registered serializer and schema
    /// hash, so a persistable foreign resource, which has neither, is absent.
    /// The reload captures this before the new generation registers its types,
    /// and the pass afterwards compares the hashes and migrates only the
    /// resources whose shape changed. The component twin is
    /// [`Self::persist_type_manifest`].
    #[must_use]
    pub fn persist_resource_manifest(&self) -> Vec<PersistResourceManifestEntry> {
        let mut entries: Vec<PersistResourceManifestEntry> = self
            .persist_resource_names
            .iter()
            .filter_map(|(id, type_name)| {
                Some(PersistResourceManifestEntry {
                    resource_id: *id,
                    type_name: type_name.clone(),
                    schema_hash: *self.persist_resource_schema_hashes.get(type_name)?,
                    serializer: *self.persist_resource_serializers.get(id)?,
                })
            })
            .collect();
        entries.sort_by(|left, right| left.type_name.cmp(&right.type_name));
        entries
    }

    /// Migrate every persistable resource whose schema changed across a reload.
    ///
    /// **Call this while the retiring generation's image is still mapped.** Each
    /// serializer in `previous_manifest` was generated for the old shape and
    /// lives in the outgoing DLL, and it is the only code that can read the
    /// stored value correctly. That is why the manifest is captured before the
    /// swap, and why this runs before the old image is released.
    ///
    /// A value is read through the old serializer, carried across as JSON, and
    /// written back through the arriving type's restorer, which merges it over
    /// the new `Default`. A field added since the previous build therefore
    /// arrives defaulted and a removed field is dropped, exactly as in the
    /// component migration. A resource whose hash is unchanged is left alone,
    /// keeping its allocation and its change ticks.
    ///
    /// Returns the names that were migrated, sorted.
    #[cfg(feature = "development")]
    pub fn migrate_changed_persistable_resources(
        &mut self,
        previous_manifest: &[PersistResourceManifestEntry],
    ) -> Vec<String> {
        // Step 1: Read every changed value through the retiring generation's
        // serializer, while that code is still loaded. All reads happen before
        // any write, so a value that cannot be serialized is warned about and
        // skipped on its own.
        let mut carried_payloads: Vec<(String, Vec<u8>)> = Vec::new();
        for entry in previous_manifest {
            let current_hash = self.persist_resource_schema_hashes.get(&entry.type_name);
            // A name this generation does not know means the resource was
            // dropped, not reshaped; `drop_resources` handles that case. An
            // equal hash means the stored bytes are still valid under the new
            // type, so the resource is left alone.
            let Some(&current_hash) = current_hash else {
                continue;
            };
            if current_hash == entry.schema_hash {
                continue;
            }
            if let Some(payload) = (entry.serializer)(self) {
                carried_payloads.push((entry.type_name.clone(), payload));
            } else {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    resource = %entry.type_name,
                    "[persistence] Reshaped resource held no readable value; leaving it to the new default"
                );
            }
        }

        // Step 2: Write each carried value back through the arriving type.
        let mut migrated_resource_names = Vec::new();
        for (type_name, payload) in carried_payloads {
            let Some(restore) = self.persist_resource_restorers.get(&type_name).copied() else {
                continue;
            };
            if restore(self, &payload) {
                migrated_resource_names.push(type_name);
            }
        }
        migrated_resource_names.sort();
        if !migrated_resource_names.is_empty() {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                migrated_resources = %migrated_resource_names.join(", "),
                "[persistence] Migrated reshaped resources"
            );
        }
        migrated_resource_names
    }
}

// =============================================================================
// Per-Type Monomorphized Functions
// =============================================================================

/// Compute the schema hash for one persistable resource type.
///
/// It works like [`calculate_schema_hash`] but leaves out the field-layout
/// fold, which has no resource counterpart: a Rust resource is not stored in a
/// typed column and declares no field table. A shared resource that does
/// declare a field layout publishes it as [`Resource::shared_schema_hash`], and
/// the claim check compares that hash separately. The two hashes are
/// deliberately different numbers: one checks that two artifacts agree about a
/// layout, the other decides whether a value has to be migrated after a
/// rebuild.
pub(super) fn calculate_resource_schema_hash<T>() -> u64
where
    T: Resource + Serialize + Default,
{
    // Reduce each value to a kind marker (string, number, array, ...) so
    // changing a default value does not look like a schema change and force a
    // pointless migration.
    let default_json = serde_json::to_value(T::default()).unwrap_or(serde_json::Value::Null);
    let normalized_schema = normalize_schema_shape(&default_json);
    let normalized_schema_string = serde_json::to_string(&normalized_schema)
        .unwrap_or_else(|_| "<schema-serialization-failed>".to_string());

    let mut hasher = DefaultHasher::new();
    crate::type_names::stable_type_name::<T>().hash(&mut hasher);
    std::mem::size_of::<T>().hash(&mut hasher);
    normalized_schema_string.hash(&mut hasher);
    hasher.finish()
}

/// Serialize the live value of resource `T` as JSON.
///
/// Returns `None` when the world holds no value for `T`, or when the value
/// cannot be turned into JSON. A failed conversion is logged at warning level.
pub(super) fn serialize_resource<T>(world: &World) -> Option<Vec<u8>>
where
    T: Resource + Serialize,
{
    serde_json::to_vec(world.get_resource::<T>()?)
        .map_err(|error| {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to serialize resource '{}': {}",
                std::any::type_name::<T>(),
                error
            );
        })
        .ok()
}

/// Rebuild resource `T` from JSON and insert it into the world.
///
/// The snapshot payload is merged over a default instance, so a field added
/// since the snapshot arrives with its default value and a field removed since
/// is ignored. That is the same name-matched reconciliation
/// [`deserialize_component`] performs, and it is why a reshaped resource keeps
/// the parts of its value that still mean something.
///
/// Returns `false` when the payload is not JSON, when `T::default()` cannot be
/// turned into JSON, or when the merged value does not deserialize into `T`. A
/// parse or deserialize failure is logged at warning level, and the world keeps
/// the value it already had.
pub(super) fn restore_resource<T>(world: &mut World, bytes: &[u8]) -> bool
where
    T: Resource + Serialize + DeserializeOwned + Default,
{
    let snapshot_json: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to parse JSON for resource '{}': {}",
                std::any::type_name::<T>(),
                error
            );
            return false;
        }
    };
    let Ok(default_json) = serde_json::to_value(T::default()) else {
        return false;
    };
    match serde_json::from_value::<T>(merge_json(default_json, snapshot_json)) {
        Ok(value) => {
            world.insert_resource(value);
            true
        }
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence] Failed to deserialize resource '{}': {}; the value is skipped",
                std::any::type_name::<T>(),
                error
            );
            false
        }
    }
}

/// Encode a foreign resource's payload as a JSON array of byte values.
///
/// Nothing interprets the bytes here, and nothing needs to: the payload is
/// plain memory and its layout is all there is to know about it. Writing it as
/// JSON rather than raw bytes keeps a snapshot in one format, so a reader never
/// has to know whether a resource is declared in Rust or in another language.
pub(super) fn serialize_foreign_resource(resource: &ErasedResource) -> Vec<u8> {
    let bytes: Vec<serde_json::Value> = resource
        .bytes()
        .iter()
        .map(|byte| serde_json::Value::from(*byte))
        .collect();
    serde_json::to_vec(&serde_json::Value::Array(bytes)).unwrap_or_else(|_| b"[]".to_vec())
}

/// Decode the byte array [`serialize_foreign_resource`] wrote.
///
/// Returns `None` unless the payload is an array of exactly `expected_size`
/// numbers that all fit in a byte. Anything else cannot be laid over the
/// declared layout without inventing bytes.
pub(super) fn decode_foreign_resource_payload(
    payload: &[u8],
    expected_size: usize,
) -> Option<Vec<u8>> {
    let parsed: serde_json::Value = serde_json::from_slice(payload).ok()?;
    let array = parsed.as_array()?;
    if array.len() != expected_size {
        return None;
    }
    array
        .iter()
        .map(|value| u8::try_from(value.as_u64()?).ok())
        .collect()
}
