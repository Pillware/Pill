//! Resource storage: registration, claims, foreign values and re-homing.
//!
//! # Responsibilities
//!
//! - Stores one value per resource type, native or foreign, and hands it out
//!   through `Res<T>` and `ResMut<T>`.
//! - Decides which type owns a shared name when two binaries both declare it,
//!   so the second one reads the same value instead of creating its own.
//! - Re-homes a live resource's function table across a reload and retires the
//!   claims a retiring generation leaves behind.
//! - Declares the asset types a generation owns, and prunes the import registry
//!   once a retired generation's registrations are dead.
//!
//! # Design
//!
//! A resource is a singleton, so it has no archetype and no row. The store is a
//! map from id to a box that holds the value together with plain data about its
//! type: a drop function, size, alignment, an optional schema hash and a
//! foreign flag. That data is not a trait object's vtable. Component columns
//! work the same way and for the same reason: the table can be re-pointed at
//! the newest generation, so the destructor that runs always belongs to code
//! that is still mapped.
//!
//! A foreign resource is the same store with no Rust type behind it: plain
//! bytes plus a declared layout. That is what lets managed code own a resource
//! the host has never heard of.

// External crates
use pill_core::telemetry::telemetry_target;
use trait_type_map::TraitAccessible;

// Current crate
use super::*;
use crate::asset::{Asset, AssetManager};
use crate::asset_import_registry::{
    ImportRegistrationError, ImportRegistrationMarker, ImportRegistry,
};

// =============================================================================
// World - Resource Management
// =============================================================================

impl World {
    /// Number of resources the world holds.
    pub fn resource_count(&self) -> usize {
        self.resources.len()
    }

    /// Inserts a resource: a value that exists once and is not attached to any
    /// entity.
    ///
    /// Replaces an existing value of the same type. If another type has already
    /// claimed this resource's shared name, the claim is refused instead: the
    /// error is recorded, so the registering `init` fails, and **nothing is
    /// stored**. Storing it anyway would leave a live resource with a shape no
    /// reader accepts. When the two layouts happen to match, it would also let
    /// one type read another type's bytes.
    pub fn insert_resource<T: Resource>(&mut self, resource: T) {
        self.insert_resource_with_table(resource, ErasedResourceOps::of::<T>());
    }

    /// Inserts a manager resource whose tuning lives in a settings asset.
    ///
    /// The value is stored exactly like [`Self::insert_resource`] stores one,
    /// and its per-type table carries the surface the type declared
    /// ([`SettingsAssetManager`](crate::settings::SettingsAssetManager)): the
    /// settings name, the asset type's extension and the two appliers that
    /// render a created file and apply an edited one. That is what lets a host
    /// create, bind and re-apply the manager's settings without knowing its
    /// document type.
    ///
    /// A manager that must survive a reload needs a `shared_name`: an id built
    /// from `TypeId` differs in every generation, so the value would be retired
    /// instead of kept (see [`Resource::shared_name`]).
    pub fn insert_settings_asset_manager<T: crate::settings::SettingsAssetManager>(
        &mut self,
        resource: T,
    ) {
        self.insert_resource_with_surface(resource, |table| table.with_settings_asset::<T>());
    }

    /// The insert half both manager paths share: keep whatever surface the
    /// recorded table already carries, so a type that declares two of them -
    /// a flat document and a settings asset - keeps both.
    fn insert_resource_with_surface<T: Resource>(
        &mut self,
        resource: T,
        add_surface: impl FnOnce(ErasedResourceOps) -> ErasedResourceOps,
    ) {
        let id = ResourceId::of::<T>();
        let base = self
            .resource_factories
            .get(&id)
            .copied()
            .filter(|resource_ops| !resource_ops.foreign)
            .unwrap_or_else(ErasedResourceOps::of::<T>);
        self.insert_resource_with_table(resource, add_surface(base));
    }

    /// The ids of the live resources whose play state is `play_state`.
    ///
    /// The stop half of a play session uses it to find the `Restore` resources
    /// the game inserted, which have no copy to put back.
    pub fn resource_ids_with_play_state(
        &self,
        play_state: crate::resource::PlayState,
    ) -> Vec<crate::resource::ResourceId> {
        self.resources
            .keys()
            .copied()
            .filter(|resource_id| self.resource_play_states.get(resource_id) == Some(&play_state))
            .collect()
    }

    /// Keeps `resource_copy` aside for the end of the play session that is
    /// starting.
    pub(crate) fn hold_resource_copy(&mut self, resource_copy: crate::world::ResourceCopy) {
        self.held_resource_copy = Some(resource_copy);
    }

    /// Puts every copied `Restore` resource back and removes the ones the game
    /// inserted.
    ///
    /// Returns whether a copy was held, which means a session was running. A
    /// `Restore` resource with no copy was not in the world at Play, so its live
    /// value belongs to the session and is removed; a `Keep` or `Rebuild`
    /// resource is not in the copy and keeps whatever the game did to it.
    pub(crate) fn restore_held_resources(&mut self) -> bool {
        let Some(resource_copy) = self.held_resource_copy.take() else {
            return false;
        };
        let mut copied_ids: Vec<crate::resource::ResourceId> =
            resource_copy.values.keys().copied().collect();
        copied_ids.extend(resource_copy.persisted_values.keys().copied());
        for (resource_id, value) in resource_copy.values {
            self.put_back_resource(resource_id, value);
        }
        // A copied resource whose type is not `Clone` comes back through its
        // own deserializer, the same path a snapshot load takes.
        for (resource_id, payload) in resource_copy.persisted_values {
            let Some(type_name) = self.persist_resource_names.get(&resource_id).cloned() else {
                continue;
            };
            let Some(restore) = self.persist_resource_restorers.get(&type_name).copied() else {
                continue;
            };
            if !restore(self, &payload) {
                pill_core::warn!(
                    target: telemetry_target::ECS,
                    resource = %type_name,
                    "[engine] a restored resource could not be read back; its live value stays"
                );
            }
        }
        let play_inserted: Vec<crate::resource::ResourceId> = self
            .resource_ids_with_play_state(crate::resource::PlayState::Restore)
            .into_iter()
            .filter(|resource_id| {
                !copied_ids.contains(resource_id)
                    && !resource_copy
                        .dropped_by_reload
                        .iter()
                        .any(|(dropped_id, _)| dropped_id == resource_id)
            })
            .collect();
        for resource_id in play_inserted {
            self.remove_resource_value(resource_id);
        }
        // A reload that dropped a copy took the value it would have put back, so
        // the session's live value stays and the person is told which one.
        for (_, type_name) in resource_copy.dropped_by_reload {
            pill_core::warn!(
                target: telemetry_target::ECS,
                resource = %type_name,
                "[engine] a reload dropped this resource's held copy during the session; its live value stays"
            );
        }
        true
    }

    /// The storage half of every insert path, taking the per-type table the
    /// caller built.
    fn insert_resource_with_table<T: Resource>(&mut self, resource: T, table: ErasedResourceOps) {
        let _zone = crate::profile_scope!(
            "insert resource",
            [(
                "Resource type being inserted: {}",
                std::any::type_name::<T>()
            )]
        );
        let id = ResourceId::of::<T>();
        // Step 1: Check the claim, and stop before storing anything when it was
        // refused.
        if !self.claim_shared_resource_name::<T>() {
            return;
        }
        // Step 2: Store the value with its change tick, and stamp the
        // registration.
        let tick = Tick::new(self.change_tick);
        self.note_resource_registration(id);
        self.resources.insert(id, ErasedResource::new(resource));
        self.resource_ticks.insert(id, ComponentTicks::new(tick));
        // The value's play-mode class and its copy function: what a Play copy
        // needs to know about it. The type itself answers the second one, since
        // this insert is generic and cannot tell whether `T` is `Clone`.
        self.resource_play_states.insert(id, T::play_state());
        self.resource_clone_functions.insert(id, T::clone_erased);
        // Step 3: Record the table too, so a later reload can re-home this
        // value even if the next generation never inserts it again.
        let table = self.table_keeping_recorded_surface(id, table);
        self.resource_factories.insert(id, table);
    }

    /// The table to record for `id`: `incoming`, keeping the settings surface an
    /// earlier registration recorded when the incoming table carries none.
    ///
    /// A module that owns a manager records the surface in its own `register`.
    /// A project that links the module declares the same derived type from its
    /// own image, and that plain table would otherwise erase the surface,
    /// leaving the host unable to find the manager's settings asset. The
    /// surface describes the type, so a declaration that says nothing about
    /// settings must not remove it.
    ///
    /// A foreign table neither gains nor donates one: its bytes have no Rust
    /// type for an applier to cast to.
    fn table_keeping_recorded_surface(
        &self,
        id: ResourceId,
        incoming: ErasedResourceOps,
    ) -> ErasedResourceOps {
        if incoming.foreign {
            return incoming;
        }
        match self.resource_factories.get(&id) {
            Some(recorded) if !recorded.foreign && incoming.settings_asset.is_none() => {
                ErasedResourceOps {
                    settings_asset: recorded.settings_asset,
                    ..incoming
                }
            }
            _ => incoming,
        }
    }

    /// Returns a shared reference to the resource, or `None` when nothing is
    /// stored under its id or the stored value cannot be read as `T`.
    pub fn get_resource<T: Resource>(&self) -> Option<&T> {
        let _zone = crate::profile_scope!(
            "get resource",
            [(
                "Resource type being accessed (immutable): {}",
                std::any::type_name::<T>()
            )]
        );
        self.resources
            .get(&ResourceId::of::<T>())
            .and_then(ErasedResource::get::<T>)
    }

    /// Returns a mutable reference to the resource, or `None` when nothing is
    /// stored under its id or the stored value cannot be read as `T`.
    ///
    /// Prefer [`get_resource_mut_tracked`] for system parameters: it also marks
    /// the resource changed when the value is mutated.
    pub fn get_resource_mut<T: Resource>(&mut self) -> Option<&mut T> {
        self.resources
            .get_mut(&ResourceId::of::<T>())
            .and_then(ErasedResource::get_mut::<T>)
    }

    /// Returns the resource mutably, with change tracking, or `None` when
    /// nothing is stored under its id or the stored value cannot be read as
    /// `T`.
    ///
    /// The result is a [`Mut<'_, T>`] wrapping the value and its
    /// [`ComponentTicks`]. Mutating through `DerefMut` sets `ticks.changed` to
    /// the current world tick, which is how [`ResMut`](crate::query::ResMut)
    /// reports a change.
    ///
    /// # Panics
    ///
    /// Panics when the resource has no change-tick entry. `insert_resource`
    /// writes the value and its ticks together, so that state means the two
    /// maps have gone out of sync.
    pub fn get_resource_mut_tracked<T: Resource>(&mut self) -> Option<Mut<'_, T>> {
        let _zone = crate::profile_scope!(
            "get resource mut tracked",
            [(
                "Resource type being accessed (mutable, tracked): {}",
                std::any::type_name::<T>()
            )]
        );
        let id = ResourceId::of::<T>();
        let value: &mut T = self
            .resources
            .get_mut(&id)
            .and_then(ErasedResource::get_mut::<T>)?;
        let ticks: &mut ComponentTicks = self.resource_ticks.get_mut(&id).unwrap_or_else(|| {
            // `insert_resource` writes `resources` and `resource_ticks`
            // together, so a resource in one map and not the other means the
            // two have gone out of sync. The panic names the resource so the
            // reader can find it.
            panic!(
                "resource {id:?} exists in resources but has no change-tick \
                     column; resource_ticks has fallen out of sync with resources"
            )
        });
        let this_run = Tick::new(self.change_tick);
        Some(Mut::new(value, ticks, this_run))
    }

    /// Declares that this artifact owns the resource type `T`, without
    /// inserting a value.
    ///
    /// The resource equivalent of [`Self::register_component`]. A resource's
    /// stored drop function points into the artifact that inserted the value.
    /// A reloaded generation that keeps the existing value never calls
    /// `insert_resource`, so without this call nothing would contribute a table
    /// pointing at code that is still mapped.
    ///
    /// An artifact does not call this. It derives `PillResource` on the type,
    /// and the artifact entry point declares every derived type before the
    /// artifact's own `register` runs - on the first load, on every reload and
    /// in a statically linked build alike. This method is what the entry point
    /// calls, and what a host uses for a resource it owns itself.
    ///
    /// It is idempotent, so a generation that declares the type again is
    /// harmless. A refused claim records its error and leaves the stored table
    /// alone, so a generation that disagrees about who owns a name, or about
    /// the name's shape, cannot re-point the live resource at its own code
    /// before its init fails.
    pub fn register_resource<T: Resource>(&mut self) {
        self.register_resource_claiming::<T>();
    }

    /// Declares that this artifact owns a manager resource type, without
    /// inserting a value.
    ///
    /// Declares that this artifact owns a manager type whose tuning lives in a
    /// settings asset, without inserting a value.
    ///
    /// The manager counterpart of [`Self::register_resource`]: the stored table
    /// gains the type's settings-asset surface, so a host that already holds the
    /// live value - or that finds no value at all - still knows the settings
    /// name, the extension and the appliers.
    ///
    /// A manager's crate calls this from its own `register`, which runs on every
    /// generation: the surface holds function pointers into the image that
    /// registered it, so each generation re-points them.
    pub fn register_settings_asset_manager<T: crate::settings::SettingsAssetManager>(&mut self) {
        self.register_resource_surface::<T>(|table| table.with_settings_asset::<T>());
    }

    /// The declaration half both manager registrations share.
    ///
    /// Whatever surface the recorded table already carries is kept: another
    /// registration in the same generation put it there, so it points at code
    /// that is still mapped.
    fn register_resource_surface<T: Resource>(
        &mut self,
        add_surface: impl FnOnce(ErasedResourceOps) -> ErasedResourceOps,
    ) {
        let id = ResourceId::of::<T>();
        if !self.claim_shared_resource_name::<T>() {
            return;
        }
        let base = self
            .resource_factories
            .get(&id)
            .copied()
            .filter(|resource_ops| !resource_ops.foreign)
            .unwrap_or_else(ErasedResourceOps::of::<T>);
        self.resource_factories.insert(id, add_surface(base));
        self.note_resource_registration(id);
    }

    /// The declaration half of every register path, taking the per-type table
    /// the caller built.
    fn register_resource_with_table<T: Resource>(&mut self, table: ErasedResourceOps) -> bool {
        let id = ResourceId::of::<T>();
        if !self.claim_shared_resource_name::<T>() {
            return false;
        }
        let table = self.table_keeping_recorded_surface(id, table);
        self.resource_factories.insert(id, table);
        self.note_resource_registration(id);
        true
    }

    /// [`Self::register_resource`], reporting whether the claim was accepted.
    ///
    /// The public call ignores the result because its callers cannot act on a
    /// refusal: the error is recorded, and the `init` that raised it fails. The
    /// persistable registration checks it, because a refused generation must
    /// not install a serializer that reads the live value through code that is
    /// about to be discarded.
    pub(crate) fn register_resource_claiming<T: Resource>(&mut self) -> bool {
        self.register_resource_with_table::<T>(ErasedResourceOps::of::<T>())
    }

    /// Declares a resource defined by another language, without storing a
    /// value.
    ///
    /// The foreign-language counterpart of [`Self::register_resource`], for a
    /// type this process has no Rust definition of. The id is a hash of the
    /// declared name. [`Resource::shared_identity`] hashes a Rust type's name
    /// the same way, so a managed declaration and a Rust type that write the
    /// same string share one resource. The scheduler sees them as one too,
    /// because it tracks resources by id.
    ///
    /// The payload is blittable bytes (plain bytes that can be copied as they
    /// are), described by `size` and `align` and stored by
    /// [`Self::insert_foreign_resource_bytes`]. Nothing here runs a destructor,
    /// so a foreign value outlives the assembly that produced it.
    ///
    /// `declaring_type` is the declaring language's own name for the type (the
    /// managed full name, for the C# path). It is recorded so that a later Rust
    /// declaration can be compared against it; two declarations from another
    /// language are never compared by name (see
    /// [`Self::claim_shared_resource`]).
    ///
    /// # Errors
    ///
    /// [`WorldError::ForeignResourceNameEmpty`] for an empty name,
    /// [`WorldError::ForeignResourceLayoutInvalid`] for a layout that cannot
    /// describe an allocation, [`WorldError::SharedResourceNameConflict`] when
    /// two Rust types disagree about the name,
    /// [`WorldError::SharedResourceIdentityCollision`] when two different names
    /// hash to one identity, [`WorldError::SharedResourceLayoutMismatch`] when
    /// the declared layout disagrees with the one already claimed,
    /// [`WorldError::SharedResourceSchemaMismatch`] when both declarations
    /// carry a field-shape hash and the two hashes differ, and
    /// [`WorldError::ForeignResourceHoldsRustValue`] when a Rust value is
    /// stored under the id.
    pub fn register_foreign_resource(
        &mut self,
        shared_name: &str,
        declaring_type: &str,
        size: usize,
        align: usize,
        schema_hash: u64,
    ) -> Result<ResourceId, WorldError> {
        // Step 1: Check the declaration itself.
        if shared_name.is_empty() {
            return Err(WorldError::ForeignResourceNameEmpty);
        }
        if !is_valid_foreign_layout(size, align) {
            return Err(WorldError::ForeignResourceLayoutInvalid { size, align });
        }
        // Step 2: Claim the name, which also checks the layout and the schema
        // hash against an earlier declaration.
        let id = ResourceId::Shared(crate::component::shared_component_identity(shared_name));
        if let Some(error) = self.claim_shared_resource(
            id,
            shared_name,
            declaring_type,
            size,
            align,
            true,
            Some(schema_hash),
        ) {
            return Err(error);
        }
        // Step 3: Refuse an id that already holds a Rust value. Such a value
        // fixes what is under the id - its destructor is bound to its type - so
        // a foreign declaration cannot take the id over.
        // `relayout_foreign_resource` refuses the same state; refusing here too
        // means the disagreement never reaches `rehome_resources`.
        if self
            .resources
            .get(&id)
            .is_some_and(|resource| !resource.is_foreign())
        {
            return Err(WorldError::ForeignResourceHoldsRustValue { id });
        }
        // Step 4: Record the table and the registration.
        self.resource_factories
            .insert(id, ErasedResourceOps::foreign(size, align, schema_hash));
        self.note_resource_registration(id);
        Ok(id)
    }

    /// Stores a value for a shared resource, from another language's bytes.
    ///
    /// Replaces a foreign payload under the id and sets the change tick, because
    /// a value appearing counts as a change. A Rust value is never replaced:
    /// its destructor is bound to its type, so accepting foreign bytes would drop
    /// it through code that never allocated it. That refusal is
    /// [`WorldError::ForeignResourceFactoryIsNative`].
    ///
    /// # Errors
    ///
    /// [`WorldError::SharedResourceNotRegistered`] when the id is not a
    /// registered shared resource, [`WorldError::ForeignResourceFactoryIsNative`]
    /// when the id's table is a Rust type's, and
    /// [`WorldError::ForeignResourceBytesMismatch`] when the payload is not
    /// exactly the registered size.
    pub fn insert_foreign_resource_bytes(
        &mut self,
        id: ResourceId,
        bytes: &[u8],
    ) -> Result<(), WorldError> {
        let Some(resource_ops) = self.shared_resource_ops(id) else {
            return Err(WorldError::SharedResourceNotRegistered { id });
        };
        if !resource_ops.foreign {
            return Err(WorldError::ForeignResourceFactoryIsNative { id });
        }
        if bytes.len() != resource_ops.size {
            return Err(WorldError::ForeignResourceBytesMismatch {
                id,
                expected: resource_ops.size,
                actual: bytes.len(),
            });
        }
        let tick = Tick::new(self.change_tick);
        self.resources.insert(
            id,
            ErasedResource::new_foreign(
                bytes,
                resource_ops.align,
                resource_ops.schema_hash.unwrap_or(0),
            ),
        );
        self.resource_ticks.insert(id, ComponentTicks::new(tick));
        self.note_resource_registration(id);
        Ok(())
    }

    /// The stored bytes of a shared resource, for another language to read.
    ///
    /// `None` when the id is not a registered shared resource, when nothing is
    /// stored under it yet, or when the stored value is a Rust one. Bytes are
    /// only handed out for foreign payloads, because their declared layout is
    /// the only description they have.
    pub fn foreign_resource_bytes(&self, id: ResourceId) -> Option<&[u8]> {
        self.foreign_resource_ops(id)?;
        self.resources
            .get(&id)
            .filter(|resource| resource.is_foreign())
            .map(ErasedResource::bytes)
    }

    /// The stored bytes of a shared resource, mutably, with its ticks.
    ///
    /// The `changed` tick is set when the bytes are handed out, not on the first
    /// write: a caller that receives raw bytes has no wrapper that could notice
    /// a write, so the resource counts as changed from the moment of the borrow.
    ///
    /// A caller writing here is responsible for writing a value the stored type
    /// accepts, exactly as a generated mirror is for the component rows it
    /// writes in place.
    ///
    /// A Rust value refuses, as it does for [`Self::foreign_resource_bytes`],
    /// and the refusal costs nothing: the `changed` tick is not set for a view
    /// that is never handed out.
    pub fn foreign_resource_bytes_mut(
        &mut self,
        id: ResourceId,
    ) -> Option<(&mut [u8], &mut ComponentTicks)> {
        self.foreign_resource_ops(id)?;
        if !self.resources.get(&id)?.is_foreign() {
            return None;
        }
        let changed = Tick::new(self.change_tick);
        let ticks = self.resource_ticks.get_mut(&id)?;
        ticks.set_changed(changed);
        let bytes = self.resources.get_mut(&id)?.bytes_mut();
        Some((bytes, ticks))
    }

    /// The address of a Rust value stored under `id`, for a mirrored call that
    /// passes the resource as `&T` or `&mut T`.
    ///
    /// The counterpart of [`Self::foreign_resource_bytes_mut`] for Rust values:
    /// the caller does not get bytes to interpret. It gets the address of the
    /// value, which a generated forwarding function casts back to the type it
    /// was compiled against. A write access sets the `changed` tick when the
    /// pointer is handed out, for the same reason the foreign view does.
    ///
    /// `None` when nothing is stored under `id`, or when the stored value is a
    /// foreign payload - there is no Rust type there to cast to.
    pub fn native_resource_pointer(
        &mut self,
        id: ResourceId,
        is_write_access: bool,
    ) -> Option<*mut u8> {
        if self.resources.get(&id)?.is_foreign() {
            return None;
        }
        if is_write_access {
            let changed = Tick::new(self.change_tick);
            if let Some(ticks) = self.resource_ticks.get_mut(&id) {
                ticks.set_changed(changed);
            }
        }
        Some(self.resources.get_mut(&id)?.bytes_mut().as_mut_ptr())
    }

    /// The declared layout of a foreign resource: size, alignment, schema
    /// hash.
    ///
    /// `None` when `id` is not a registered foreign declaration.
    /// [`Self::remap_foreign_resource`] reads it to find the payload sizes its
    /// plan converts between.
    pub fn foreign_resource_layout(&self, id: ResourceId) -> Option<(usize, usize, u64)> {
        let resource_ops = self.resource_factories.get(&id).copied()?;
        if !resource_ops.foreign {
            return None;
        }
        Some((
            resource_ops.size,
            resource_ops.align,
            resource_ops.schema_hash?,
        ))
    }

    /// Replaces a foreign resource's declared layout, migrating its value.
    ///
    /// The resource counterpart of [`Self::relayout_descriptor_component`], and
    /// a much smaller one: a resource holds a single value, so there are no
    /// archetypes and no rows to reshape. `plan` comes from the same two field
    /// lists the component path uses ([`FieldPlan::between`]), and any bytes it
    /// does not cover are left zero.
    ///
    /// Only a foreign payload is rewritten. A Rust value's shape *is* its type,
    /// so a declaration that disagrees with it is a conflict to report, not a
    /// migration to perform.
    ///
    /// Returns 1 when a value was rewritten and 0 when only the declaration
    /// moved, which is the case for a resource nobody has stored yet.
    ///
    /// # Errors
    ///
    /// [`WorldError::ForeignResourceNotRegistered`] when the id is not a
    /// registered foreign declaration,
    /// [`WorldError::ForeignResourceLayoutInvalid`] for a layout that cannot
    /// describe an allocation,
    /// [`WorldError::ForeignResourcePlanOutOfBounds`] when the plan does not fit
    /// the old or the new payload, and
    /// [`WorldError::ForeignResourceHoldsRustValue`] when a Rust value is stored
    /// under the id.
    #[cfg(feature = "development")]
    pub fn relayout_foreign_resource(
        &mut self,
        id: ResourceId,
        size: usize,
        align: usize,
        schema_hash: u64,
        plan: &FieldPlan,
    ) -> Result<usize, WorldError> {
        // Step 1: Check the new declaration against the id's recorded table.
        if !self
            .resource_factories
            .get(&id)
            .is_some_and(|resource_ops| resource_ops.foreign)
        {
            return Err(WorldError::ForeignResourceNotRegistered { id });
        }
        if !is_valid_foreign_layout(size, align) {
            return Err(WorldError::ForeignResourceLayoutInvalid { size, align });
        }
        if self
            .resources
            .get(&id)
            .is_some_and(|resource| !resource.is_foreign())
        {
            return Err(WorldError::ForeignResourceHoldsRustValue { id });
        }

        // Step 2: Move the recorded claim to the new shape.
        let next_resource_ops = ErasedResourceOps::foreign(size, align, schema_hash);
        // The claim records the shape a new declaration is checked against, so
        // it moves with the table on both paths. If it stayed behind, the
        // migrated shape could not be declared again, and a stale declaration
        // could put the old table back.
        if let Some(claim) = self.shared_resource_claims.get_mut(&id) {
            claim.size = size;
            claim.align = align;
            claim.schema_hash = Some(schema_hash);
        }
        // Step 3: Reshape a stored payload, or record the new table alone when
        // no value is stored yet.
        let Some(stored_resource) = self.resources.get_mut(&id) else {
            self.resource_factories.insert(id, next_resource_ops);
            self.resource_field_layouts.remove(&id);
            return Ok(0);
        };
        stored_resource
            .migrate_bytes(size, align, plan)
            .map_err(|_error| WorldError::ForeignResourcePlanOutOfBounds { id })?;
        self.resource_factories.insert(id, next_resource_ops);
        // The stored field layout described the shape that just changed, so it
        // is removed instead of being read against the migrated bytes. The
        // declarer registers it again, exactly as it does for a component.
        self.resource_field_layouts.remove(&id);
        Ok(1)
    }

    /// Moves a foreign resource's value onto another id and retires the source
    /// declaration.
    ///
    /// The resource counterpart of [`Self::remap_descriptor_component`], and
    /// the rename step of a managed resource manifest. A renamed type declares
    /// a new name, and with it a new id, so the stored value is reshaped
    /// through `plan` (built from the source's field list and the successor's)
    /// and written under the successor. The source is then dropped. A payload
    /// the successor already holds is replaced; in the common case the
    /// successor is freshly declared and holds an all-zero seed.
    ///
    /// The source's claim count moves to the successor, so the value is released
    /// only when no other artifact still declares the source's name, and the
    /// successor ends up protected exactly as the source was.
    ///
    /// Returns 1 when a value was moved and 0 when the source had none stored,
    /// which is the case for a resource nobody has written yet.
    ///
    /// # Errors
    ///
    /// [`WorldError::ForeignResourceRemapSelf`] when the source and the
    /// successor are the same id,
    /// [`WorldError::ForeignResourceNotRegistered`] when either id is not a
    /// registered foreign declaration,
    /// [`WorldError::ForeignResourcePlanOutOfBounds`] when the plan does not
    /// fit the source or the successor payload, and
    /// [`WorldError::ForeignResourceHoldsRustValue`] when a Rust value is
    /// stored under either id.
    #[cfg(feature = "development")]
    pub fn remap_foreign_resource(
        &mut self,
        source_id: ResourceId,
        successor_id: ResourceId,
        plan: &FieldPlan,
    ) -> Result<usize, WorldError> {
        if source_id == successor_id {
            return Err(WorldError::ForeignResourceRemapSelf);
        }
        let Some((source_size, _, _)) = self.foreign_resource_layout(source_id) else {
            return Err(WorldError::ForeignResourceNotRegistered { id: source_id });
        };
        let Some((successor_size, _, _)) = self.foreign_resource_layout(successor_id) else {
            return Err(WorldError::ForeignResourceNotRegistered { id: successor_id });
        };
        if self
            .resources
            .get(&source_id)
            .is_some_and(|resource| !resource.is_foreign())
            || self
                .resources
                .get(&successor_id)
                .is_some_and(|resource| !resource.is_foreign())
        {
            return Err(WorldError::ForeignResourceHoldsRustValue { id: source_id });
        }

        // Step 1: Read and reshape the stored value before anything moves.
        // `FieldPlan::validate` reports its own error type (it is written for
        // component rows), so the refusal is translated into the resource's
        // variant here.
        let mut moved_value_count = 0;
        if let Some(source_bytes) = self.foreign_resource_bytes(source_id).map(<[u8]>::to_vec) {
            plan.validate(source_size, successor_size)
                .map_err(|_| WorldError::ForeignResourcePlanOutOfBounds { id: source_id })?;
            let mut successor_bytes = vec![0_u8; successor_size];
            plan.apply(&source_bytes, &mut successor_bytes);
            // Step 2: Write it under the successor. The insert stamps the
            // change tick, so a reader sees the move as a change.
            self.insert_foreign_resource_bytes(successor_id, &successor_bytes)?;
            moved_value_count = 1;
        }

        // Step 3: Move the claims and drop the source. The count moves in one
        // step rather than being released one claim at a time: whoever declared
        // the source's name still needs the value under the successor's id.
        let claim_count = self.resource_claim_counts.remove(&source_id).unwrap_or(0);
        if claim_count > 0 {
            *self.resource_claim_counts.entry(successor_id).or_insert(0) += claim_count;
        }
        self.drop_resources(&[source_id]);
        Ok(moved_value_count)
    }

    /// The per-type table of a registered shared resource, if the id names one.
    ///
    /// Only a shared id has a table another language can reach. An ordinary
    /// resource is identified by its `TypeId`, which no other language can
    /// name, so it also has no byte view to hand out. The `?` on
    /// [`ResourceId::shared_identity`] is what makes an ordinary id return
    /// `None` here.
    fn shared_resource_ops(&self, id: ResourceId) -> Option<ErasedResourceOps> {
        id.shared_identity()?;
        self.resource_factories.get(&id).copied()
    }

    /// The per-type table of a registered shared resource, when that table
    /// belongs to foreign bytes.
    ///
    /// The byte accessors go through this one. `shared_resource_ops` only says
    /// whether the id is shared at all, which is all the foreign insert needs:
    /// it tells an ordinary id apart from one whose table belongs to a Rust
    /// type. A byte view needs more. It only makes sense for a payload with no
    /// Rust type anywhere, so the table must say foreign here, and the callers
    /// also check the stored box before they hand out any bytes.
    fn foreign_resource_ops(&self, id: ResourceId) -> Option<ErasedResourceOps> {
        self.shared_resource_ops(id)
            .filter(|resource_ops| resource_ops.foreign)
    }

    /// Stamps one resource registration, keeping one entry per id.
    ///
    /// The stamp is the id's most recent registration. After an artifact's
    /// `init`, the host compares [`Self::resource_ids_registered_since`] with
    /// the list the previous generation produced, to learn which resources the
    /// new generation still claims. A generation keeps a claim by registering
    /// the type again, through `insert_resource` or `register_resource`. One
    /// entry per id keeps the map as small as the number of distinct resources,
    /// and the comparison reports each id once.
    fn note_resource_registration(&mut self, id: ResourceId) {
        self.resource_registration_stamps
            .insert(id, self.resource_registration_sequence);
        self.resource_registration_sequence = self.resource_registration_sequence.wrapping_add(1);
    }

    /// Sequence marker to capture before an artifact's `init`, so the resource
    /// types that `init` registers can be enumerated afterwards.
    ///
    /// Mirrors [`Self::component_registration_sequence`].
    pub fn resource_registration_sequence(&self) -> u64 {
        self.resource_registration_sequence
    }

    /// Resource ids registered at or after `sequence`.
    ///
    /// The comparison is `>=`: a registration reads the current value and then
    /// increments it, so one happening immediately after the capture carries
    /// exactly the captured value.
    pub fn resource_ids_registered_since(&self, sequence: u64) -> Vec<ResourceId> {
        let mut ids: Vec<ResourceId> = self
            .resource_registration_stamps
            .iter()
            .filter(|(_, stamped)| **stamped >= sequence)
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// The layout declaration of the type registered under `id`, copied out of
    /// the artifact that registered it.
    ///
    /// Returns `None` when nothing is registered under the id, or when the type
    /// declared neither sources nor crates. `None` is not an empty declaration:
    /// it is the state that keeps the behaviour which predates the declaration,
    /// so a caller has no decision to make.
    ///
    /// The copy is the point. The declaration sits in the image of the artifact
    /// that registered it, and a reader such as the development host keeps its
    /// answer past that image's retirement, so it must not keep the slices.
    #[must_use]
    #[cfg(feature = "development")]
    pub fn kept_layout_declaration(&self, id: ResourceId) -> Option<KeptLayoutDeclaration> {
        let resource_ops = self.resource_factories.get(&id)?;
        KeptLayoutDeclaration::from_ops(*resource_ops)
    }

    /// Names declared by the shared resources this world holds a claim for,
    /// sorted and deduplicated.
    ///
    /// The resource counterpart of the component registry's shared-identity
    /// list, and what the ECS report prints. A shared resource's whole point is
    /// that no `TypeId` names it, so the declared name is all there is to
    /// report.
    #[must_use]
    pub fn shared_resource_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .shared_resource_claims
            .values()
            .map(|claim| claim.shared_name.clone())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Checks and records which type owns a shared resource name.
    ///
    /// Returns whether the caller may go on to register. An ordinary resource
    /// always may: its id *is* its `TypeId`, so a second claim on one id comes
    /// from the same type. A shared claim may when it is accepted, and may not
    /// when it was refused; a refusal also records its error.
    ///
    /// A refusal must stop the caller before it touches stored state, not only
    /// fail the init afterwards. A failed reload rolls the generation back by
    /// re-running the previous `init`, which re-registers the types it owned
    /// rather than restoring values. A value written under a refused claim would
    /// therefore outlive the generation that wrote it, in a shape nothing can
    /// read.
    ///
    /// A shared resource's id comes from a name written down in code, so two
    /// unrelated types can reach the same id. The last segment of the declaring
    /// type's path tells them apart: two copies of one type agree on it, two
    /// different types do not. `ComponentRegistry` applies the same rule to
    /// shared components.
    fn claim_shared_resource_name<T: Resource>(&mut self) -> bool {
        let Some(shared_name) = T::shared_name() else {
            return true;
        };
        match self.claim_shared_resource(
            ResourceId::of::<T>(),
            shared_name,
            crate::component::ComponentRegistry::declaring_type_name::<T>(),
            std::mem::size_of::<T>(),
            std::mem::align_of::<T>(),
            false,
            <T as Resource>::shared_schema_hash(),
        ) {
            None => true,
            Some(error) => {
                // Recorded here rather than in `claim_shared_resource`: the
                // Rust path is the one whose refusal has to fail the init that
                // raised it. A foreign declaration's caller gets the error back
                // and acts on it itself.
                self.record_registration_error(error);
                false
            }
        }
    }

    /// The type-independent half of [`Self::claim_shared_resource_name`].
    ///
    /// A declaration from another language has no Rust type to ask, so it hands
    /// over the same facts its manifest carries. `foreign` decides one thing:
    /// whether the declaring names are compared. A foreign declaration has no
    /// Rust name to compare with the last segment of a type path, and writing
    /// the shared name down is itself a statement of which resource it means.
    /// For a foreign declaration, then, the layout is the check.
    ///
    /// The recorded claim keeps its Rust declarer when a foreign declaration
    /// joins, because that record is what a later Rust declaration is compared
    /// against. A Rust declaration that arrives at a foreign claim does take the
    /// record, for the same reason in reverse. This function does not record the
    /// error it returns: the Rust wrapper above records it as a registration
    /// error, while a foreign declaration's caller acts on the refusal itself.
    // Seven arguments describe one declaration. The name, the declaring type,
    // the layout, the foreign flag and the schema hash all come from the
    // manifest or the Rust type, and the caller passes each one because none
    // can be worked out from the others. `id` is the exception: it is the
    // name's hash, but the caller has already built it.
    #[allow(clippy::too_many_arguments)]
    fn claim_shared_resource(
        &mut self,
        id: ResourceId,
        shared_name: &str,
        declaring_type: &str,
        size: usize,
        align: usize,
        foreign: bool,
        schema_hash: Option<u64>,
    ) -> Option<WorldError> {
        let incoming = SharedResourceClaim {
            shared_name: shared_name.to_string(),
            declaring_type: declaring_type.to_string(),
            foreign,
            size,
            align,
            schema_hash,
        };
        if let Some(existing) = self.shared_resource_claims.get(&id) {
            // The id is a hash of the name, so two different names sharing it
            // are an identity collision: one slot would answer to both. The
            // recorded string is what tells the two apart.
            if existing.shared_name != shared_name {
                let error = WorldError::SharedResourceIdentityCollision {
                    shared_name: shared_name.to_string(),
                    existing_name: existing.shared_name.clone(),
                };
                error!(
                    target: telemetry_target::ECS,
                    shared_name = %shared_name,
                    existing_name = %existing.shared_name,
                    error = %error,
                    "two shared resource names hash to one identity"
                );
                return Some(error);
            }
            if !existing.foreign
                && !incoming.foreign
                && existing.declaring_type != incoming.declaring_type
            {
                // Logged as well as returned, exactly as `register_component`
                // logs its own failures: the log line is what names the cause
                // to whoever reads the host output.
                let error = WorldError::SharedResourceNameConflict {
                    shared_name: shared_name.to_string(),
                    existing_type: existing.declaring_type.clone(),
                    incoming_type: incoming.declaring_type.clone(),
                };
                error!(
                    target: telemetry_target::ECS,
                    shared_name = %shared_name,
                    existing_type = %existing.declaring_type,
                    incoming_type = %incoming.declaring_type,
                    error = %error,
                    "shared resource name claimed by two different types"
                );
                return Some(error);
            }
            if existing.size != incoming.size || existing.align != incoming.align {
                let error = WorldError::SharedResourceLayoutMismatch {
                    shared_name: shared_name.to_string(),
                    existing_size: existing.size,
                    existing_align: existing.align,
                    incoming_size: incoming.size,
                    incoming_align: incoming.align,
                };
                error!(
                    target: telemetry_target::ECS,
                    shared_name = %shared_name,
                    existing_size = existing.size,
                    existing_align = existing.align,
                    incoming_size = incoming.size,
                    incoming_align = incoming.align,
                    error = %error,
                    "shared resource registered with two different layouts"
                );
                return Some(error);
            }
            // Both sides carrying a hash means both declared a field shape, and
            // equal size/align is exactly the case a hash catches: {u32, u32}
            // and {f32, f32} agree on layout and not on fields.
            if let (Some(existing_hash), Some(incoming_hash)) =
                (existing.schema_hash, incoming.schema_hash)
            {
                if existing_hash != incoming_hash {
                    let error = WorldError::SharedResourceSchemaMismatch {
                        shared_name: shared_name.to_string(),
                        existing_hash,
                        incoming_hash,
                    };
                    error!(
                        target: telemetry_target::ECS,
                        shared_name = %shared_name,
                        error = %error,
                        "shared resource registered with two different field shapes"
                    );
                    return Some(error);
                }
            }
        }
        match self.shared_resource_claims.get(&id) {
            None => {
                self.shared_resource_claims.insert(id, incoming);
            }
            // A foreign declaration does not overwrite the record: keeping a
            // Rust declarer's name is what lets a later Rust declaration still
            // be compared against it. A Rust declaration joining a foreign claim
            // does take the record, because from then on there is a Rust name
            // worth comparing.
            Some(existing) if existing.foreign && !incoming.foreign => {
                self.shared_resource_claims.insert(id, incoming);
            }
            Some(_) => {}
        }
        None
    }

    /// Re-homes every stored resource's per-type function table.
    ///
    /// The resource counterpart of [`Self::rehome_native_columns`], called at
    /// the same point in the reload. A resource's drop function belongs to
    /// whichever artifact inserted it, and that artifact's image is about to be
    /// released from the reload graveyard (the retired images the host keeps
    /// mapped for a while). The table is refreshed here, while every image is
    /// still mapped.
    ///
    /// Three cases are left alone. A type that no artifact registered this
    /// generation has nothing newer to point at - a resource whose owning
    /// artifact retired, which [`Self::drop_resources`] releases while that
    /// image is still mapped. A foreign box's table drops nothing and points at
    /// this crate's code; refreshing it from the factories would let a Rust type
    /// sharing the name hand its destructor to bytes it does not own. And a
    /// table that claims foreign ownership over a Rust value - a state the
    /// registration paths refuse, though a stale table could still reach it - is
    /// reported rather than installed, since the value would otherwise leak
    /// without a word.
    #[cfg(feature = "development")]
    pub fn rehome_resources(&mut self) {
        for (id, resource) in &mut self.resources {
            if resource.is_foreign() {
                continue;
            }
            if let Some(&resource_ops) = self.resource_factories.get(id) {
                if resource_ops.foreign {
                    error!(
                        target: telemetry_target::ECS,
                        resource = ?id,
                        "a Rust resource is declared foreign; keeping its own drop"
                    );
                    continue;
                }
                resource.refresh_ops(resource_ops);
            }
        }
        // A play session's copies are values of the same types, so they need the
        // arriving generation's table exactly as the live values do.
        if let Some(resource_copy) = &mut self.held_resource_copy {
            for (id, value) in &mut resource_copy.values {
                if value.is_foreign() {
                    continue;
                }
                if let Some(&resource_ops) = self.resource_factories.get(id) {
                    if !resource_ops.foreign {
                        value.refresh_ops(resource_ops);
                    }
                }
            }
        }
    }

    /// Declares an asset type the calling generation owns.
    ///
    /// The asset counterpart of [`Self::register_component`]: an asset column's
    /// per-type table is code from the artifact that filled it. An artifact that
    /// can be reloaded therefore declares the asset types it owns during
    /// registration, and [`Self::rehome_assets`] later points those columns at
    /// the generation that is still mapped. Using the type
    /// ([`AssetManager::add`]) declares it too, so this call only matters for a
    /// type the generation owns but does not touch in this run.
    pub fn register_asset<T>(&mut self)
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        match self.get_resource_mut::<AssetManager>() {
            Some(asset_manager) => asset_manager.register::<T>(),
            None => {
                let mut asset_manager = AssetManager::new();
                asset_manager.register::<T>();
                self.insert_resource(asset_manager);
            }
        }
    }

    /// Declares an asset type that can be imported from a source file, by
    /// extension, through the world's
    /// [`ImportRegistry`](crate::asset_import_registry::ImportRegistry).
    ///
    /// Declares the asset type ([`Self::register_asset`]) and records `T`'s
    /// import entry points, which are compiled into the calling binary.
    ///
    /// An artifact does not call this. The crate that owns `T` derives
    /// `PillAsset` with `#[pill(imported)]`, and the artifact entry point
    /// declares every derived type on every generation, before the artifact's
    /// own `register` runs. This method is what the entry point calls, and what
    /// a host uses for an asset type it owns itself.
    ///
    /// The entry points live in the calling binary, so this also inserts a
    /// marker resource that the calling generation claims. When the host retires
    /// that claim - because the module stopped registering `T` or was unloaded -
    /// it drops the marker while the image is still mapped, and the registry
    /// stops calling into it.
    ///
    /// Registering a type again adds a newer registration that takes
    /// precedence, so a project that links the crate and a module that owns it
    /// can both declare it; the older one resumes when the newer generation is
    /// retired.
    ///
    /// A type whose extension another live type already imports is refused
    /// with [`WorldError::ImportExtensionClaimedTwice`], recorded as a
    /// registration error so the registering `init` fails.
    pub fn register_imported_asset<T>(&mut self)
    where
        T: crate::asset_metadata::ImportedAsset,
    {
        self.register_asset::<T>();
        self.record_import_registration::<T>(|registry, alive| registry.register::<T>(alive));
    }

    /// Declares an asset type stored as its own file in `res` - a standalone
    /// asset such as a material - so it loads by its file extension through
    /// the world's [`ImportRegistry`](crate::asset_import_registry::ImportRegistry).
    ///
    /// The standalone counterpart of [`Self::register_imported_asset`], with the
    /// same liveness marker and the same refusal of an extension another type
    /// already imports. An artifact declares such a type by deriving `PillAsset`
    /// with `#[pill(standalone)]`, and never by calling this method.
    pub fn register_standalone_asset<T>(&mut self)
    where
        T: crate::asset_standalone::StandaloneAsset,
    {
        self.register_asset::<T>();
        self.record_import_registration::<T>(|registry, alive| {
            registry.register_standalone::<T>(alive)
        });
    }

    /// Records one import registration of `T` with `register_import`, behind a
    /// fresh liveness marker this generation claims.
    fn record_import_registration<T: 'static>(
        &mut self,
        register_import: impl FnOnce(
            &mut crate::asset_import_registry::ImportRegistry,
            std::sync::Weak<()>,
        ) -> Result<
            (),
            crate::asset_import_registry::ImportRegistrationError,
        >,
    ) {
        let import_registration_marker = ImportRegistrationMarker::<T>::new();
        // The registry is claimed on every generation, not only by the first
        // insert. A reload that kept the registry but did not claim it would
        // look like this module had stopped owning it, and the host would drop
        // it when the init finished.
        self.register_resource::<ImportRegistry>();
        if self.get_resource::<ImportRegistry>().is_none() {
            self.insert_resource(ImportRegistry::default());
        }
        let import_registration = register_import(
            self.get_resource_mut::<ImportRegistry>()
                .expect("inserted just above"),
            import_registration_marker.alive(),
        );
        match import_registration {
            // Replacing a previous generation's marker ends that generation's
            // registration; the one just recorded holds this marker instead.
            Ok(()) => self.insert_resource(import_registration_marker),
            Err(ImportRegistrationError::ExtensionClaimedTwice {
                extension,
                registered_type,
                incoming_type,
            }) => self.record_registration_error(WorldError::ImportExtensionClaimedTwice {
                extension,
                registered_type,
                incoming_type,
            }),
        }
    }

    /// Re-homes every asset column's per-type function table.
    ///
    /// The third member of the re-homing family, called by the reload beside
    /// [`Self::rehome_native_columns`] and [`Self::rehome_resources`] and for
    /// the same reason: the asset store outlives every reload, so a column whose
    /// table still belongs to a retiring image has to be re-pointed while that
    /// image is mapped.
    ///
    /// This also prunes the import registry. The reload drops the marker
    /// resources of a retired generation's import registrations before
    /// re-homing, so this is when those registrations become dead; pruning here
    /// removes them at once instead of at the next registration. A dead entry is
    /// never called either way, so this only stops the registry from holding
    /// stale entries.
    #[cfg(feature = "development")]
    pub fn rehome_assets(&mut self) {
        if let Some(asset_manager) = self.get_resource_mut::<AssetManager>() {
            asset_manager.rehome();
        }
        if let Some(import_registry) =
            self.get_resource_mut::<crate::asset_import_registry::ImportRegistry>()
        {
            import_registry.prune();
        }
    }

    /// Drops the named resources, releasing their values.
    ///
    /// The resource counterpart of
    /// [`drop_forgotten_components`](Self::drop_forgotten_components), with the
    /// same timing requirement: **call it while the artifact that inserted the
    /// value is still mapped.** A resource holds a drop function belonging to
    /// that artifact, so dropping it after the image is evicted would call
    /// through a dangling pointer.
    ///
    /// The host uses it when a reloaded module stops registering a resource
    /// type the retiring generation owned. Compare
    /// [`resource_ids_registered_since`](Self::resource_ids_registered_since)
    /// across the two generations to find those ids. An id that another artifact
    /// still claims through [`retain_resource_claims`](Self::retain_resource_claims)
    /// is skipped, because the value's drop function belongs to the generation
    /// that inserted it, not necessarily to the artifact retiring now.
    ///
    /// Returns how many were actually present and dropped.
    pub fn drop_resources(&mut self, ids: &[ResourceId]) -> usize {
        let mut dropped_resource_count = 0;
        for id in ids {
            // Another artifact's claim keeps the value alive.
            if self
                .resource_claim_counts
                .get(id)
                .is_some_and(|count| *count > 0)
            {
                continue;
            }
            if self.resources.remove(id).is_some() {
                dropped_resource_count += 1;
            }
            // A play session's copy of the same value came from that artifact
            // too, so it must be released while the image is still mapped.
            // Stop then has nothing to put back, so the type is remembered and
            // named when the session ends.
            let type_name = self
                .resource_factories
                .get(id)
                .and_then(|ops| ops.declaring_type_name)
                .map(str::to_string)
                .unwrap_or_else(|| format!("resource {id:?}"));
            let held_dropped = self
                .held_resource_copy
                .as_mut()
                .is_some_and(|resource_copy| {
                    if resource_copy.values.remove(id).is_some() {
                        resource_copy
                            .dropped_by_reload
                            .push((*id, type_name.clone()));
                        true
                    } else {
                        false
                    }
                });
            if held_dropped {
                dropped_resource_count += 1;
            }
            // The id is unregistered now, so a later registration is a fresh
            // one and has to be visible to the next "registered since" diff.
            self.remove_resource_records(*id);
        }
        dropped_resource_count
    }

    /// Records one claim per id on a resource the calling artifact registered.
    ///
    /// [`drop_resources`](Self::drop_resources) consults these counts before
    /// releasing an id that one artifact retired. A shared name can be
    /// registered by several artifacts, and only the last of them to let go
    /// should destroy the value.
    pub fn retain_resource_claims(&mut self, ids: &[ResourceId]) {
        for id in ids {
            *self.resource_claim_counts.entry(*id).or_insert(0) += 1;
        }
    }

    /// Drops one claim per id, releasing a value once its last claim goes.
    ///
    /// A claim an id never had is a no-op, and a release that takes the count to
    /// zero removes the entry, so a later `retain` starts a fresh count.
    pub fn release_resource_claims(&mut self, ids: &[ResourceId]) {
        for id in ids {
            if let Some(claim_count) = self.resource_claim_counts.get_mut(id) {
                *claim_count = claim_count.saturating_sub(1);
                if *claim_count == 0 {
                    self.resource_claim_counts.remove(id);
                }
            }
        }
    }

    /// Removes every per-id record this world keeps for a resource.
    ///
    /// One function decides which maps a removal clears, so a removal path cannot
    /// leave one behind; a stale `resource_field_layouts` entry, for example,
    /// would apply the previous value's offsets to a value registered under the
    /// same id again. The claim count is deliberately left alone: it belongs to
    /// [`release_resource_claims`](Self::release_resource_claims), and
    /// [`drop_resources`](Self::drop_resources) checks it before calling here.
    fn remove_resource_records(&mut self, id: ResourceId) {
        self.resource_ticks.remove(&id);
        self.resource_factories.remove(&id);
        self.resource_field_layouts.remove(&id);
        self.shared_resource_claims.remove(&id);
        self.resource_registration_stamps.remove(&id);
    }

    /// Removes a resource and returns it if it existed.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::SharedResourceHoldsAnotherType`] when the id holds
    /// a value this `T` cannot be read as - the case a shared, name-derived id
    /// makes reachable. The box is taken out first and put back on a refusal, so
    /// nothing about the stored resource changes: not its value, its ticks, its
    /// factory or its claim.
    pub fn remove_resource<T: Resource>(&mut self) -> Result<Option<T>, WorldError> {
        let id = ResourceId::of::<T>();
        let Some(erased_resource) = self.resources.remove(&id) else {
            return Ok(None);
        };
        let resource_value = match erased_resource.take::<T>() {
            Ok(resource_value) => resource_value,
            Err(erased_resource) => {
                // Put the box back unchanged: a refused take must leave the
                // value, and every record of it, exactly as it was.
                self.resources.insert(id, erased_resource);
                return Err(WorldError::SharedResourceHoldsAnotherType {
                    id,
                    requested_type: std::any::type_name::<T>(),
                });
            }
        };
        self.remove_resource_records(id);
        Ok(Some(resource_value))
    }

    /// Checks whether a resource exists *and* can be read as `T`.
    ///
    /// This is the same question [`Self::get_resource`] answers: under a shared
    /// id, a value of a different shape is not a `T`, so existence and
    /// readability agree here instead of one check reporting the id and the
    /// other the readable shape. [`Self::resource_holder`] answers the shape
    /// question, for a caller that needs to know what is stored.
    #[must_use]
    pub fn has_resource<T: Resource>(&self) -> bool {
        self.resources
            .get(&ResourceId::of::<T>())
            .is_some_and(|erased_resource| erased_resource.holds::<T>())
    }

    /// The stored box under `T`'s resource id, whatever shape it holds.
    ///
    /// This is for a caller that needs the box itself when `T` does not read the
    /// stored value: the box answers `holds::<T>()` and `has_shared_identity()`.
    /// `None` means the id has no resource at all.
    #[must_use]
    pub fn resource_holder<T: Resource>(&self) -> Option<&crate::resource::ErasedResource> {
        self.resources.get(&ResourceId::of::<T>())
    }

    /// Takes one resource's debug write lock and reports an overlapping holder.
    ///
    /// [`ResMut`](crate::query::ResMut) acquires the lock when it is built and
    /// releases it when it drops, so the lock covers exactly one system's access.
    ///
    /// The receiver is `&self`, not `&mut self`, because the systems of a
    /// parallel batch hold aliasing `&mut World`s and still need to call this.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_acquire_resource_lock(&self, id: ResourceId) {
        let newly_inserted = self.debug_resource_write_locks().insert(id);
        debug_assert!(
            newly_inserted,
            "Resource {id:?} is already mutably borrowed by another live \
             `ResMut` - possible scheduler bug or concurrent system access"
        );
    }

    /// Releases one resource's debug write lock; see
    /// [`Self::debug_acquire_resource_lock`].
    #[cfg(debug_assertions)]
    pub(crate) fn debug_release_resource_lock(&self, id: ResourceId) {
        self.debug_resource_write_locks().remove(&id);
    }

    /// Clears the set of mutably-borrowed resources.
    ///
    /// [`Engine::process_frame`](crate::engine::Engine::process_frame) calls
    /// this at the start of every frame, so the isolation check only looks at
    /// concurrent access within a single frame.
    #[cfg(debug_assertions)]
    pub(crate) fn debug_clear_resource_locks(&mut self) {
        self.debug_resource_write_locks().clear();
    }

    /// The debug write-lock set, taken even when a panicking holder poisoned
    /// it: the set is bookkeeping for an assertion, and a poisoned one is
    /// still the best record there is.
    #[cfg(debug_assertions)]
    fn debug_resource_write_locks(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashSet<ResourceId>> {
        self.debug_resource_write_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
