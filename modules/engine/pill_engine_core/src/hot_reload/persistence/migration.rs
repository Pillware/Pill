//! Selective migration: rewriting only the component types a reload changed.
//!
//! # Responsibilities
//!
//! - Runs the selective pass: for each changed type name, it reads the old
//!   value with the retiring generation's serializer and writes it back with
//!   the arriving generation's deserializer and inserter.
//! - Picks one of two strategies for each type: an in-place column swap when
//!   the component id is unchanged, or a full archetype move when the reload
//!   changed the id.
//! - Reports what it migrated and what it skipped, so the host can log both.
//!
//! # Design
//!
//! Everything here must run while the retiring generation's image is still
//! mapped: the serializer came from that image and is the only code that can
//! read the old bytes correctly.
//!
//! In the in-place path, entities the incoming generation spawned during
//! `init` already hold values in the current layout, so those rows are read
//! with the current serializer while every older row goes through the
//! retiring one.

// External crates
use pill_core::telemetry::telemetry_target;

use super::*;

// =============================================================================
// World — Selective Migration
// =============================================================================

impl World {
    /// Migrates only the persistable component types a reload changed.
    ///
    /// For each changed type name it reads the old value with the serializer
    /// captured before the reload, then writes it back through the
    /// deserializer and inserter the new `init` registered. Only the affected
    /// component columns are rewritten.
    ///
    /// `pre_swap_entities` holds the entities that existed before the incoming
    /// generation's `init` ran (see [`Self::capture_live_entities`]). Entities
    /// outside that set were spawned by the new generation and already hold
    /// current-layout values, so a column rebuild reads their rows with the
    /// current serializer instead of the retiring one. Pass `None` when the
    /// arriving generation spawned no entity during `init`, so every entity
    /// predates the migration.
    pub fn migrate_changed_persistable_components(
        &mut self,
        previous_metadata_by_name: &HashMap<String, PersistTypeMetadata>,
        changed_type_names: &HashSet<String>,
        pre_swap_entities: Option<&HashSet<Entity>>,
    ) -> SelectiveMigrationReport {
        let mut migration_report = SelectiveMigrationReport::default();

        // Collect borrowed `&str` names instead of cloning every string into a
        // `Vec<String>`: the sort only makes the report's order stable for the
        // caller's logs, and the names are unique, so `sort_unstable` is fine.
        // The sort runs only when the hot-reload info log is enabled.
        let mut sorted_changed_type_names: Vec<&str> =
            changed_type_names.iter().map(String::as_str).collect();
        if tracing::enabled!(
            target: telemetry_target::HOT_RELOAD,
            tracing::Level::INFO
        ) {
            sorted_changed_type_names.sort_unstable();
        }

        info!(
            target: telemetry_target::HOT_RELOAD,
            "[persistence] Selective migration starting for {} component type(s)",
            sorted_changed_type_names.len(),
        );

        for type_name in &sorted_changed_type_names {
            let Some(previous_metadata) = previous_metadata_by_name.get(*type_name) else {
                let reason = "missing previous metadata".to_string();
                debug!(
                    target: telemetry_target::HOT_RELOAD,
                    "[persistence]   '{}' -> SKIP ({})",
                    type_name,
                    reason,
                );
                migration_report.skipped_types.push(SkippedType {
                    type_name: (*type_name).to_string(),
                    reason,
                });
                continue;
            };

            let current_schema_hash = self
                .persist_schema_hashes
                .get(*type_name)
                .copied()
                .unwrap_or(0);
            info!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence]   '{}' -> migrating (schema {} -> {})",
                type_name, previous_metadata.schema_hash, current_schema_hash,
            );

            // Step 2: Migrate the type in the live storage and, when one is
            // held, in the held storage too. A held column carries the
            // retiring generation's rows and function table, so leaving it
            // alone would leave a column that calls unmapped code.
            let mut migrated_entity_count_for_type = 0;
            let mut failure: Option<String> = None;
            self.run_on_every_entity_storage(|world| {
                match world.migrate_single_component_type(
                    type_name,
                    previous_metadata,
                    pre_swap_entities,
                ) {
                    Ok(migrated_entity_count) => {
                        migrated_entity_count_for_type += migrated_entity_count
                    }
                    Err(error) => failure = Some(error.to_string()),
                }
            });
            match failure {
                None => {
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        "[persistence]   '{}' -> OK ({} entities)",
                        type_name, migrated_entity_count_for_type,
                    );
                    migration_report.migrated_type_count += 1;
                    migration_report.migrated_entity_count += migrated_entity_count_for_type;
                }
                Some(reason) => {
                    debug!(
                        target: telemetry_target::HOT_RELOAD,
                        "[persistence]   '{type_name}' -> SKIP ({reason})",
                    );
                    migration_report.skipped_types.push(SkippedType {
                        type_name: (*type_name).to_string(),
                        reason,
                    });
                }
            }
        }

        info!(
            target: telemetry_target::HOT_RELOAD,
            "[persistence] Selective migration finished: {} type(s) migrated, {} entities touched, {} type(s) skipped",
            migration_report.migrated_type_count,
            migration_report.migrated_entity_count,
            migration_report.skipped_types.len(),
        );

        migration_report
    }

    /// Migrates one persistable component type from its previous registration
    /// to the current one.
    ///
    /// Picks an in-place column swap when the [`ComponentId`] is unchanged, or
    /// a full archetype move when a reload gave the type a new id.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::ComponentTypeUnregistered`] when the type
    /// name no longer resolves to a registered component,
    /// [`PersistenceError::ComponentTypeAmbiguous`] when the name matches more
    /// than one component, [`PersistenceError::DeserializerMissing`] when no
    /// deserializer is registered for the type name,
    /// [`PersistenceError::InserterMissing`] when no inserter is registered
    /// for the resolved component id, and [`PersistenceError::SerializerMissing`]
    /// when the id is unchanged but no serializer is registered for it. Errors
    /// from the in-place or cross-archetype migration below propagate as they
    /// are.
    pub(super) fn migrate_single_component_type(
        &mut self,
        type_name: &str,
        previous_metadata: &PersistTypeMetadata,
        pre_swap_entities: Option<&HashSet<Entity>>,
    ) -> Result<usize, PersistenceError> {
        let current_component_id = match self.resolve_component_id_by_name(type_name) {
            Ok(Some(component_id)) => component_id,
            Ok(None) => {
                return Err(PersistenceError::ComponentTypeUnregistered {
                    type_name: type_name.to_string(),
                })
            }
            Err(WorldError::ComponentNameAmbiguous { count, .. }) => {
                return Err(PersistenceError::ComponentTypeAmbiguous {
                    type_name: type_name.to_string(),
                    count,
                })
            }
            Err(_) => {
                return Err(PersistenceError::ComponentTypeUnregistered {
                    type_name: type_name.to_string(),
                })
            }
        };

        let Some(&deserialize_current_component) = self.persist_deserializers.get(type_name) else {
            return Err(PersistenceError::DeserializerMissing {
                type_name: type_name.to_string(),
            });
        };

        let Some(&insert_current_component) = self.persist_inserters.get(&current_component_id)
        else {
            return Err(PersistenceError::InserterMissing {
                type_name: type_name.to_string(),
            });
        };

        if previous_metadata.component_id == current_component_id {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence]     strategy: in-place column swap for '{}'",
                type_name,
            );
            // Rows spawned by the new generation hold current-layout values,
            // so the rebuild needs the current serializer to read them; the
            // retiring one would read those bytes as the old shape.
            let Some(&serialize_current_component) =
                self.persist_serializers.get(&current_component_id)
            else {
                return Err(PersistenceError::SerializerMissing {
                    type_name: type_name.to_string(),
                });
            };
            self.migrate_component_column_in_place(
                previous_metadata.component_id,
                previous_metadata.serializer,
                serialize_current_component,
                deserialize_current_component,
                insert_current_component,
                pre_swap_entities,
            )
        } else {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                "[persistence]     strategy: archetype remap for '{}' (component id changed)",
                type_name,
            );
            self.migrate_component_across_archetypes(
                previous_metadata.component_id,
                current_component_id,
                previous_metadata.serializer,
                deserialize_current_component,
                insert_current_component,
            )
        }
    }

    /// Rewrites one component column in place in every archetype that holds the
    /// old component id.
    ///
    /// For each such archetype it serializes the old values, decodes them with
    /// the new schema (an empty `{}` is tried when the bytes no longer parse),
    /// removes the old storage column, builds a new one from the registered
    /// storage factory, and inserts the migrated values.
    ///
    /// `serialize_current_component` reads the rows the incoming generation
    /// spawned during `init`. Those rows already hold current-layout values,
    /// and the retiring serializer would read their bytes as the old shape.
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::SerializationFailed`] when the serializer of
    /// the generation that wrote a value refuses it, and
    /// [`PersistenceError::DeserializationFailed`] when a value cannot be
    /// decoded from either its own bytes or `{}`.
    ///
    /// Also returns [`PersistenceError::StorageRemovalFailed`] when the old
    /// storage column cannot be removed,
    /// [`PersistenceError::StorageFactoryMissing`] when no storage factory is
    /// registered, and [`PersistenceError::NativeStorageExpected`] when the
    /// component id is not native storage or its registered factory is not a
    /// native factory.
    ///
    /// # Panics
    ///
    /// Panics when the registered native layout cannot describe an allocation.
    /// The layout comes from a Rust type at registration time, so this can only
    /// happen if the registration itself is broken.
    pub(super) fn migrate_component_column_in_place(
        &mut self,
        component_id: ComponentId,
        serialize_retiring_component: SerializeComponentFn,
        serialize_current_component: SerializeComponentFn,
        deserialize_current_component: DeserializeComponentFn,
        insert_current_component: InsertComponentFn,
        pre_swap_entities: Option<&HashSet<Entity>>,
    ) -> Result<usize, PersistenceError> {
        // Step 1: Find every archetype that stores the old component id.
        let archetype_ids: Vec<_> = self
            .storage
            .archetypes
            .iter()
            .filter(|(_, archetype)| archetype.component_types.contains(&component_id))
            .map(|(archetype_id, _)| *archetype_id)
            .collect();

        let mut migrated_entity_count: usize = 0;

        for archetype_id in archetype_ids {
            // Step 2: Serialize every value before the column is removed.
            // Entities the new generation spawned are missing from the pre-swap
            // set, so their rows already hold current-layout values and go
            // through the current serializer.
            let serialized_components: Vec<Vec<u8>> = {
                let Some(archetype) = self.storage.archetypes.get(&archetype_id) else {
                    continue;
                };

                let mut serialized = Vec::with_capacity(archetype.entities.len());
                for entity_index in 0..archetype.entities.len() {
                    let spawned_this_generation = pre_swap_entities.is_some_and(|entities| {
                        !entities.contains(&archetype.entities[entity_index])
                    });
                    // If a serializer refuses a value, the error is returned
                    // before the column is touched, so the archetype still
                    // holds every row.
                    let Some(bytes) = (if spawned_this_generation {
                        serialize_current_component(&archetype.component_storages, entity_index)
                    } else {
                        serialize_retiring_component(&archetype.component_storages, entity_index)
                    }) else {
                        return Err(PersistenceError::SerializationFailed { component_id });
                    };
                    serialized.push(bytes);
                }
                serialized
            };

            // Step 3: Decode each value with the new schema. When the bytes no
            // longer parse, an empty `{}` object is tried instead.
            let mut migrated_components: Vec<Box<dyn Component>> =
                Vec::with_capacity(serialized_components.len());

            for bytes in &serialized_components {
                let component = deserialize_current_component(bytes)
                    .or_else(|| deserialize_current_component(b"{}"))
                    .ok_or(PersistenceError::DeserializationFailed { component_id })?;
                migrated_components.push(component);
            }

            // Step 4: Replace the storage column. Remove the old one, build a
            // new one from the registered storage factory, and insert the
            // migrated values.
            let Some(archetype) = self.storage.archetypes.get_mut(&archetype_id) else {
                continue;
            };

            if !component_id.is_native_storage() {
                return Err(PersistenceError::NativeStorageExpected { component_id });
            }
            // Drop the old column through the code that wrote it. The re-home
            // pass already pointed every column at the arriving generation's
            // function table, and old-layout values must not be released by
            // code that never allocated them. `register_component_inner` saved
            // the retiring table when the incoming registration replaced it.
            if let Some(retiring_ops) = self.retired_native_storage_ops.get(&component_id) {
                if let Some(column) = archetype.component_storages.get_mut(component_id) {
                    column.refresh_ops(*retiring_ops);
                }
            }
            if archetype.component_storages.remove(component_id).is_none() {
                return Err(PersistenceError::StorageRemovalFailed { component_id });
            }

            let Some(storage_factory) = self.storage_factories.get(&component_id) else {
                return Err(PersistenceError::StorageFactoryMissing { component_id });
            };
            let crate::archetype::StorageFactory::Native(native_column_info) = storage_factory
            else {
                return Err(PersistenceError::NativeStorageExpected { component_id });
            };
            // Build the replacement column from the native layout rather than a
            // trait object, so it stays valid across module unloads.
            archetype.component_storages.insert(
                component_id,
                crate::archetype::ComponentColumn::from_native_info(*native_column_info, 0)
                    .expect("a registered native layout is valid by construction"),
            );

            for component in migrated_components {
                insert_current_component(&mut archetype.component_storages, component);
            }

            migrated_entity_count += serialized_components.len();
        }

        Ok(migrated_entity_count)
    }

    /// Moves every entity from the archetypes that hold the old component id
    /// into archetypes that hold the new one.
    ///
    /// For each entity it serializes the old component value and decodes it
    /// with the new schema (an empty `{}` is tried when the bytes no longer
    /// parse), moves the unchanged components across byte for byte, and inserts
    /// the migrated value. A reload uses this path when it gives a type a new
    /// [`ComponentId`].
    ///
    /// # Errors
    ///
    /// Returns [`PersistenceError::SerializationFailed`] when the retiring
    /// generation's serializer refuses a value; the source archetype is put
    /// back before that error is returned.
    ///
    /// Returns [`PersistenceError::DeserializationFailed`] when a migrated value
    /// cannot be decoded from either its stored bytes or `{}`,
    /// [`PersistenceError::DestinationArchetypeMissing`] when the destination
    /// archetype cannot be created, and [`PersistenceError::ColumnMoveFailed`]
    /// when an unchanged column cannot hand its row to the destination.
    pub(super) fn migrate_component_across_archetypes(
        &mut self,
        retiring_component_id: ComponentId,
        current_component_id: ComponentId,
        serialize_retiring_component: SerializeComponentFn,
        deserialize_current_component: DeserializeComponentFn,
        insert_current_component: InsertComponentFn,
    ) -> Result<usize, PersistenceError> {
        // Step 1: Find every archetype that holds the old component id.
        let source_archetype_ids: Vec<_> = self
            .storage
            .archetypes
            .iter()
            .filter(|(_, archetype)| archetype.component_types.contains(&retiring_component_id))
            .map(|(archetype_id, _)| *archetype_id)
            .collect();

        let mut migrated_entity_count: usize = 0;

        for source_archetype_id in source_archetype_ids {
            // Step 2: Remove the source archetype and work out which ids its
            // destination holds: the same set with the old id replaced by the
            // new one.
            let Some(mut source_archetype) = self.storage.archetypes.remove(&source_archetype_id)
            else {
                continue;
            };

            let mut destination_component_ids: Vec<ComponentId> = source_archetype
                .component_types
                .iter()
                .map(|component_id| {
                    if *component_id == retiring_component_id {
                        current_component_id
                    } else {
                        *component_id
                    }
                })
                .collect();

            destination_component_ids.sort();
            destination_component_ids.dedup();

            let destination_archetype_id = self.get_or_create_archetype(destination_component_ids);

            // Step 3: Serialize the old component values and decode them with
            // the new schema, trying `{}` when the bytes no longer parse. A
            // refused serialization puts the source archetype back before the
            // error is returned, so a skipped type leaves the world as it was.
            let mut serialized_components: Vec<Vec<u8>> =
                Vec::with_capacity(source_archetype.entities.len());
            for entity_index in 0..source_archetype.entities.len() {
                let Some(bytes) = serialize_retiring_component(
                    &source_archetype.component_storages,
                    entity_index,
                ) else {
                    self.storage
                        .archetypes
                        .insert(source_archetype_id, source_archetype);
                    return Err(PersistenceError::SerializationFailed {
                        component_id: retiring_component_id,
                    });
                };
                serialized_components.push(bytes);
            }

            let mut migrated_components: Vec<Box<dyn Component>> =
                Vec::with_capacity(serialized_components.len());

            for bytes in &serialized_components {
                let component = deserialize_current_component(bytes)
                    .or_else(|| deserialize_current_component(b"{}"))
                    .ok_or(PersistenceError::DeserializationFailed {
                        component_id: retiring_component_id,
                    })?;
                migrated_components.push(component);
            }

            // Step 4: Move each entity into the destination archetype, hand
            // over its unchanged components, insert the migrated one, then
            // record the entity's new location and its components' ticks.
            //
            // The rows are taken back to front. `take_row_from` removes a row
            // by moving the column's last row into that slot, and going back to
            // front keeps the taken row last, so no other row moves and the
            // waiting rows keep the index their value was read at.
            let Some(destination_archetype) =
                self.storage.archetypes.get_mut(&destination_archetype_id)
            else {
                return Err(PersistenceError::DestinationArchetypeMissing);
            };

            let current_tick = crate::component::Tick::new(self.change_tick);

            for (entity_index, component) in migrated_components.into_iter().enumerate().rev() {
                let entity = source_archetype.entities[entity_index];
                let destination_index = destination_archetype.entities.len();
                destination_archetype.entities.push(entity);

                for source_component_id in &source_archetype.component_types {
                    // The old column keeps its rows: they hold the retiring
                    // generation's values, which were read above, and they are
                    // released when the source archetype is dropped.
                    if *source_component_id == retiring_component_id {
                        continue;
                    }

                    let Some(source_column) = source_archetype
                        .component_storages
                        .get_mut(*source_component_id)
                    else {
                        continue;
                    };
                    let Some(destination_column) = destination_archetype
                        .component_storages
                        .get_mut(*source_component_id)
                    else {
                        return Err(PersistenceError::ColumnMoveFailed {
                            component_id: *source_component_id,
                        });
                    };
                    // The row moves instead of being copied: its bytes and
                    // whatever they own travel together, and the source column
                    // releases the row without running its destructor.
                    destination_column
                        .take_row_from(source_column, entity_index)
                        .map_err(|_| PersistenceError::ColumnMoveFailed {
                            component_id: *source_component_id,
                        })?;
                }

                insert_current_component(&mut destination_archetype.component_storages, component);

                // An unchanged column moved its row's ticks along with the row.
                // The migrated component did not, because its value was decoded
                // from the old column, so its ticks are copied across from that
                // column now. That keeps a migrated component from looking
                // freshly added to every `Changed<T>` system.
                let rewritten_ticks = source_archetype
                    .component_storages
                    .get(retiring_component_id)
                    .and_then(|column| column.row_ticks(entity_index))
                    .copied()
                    .unwrap_or(crate::component::ComponentTicks::new(current_tick));
                if let Some(column) = destination_archetype
                    .component_storages
                    .get_mut(current_component_id)
                {
                    column.set_row_ticks(destination_index, rewritten_ticks);
                }

                self.storage.entity_locations.insert(
                    entity,
                    crate::world::EntityLocation {
                        archetype_id: destination_archetype_id,
                        index_in_archetype: destination_index,
                    },
                );
            }

            migrated_entity_count += source_archetype.entities.len();
        }

        // Step 5: Add one to the archetype generation, so cached query plans
        // notice the new archetype layout.
        self.storage.archetype_generation = self.storage.archetype_generation.wrapping_add(1);
        Ok(migrated_entity_count)
    }
}
