//! Reflection and diagnostics: what the world can say about itself.
//!
//! # Responsibilities
//!
//! - Answers the field-layout questions an editor asks: which fields a
//!   component or resource declares, and where each one sits in a row.
//! - Enumerates the world for a tool rather than for a system: the entities
//!   that exist, the component names attached to each, and which types are
//!   registered at all.
//! - Prints the archetype list, builds a Graphviz DOT description of the
//!   archetype graph, and estimates the world's memory for telemetry.
//!
//! # Design
//!
//! Most of these methods only read the world, and they identify components and
//! resources by name or id rather than by type, because the callers are tools
//! (the editor, the managed side) that never hold the Rust type. A name is
//! also the one identity that survives a reload: a tool keeps names between
//! generations and resolves the ids again in each new one.

// External crates
use pill_core::telemetry::telemetry_target;

use super::*;

// =============================================================================
// World - Reflection and Diagnostics
// =============================================================================

impl World {
    /// Returns the field layout the editor reads: nested `struct:` fields
    /// flattened into one dotted row per leaf (`position.x`).
    ///
    /// `None` when the component was registered without field metadata. The
    /// editor treats that as a component it cannot edit field by field.
    pub fn component_field_layout(
        &self,
        component_id: ComponentId,
    ) -> Option<&[crate::component_registry::ComponentFieldDescriptor]> {
        self.component_field_layouts
            .get(&component_id)
            .map(ComponentFieldLayout::fields)
    }

    /// Returns the same layout as declared, with no nested field flattened.
    ///
    /// This is the form a generated mirror has to describe: the editor's view
    /// names leaf rows (`position.x`, `points[0].y`) that no struct declares.
    /// The C# codegen reads this to emit a typed mirror, and falls back to the
    /// opaque ABI blob when a component was registered without field metadata
    /// (`None` here).
    pub fn component_declared_field_layout(
        &self,
        component_id: ComponentId,
    ) -> Option<&[crate::component_registry::ComponentFieldDescriptor]> {
        self.component_field_layouts
            .get(&component_id)
            .map(ComponentFieldLayout::declared)
    }

    /// Returns the field layout of a foreign resource.
    ///
    /// This is the resource twin of [`Self::component_field_layout`]. A
    /// resource's layout is never flattened, so its declared and editor views
    /// are the same list. `None` means no layout was recorded, the ordinary
    /// state for a Rust resource: its fields belong to a Rust type, not to a
    /// manifest.
    #[must_use]
    pub fn resource_field_layout(
        &self,
        resource_id: ResourceId,
    ) -> Option<&[crate::component_registry::ComponentFieldDescriptor]> {
        self.resource_field_layouts
            .get(&resource_id)
            .map(ComponentFieldLayout::fields)
    }

    /// Records the declared field layout of a foreign resource.
    ///
    /// Overwrites any previous layout for the same id, as
    /// [`Self::register_component_descriptor_with_layout`] does. **A relayout
    /// clears it**, so it has to be called again after one. Serving the
    /// previous generation's offsets over migrated bytes would be worse than
    /// serving nothing, so a caller that forgets loses inspectability, not
    /// correctness.
    ///
    /// # Errors
    ///
    /// Returns [`ComponentFieldError::UnsupportedField`](crate::component_field::ComponentFieldError::UnsupportedField)
    /// when a field's offset and size overflow, when a field ends past the
    /// registered size of the resource, or when a field carries a container
    /// tag. The size check keeps a reader inside the value, and the tag check
    /// keeps it from following a `(pointer, length)` pair that the bytes never
    /// held. A resource with no registered factory has no size, so the check
    /// has nothing to compare against.
    pub fn register_resource_field_layout(
        &mut self,
        resource_id: ResourceId,
        fields: Vec<crate::component_registry::ComponentFieldDescriptor>,
    ) -> Result<(), crate::component_field::ComponentFieldError> {
        let resource_size = self
            .resource_factories
            .get(&resource_id)
            .map(|ops| ops.size);
        for field in &fields {
            let Some(field_end) = field.offset.checked_add(field.size) else {
                return Err(
                    crate::component_field::ComponentFieldError::UnsupportedField {
                        field: field.name.to_string(),
                        reason: "the field's range overflows",
                    },
                );
            };
            if resource_size.is_some_and(|size| field_end > size) {
                return Err(
                    crate::component_field::ComponentFieldError::UnsupportedField {
                        field: field.name.to_string(),
                        reason: "the field extends past the resource's registered size",
                    },
                );
            }
            if crate::component_field::is_container_tag(field.type_tag) {
                return Err(crate::component_field::ComponentFieldError::UnsupportedField {
                    field: field.name.to_string(),
                    reason: "a container tag is read as a native pointer and length, which a foreign resource's bytes do not hold",
                });
            }
        }
        self.resource_field_layouts
            .insert(resource_id, ComponentFieldLayout::from_owned(fields));
        Ok(())
    }

    /// Returns every live entity with the type names of its components, sorted
    /// by the entity's slot id.
    ///
    /// Read-only: nothing is mutated and no change tick is touched, so the
    /// editor can call it on every refresh without disturbing the simulation.
    /// Component names are resolved once per archetype and copied to each of
    /// that archetype's entities.
    pub fn entity_rows(&self) -> Vec<EntityRow> {
        // Step 1: Name each archetype's components once, so the string work is
        // per archetype instead of per entity.
        let mut component_names_by_archetype: HashMap<ArchetypeId, Vec<String>> =
            HashMap::with_capacity(self.storage.archetypes.len());
        for (archetype_id, archetype) in &self.storage.archetypes {
            let component_names: Vec<String> = archetype
                .component_types
                .iter()
                .filter_map(|component_id| {
                    self.component_registry
                        .get_name(component_id)
                        .map(str::to_string)
                })
                .collect();
            component_names_by_archetype.insert(*archetype_id, component_names);
        }

        // Step 2: Build one row per live entity, copying its archetype's names.
        let mut rows: Vec<EntityRow> = self
            .storage
            .entity_locations
            .iter()
            .map(|(entity, location)| EntityRow {
                entity: *entity,
                components: component_names_by_archetype
                    .get(&location.archetype_id)
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect();

        // Step 3: Sort by slot id so the editor's list keeps the same order.
        rows.sort_by_key(|row| row.entity.id());
        rows
    }

    /// Returns every live entity, sorted by the entity's slot id.
    ///
    /// The handle-only twin of [`Self::entity_rows`], in the same order. No
    /// component names are copied, so a caller that walks a large world every
    /// frame pays one allocation for the whole list.
    pub fn live_entities(&self) -> Vec<Entity> {
        let mut entities: Vec<Entity> = self.storage.entity_locations.keys().copied().collect();
        entities.sort_by_key(|entity| entity.id());
        entities
    }

    /// Returns the component type names of one entity, or `None` when the
    /// entity is not alive.
    ///
    /// The names come from the components the entity's own archetype stores,
    /// not from a scan of the registry, so a type that another generation
    /// registered, but this entity does not have, never appears.
    pub fn entity_component_names(&self, entity: Entity) -> Option<Vec<String>> {
        let location = self.storage.entity_locations.get(&entity)?;
        let archetype = self.storage.archetypes.get(&location.archetype_id)?;
        Some(
            archetype
                .component_types
                .iter()
                .filter_map(|component_id| {
                    self.component_registry
                        .get_name(component_id)
                        .map(str::to_string)
                })
                .collect(),
        )
    }

    /// Returns every component the registry still holds: type name plus its
    /// [`ComponentId`], sorted by name.
    ///
    /// The name is the key that survives a reload. A native component's id is
    /// its Rust `TypeId`, which belongs to the build that declared the type
    /// and is not stable across a reload, so an id must not be kept. The
    /// registry may hold one entry per generation of a name.
    ///
    /// Used by the editor's component list (the add-component picker and the
    /// snapshot's component tier) and by the C# bridge, to name the component a
    /// binding points at.
    pub fn registered_components(&self) -> Vec<(String, ComponentId)> {
        let mut components: Vec<(String, ComponentId)> = self
            .component_registry
            .registered_components()
            .map(|(component_id, _, name)| (name.to_string(), component_id))
            .collect();
        components.sort();
        components
    }

    /// Returns whether a component type was registered as persistable.
    ///
    /// A persistable type registers the functions that save and restore its
    /// rows, and a hash of its field schema, which is how a reload knows which
    /// columns to migrate when the type's layout changed. The editor records
    /// this per component in its snapshot.
    pub fn component_is_persistable(&self, component_id: ComponentId) -> bool {
        self.persist_inserters.contains_key(&component_id)
    }

    /// Returns the component id an entity's archetype stores for a registered
    /// type name, or `None` when the entity does not have that component.
    ///
    /// Unlike [`Self::resolve_component_id_by_name_any`], which searches every
    /// generation the registry still remembers, this looks only at the columns
    /// the entity really has. The answer therefore stays correct across
    /// reloads, even when a bit index was reused, and it always names a column
    /// of this entity's own archetype rather than one from another generation.
    pub fn resolve_entity_component_id(
        &self,
        entity: Entity,
        type_name: &str,
    ) -> Option<ComponentId> {
        let location = self.storage.entity_locations.get(&entity)?;
        let archetype = self.storage.archetypes.get(&location.archetype_id)?;
        archetype
            .component_types
            .iter()
            .copied()
            .find(|component_id| {
                self.component_registry
                    .name_matches(component_id, type_name)
            })
    }

    /// Returns the byte size and alignment of a registered component's
    /// storage.
    ///
    /// Works for native (Rust) and descriptor (foreign-language) components
    /// alike, and returns `None` when no component is registered under the id.
    /// The C# backend uses it to check that a managed mirror struct has the
    /// same ABI layout as the component an extension registered.
    pub fn component_layout(&self, component_id: ComponentId) -> Option<(usize, usize)> {
        match self.storage_factories.get(&component_id) {
            Some(StorageFactory::Native(info)) => Some((info.size, info.align)),
            Some(StorageFactory::Descriptor(layout)) => Some((layout.size, layout.align)),
            None => None,
        }
    }

    /// Logs one block describing every archetype in the world.
    ///
    /// Each line is one archetype with its id, its entity count and its
    /// component type names, followed by a line with the world's total entity
    /// count. Useful when debugging the ECS.
    pub fn print_archetypes(&self) {
        let mut lines: Vec<String> = self
            .storage
            .archetypes
            .values()
            .map(|archetype| archetype.get_archetype_info(&self.component_registry))
            .collect();
        lines.push(format!(
            "Total entities: {}",
            self.storage.entity_locations.len()
        ));
        pill_core::info!(
            target: telemetry_target::ECS,
            "{}",
            pill_core::telemetry::log_block(
                &format!("World archetypes (total: {})", self.storage.archetypes.len()),
                lines
            )
        );
    }

    /// Estimates the world's total memory use in bytes.
    ///
    /// Adds up the archetype storage (component columns, entities and change
    /// ticks), the entity location map, the two stable-id maps, the resources,
    /// the free-id list, the storage factories and the script updaters. The
    /// per-entry numbers are fixed allowances rather than exact sizes, so the
    /// result is an estimate.
    pub fn memory_estimate(&self) -> usize {
        let mut total_bytes = 0usize;

        // Archetype storage: component columns, entities and change ticks.
        for archetype in self.storage.archetypes.values() {
            total_bytes += archetype.memory_estimate();
        }

        // Entity location map, charged at a flat 48 bytes per entry for the
        // key, the value and the table overhead.
        total_bytes += self.storage.entity_locations.len() * 48;

        // The two stable-id maps, handle to id and id back to handle. Between
        // them they hold two entries per live entity, with 16-byte keys and
        // values, each charged at 48 bytes.
        total_bytes += self.storage.entity_ids.len() * 48;
        total_bytes += self.storage.entity_handles.len() * 48;

        // Resources, at a flat allowance per type.
        total_bytes += self.resources.len() * 128;

        // The recycled-id free list, at its reserved capacity.
        total_bytes += self.storage.free_entity_ids.capacity() * 12;

        // Storage factories and the registered scripts, whose record is a
        // component id, an optional adapter and an order.
        total_bytes += self.storage_factories.len() * 128;
        total_bytes += self.script_registrations.len() * 64;

        total_bytes
    }

    /// Builds a Graphviz DOT description of the world: one node per archetype,
    /// labelled with its component names and entity count.
    ///
    /// A component whose id has no registered name appears as `?`.
    ///
    /// ```dot
    /// digraph World {
    ///     rankdir=LR;
    ///     node [shape=record];
    ///     "arch_0" [label="Position, Velocity | 3 entities"];
    /// }
    /// ```
    #[cold]
    pub fn to_dot_graph(&self) -> String {
        let mut dot = String::from("digraph World {\n    rankdir=LR;\n    node [shape=record];\n");

        for (_, archetype) in self.storage.archetypes.iter() {
            let component_names: Vec<String> = archetype
                .component_types
                .iter()
                .map(|component_id| {
                    self.component_registry
                        .get_name(component_id)
                        .unwrap_or("?")
                        .to_string()
                })
                .collect();
            let label = format!(
                "{} | {} entit{}",
                component_names.join(", "),
                archetype.entities.len(),
                if archetype.entities.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            dot.push_str(&format!(
                "    \"arch_{:?}\" [label=\"{}\"];\n",
                archetype.id.0, label
            ));
        }

        dot.push_str("}\n");
        dot
    }
}
