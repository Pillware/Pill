//! Thread-local access scope installed around one scheduled C# system or one
//! startup method.
//!
//! # Responsibilities
//!
//! - Publish the active world, command queue, and component bindings for
//!   managed callbacks.
//! - Reject nested or out-of-scope managed invocation.
//! - Clear the complete thread-local scope when the invocation ends.
//!
//! # Design
//!
//! The raw pointers (world, command queue, access list, bindings) are kept in
//! one [`ActiveScopeData`] value inside a single thread-local [`Cell`]. The
//! reserved entities, the observed archetypes, and the invocation counter are
//! separate thread-locals. Installing the scope and clearing it are each one
//! assignment, so a panic can never leave half of the pointers installed: the
//! value is either fully there or not there at all.

// Standard library
use std::cell::{Cell, RefCell};
use std::collections::HashSet;

// External crates
use pill_core::error;
use pill_core::telemetry::telemetry_target;
use pill_engine::archetype::ArchetypeId;
use pill_engine::commands::CommandQueue;
use pill_engine::{Entity, World};

// Current crate
use super::abi::NativeSystemAccess;
use super::components::{ComponentBindings, StableComponentId};

// =============================================================================
// Constants
// =============================================================================

/// Marks an access entry that names a component.
pub(super) const ACCESS_KIND_COMPONENT: u8 = 0;

/// Marks an access entry that names a resource from the managed manifest.
pub(super) const ACCESS_KIND_RESOURCE: u8 = 1;

/// Marks an access entry that names a Rust-owned resource by its shared name
/// (`ResMut<AssetManager>`). Managed code never sees this resource's bytes.
///
/// This kind is separate from [`ACCESS_KIND_RESOURCE`] because the two are
/// looked up in different tables: a managed resource through the manifest's
/// resource bindings, a native one straight to `ResourceId::Shared` of its
/// name.
pub(super) const ACCESS_KIND_NATIVE_RESOURCE: u8 = 2;

thread_local! {
    /// The complete managed invocation scope for the current thread.
    ///
    /// `None` means no managed code is running on this thread, whether a
    /// system or a startup method.
    static ACTIVE_SCOPE: Cell<Option<ActiveScopeData>> = const { Cell::new(None) };
    /// Entity handles reserved during this invocation and not yet used by a
    /// create.
    static RESERVED_ENTITIES: RefCell<HashSet<Entity>> = RefCell::new(HashSet::new());
    /// Archetypes this invocation has already served to managed code.
    ///
    /// Only these are answered by the entity path (`mode == 2` of the
    /// archetype chunk callback). Managed code reaches an archetype through a
    /// validated component or entity lookup first; without that requirement,
    /// the entity path could enumerate the component sets of the whole world.
    static OBSERVED_ARCHETYPES: RefCell<HashSet<ArchetypeId>> = RefCell::new(HashSet::new());
    /// Counter of managed invocations on this thread, copied into every chunk.
    ///
    /// Zero means no managed code is running on this thread, so it also gives
    /// the out-of-scope answer. The counter only moves forward and skips zero
    /// when it wraps, so a token from an earlier invocation never equals the
    /// current one and nothing has to be cleared when the invocation ends.
    static SCOPE_TOKEN: Cell<u32> = const { Cell::new(0) };
}

// =============================================================================
// Types + Impls
// =============================================================================

/// One complete managed invocation scope.
///
/// Keeping every pointer in one value means installing the scope and clearing
/// it are one [`Cell::set`] call each, so a panic can never leave a partially
/// installed scope on this thread.
#[derive(Clone, Copy)]
struct ActiveScopeData {
    world: *mut World,
    queue: *mut CommandQueue,
    accesses: (*const NativeSystemAccess, usize),
    bindings: *const ComponentBindings,
    uses_commands: bool,
    /// Whether the invocation holds the whole world, as a startup method does.
    /// Every resource access is then allowed without a declaration, because
    /// nothing else can be running.
    exclusive: bool,
    /// Invocation token copied into every chunk issued during this invocation.
    scope_token: u32,
}

/// Keeps the thread-local invocation scope installed for exactly one managed
/// invocation.
///
/// The scope installers create it, and it is dropped when the invocation ends.
/// Its drop clears every raw pointer from the thread-local scope, including
/// when managed execution unwinds through a panic.
pub(super) struct ActiveScopeGuard;

impl ActiveScopeGuard {
    /// Installs a query-only invocation scope for native bridge tests.
    ///
    /// Production systems use [`Self::set_with_commands`] instead, because the
    /// scheduler always supplies a queue even when the managed signature takes
    /// no Commands parameter.
    #[cfg(test)]
    pub(super) fn install_query_scope(
        world: &mut World,
        access: &[NativeSystemAccess],
        bindings: &ComponentBindings,
    ) -> Option<Self> {
        Self::install_scope(world, std::ptr::null_mut(), access, bindings, false, false)
    }

    /// Publishes one scheduled system's world, command queue, component
    /// bindings, and declared access list for its synchronous managed
    /// callbacks.
    pub(super) fn install_scope_with_commands(
        world: &mut World,
        queue: &mut CommandQueue,
        access: &[NativeSystemAccess],
        bindings: &ComponentBindings,
        uses_commands: bool,
    ) -> Option<Self> {
        Self::install_scope(world, queue, access, bindings, uses_commands, false)
    }

    /// Publishes a startup method's scope. It holds the whole world, a command
    /// queue, and the component bindings, and every resource is reachable.
    ///
    /// Startup methods run one at a time before the scheduler starts, so the
    /// scope holds the world exclusively and needs no access list. That is why
    /// a startup method can take `Res<T>` / `ResMut<T>` parameters it never
    /// declared to a scheduler.
    pub(super) fn install_exclusive_scope(
        world: &mut World,
        queue: &mut CommandQueue,
        bindings: &ComponentBindings,
    ) -> Option<Self> {
        Self::install_scope(world, queue, &[], bindings, true, true)
    }

    /// Installs the complete scope in one assignment, after rejecting a nested
    /// invocation that would overwrite the raw pointers already stored.
    ///
    /// Returns `None` when a scope is already installed on this thread. That
    /// means a C# system re-entered the host while it was running, which is a
    /// mistake in the managed code. It must not panic: unwinding through the
    /// .NET frames that called in is undefined behaviour at the FFI boundary,
    /// so the condition is reported instead of raised.
    ///
    /// The nested-invocation check is the only fallible step and it runs before
    /// anything is changed, so a rejection leaves no stale state behind.
    fn install_scope(
        world: &mut World,
        queue: *mut CommandQueue,
        access: &[NativeSystemAccess],
        bindings: &ComponentBindings,
        uses_commands: bool,
        exclusive: bool,
    ) -> Option<Self> {
        // Step 1: Reject nested invocation before touching any thread-local.
        let already_active = ACTIVE_SCOPE.with(|scope_slot| scope_slot.get().is_some());
        if already_active {
            error!(
                target: telemetry_target::ECS,
                "nested managed ECS system invocation rejected; a managed system                  re-entered the host while one was already running"
            );
            return None;
        }

        // Step 2: Clear the reservations left by the previous invocation,
        // without panicking. A reservation borrow can only exist while a scope
        // is active, and Step 1 has already rejected that case, so this is only
        // a second safeguard.
        RESERVED_ENTITIES.with(|reserved_slot| {
            if let Ok(mut reserved) = reserved_slot.try_borrow_mut() {
                reserved.clear();
            }
        });
        // Every invocation starts with no archetype observed. The entity path
        // answers only for what this invocation reached through a validated
        // lookup.
        OBSERVED_ARCHETYPES.with(|observed_slot| {
            if let Ok(mut observed) = observed_slot.try_borrow_mut() {
                observed.clear();
            }
        });

        // Step 3: Store the whole scope with one assignment. `Cell::set` cannot
        // panic, so from here on the guard's `Drop` does the clearing. The
        // invocation token is taken first. The counter skips zero when it
        // wraps, and zero means "no scope", so an old chunk can never look
        // current after the wrap.
        let scope_token = SCOPE_TOKEN.with(|token_slot| {
            let next_token = match token_slot.get().wrapping_add(1) {
                0 => 1,
                incremented => incremented,
            };
            token_slot.set(next_token);
            next_token
        });

        ACTIVE_SCOPE.with(|scope_slot| {
            scope_slot.set(Some(ActiveScopeData {
                world: world as *mut World,
                queue,
                accesses: (access.as_ptr(), access.len()),
                bindings: bindings as *const ComponentBindings,
                uses_commands,
                exclusive,
                scope_token,
            }));
        });
        Some(Self)
    }
}

impl Drop for ActiveScopeGuard {
    /// Returns unused reservations to the entity allocator and clears the
    /// complete scope before the borrowed world and queue are released.
    fn drop(&mut self) {
        ACTIVE_SCOPE.with(|scope_slot| {
            let world_pointer = scope_slot.get().map(|scope| scope.world);
            // Step 1: Give back reserved handles that no create used.
            RESERVED_ENTITIES.with(|reserved_slot| {
                if let Ok(mut reserved) = reserved_slot.try_borrow_mut() {
                    // Handles reserved but never used by a create go back to
                    // the allocator, so a managed allocation that is only
                    // planned cannot leak an entity id and generation pair.
                    if let Some(world_pointer) = world_pointer.filter(|pointer| !pointer.is_null())
                    {
                        // SAFETY: the scope stays installed for the whole of
                        // this drop, so the world pointer is still valid here;
                        // it is cleared only at the end of this closure.
                        let world = unsafe { &mut *world_pointer };
                        for entity in reserved.drain() {
                            world.release_entity(entity);
                        }
                    }
                }
            });
            // Step 2: Forget the archetypes this invocation observed.
            OBSERVED_ARCHETYPES.with(|observed_slot| {
                if let Ok(mut observed) = observed_slot.try_borrow_mut() {
                    observed.clear();
                }
            });
            // Step 3: Close profiling zones the managed code left open.
            // A managed system that opened a profiling zone and never ended it,
            // or threw between the two calls, would leave a Tracy zone open
            // past the invocation. Closing it here keeps the zone list from
            // growing by one entry per frame, and keeps the zone on the
            // timeline of the invocation that opened it.
            super::abi::end_unfinished_zones();
            // Step 4: Clear the scope last, so the pointers above stay valid.
            scope_slot.set(None);
        });
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Runs one managed FFI callback and turns a panic into its failure status.
///
/// Since Rust 1.81 a panic that crosses an `extern "C"` boundary aborts the
/// process instead of being undefined. This function reports the failure status
/// instead, so the process keeps running, and the managed side then fails the
/// running system through its per-system error channel. The panic payload is
/// logged here because a status code cannot carry it.
///
/// `AssertUnwindSafe` is required because these callbacks take raw pointers,
/// which are never `UnwindSafe`. The assertion is sound because a failed
/// callback leaves the frame's work incomplete, and both the deferred command
/// queue and the invocation scope are torn down when the system reports
/// failure.
pub(super) fn guard_managed_callback(
    callback_name: &'static str,
    failure_status: u8,
    body: impl FnOnce() -> u8,
) -> u8 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(status) => status,
        Err(payload) => {
            let detail = payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a non-string panic payload".to_string());
            error!(
                target: telemetry_target::ECS,
                callback = callback_name,
                detail = detail.as_str(),
                "a managed callback panicked; reporting failure to the calling system instead of aborting"
            );
            failure_status
        }
    }
}

/// Runs a callback with the active world, deferred command queue, component
/// bindings, and reserved-entity set, but only when the system declared a
/// Commands parameter.
pub(super) fn with_active_command_context<R>(
    body: impl FnOnce(&mut World, &mut CommandQueue, &ComponentBindings, &mut HashSet<Entity>) -> R,
) -> Option<R> {
    ACTIVE_SCOPE.with(|scope_slot| {
        let scope = scope_slot.get()?;
        if !scope.uses_commands {
            return None;
        }
        let world = scope.world;
        let queue = scope.queue;
        let bindings = scope.bindings;
        if world.is_null() || queue.is_null() || bindings.is_null() {
            return None;
        }
        RESERVED_ENTITIES.with(|reserved_slot| {
            // SAFETY: the guard installs the scope for exactly one managed
            // invocation and clears it before the world, queue, and bindings
            // borrows it was built from end.
            Some(unsafe {
                body(
                    &mut *world,
                    &mut *queue,
                    &*bindings,
                    &mut reserved_slot.borrow_mut(),
                )
            })
        })
    })
}

/// Runs a callback with the world of the active managed invocation, returning
/// `None` when no managed invocation is running on this thread.
pub(super) fn with_active_world<R>(body: impl FnOnce(&mut World) -> R) -> Option<R> {
    ACTIVE_SCOPE.with(|scope_slot| {
        let world_pointer = scope_slot.get().map(|scope| scope.world)?;
        // SAFETY: the guard installs this pointer just before the managed
        // invocation and clears it before the world borrow it came from ends.
        (!world_pointer.is_null()).then(|| unsafe { body(&mut *world_pointer) })
    })
}

/// Returns the token of the invocation active on this thread, or zero when
/// there is none.
///
/// Every chunk carries the token that was current when it was issued, so
/// managed code can tell a chunk from the call it is inside apart from one
/// kept since an earlier invocation.
pub(super) fn active_scope_token() -> u32 {
    ACTIVE_SCOPE.with(|scope_slot| scope_slot.get().map_or(0, |scope| scope.scope_token))
}

/// Returns [`active_scope_token`] to managed code through the engine API table.
pub(super) extern "C" fn ffi_current_scope_token() -> u32 {
    active_scope_token()
}

/// Reports whether a managed scope exists, without dereferencing its world
/// pointer.
pub(super) fn active_world_exists() -> bool {
    ACTIVE_SCOPE.with(|scope_slot| scope_slot.get().is_some_and(|scope| !scope.world.is_null()))
}

/// Runs a callback with the active world and its stable component binding
/// table, which the same guard always installs and clears together.
pub(super) fn with_active_context<R>(
    body: impl FnOnce(&mut World, &ComponentBindings) -> R,
) -> Option<R> {
    ACTIVE_SCOPE.with(|scope_slot| {
        let scope = scope_slot.get()?;
        if scope.world.is_null() || scope.bindings.is_null() {
            return None;
        }
        // SAFETY: the guard installs the complete scope for exactly one
        // managed invocation and clears it when that invocation ends.
        Some(unsafe { body(&mut *scope.world, &*scope.bindings) })
    })
}

/// Records that a validated term served an archetype to managed code during
/// the active invocation.
///
/// The chunk callbacks call this after every successful component or entity
/// lookup. The entity path may then serve this archetype's entity column later
/// in the same invocation.
pub(super) fn record_observed_archetype(archetype: ArchetypeId) {
    OBSERVED_ARCHETYPES.with(|observed_slot| {
        if let Ok(mut observed) = observed_slot.try_borrow_mut() {
            observed.insert(archetype);
        }
    });
}

/// Returns whether a validated term has already served this archetype in the
/// active invocation.
///
/// `None` means no managed invocation is active on this thread, which is the
/// same answer [`access_is_authorized`] gives in that state.
pub(super) fn archetype_was_observed(archetype: ArchetypeId) -> Option<bool> {
    ACTIVE_SCOPE.with(|scope_slot| {
        scope_slot.get()?;
        OBSERVED_ARCHETYPES.with(|observed_slot| {
            Some(
                observed_slot
                    .try_borrow()
                    .is_ok_and(|observed| observed.contains(&archetype)),
            )
        })
    })
}

/// Checks whether the active system declared the requested component mode.
///
/// A write declaration also allows reads. A read declaration never allows
/// writes. `None` means no managed invocation is active on this thread.
pub(super) fn access_is_authorized(key: StableComponentId, requested_mode: u8) -> Option<bool> {
    declared_access_allows(ACCESS_KIND_COMPONENT, key, requested_mode)
}

/// The resource counterpart of [`access_is_authorized`].
///
/// This is a separate function instead of a flag on the component check
/// because the two look at different declarations that happen to share one key
/// space. A resource and a component can have the same key, and a component
/// entry must never give access to resource storage.
pub(super) fn resource_access_is_authorized(
    key: StableComponentId,
    requested_mode: u8,
) -> Option<bool> {
    declared_access_allows(ACCESS_KIND_RESOURCE, key, requested_mode)
}

/// The counterpart of [`resource_access_is_authorized`] for a Rust-owned
/// resource that is reached by its shared name.
pub(super) fn native_resource_access_is_authorized(
    key: StableComponentId,
    requested_mode: u8,
) -> Option<bool> {
    declared_access_allows(ACCESS_KIND_NATIVE_RESOURCE, key, requested_mode)
}

/// Returns whether the active system's declared access list allows one request.
///
/// An exclusive scope (a startup method) allows every resource access and no
/// component access, because a startup has no queries that could have declared
/// one.
fn declared_access_allows(
    access_kind: u8,
    key: StableComponentId,
    requested_mode: u8,
) -> Option<bool> {
    ACTIVE_SCOPE.with(|scope_slot| {
        let scope = scope_slot.get()?;
        if scope.exclusive && access_kind != ACCESS_KIND_COMPONENT {
            return Some(true);
        }
        let (pointer, len) = scope.accesses;
        if pointer.is_null() {
            return None;
        }
        // SAFETY: `ActiveSystemGuard` stores a slice owned by the registered
        // system closure, and it clears the scope before that invocation
        // returns, so the pointer and length stay valid for this read.
        let accesses = unsafe { std::slice::from_raw_parts(pointer, len) };
        Some(accesses.iter().any(|access| {
            access.kind == access_kind
                && StableComponentId::from_halves(access.component_key, access.component_key_high)
                    == key
                && (requested_mode == 0 || access.mode == requested_mode)
        }))
    })
}
