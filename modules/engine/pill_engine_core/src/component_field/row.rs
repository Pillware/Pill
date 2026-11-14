//! Finds, reads and writes single fields of a component row.
//!
//! # Responsibilities
//!
//! - Resolve a component's field to a live row: the entity's own archetype, its
//!   column, and the byte range a field occupies there.
//! - Read one field or every field of a row as [`FieldValue`](super::FieldValue),
//!   and write one back, stamping the row's `changed` tick so `Changed<T>`
//!   observes the edit next frame.
//! - Report every refusal as a [`ComponentFieldError`](super::ComponentFieldError)
//!   that names the component, and the field when it is known, without handing
//!   a raw pointer out.
//!
//! # Design
//!
//! Every entry point that targets a live entity resolves the component through
//! that entity's own archetype. That archetype is the one place that names the
//! generation which created the data, while a lookup by name across the whole
//! registry can see several registrations under one name.
//! [`World::build_component_image`] has no entity yet, so it is the one entry
//! point that resolves through the registry.
//!
//! Values are validated against the registered layout before anything is
//! written, and a refused write leaves the row untouched.

// Current crate
use super::codec::{decode_field, encode_field_write, encode_scalar, is_scalar_tag, scalar_size};
use super::{ComponentFieldError, FieldValue};
use crate::archetype::{ArchetypeId, ComponentColumn};
use crate::component::{ComponentId, Tick};
use crate::component_registry::ComponentFieldDescriptor;
use crate::entity::Entity;
use crate::world::World;

// =============================================================================
// ResolvedField
// =============================================================================

/// The archetype, row, component id and field descriptor of one field.
///
/// Together these four values say where a field lives. Every single-field
/// entry point needs them before it can read or write and gets them from
/// [`World::resolve_field`], so the rule and its error payloads live in one
/// place instead of in each entry point.
struct ResolvedField {
    /// The archetype holding the entity.
    archetype_id: ArchetypeId,
    /// The entity's row within that archetype.
    row: usize,
    /// The component carried by that entity.
    component_id: ComponentId,
    /// The requested field's descriptor.
    descriptor: ComponentFieldDescriptor,
}

// =============================================================================
// RowWriteTarget
// =============================================================================

/// The row one field write targets, and the names its refusals report.
pub(super) struct RowWriteTarget<'a> {
    /// Archetype holding the row.
    pub(super) archetype_id: ArchetypeId,
    /// Row within that archetype.
    pub(super) row: usize,
    /// Component whose column holds the field.
    pub(super) component_id: ComponentId,
    /// Component name the error paths report.
    pub(super) component_name: &'a str,
    /// Field name the error paths report.
    pub(super) field_name: &'a str,
}

// =============================================================================
// FieldWrite
// =============================================================================

/// A validated byte payload and where in the component row it belongs.
pub(super) struct FieldWrite {
    /// Byte offset of the payload within the component row.
    pub(super) row_offset: usize,
    /// The payload bytes (a scalar, one array element, or a whole array).
    pub(super) bytes: Vec<u8>,
}

// =============================================================================
// World - Row Lookup
// =============================================================================

impl World {
    /// Returns the archetype and row that hold a live entity, or `None` when
    /// the entity is not alive.
    pub(super) fn entity_row_location(&self, entity: Entity) -> Option<(ArchetypeId, usize)> {
        let location = self.entity_locations.get(&entity)?;
        Some((location.archetype_id, location.index_in_archetype))
    }

    /// The registered field layout of a component.
    ///
    /// A component with no registered layout, or an empty one, is refused. The
    /// layout is borrowed rather than copied. The field entry points read one
    /// descriptor from it or every descriptor, and the inspector asks for it
    /// once per component row per frame.
    fn required_component_field_layout(
        &self,
        component_id: ComponentId,
        component_name: &str,
    ) -> Result<&[ComponentFieldDescriptor], ComponentFieldError> {
        let layout = self.component_field_layout(component_id).ok_or(
            ComponentFieldError::ComponentHasNoFieldLayout {
                component: component_name.to_string(),
            },
        )?;
        if layout.is_empty() {
            return Err(ComponentFieldError::ComponentHasNoFieldLayout {
                component: component_name.to_string(),
            });
        }
        Ok(layout)
    }

    /// Locate one field of one component on one entity.
    ///
    /// This is the one place a single field is resolved, so a new single-field
    /// entry point cannot invent its own error path: every miss reports the
    /// same variant with the same payload.
    fn resolve_field(
        &self,
        entity: Entity,
        component_name: &str,
        field_name: &str,
    ) -> Result<ResolvedField, ComponentFieldError> {
        let (archetype_id, row) = self
            .entity_row_location(entity)
            .ok_or(ComponentFieldError::EntityNotFound)?;
        let component_id = self
            .resolve_entity_component_id(entity, component_name)
            .ok_or(ComponentFieldError::ComponentNotFound {
                component: component_name.to_string(),
            })?;
        let descriptor = self
            .required_component_field_layout(component_id, component_name)?
            .iter()
            .find(|descriptor| descriptor.name == field_name)
            .ok_or(ComponentFieldError::FieldNotFound {
                component: component_name.to_string(),
                field: field_name.to_string(),
            })?;
        Ok(ResolvedField {
            archetype_id,
            row,
            component_id,
            descriptor: descriptor.clone(),
        })
    }

    /// One component column of one archetype, checked to hold the given row.
    ///
    /// `component_name` only feeds the error path: the caller resolved the
    /// component through the entity, so a missing column or a row past its end
    /// is a storage desync and reports the component the caller named.
    fn row_column(
        &self,
        archetype_id: ArchetypeId,
        row: usize,
        component_id: ComponentId,
        component_name: &str,
    ) -> Result<&ComponentColumn, ComponentFieldError> {
        let column = self
            .archetypes
            .get(&archetype_id)
            .ok_or(ComponentFieldError::EntityNotFound)?
            .component_storages
            .get(component_id)
            .ok_or(ComponentFieldError::ComponentStorageMissing {
                component: component_name.to_string(),
            })?;
        if row >= column.len() {
            return Err(ComponentFieldError::ComponentStorageMissing {
                component: component_name.to_string(),
            });
        }
        Ok(column)
    }

    /// [`Self::row_column`]'s mutable twin, for the write paths.
    fn row_column_mut(
        &mut self,
        archetype_id: ArchetypeId,
        row: usize,
        component_id: ComponentId,
        component_name: &str,
    ) -> Result<&mut ComponentColumn, ComponentFieldError> {
        let column = self
            .archetypes
            .get_mut(&archetype_id)
            .ok_or(ComponentFieldError::EntityNotFound)?
            .component_storages
            .get_mut(component_id)
            .ok_or(ComponentFieldError::ComponentStorageMissing {
                component: component_name.to_string(),
            })?;
        if row >= column.len() {
            return Err(ComponentFieldError::ComponentStorageMissing {
                component: component_name.to_string(),
            });
        }
        Ok(column)
    }

    /// Copy one entity's component row for `component_id` into an owned buffer.
    ///
    /// Works for native (Rust) and descriptor (foreign-language) columns alike.
    /// The caller owns the returned bytes, so no borrow outlives this call.
    fn copy_component_row_bytes(
        &self,
        archetype_id: ArchetypeId,
        row: usize,
        component_id: ComponentId,
        component_name: &str,
    ) -> Result<Vec<u8>, ComponentFieldError> {
        let column = self.row_column(archetype_id, row, component_id, component_name)?;
        if !component_id.is_native_storage() {
            return column.bytes(row).map(<[u8]>::to_vec).ok_or(
                ComponentFieldError::ComponentStorageMissing {
                    component: component_name.to_string(),
                },
            );
        }
        let element_size = column.element_size();
        let mut buffer = vec![0u8; element_size];
        // SAFETY: the row is within the column (`row < len`, checked by the
        // lookup) and the buffer is exactly one element long; nothing mutates
        // the column while this shared borrow is alive.
        unsafe {
            std::ptr::copy_nonoverlapping(
                column.raw_ptr().add(row * element_size),
                buffer.as_mut_ptr(),
                element_size,
            );
        }
        Ok(buffer)
    }
}

// =============================================================================
// World - Reads and Asset Types
// =============================================================================

impl World {
    /// The value of one field of one component on one entity.
    ///
    /// The component is resolved through the entity's own archetype (see
    /// [`Self::resolve_entity_component_id`]), so the returned value always
    /// describes data the entity really holds. A read never mutates and never
    /// stamps a change tick.
    ///
    /// # Errors
    ///
    /// [`ComponentFieldError::EntityNotFound`] when the entity is not alive,
    /// [`ComponentFieldError::ComponentNotFound`] when it does not carry that
    /// component, [`ComponentFieldError::ComponentHasNoFieldLayout`] when the
    /// component has no field layout or an empty one, and
    /// [`ComponentFieldError::FieldNotFound`] when the layout does not name
    /// that field. After those checks pass, a missing archetype reports
    /// [`ComponentFieldError::EntityNotFound`] again and a missing column or
    /// row reports [`ComponentFieldError::ComponentStorageMissing`]; both mean
    /// that the storage no longer matches the world's own bookkeeping.
    pub fn read_component_field(
        &self,
        entity: Entity,
        component_name: &str,
        field_name: &str,
    ) -> Result<FieldValue, ComponentFieldError> {
        let ResolvedField {
            archetype_id,
            row,
            component_id,
            descriptor,
        } = self.resolve_field(entity, component_name, field_name)?;

        // Copy the row and decode the single field. A copy that comes back
        // missing after both lookups succeeded is a storage desync, not a dead
        // handle, so it reports the component by name.
        let row_bytes =
            self.copy_component_row_bytes(archetype_id, row, component_id, component_name)?;
        Ok(self.with_asset_type(
            component_id,
            &descriptor,
            decode_field(&row_bytes, &descriptor),
        ))
    }

    /// Every field of one component, in layout order.
    ///
    /// Cheaper than calling [`Self::read_component_field`] once per field: the
    /// row is located and copied once. The inspector reads a whole component
    /// this way, and so does a scene save.
    ///
    /// # Errors
    ///
    /// [`ComponentFieldError::EntityNotFound`] when the entity is not alive,
    /// [`ComponentFieldError::ComponentNotFound`] when it does not carry that
    /// component, and [`ComponentFieldError::ComponentHasNoFieldLayout`] when
    /// the component has no field layout or an empty one. After those checks
    /// pass, a missing archetype reports [`ComponentFieldError::EntityNotFound`]
    /// again and a missing column or row reports
    /// [`ComponentFieldError::ComponentStorageMissing`].
    pub fn read_component_fields(
        &self,
        entity: Entity,
        component_name: &str,
    ) -> Result<Vec<FieldValue>, ComponentFieldError> {
        // The entity and component half of `resolve_field`. This path reads
        // every descriptor, so it borrows the layout instead of cloning the
        // one descriptor it needs.
        let (archetype_id, row) = self
            .entity_row_location(entity)
            .ok_or(ComponentFieldError::EntityNotFound)?;
        let component_id = self
            .resolve_entity_component_id(entity, component_name)
            .ok_or(ComponentFieldError::ComponentNotFound {
                component: component_name.to_string(),
            })?;

        // Borrow the layout and copy the whole row once.
        let layout = self.required_component_field_layout(component_id, component_name)?;
        let row_bytes =
            self.copy_component_row_bytes(archetype_id, row, component_id, component_name)?;

        // Decode every field from that one copy of the row.
        Ok(layout
            .iter()
            .map(|descriptor| {
                self.with_asset_type(
                    component_id,
                    descriptor,
                    decode_field(&row_bytes, descriptor),
                )
            })
            .collect())
    }

    /// Adds the field's recorded asset type to an asset value that carries
    /// none, and returns every other value unchanged. A field with no recorded
    /// type leaves the value as it was.
    fn with_asset_type(
        &self,
        component_id: ComponentId,
        descriptor: &ComponentFieldDescriptor,
        value: FieldValue,
    ) -> FieldValue {
        match value {
            FieldValue::Asset {
                asset_type: None,
                index,
                generation,
            } => FieldValue::Asset {
                asset_type: self
                    .asset_field_type(component_id, descriptor.name)
                    .map(str::to_owned),
                index,
                generation,
            },
            other => other,
        }
    }

    /// Record that `component`'s handle field `field` references assets of the
    /// type registered as `asset_type`.
    ///
    /// Called by `#[derive(PillComponent)]`'s generated registration for each
    /// `Handle<T>` field, with the asset's shared name (or its Rust type name
    /// when it has no shared name), and by a foreign-language backend for its
    /// components.
    ///
    /// The name is copied: the caller's string may live in a module that a
    /// reload unmaps.
    pub fn record_asset_field_type(
        &mut self,
        component: ComponentId,
        field: &str,
        asset_type: &str,
    ) {
        self.asset_field_types
            .entry(component)
            .or_default()
            .insert(field.to_owned(), asset_type.to_owned());
    }

    /// Refuses an asset value whose type differs from the one recorded for the
    /// field. A value with no type, or a field with no recorded type, passes.
    fn check_asset_type(
        &self,
        component_id: ComponentId,
        field_name: &str,
        value: &FieldValue,
    ) -> Result<(), ComponentFieldError> {
        let FieldValue::Asset {
            asset_type: Some(found),
            ..
        } = value
        else {
            return Ok(());
        };
        match self.asset_field_type(component_id, field_name) {
            Some(expected) if expected != found => Err(ComponentFieldError::AssetTypeMismatch {
                field: field_name.to_owned(),
                expected: expected.to_owned(),
                found: found.clone(),
            }),
            _ => Ok(()),
        }
    }

    /// The asset type `component`'s handle field `field` references, if
    /// recorded.
    pub fn asset_field_type(&self, component: ComponentId, field: &str) -> Option<&str> {
        self.asset_field_types
            .get(&component)?
            .get(field)
            .map(String::as_str)
    }
}

// =============================================================================
// World - Writes
// =============================================================================

impl World {
    /// Writes one field of a live component.
    ///
    /// The component is resolved through the entity's own archetype, and the
    /// value is checked against the registered field layout before anything is
    /// written. The write goes through the component's column and stamps that
    /// row's `ComponentTicks::changed`, so `Changed<T>` systems observe the
    /// edit on their next run; [`Self::get_component_mut`] hands out the same
    /// column slot without that stamp. Nothing is written when any validation
    /// fails.
    ///
    /// # Errors
    ///
    /// Fails before writing anything when the field cannot be resolved, with
    /// the variants [`Self::read_component_field`] reports, or when the value
    /// is refused: [`ComponentFieldError::AssetTypeMismatch`] for an asset
    /// value of another type, [`ComponentFieldError::UnsupportedField`] for a
    /// heap-backed field, a tag outside the editable vocabulary or a fixed
    /// array of non-scalars, [`ComponentFieldError::TypeMismatch`] when the
    /// value does not match the field's tag, and
    /// [`ComponentFieldError::ArrayLengthMismatch`] when a whole array has the
    /// wrong element count. A payload that would extend past the row reports
    /// [`ComponentFieldError::UnsupportedField`], and a column or row that has
    /// gone missing reports [`ComponentFieldError::ComponentStorageMissing`].
    pub fn write_component_field(
        &mut self,
        entity: Entity,
        component_name: &str,
        field_name: &str,
        value: FieldValue,
    ) -> Result<(), ComponentFieldError> {
        // Step 1: resolve the field, then validate and encode the payload
        // before touching a column, so a refused edit leaves the row as it was.
        let ResolvedField {
            archetype_id,
            row,
            component_id,
            descriptor,
        } = self.resolve_field(entity, component_name, field_name)?;
        self.check_asset_type(component_id, field_name, &value)?;
        let write = encode_field_write(&descriptor, field_name, &value)?;

        // Step 2: write the payload, then stamp the row's changed tick. Each
        // runs under its own archetype borrow, so the column and tick lookups
        // never overlap.
        //
        // The world tick is increased first, so the stamped `changed` value is
        // strictly newer than every system's `last_run` baseline. An edit
        // applied between frames would otherwise stamp the tick the last frame
        // left behind, and a `Changed<T>` filter would compare the new value
        // against an equal baseline and never see the edit.
        let current_tick = self.increment_change_tick();
        self.apply_row_write(
            RowWriteTarget {
                archetype_id,
                row,
                component_id,
                component_name,
                field_name,
            },
            write.row_offset,
            &write.bytes,
        )?;
        self.stamp_row_changed(
            archetype_id,
            row,
            component_id,
            component_name,
            current_tick,
        )
    }

    /// Records the bytes of `T::default()` as the value a component built from
    /// field values starts from ([`Self::build_component_image`]): the
    /// editor's Add Component, and a scene field the document leaves out.
    ///
    /// Call it after the component registered with its field layout. A
    /// component with a heap-owning field (`Vec`, `String`, a dynamic buffer)
    /// records nothing and keeps starting from zeros: every image built from
    /// one copy of its default would share that value's buffer.
    pub fn record_component_default<T>(&mut self)
    where
        T: crate::component::Component + Default,
    {
        let component_id = crate::component::ComponentId::of::<T>();
        let Some(layout) = self.component_field_layout(component_id) else {
            return;
        };
        if layout
            .iter()
            .any(|descriptor| super::is_container_tag(descriptor.type_tag))
        {
            return;
        }
        let default_value = T::default();
        let size = std::mem::size_of::<T>();
        let mut image = vec![0u8; size];
        // SAFETY: `default_value` is a live `T` of exactly `size` bytes and
        // `image` holds `size` writable bytes; the two do not overlap. The copy
        // is untyped, the same byte image a component row is stored as, and
        // the type owns no heap buffer (checked above), so the copied bytes
        // own nothing the dropped original frees.
        unsafe {
            std::ptr::copy_nonoverlapping(
                (&default_value as *const T).cast::<u8>(),
                image.as_mut_ptr(),
                size,
            );
        }
        drop(default_value);
        self.component_default_images.insert(component_id, image);
    }

    /// Builds a zero-initialised component image for a registered type name,
    /// then writes the supplied field values over it.
    ///
    /// Used where the entity does not exist yet: the editor creating an entity
    /// or adding a component from field values without knowing the concrete
    /// Rust type, and a scene load attaching a component the document lists.
    /// With no entity to look through, the component is resolved through the
    /// registry, which is the correct resolver for this path.
    ///
    /// Zero-initialisation is only valid for a component with a non-empty
    /// registered field layout, and a missing or empty layout is refused:
    /// without a layout the image cannot be described field by field.
    ///
    /// # Errors
    ///
    /// [`ComponentFieldError::ComponentNotFound`] when no component is
    /// registered under that name, or when two registrations claim it;
    /// [`ComponentFieldError::ComponentHasNoFieldLayout`] when it has no field
    /// layout, an empty one, or no registered size; and
    /// [`ComponentFieldError::FieldNotFound`] when a supplied field name is
    /// not in the layout. A value is refused the way a field write refuses it:
    /// [`ComponentFieldError::AssetTypeMismatch`],
    /// [`ComponentFieldError::TypeMismatch`],
    /// [`ComponentFieldError::ArrayLengthMismatch`] or
    /// [`ComponentFieldError::UnsupportedField`], the last also for a value
    /// that would extend past the image.
    pub fn build_component_image(
        &self,
        component_name: &str,
        fields: &[(String, FieldValue)],
    ) -> Result<Vec<u8>, ComponentFieldError> {
        // Step 1: resolve the component through the registry. An ambiguous
        // name means two registrations claim it, so there is no single
        // component to build an image for. It is treated as "not found", which
        // is the safe answer for both the editor and a scene load.
        let component_id = self
            .resolve_component_id_by_name_any(component_name)
            .unwrap_or(None)
            .ok_or(ComponentFieldError::ComponentNotFound {
                component: component_name.to_string(),
            })?;
        let layout = self.required_component_field_layout(component_id, component_name)?;
        let (size, _) = self.component_layout(component_id).ok_or(
            ComponentFieldError::ComponentHasNoFieldLayout {
                component: component_name.to_string(),
            },
        )?;
        // Step 2: start from the component's default value when its
        // registration recorded one, and from zeros otherwise, then write each
        // supplied value into it.
        let mut image = self
            .component_default_images
            .get(&component_id)
            .filter(|default_image| default_image.len() == size)
            .cloned()
            .unwrap_or_else(|| vec![0u8; size]);
        for (field_name, value) in fields {
            let descriptor = layout
                .iter()
                .find(|descriptor| descriptor.name == *field_name)
                .ok_or(ComponentFieldError::FieldNotFound {
                    component: component_name.to_string(),
                    field: field_name.clone(),
                })?;
            self.check_asset_type(component_id, field_name, value)?;
            let write = encode_field_write(descriptor, field_name, value)?;
            let target = image
                .get_mut(write.row_offset..write.row_offset + write.bytes.len())
                .ok_or(ComponentFieldError::UnsupportedField {
                    field: field_name.clone(),
                    reason: "write extends past the component image",
                })?;
            target.copy_from_slice(&write.bytes);
        }
        Ok(image)
    }

    /// Writes one element of a fixed-size array field.
    ///
    /// # Errors
    ///
    /// Fails before writing anything when the field cannot be resolved, with
    /// the variants [`Self::read_component_field`] reports, when the field is
    /// not a fixed array of scalars ([`ComponentFieldError::TypeMismatch`] for
    /// another tag, [`ComponentFieldError::UnsupportedField`] for non-scalar
    /// elements or an unknown element size), when `index` is past the array
    /// ([`ComponentFieldError::ArrayIndexOutOfRange`]), or when the value does
    /// not encode into one element ([`ComponentFieldError::TypeMismatch`]). A
    /// column or row that has gone missing reports
    /// [`ComponentFieldError::ComponentStorageMissing`].
    pub fn write_component_array_element(
        &mut self,
        entity: Entity,
        component_name: &str,
        field_name: &str,
        index: usize,
        value: FieldValue,
    ) -> Result<(), ComponentFieldError> {
        let ResolvedField {
            archetype_id,
            row,
            component_id,
            descriptor,
        } = self.resolve_field(entity, component_name, field_name)?;

        // Step 1: the field must be a fixed array of scalar primitives, and
        // the value must encode into one element.
        let Some(inner_tag) = descriptor.type_tag.strip_prefix("array:") else {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: descriptor.type_tag,
                found: value.variant_tag(),
            });
        };
        if !is_scalar_tag(inner_tag) {
            return Err(ComponentFieldError::UnsupportedField {
                field: field_name.to_string(),
                reason: "array elements are not scalar primitives",
            });
        }
        if index >= descriptor.element_count {
            return Err(ComponentFieldError::ArrayIndexOutOfRange {
                field: field_name.to_string(),
                index,
                length: descriptor.element_count,
            });
        }
        let element_size = scalar_size(inner_tag).ok_or(ComponentFieldError::UnsupportedField {
            field: field_name.to_string(),
            reason: "array element size is unknown",
        })?;
        let mut bytes = Vec::with_capacity(element_size);
        if !encode_scalar(inner_tag, &value, &mut bytes) {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: inner_tag,
                found: value.variant_tag(),
            });
        }

        // Step 2: write the element and stamp the row's changed tick. The tick
        // is increased first for the same reason the whole-field write does
        // it: a between-frame edit must be visible to `Changed<T>` filters.
        let row_offset = descriptor.offset + index * element_size;
        let current_tick = self.increment_change_tick();
        self.apply_row_write(
            RowWriteTarget {
                archetype_id,
                row,
                component_id,
                component_name,
                field_name,
            },
            row_offset,
            &bytes,
        )?;
        self.stamp_row_changed(
            archetype_id,
            row,
            component_id,
            component_name,
            current_tick,
        )
    }

    /// Writes `bytes.len()` bytes at `row_offset` of one component row.
    ///
    /// `target.component_name` only feeds the error paths: the caller resolved
    /// the component through the entity, so a missing column or row here is a
    /// storage desync and reports the component the caller named.
    pub(super) fn apply_row_write(
        &mut self,
        target: RowWriteTarget<'_>,
        row_offset: usize,
        bytes: &[u8],
    ) -> Result<(), ComponentFieldError> {
        let RowWriteTarget {
            archetype_id,
            row,
            component_id,
            component_name,
            field_name,
        } = target;
        // One construction for every "the payload does not fit the row" refusal
        // in this function, so the message always names the field.
        let past_the_row = || ComponentFieldError::UnsupportedField {
            field: field_name.to_string(),
            reason: "write extends past the component row",
        };
        match component_id {
            _ if component_id.is_native_storage() => {
                let column =
                    self.row_column_mut(archetype_id, row, component_id, component_name)?;
                let element_size = column.element_size();
                // SAFETY: the lookup checked `row < column.len()`, so the
                // pointer stays inside the column's allocation, and the column
                // borrow keeps that allocation alive through this statement.
                let destination = unsafe { column.as_mut_ptr().add(row * element_size) };
                // Checked arithmetic on the way to the write: a wrapping sum
                // would pass a caller-supplied offset near `usize::MAX` and
                // let the copy below run off the row.
                let Some(end) = row_offset
                    .checked_add(bytes.len())
                    .filter(|end| *end <= element_size)
                else {
                    return Err(past_the_row());
                };
                // SAFETY: the destination row is initialised (`row < len`) and
                // the payload fits within it (checked above); the column is not
                // reallocated while this borrow is held.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        destination.add(row_offset),
                        end - row_offset,
                    );
                }
                Ok(())
            }
            _ => {
                let mut row_bytes = self
                    .row_column_mut(archetype_id, row, component_id, component_name)?
                    .bytes(row)
                    .ok_or(ComponentFieldError::ComponentStorageMissing {
                        component: component_name.to_string(),
                    })?
                    .to_vec();
                let element_size = row_bytes.len();
                // The same checked arithmetic as the native branch, so a
                // wrapped sum cannot select the wrong part of the row slice.
                let Some(end) = row_offset
                    .checked_add(bytes.len())
                    .filter(|end| *end <= element_size)
                else {
                    return Err(past_the_row());
                };
                let target = row_bytes
                    .get_mut(row_offset..end)
                    .ok_or_else(past_the_row)?;

                target.copy_from_slice(bytes);
                self.row_column_mut(archetype_id, row, component_id, component_name)?
                    .set_bytes(row, &row_bytes)
                    .map_err(|_| ComponentFieldError::ComponentStorageMissing {
                        component: component_name.to_string(),
                    })
            }
        }
    }

    /// Stamps one row's `changed` tick so `Changed<T>` systems react to the
    /// write.
    fn stamp_row_changed(
        &mut self,
        archetype_id: ArchetypeId,
        row: usize,
        component_id: ComponentId,
        component_name: &str,
        tick: Tick,
    ) -> Result<(), ComponentFieldError> {
        let row_ticks = self
            .row_column_mut(archetype_id, row, component_id, component_name)?
            .row_ticks_mut(row)
            .ok_or(ComponentFieldError::ComponentStorageMissing {
                component: component_name.to_string(),
            })?;
        row_ticks.set_changed(tick);
        Ok(())
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Formats a copied component row for native debugger helpers.
///
/// It takes the row's bytes and the registered field layout instead of a
/// component type, so it works for Rust, renderer and managed descriptor
/// columns alike.
pub fn format_component_debug(fields: &[ComponentFieldDescriptor], bytes: &[u8]) -> String {
    fields
        .iter()
        .map(|field| format!("{}={:?}", field.name, decode_field(bytes, field)))
        .collect::<Vec<_>>()
        .join(", ")
}
