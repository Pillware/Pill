//! Native callbacks used by C# query enumerators.
//!
//! # Responsibilities
//!
//! - Return validated component chunks to managed query iterators.
//! - Return entity columns and live entity counts during scheduled systems.
//! - Enforce the active system's declared read/write access.
//!
//! # Design
//!
//! Each `ffi_*` function is an `extern "C"` entry point that C# query iterators
//! and scheduled systems call across the host/managed boundary. Access checks
//! go through [`access_is_authorized`], and world and binding lookups go
//! through [`with_active_context`] and [`with_active_world`]. When no managed
//! system is active, every callback answers status code `3` instead of serving
//! data.
//!
//! Pointers returned in [`ComponentChunk`] stay owned by the active world's
//! archetype and are valid only while the managed invocation runs. Entity rows
//! arrive in `ComponentChunk::entities`, a const slot. The archetype-scoped
//! path serves them only for an archetype this invocation has already received
//! a chunk for, so it cannot be used to probe arbitrary archetypes.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::archetype::ArchetypeId;
use pill_engine::{ComponentId, ComponentTicks, Entity};

// Current crate
use super::abi::ComponentChunk;
use super::components::{ComponentBinding, StableComponentId};
use super::context::{
    access_is_authorized, active_scope_token, archetype_was_observed, record_observed_archetype,
    with_active_context, with_active_world,
};

// =============================================================================
// Free Functions
// =============================================================================

/// Record the archetype of a chunk a validated term just served.
///
/// The entity path answers only for archetypes this invocation has already
/// served a chunk for. Recording happens exactly where a chunk is returned, so
/// a status that served nothing records nothing.
fn record_served_archetype(status: u8, chunk_output: *mut ComponentChunk) {
    if status != 1 {
        return;
    }
    // SAFETY: status `1` is returned only after the producing arm wrote a
    // complete chunk into `chunk_output`, which was checked non-null on entry.
    let chunk = unsafe { &*chunk_output };
    record_observed_archetype(ArchetypeId(
        ((chunk.archetype_high as u128) << 64) | chunk.archetype_low as u128,
    ));
}

/// Write one component column chunk into the managed caller's output buffer.
///
/// `column_rows` and `ticks` stay owned by the active world's archetype and
/// are valid only during the managed invocation that requested the chunk,
/// exactly as [`ComponentChunk`] documents.
// The eight fields belong to one ABI struct, so they stay separate arguments:
// a wrapper struct would exist only to satisfy the lint.
#[allow(clippy::too_many_arguments)]
fn write_component_chunk(
    chunk_output: *mut ComponentChunk,
    archetype: ArchetypeId,
    column_rows: *mut u8,
    row_count: usize,
    element_size: usize,
    ticks: *mut ComponentTicks,
    change_tick: u32,
    scope_token: u32,
) {
    let archetype_bits = archetype.0;
    // SAFETY: `chunk_output` is non-null, checked by the caller, and points at
    // a caller-owned slot. `column_rows` and `ticks` point into a live column
    // of the active world and stay valid while the managed invocation runs.
    unsafe {
        chunk_output.write(ComponentChunk {
            archetype_low: archetype_bits as u64,
            archetype_high: (archetype_bits >> 64) as u64,
            data: column_rows.cast(),
            entities: std::ptr::null(),
            len: row_count as u32,
            element_size: element_size as u32,
            ticks,
            change_tick,
            scope_token,
        });
    }
}

/// Write one archetype's entity column chunk into the managed caller's output
/// buffer.
///
/// Entity rows arrive through the const `entities` slot, with `data` and
/// `ticks` null. That is the read-only contract the ABI documents, so managed
/// code has no way to express a write through an entity column.
fn write_entity_chunk(
    chunk_output: *mut ComponentChunk,
    archetype: ArchetypeId,
    entities: &[Entity],
    change_tick: u32,
    scope_token: u32,
) {
    let archetype_bits = archetype.0;
    // SAFETY: `chunk_output` is non-null, checked by the caller, and points at
    // a caller-owned slot. The entity slice stays borrowed from the active
    // world while the managed invocation runs.
    unsafe {
        chunk_output.write(ComponentChunk {
            archetype_low: archetype_bits as u64,
            archetype_high: (archetype_bits >> 64) as u64,
            data: std::ptr::null_mut(),
            entities: entities.as_ptr().cast(),
            len: entities.len() as u32,
            element_size: std::mem::size_of::<Entity>() as u32,
            ticks: std::ptr::null_mut(),
            change_tick,
            scope_token,
        });
    }
}

/// Whether a module's native binding still matches the live column's layout.
///
/// A binding can describe an older layout than the column it points at: the
/// module reloaded with a new schema, or a mirror arrived against a stale one.
/// A chunk of the wrong size would be read as the managed mirror's fields, so
/// the caller refuses the request and this check logs the mismatch.
fn module_binding_matches_live_column(
    component_id: ComponentId,
    binding_element_size: usize,
    column_element_size: usize,
) -> bool {
    if column_element_size == binding_element_size {
        return true;
    }
    pill_core::error!(
        target: telemetry_target::ECS,
        component_id = ?component_id,
        binding_size = binding_element_size,
        column_size = column_element_size,
        "module native binding size disagrees with the live column"
    );
    false
}

/// Return a component chunk to managed code after checking the system's
/// declared access.
///
/// `Engine.TryGetChunk` on the managed side reads these status codes:
///
/// - `0`: nothing to serve, so iteration ends.
/// - `1`: the output holds a complete chunk.
/// - `2`: the component is unknown, or a module binding no longer matches the
///   live column's element size.
/// - `3`: no managed system is running on this thread.
/// - `4`: the running system did not declare the requested access.
/// - `5`: the caller passed no output buffer. Managed code maps this to an
///   `ArgumentException`, because its own call sites always pass a real one.
fn ffi_get_component_chunk_guarded(
    stable_id_low: u64,
    stable_id_high: u64,
    request_mode: u8,
    chunk_index: u32,
    chunk_output: *mut ComponentChunk,
) -> u8 {
    // Step 1: Reject a null output buffer with the caller-bug status, not the
    // end-of-iteration code. Then check the requested access.
    if chunk_output.is_null() {
        return 5;
    }
    let stable_id = StableComponentId::from_halves(stable_id_low, stable_id_high);
    match access_is_authorized(stable_id, request_mode) {
        None => return 3,
        Some(false) => return 4,
        Some(true) => {}
    }

    // Step 2: Look up the component binding and write the requested chunk into
    // the managed caller's output buffer.
    let status = with_active_context(|world, bindings| match bindings.get(&stable_id).copied() {
        Some(ComponentBinding::Managed { component_id, .. }) => {
            let change_tick = world.change_tick().get();
            let scope_token = active_scope_token();
            // The row stride comes from the live column inside the accessor,
            // never from the binding's copy of the layout. The chunk written
            // here points into that column, and only the column describes what
            // is stored there, so a stale binding cannot make managed row
            // arithmetic read past the column. An index that finds no column
            // serves nothing, the same as an index past the end.
            let Some(chunk) =
                world.descriptor_component_chunk_mut(component_id, chunk_index as usize)
            else {
                return 0;
            };
            write_component_chunk(
                chunk_output,
                chunk.archetype,
                chunk.data,
                chunk.len,
                chunk.element_size,
                chunk.ticks.as_mut_ptr(),
                change_tick,
                scope_token,
            );
            1
        }
        Some(ComponentBinding::ModuleNative {
            component_id, size, ..
        }) => {
            let change_tick = world.change_tick().get();
            let scope_token = active_scope_token();
            // The native twin of the descriptor path: serve the module's own
            // column as raw bytes, so managed code and Rust use the same
            // storage.
            let Some(chunk) = world.native_component_chunk_mut(component_id, chunk_index as usize)
            else {
                return 0;
            };
            if !module_binding_matches_live_column(component_id, size, chunk.element_size) {
                return 2;
            }
            write_component_chunk(
                chunk_output,
                chunk.archetype,
                chunk.data,
                chunk.len,
                chunk.element_size,
                chunk.ticks.as_mut_ptr(),
                change_tick,
                scope_token,
            );
            1
        }
        None => 2,
    })
    .unwrap_or(3);
    record_served_archetype(status, chunk_output);
    status
}

/// Panic-guarded entry point for [`ffi_get_component_chunk_guarded`].
///
/// A panic that crosses the `extern "C"` boundary aborts the process. This
/// wrapper logs the panic and answers `5` instead, so the failure is recorded
/// against the running system and the host keeps running.
pub(super) extern "C" fn ffi_get_component_chunk(
    stable_id_low: u64,
    stable_id_high: u64,
    request_mode: u8,
    chunk_index: u32,
    chunk_output: *mut ComponentChunk,
) -> u8 {
    super::context::guard_managed_callback("ffi_get_component_chunk", 5, || {
        ffi_get_component_chunk_guarded(
            stable_id_low,
            stable_id_high,
            request_mode,
            chunk_index,
            chunk_output,
        )
    })
}

/// Return one component chunk of an archetype the managed enumerator already
/// identified through its driver chunk.
///
/// Managed query iterators call this once per archetype for each non-driver
/// term, instead of scanning chunk indices until the archetypes match.
///
/// `Engine.TryGetArchetypeChunk` on the managed side reads these status codes:
///
/// - `0`: nothing to serve. For a component term, the archetype does not carry
///   the component, so an optional term is absent.
/// - `1`: the output holds a complete chunk.
/// - `2`: the component is unknown, or a module binding no longer matches the
///   live column's element size.
/// - `3`: no managed system is running on this thread.
/// - `4`: the running system did not declare the requested access, or this
///   invocation has not served the archetype yet.
/// - `5`: the caller passed no output buffer.
///
/// A `request_mode` of `2` asks for the archetype's entity column instead of a
/// component. That column belongs to no component, so there is no access
/// declaration to check.
fn ffi_get_archetype_chunk_guarded(
    archetype_low: u64,
    archetype_high: u64,
    stable_id_low: u64,
    stable_id_high: u64,
    request_mode: u8,
    chunk_output: *mut ComponentChunk,
) -> u8 {
    // Step 1: Reject a null output buffer before rebuilding the identity. The
    // status reports the caller's bug; it does not end iteration.
    if chunk_output.is_null() {
        return 5;
    }
    let archetype_id = ArchetypeId(((archetype_high as u128) << 64) | archetype_low as u128);

    // Step 2: An entity term reads the archetype's entity column, which needs
    // no component access check - the same rule as the dedicated entity
    // callback. The archetype must already have been served in this
    // invocation: otherwise managed code could ask about any archetype id and
    // learn which components it holds without declaring access to them.
    if request_mode == 2 {
        match archetype_was_observed(archetype_id) {
            None => return 3,
            Some(false) => return 4,
            Some(true) => {}
        }
        let status = with_active_world(|world| {
            let Some((archetype, entities)) = world.entity_chunk_in_archetype(archetype_id) else {
                return 0;
            };
            write_entity_chunk(
                chunk_output,
                archetype,
                entities,
                world.change_tick().get(),
                active_scope_token(),
            );
            1
        })
        .unwrap_or(3);
        record_served_archetype(status, chunk_output);
        return status;
    }

    // Step 3: A component term checks the declared access exactly like the
    // index-based lookup, so both entry points answer the same status.
    let stable_id = StableComponentId::from_halves(stable_id_low, stable_id_high);
    match access_is_authorized(stable_id, request_mode) {
        None => return 3,
        Some(false) => return 4,
        Some(true) => {}
    }

    // Step 4: Look up the binding and write the column from that one
    // archetype. No other chunk of the world is touched.
    let status = with_active_context(|world, bindings| match bindings.get(&stable_id).copied() {
        Some(ComponentBinding::Managed { component_id, .. }) => {
            // As in the index-based callback, the row stride is the live
            // column's, so a stale binding cannot mislead managed row
            // arithmetic.
            let change_tick = world.change_tick().get();
            let scope_token = active_scope_token();
            let Some(chunk) =
                world.descriptor_component_chunk_in_archetype(component_id, archetype_id)
            else {
                return 0;
            };
            write_component_chunk(
                chunk_output,
                chunk.archetype,
                chunk.data,
                chunk.len,
                chunk.element_size,
                chunk.ticks.as_mut_ptr(),
                change_tick,
                scope_token,
            );
            1
        }
        Some(ComponentBinding::ModuleNative {
            component_id, size, ..
        }) => {
            let change_tick = world.change_tick().get();
            let scope_token = active_scope_token();
            let Some(chunk) = world.native_component_chunk_in_archetype(component_id, archetype_id)
            else {
                return 0;
            };
            if !module_binding_matches_live_column(component_id, size, chunk.element_size) {
                return 2;
            }
            write_component_chunk(
                chunk_output,
                chunk.archetype,
                chunk.data,
                chunk.len,
                chunk.element_size,
                chunk.ticks.as_mut_ptr(),
                change_tick,
                scope_token,
            );
            1
        }
        None => 2,
    })
    .unwrap_or(3);
    record_served_archetype(status, chunk_output);
    status
}

/// Panic-guarded entry point for [`ffi_get_archetype_chunk_guarded`].
///
/// A panic that crosses the `extern "C"` boundary aborts the process. This
/// wrapper logs the panic and answers `5` instead, so the failure is recorded
/// against the running system and the host keeps running.
pub(super) extern "C" fn ffi_get_archetype_chunk(
    archetype_low: u64,
    archetype_high: u64,
    stable_id_low: u64,
    stable_id_high: u64,
    request_mode: u8,
    chunk_output: *mut ComponentChunk,
) -> u8 {
    super::context::guard_managed_callback("ffi_get_archetype_chunk", 5, || {
        ffi_get_archetype_chunk_guarded(
            archetype_low,
            archetype_high,
            stable_id_low,
            stable_id_high,
            request_mode,
            chunk_output,
        )
    })
}

/// Return the entity column of the archetype at `chunk_index`.
///
/// The slice written to `chunk_output` holds the `Entity` values of one
/// archetype. It is valid only while the managed system that made the call is
/// running, so the caller must not keep it after the call returns.
fn ffi_get_entity_chunk_guarded(chunk_index: u32, chunk_output: *mut ComponentChunk) -> u8 {
    // Step 1: Reject a null output buffer before touching the world. Status
    // `5` reports the caller's bug; it does not end iteration.
    if chunk_output.is_null() {
        return 5;
    }
    // Step 2: Find the entity column and write it into the output buffer.
    with_active_world(|world| {
        let Some((archetype, entities)) = world.entity_chunk(chunk_index as usize) else {
            return 0;
        };
        write_entity_chunk(
            chunk_output,
            archetype,
            entities,
            world.change_tick().get(),
            active_scope_token(),
        );
        record_observed_archetype(archetype);
        1
    })
    .unwrap_or(3)
}

/// Panic-guarded entry point for [`ffi_get_entity_chunk_guarded`].
///
/// A panic that crosses the `extern "C"` boundary aborts the process. This
/// wrapper logs the panic and answers `5` instead, so the failure is recorded
/// against the running system and the host keeps running.
pub(super) extern "C" fn ffi_get_entity_chunk(
    chunk_index: u32,
    chunk_output: *mut ComponentChunk,
) -> u8 {
    super::context::guard_managed_callback("ffi_get_entity_chunk", 5, || {
        ffi_get_entity_chunk_guarded(chunk_index, chunk_output)
    })
}

/// Write the current entity count while a managed system is active.
///
/// Status codes:
///
/// - `0`: the count was written.
/// - `3`: no managed system is scheduled. The count alone could not show the
///   difference from an empty world, so it gets its own status, which the
///   managed wrapper turns into an exception.
/// - `5`: the caller passed no output buffer.
///
/// The count is `u32` by ABI design, so worlds above ~4.29 billion entities
/// are unsupported. See the `ComponentChunk` layout-limits documentation.
fn ffi_entity_count_guarded(count_output: *mut u32) -> u8 {
    if count_output.is_null() {
        return 5;
    }
    let Some(count) = with_active_world(|world| world.entity_count() as u32) else {
        return 3;
    };
    // SAFETY: `count_output` was checked non-null above and points at a
    // caller-owned `u32` for the duration of this call.
    unsafe { count_output.write(count) };
    0
}

/// Panic-guarded entry point for [`ffi_entity_count_guarded`].
///
/// A panic that crosses the `extern "C"` boundary aborts the process. This
/// wrapper logs the panic and answers `5` instead, so the failure is recorded
/// against the running system and the host keeps running.
pub(super) extern "C" fn ffi_entity_count(count_output: *mut u32) -> u8 {
    super::context::guard_managed_callback("ffi_entity_count", 5, || {
        ffi_entity_count_guarded(count_output)
    })
}

/// Write a live entity's stable id, split into two `u64` halves.
///
/// Status codes:
///
/// - `0`: the id was written.
/// - `1`: the entity is not live. It was destroyed, or the handle belongs to
///   another world.
/// - `3`: no managed system is scheduled.
/// - `5`: the entity handle or one of the two output halves was null.
///
/// Two `u64` halves stand in for one `u128`, because no C ABI in the engine
/// table carries one. The managed side joins the pair back into the 32-digit
/// hex form that saved data records.
fn ffi_entity_stable_id_guarded(
    entity: *const Entity,
    low_half_output: *mut u64,
    high_half_output: *mut u64,
) -> u8 {
    if entity.is_null() || low_half_output.is_null() || high_half_output.is_null() {
        return 5;
    }
    // SAFETY: `entity` was checked non-null above and points at a caller-owned
    // handle for the duration of this call.
    let entity = unsafe { entity.read() };
    let Some(entity_id) = with_active_world(|world| world.entity_id(entity)) else {
        return 3;
    };
    let Some(entity_id) = entity_id else {
        return 1;
    };
    let stable_id_value = entity_id.value();
    // SAFETY: `low_half_output` and `high_half_output` were checked non-null
    // above, and both point at caller-owned `u64`s for the duration of this
    // call.
    unsafe {
        low_half_output.write(stable_id_value as u64);
        high_half_output.write((stable_id_value >> 64) as u64);
    }
    0
}

/// Panic-guarded entry point for [`ffi_entity_stable_id_guarded`].
pub(super) extern "C" fn ffi_entity_stable_id(
    entity: *const Entity,
    low_half_output: *mut u64,
    high_half_output: *mut u64,
) -> u8 {
    super::context::guard_managed_callback("ffi_entity_stable_id", 5, || {
        ffi_entity_stable_id_guarded(entity, low_half_output, high_half_output)
    })
}
