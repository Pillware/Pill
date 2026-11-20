//! Native callbacks that turn C# entity requests into deferred ECS commands.
//!
//! # Responsibilities
//!
//! - Reserve generation-checked entity handles for managed callers.
//! - Decode managed component blobs into native adders or descriptor bytes.
//! - Queue create, destroy, add, and remove commands against the active
//!   world, rejecting stale entity handles, components with no registered
//!   binding, and calls from a system that declared no command access.
//!
//! # Design
//!
//! Every callback runs inside [`with_active_command_context`], which passes
//! the closure the active world, the command queue, the component binding
//! table, and the set of entity handles reserved during this invocation. Each
//! blob is decoded through its [`ComponentBinding`] into a
//! [`DecodedCommandComponent`] before it reaches the queue: a byte adder for a
//! native component, plain bytes for a descriptor component. Managed code
//! therefore never names a concrete Rust type.

// Standard library
use std::collections::HashSet;

// External crates
use pill_engine::commands::{ByteComponentAdder, ComponentAdder};
use pill_engine::{ComponentId, Entity};

// Current crate
use super::abi::NativeComponentBlob;
use super::components::{ComponentBinding, StableComponentId};
use super::context::{active_world_exists, with_active_command_context};

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of component blobs one managed create may carry.
///
/// This bound is part of the ABI contract: the `queue_create` entry point
/// refuses a larger count, so managed code must keep its creates at or below
/// it.
pub(super) const MAX_COMPONENTS_PER_CREATE: u32 = 1024;

// =============================================================================
// Types
// =============================================================================

/// One component blob, decoded into a form the command queue accepts.
///
/// The native variant holds a boxed [`ComponentAdder`] built from the blob,
/// which writes the bytes into a native component column. The descriptor
/// variant holds the engine component id plus the raw bytes, which the queue
/// writes into the component's storage when the command runs.
enum DecodedCommandComponent {
    /// A native Rust component, added through the byte adder its column
    /// accepts.
    Native(Box<dyn ComponentAdder>),
    /// Raw bytes for a component the managed side registered.
    Descriptor(ComponentId, Vec<u8>),
}

// =============================================================================
// Free Functions
// =============================================================================

/// Return the status code for a callback that could not enter the command
/// scope.
///
/// Returns 3 when no world is active on this thread. Returns 4 when a world is
/// active but its system declared no `Commands` parameter, so the callback may
/// not use the queue.
fn command_scope_error() -> u8 {
    if active_world_exists() {
        4
    } else {
        3
    }
}

/// Reserve a generation-checked entity handle without inserting the entity
/// into an archetype.
///
/// A reservation that no create consumes goes back to the entity allocator
/// when the invocation scope ends.
fn ffi_reserve_entity_guarded(entity_output: *mut Entity) -> u8 {
    if entity_output.is_null() {
        return 6;
    }
    with_active_command_context(|world, _queue, _bindings, reserved| {
        // Step 1: allocate a fresh generation-checked entity handle.
        let entity = world.reserve_entity();
        // Step 2: track the handle so an unconsumed reservation is returned
        // to the allocator when the invocation scope ends.
        reserved.insert(entity);
        // Step 3: write the handle back through the managed out pointer.
        // SAFETY: the pointer was checked for null above, and managed code
        // points it at an `Entity` value whose C# mirror declares the same
        // `#[repr(C)]` layout.
        unsafe { entity_output.write(entity) };
        1
    })
    .unwrap_or_else(command_scope_error)
}

/// Panic-guarded entry point for [`ffi_reserve_entity_guarded`].
///
/// A panic that crosses the `extern "C"` boundary would abort the process.
/// This catches it, logs it, and returns `6` instead, which the managed side
/// reports as a failure of the running system, so the host keeps running.
pub(super) extern "C" fn ffi_reserve_entity(entity_output: *mut Entity) -> u8 {
    super::context::guard_managed_callback("ffi_reserve_entity", 6, || {
        ffi_reserve_entity_guarded(entity_output)
    })
}

/// Check one component blob and copy it into the form the command queue takes
/// for its binding.
///
/// # Errors
///
/// Returns an error string when the blob's data pointer is null, or when the
/// blob size differs from the size the binding declares.
fn decode_command_component(
    binding: ComponentBinding,
    blob_bytes: *const u8,
    blob_size: usize,
) -> Result<DecodedCommandComponent, String> {
    if blob_bytes.is_null() {
        return Err("component data pointer is null".into());
    }
    match binding {
        ComponentBinding::Managed {
            component_id,
            size: expected_size,
            ..
        } => {
            if blob_size != expected_size {
                return Err("descriptor component blob has the wrong size".into());
            }
            // SAFETY: the managed caller keeps a buffer of `blob_size` bytes
            // valid for this call, and the bytes are copied before it returns.
            let bytes = unsafe { std::slice::from_raw_parts(blob_bytes, blob_size) }.to_vec();
            Ok(DecodedCommandComponent::Descriptor(component_id, bytes))
        }
        ComponentBinding::ModuleNative {
            component_id,
            size: expected_size,
            ..
        } => {
            if blob_size != expected_size {
                return Err("module component blob has the wrong size".into());
            }
            // SAFETY: the managed caller keeps a buffer of `blob_size` bytes
            // valid for this call, and the bytes are copied before it returns.
            let bytes = unsafe { std::slice::from_raw_parts(blob_bytes, blob_size) }.to_vec();
            Ok(DecodedCommandComponent::Native(Box::new(
                ByteComponentAdder::new(component_id, bytes),
            )))
        }
    }
}

/// Queue entity creation, after checking every supplied component blob.
///
/// Nothing is queued unless every blob passes, and a count above
/// [`MAX_COMPONENTS_PER_CREATE`] is refused as the ABI contract requires.
fn ffi_queue_create_guarded(
    entity: *const Entity,
    blobs: *const NativeComponentBlob,
    blob_count: u32,
) -> u8 {
    if entity.is_null()
        || (blob_count != 0 && blobs.is_null())
        || blob_count > MAX_COMPONENTS_PER_CREATE
    {
        return 6;
    }
    with_active_command_context(|_world, queue, bindings, reserved| {
        // Step 1: copy the reserved handle out of the managed pointer,
        // rejecting a handle this invocation did not reserve.
        // SAFETY: the pointer is non-null (checked above) and stays valid for
        // this synchronous call.
        let entity = unsafe { *entity };
        if !reserved.contains(&entity) {
            return 5;
        }
        // Step 2: view the blob array as a slice. The pointer is null when
        // the managed caller supplied no components, so the zero-count case
        // never builds a slice from raw parts.
        let blobs: &[NativeComponentBlob] = if blob_count == 0 {
            &[]
        } else {
            // SAFETY: `blobs` is non-null whenever `blob_count` is non-zero
            // (checked above), and the managed caller pins an array of exactly
            // `blob_count` blobs for the duration of this call.
            unsafe { std::slice::from_raw_parts(blobs, blob_count as usize) }
        };
        // Step 3: decode every blob, rejecting duplicate component
        // identities, components with no binding, and malformed payloads.
        let mut seen_component_ids = HashSet::with_capacity(blobs.len());
        let mut native_components = Vec::new();
        let mut descriptor_components = Vec::new();
        for blob in blobs {
            let stable_component_id =
                StableComponentId::from_halves(blob.component_key, blob.component_key_high);
            if !seen_component_ids.insert(stable_component_id) {
                return 6;
            }
            let Some(binding) = bindings.get(&stable_component_id).copied() else {
                return 2;
            };
            match decode_command_component(binding, blob.data, blob.size as usize) {
                Ok(DecodedCommandComponent::Native(component)) => {
                    native_components.push(component);
                }
                Ok(DecodedCommandComponent::Descriptor(id, bytes)) => {
                    descriptor_components.push((id, bytes));
                }
                Err(_) => return 6,
            }
        }
        // Step 4: queue the creation and mark the handle as used, so the
        // invocation scope does not return it to the allocator.
        reserved.remove(&entity);
        queue.create_mixed_entity(entity, native_components, descriptor_components);
        1
    })
    .unwrap_or_else(command_scope_error)
}

/// Panic-guarded entry point for [`ffi_queue_create_guarded`].
///
/// A panic that crosses the `extern "C"` boundary would abort the process.
/// This catches it, logs it, and returns `6` instead, which the managed side
/// reports as a failure of the running system, so the host keeps running.
pub(super) extern "C" fn ffi_queue_create(
    entity: *const Entity,
    blobs: *const NativeComponentBlob,
    blob_count: u32,
) -> u8 {
    super::context::guard_managed_callback("ffi_queue_create", 6, || {
        ffi_queue_create_guarded(entity, blobs, blob_count)
    })
}

/// Queue destroying an entity, but only while its handle is still live.
fn ffi_queue_destroy_guarded(entity: *const Entity) -> u8 {
    if entity.is_null() {
        return 6;
    }
    with_active_command_context(|world, queue, _bindings, _reserved| {
        // Step 1: check the handle against the live world.
        // SAFETY: the pointer is non-null (checked above) and stays valid for
        // this synchronous call.
        let entity = unsafe { *entity };
        if !world.is_entity_valid(entity) {
            return 5;
        }
        // Step 2: queue the destruction for the checked handle.
        queue.destroy_entity(entity);
        1
    })
    .unwrap_or_else(command_scope_error)
}

/// Panic-guarded entry point for [`ffi_queue_destroy_guarded`].
///
/// A panic that crosses the `extern "C"` boundary would abort the process.
/// This catches it, logs it, and returns `6` instead, which the managed side
/// reports as a failure of the running system, so the host keeps running.
pub(super) extern "C" fn ffi_queue_destroy(entity: *const Entity) -> u8 {
    super::context::guard_managed_callback("ffi_queue_destroy", 6, || {
        ffi_queue_destroy_guarded(entity)
    })
}

/// Queue adding one component, selected by its stable managed identity.
fn ffi_queue_add_component_guarded(
    entity: *const Entity,
    key_low: u64,
    key_high: u64,
    blob_bytes: *const u8,
    blob_size: u32,
) -> u8 {
    if entity.is_null() {
        return 6;
    }
    with_active_command_context(|world, queue, bindings, _reserved| {
        // Step 1: check the handle against the live world.
        // SAFETY: the pointer is non-null (checked above) and stays valid for
        // this synchronous call.
        let entity = unsafe { *entity };
        if !world.is_entity_valid(entity) {
            return 5;
        }
        // Step 2: look up the stable component identity in the binding table.
        let stable_component_id = StableComponentId::from_halves(key_low, key_high);
        let Some(binding) = bindings.get(&stable_component_id).copied() else {
            return 2;
        };
        // Step 3: decode the blob and queue the matching add command.
        match decode_command_component(binding, blob_bytes, blob_size as usize) {
            Ok(DecodedCommandComponent::Native(component)) => {
                queue.add_component_adder_to_entity(entity, component)
            }
            Ok(DecodedCommandComponent::Descriptor(component_id, bytes)) => {
                queue.add_descriptor_component_to_entity(entity, component_id, bytes)
            }
            Err(_) => return 6,
        }
        1
    })
    .unwrap_or_else(command_scope_error)
}

/// Panic-guarded entry point for [`ffi_queue_add_component_guarded`].
///
/// A panic that crosses the `extern "C"` boundary would abort the process.
/// This catches it, logs it, and returns `6` instead, which the managed side
/// reports as a failure of the running system, so the host keeps running.
pub(super) extern "C" fn ffi_queue_add_component(
    entity: *const Entity,
    key_low: u64,
    key_high: u64,
    blob_bytes: *const u8,
    blob_size: u32,
) -> u8 {
    super::context::guard_managed_callback("ffi_queue_add_component", 6, || {
        ffi_queue_add_component_guarded(entity, key_low, key_high, blob_bytes, blob_size)
    })
}

/// Queue removing one component, named by its stable managed identity.
fn ffi_queue_remove_component_guarded(entity: *const Entity, key_low: u64, key_high: u64) -> u8 {
    if entity.is_null() {
        return 6;
    }
    with_active_command_context(|world, queue, bindings, _reserved| {
        // Step 1: check the handle against the live world.
        // SAFETY: the pointer is non-null (checked above) and stays valid for
        // this synchronous call.
        let entity = unsafe { *entity };
        if !world.is_entity_valid(entity) {
            return 5;
        }
        // Step 2: look up the stable component identity in the binding table.
        let stable_component_id = StableComponentId::from_halves(key_low, key_high);
        let Some(binding) = bindings.get(&stable_component_id).copied() else {
            return 2;
        };
        // Step 3: queue the removal by the engine component id the binding
        // names.
        queue.remove_component_by_id(entity, binding.component_id());
        1
    })
    .unwrap_or_else(command_scope_error)
}

/// Panic-guarded entry point for [`ffi_queue_remove_component_guarded`].
///
/// A panic that crosses the `extern "C"` boundary would abort the process.
/// This catches it, logs it, and returns `6` instead, which the managed side
/// reports as a failure of the running system, so the host keeps running.
pub(super) extern "C" fn ffi_queue_remove_component(
    entity: *const Entity,
    key_low: u64,
    key_high: u64,
) -> u8 {
    super::context::guard_managed_callback("ffi_queue_remove_component", 6, || {
        ffi_queue_remove_component_guarded(entity, key_low, key_high)
    })
}
