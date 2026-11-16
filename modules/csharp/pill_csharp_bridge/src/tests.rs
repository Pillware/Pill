//! Integration-style unit tests for the native/C# ECS boundary.
//!
//! # Responsibilities
//!
//! - Verify command-callback ABI behaviour against a live engine.
//! - Verify component-chunk queries and scheduler access derivation.
//!
//! # Design
//!
//! Every test drives the [`Engine`] through the same ABI entry points the
//! managed runtime uses, or through the engine API those entry points reach.
//! A callback test runs inside an [`ActiveSystemGuard`] scope, so the declared
//! scheduler access is enforced exactly as in a live frame. Shared fixtures
//! build a representative world of shared and descriptor components.
//! Scheduler tests derive [`SystemAccess`] metadata from short native access
//! declarations.

// Standard library
use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

// External crates
use pill_core::error::EngineMessage;
use pill_engine::component_registry::FieldDecorator;
use pill_engine::{
    ComponentId, ComponentTicks, Engine, Entity, RunPhase, SystemAccess, SystemError,
    SystemScheduler,
};

// Current crate
use super::abi::{
    begin_managed_zone_for_test, managed_zone_count_for_test, ComponentChunk, NativeComponentBlob,
    NativeSystemAccess,
};
use super::backend::{
    checked_access_count, checked_system_count, derive_system_access, run_phase_from_export,
    MAX_ACCESSES_PER_SYSTEM, MAX_COMPONENT_MANIFEST_BYTES, MAX_SYSTEMS_PER_ASSEMBLY,
};
#[cfg(feature = "hot_reload")]
use super::backend::{
    poll_status_is_known, record_poll_answer, POLL_MANIFEST_PENDING, POLL_NO_CHANGE, POLL_REJECTED,
    POLL_RELOADED,
};
use super::commands::{
    ffi_queue_add_component, ffi_queue_create, ffi_queue_destroy, ffi_queue_remove_component,
    ffi_reserve_entity,
};
use super::components::{
    apply_component_manifest_on_reload, exposed_components_from_names, module_native_bindings,
    register_component_manifest, stable_component_id, BindingStore, ComponentBinding,
    ComponentBindings, ModuleExposedComponent, StableComponentId,
};
use super::context::ActiveScopeGuard;
use super::managed_buffer::{fetch_managed_buffer, ManagedBufferError};
use super::manifest::parse_and_validate_manifest;
use super::parallel::{
    ffi_parallel_for, parallel_for, PARALLEL_NESTED, PARALLEL_NO_SCOPE, PARALLEL_OK,
};
use super::queries::ffi_entity_stable_id;
use super::queries::{
    ffi_entity_count, ffi_get_archetype_chunk, ffi_get_component_chunk, ffi_get_entity_chunk,
};
use super::resources::bound_resource_id_and_width;
// The shared-ABI fixtures: the engine's `TransformComponent` and the renderer
// data's `MeshRendererComponent`, bound the way a managed project gets them.
use pill_engine::common_components::TransformComponent;
// The host depends on the renderer data crate only for its dependency graph
// (see `modules/hosting/pill_development_host/Cargo.toml`). Only this module's
// code names it, to bind the real renderer components and call its asset
// functions.
use renderer_data_dependency_graph as pill_master_renderer_data;
use renderer_data_dependency_graph::MeshRendererComponent;

// =============================================================================
// Constants
// =============================================================================

// ABI status codes the native callbacks under test return. The command and
// chunk callbacks report `1` for success. The entity count and the stable-id
// query report `0` for success instead. The chunk queries report `3` when no
// managed system is active. The command callbacks also report `4` when the
// declared command scope cannot be entered, and `5` when the entity
// generation no longer matches the live handle.
const ABI_SUCCESS: u8 = 1;
const ABI_OUT_OF_SCOPE: u8 = 3;
const ABI_COMMAND_SCOPE_DENIED: u8 = 4;
const ABI_STALE_ENTITY_GENERATION: u8 = 5;

/// Undeclared access on the query path: a component mode the system never
/// declared, or an archetype that no validated term served before the entity
/// path asked for it. Both cases share one code because both mean "not
/// declared".
const ABI_UNDECLARED_ACCESS: u8 = 4;

/// Number of entities populated by [`setup_test_world`]; row-count assertions
/// must agree with this bound.
const TEST_WORLD_ENTITY_COUNT: usize = 100;

// =============================================================================
// Test Helpers
// =============================================================================

/// The managed full name a test component is declared under.
///
/// The two shared-ABI fixtures use the name their generated mirrors carry:
/// the Rust path with every `::` written as `.`. Every other test component
/// lives in the project's `TracyLive` namespace.
fn test_full_name(name: &str) -> String {
    match name {
        "TransformComponent" => "pill_master_renderer.component.TransformComponent".to_owned(),
        "MeshRendererComponent" => {
            "pill_master_renderer.component.MeshRendererComponent".to_owned()
        }
        other => format!("TracyLive.{other}"),
    }
}

/// Return the stable ID of a test component, under [`test_full_name`].
fn test_stable_id(name: &str) -> StableComponentId {
    stable_component_id(&test_full_name(name))
}

/// The bindings a managed project gets for the renderer's data.
///
/// This is the shape of the production path: the renderer data crate registers
/// (it is the first extension a host loads), and the components that
/// registration added are exposed like any extension's, as type-erased native
/// bindings. Nothing here names a component type.
fn shared_component_bindings(engine: &mut Engine) -> ComponentBindings {
    let sequence = engine.world().component_registration_sequence();
    pill_master_renderer_data::register(engine);
    let registered_names = engine.world().registered_component_names_since(sequence);
    let exposed_components = exposed_components_from_names(engine.world(), &registered_names);
    module_native_bindings(engine, &exposed_components)
}

/// The witness these tests hand to `register_component_descriptor`.
///
/// The descriptors registered here hold four-byte values, and the tests name
/// their types literally. The production path gets the same evidence by
/// checking the field types in a manifest.
fn test_blittability_witness() -> pill_engine::archetype::Blittability {
    pill_engine::archetype::Blittability::from_manifest_fields()
}

/// Build one ABI access descriptor for a named test component.
fn native_access(name: &str, mode: u8) -> NativeSystemAccess {
    let id = test_stable_id(name).0;
    NativeSystemAccess {
        component_key: id as u64,
        component_key_high: (id >> 64) as u64,
        mode,
        kind: 0,
    }
}

/// Request one native component chunk using the managed component identity.
fn request_component_chunk(name: &str, mode: u8, index: u32, output: *mut ComponentChunk) -> u8 {
    let id = test_stable_id(name).0;
    ffi_get_component_chunk(id as u64, (id >> 64) as u64, mode, index, output)
}

/// Convert concise test access declarations into scheduler metadata.
fn managed_access(declarations: &[(&str, u8)]) -> SystemAccess {
    // Step 1: Build one native access descriptor per test declaration.
    let native_accesses: Vec<_> = declarations
        .iter()
        .map(|(name, mode)| native_access(name, *mode))
        .collect();
    // Step 2: Register a managed binding for every component not yet shared.
    let mut engine = Engine::new();
    let mut bindings = shared_component_bindings(&mut engine);
    for (name, _) in declarations {
        let stable_id = test_stable_id(name);
        if bindings.contains_key(&stable_id) {
            continue;
        }
        let component_id = engine
            .world_mut()
            .register_component_descriptor(
                stable_id.0,
                format!("TracyLive.{name}"),
                4,
                4,
                1,
                test_blittability_witness(),
            )
            .unwrap();
        bindings.insert(
            stable_id,
            ComponentBinding::Managed {
                component_id,
                size: 4,
                align: 4,
                schema_hash: 1,
            },
        );
    }
    // Step 3: Derive scheduler access from the bound component identities.
    derive_system_access(&native_accesses, &bindings)
        .expect("managed access should map to native components")
}

/// Populate a representative world containing shared and descriptor components.
fn setup_test_world(engine: &mut Engine) -> ComponentBindings {
    // Step 1: Register the `PhysicsState` descriptor component from a manifest.
    let shared = shared_component_bindings(engine);
    let stable_id = stable_component_id("TracyLive.PhysicsState");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.PhysicsState",
        "size": 28,
        "alignment": 4,
        "schema_hash": 1,
        "shared": false,
        "fields": []
    }]);
    let bindings =
        register_component_manifest(engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .unwrap();
    let physics = bindings[&stable_id].component_id();
    // Step 2: Populate entities with both shared and descriptor components.
    for _ in 0..TEST_WORLD_ENTITY_COUNT {
        let entity = engine
            .world_mut()
            .create_entity()
            .with(TransformComponent::default())
            .with(MeshRendererComponent::default())
            .build()
            .unwrap();
        engine
            .world_mut()
            .add_descriptor_component_default(entity, physics)
            .unwrap();
    }
    bindings
}

/// Build an execution graph for the supplied managed system accesses.
fn scheduler_for(accesses: impl IntoIterator<Item = SystemAccess>) -> SystemScheduler {
    let accesses: Vec<SystemAccess> = accesses.into_iter().collect();
    let mut scheduler = SystemScheduler::new();
    scheduler.ensure_execution_graph(accesses.iter());
    scheduler
}

// =============================================================================
// Tests
// =============================================================================

/// Verify managed commands can create, migrate, and destroy a mixed entity.
#[test]
fn managed_command_abi_runs_mixed_lifecycle_through_the_native_queue() {
    let mut engine = Engine::new();
    let mut bindings = shared_component_bindings(&mut engine);
    let descriptor_a_key = stable_component_id("TracyLive.DescriptorA");
    let descriptor_b_key = stable_component_id("TracyLive.DescriptorB");
    let descriptor_a = engine
        .world_mut()
        .register_component_descriptor(
            descriptor_a_key.0,
            "TracyLive.DescriptorA",
            4,
            4,
            1,
            test_blittability_witness(),
        )
        .unwrap();
    let descriptor_b = engine
        .world_mut()
        .register_component_descriptor(
            descriptor_b_key.0,
            "TracyLive.DescriptorB",
            4,
            4,
            2,
            test_blittability_witness(),
        )
        .unwrap();
    bindings.insert(
        descriptor_a_key,
        ComponentBinding::Managed {
            component_id: descriptor_a,
            size: 4,
            align: 4,
            schema_hash: 1,
        },
    );
    bindings.insert(
        descriptor_b_key,
        ComponentBinding::Managed {
            component_id: descriptor_b,
            size: 4,
            align: 4,
            schema_hash: 2,
        },
    );
    let transform_key = test_stable_id("TransformComponent");
    let transform = TransformComponent {
        translation: [9.0, 12.0, 0.0],
        ..TransformComponent::default()
    };
    let descriptor_a_value = 41_u32;
    let mut created = None;

    // Step 1: Create a mixed entity holding the transform and DescriptorA
    // through the native command queue.
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, true);
            let mut entity = std::mem::MaybeUninit::uninit();
            assert_eq!(ffi_reserve_entity(entity.as_mut_ptr()), ABI_SUCCESS);
            // SAFETY: successful reserve initialized the output.
            let entity = unsafe { entity.assume_init() };
            let blobs = [
                NativeComponentBlob {
                    component_key: transform_key.0 as u64,
                    component_key_high: (transform_key.0 >> 64) as u64,
                    data: std::ptr::from_ref(&transform).cast(),
                    size: std::mem::size_of::<TransformComponent>() as u32,
                },
                NativeComponentBlob {
                    component_key: descriptor_a_key.0 as u64,
                    component_key_high: (descriptor_a_key.0 >> 64) as u64,
                    data: std::ptr::from_ref(&descriptor_a_value).cast(),
                    size: 4,
                },
            ];
            assert_eq!(
                ffi_queue_create(&entity, blobs.as_ptr(), blobs.len() as u32),
                ABI_SUCCESS
            );
            created = Some(entity);
        })
        .unwrap();

    let entity = created.unwrap();
    assert_eq!(engine.world().entity_count(), 1);
    assert_eq!(
        engine
            .world()
            .get_component::<TransformComponent>(entity)
            .unwrap()
            .translation[0],
        9.0
    );

    // The managed side reads the stable id of the entity it just created: the
    // two halves re-assemble into the id the world indexes.
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, true);
            let expected = world.entity_id(entity).expect("the entity has an id");
            let mut low = 0u64;
            let mut high = 0u64;
            // The query family reports success as `0`, like `ffi_entity_count`;
            // the command family uses `ABI_SUCCESS`.
            assert_eq!(ffi_entity_stable_id(&entity, &mut low, &mut high), 0);
            assert_eq!(
                u128::from(low) | (u128::from(high) << 64),
                expected.value(),
                "the halves re-assemble into the world's id"
            );
        })
        .unwrap();
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, descriptor_a)
            .unwrap(),
        41_u32.to_ne_bytes()
    );

    let descriptor_b_value = 77_u32;
    // Step 2: Swap DescriptorA for DescriptorB through the native command queue.
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, true);
            assert_eq!(
                ffi_queue_add_component(
                    &entity,
                    descriptor_b_key.0 as u64,
                    (descriptor_b_key.0 >> 64) as u64,
                    std::ptr::from_ref(&descriptor_b_value).cast(),
                    4,
                ),
                ABI_SUCCESS
            );
            assert_eq!(
                ffi_queue_remove_component(
                    &entity,
                    descriptor_a_key.0 as u64,
                    (descriptor_a_key.0 >> 64) as u64,
                ),
                ABI_SUCCESS
            );
        })
        .unwrap();
    assert!(engine
        .world()
        .descriptor_component_bytes(entity, descriptor_a)
        .is_none());
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, descriptor_b)
            .unwrap(),
        77_u32.to_ne_bytes()
    );
    assert!(engine
        .world()
        .get_component::<TransformComponent>(entity)
        .is_some());

    // Step 3: Destroy the entity through the native command queue.
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, true);
            assert_eq!(ffi_queue_destroy(&entity), ABI_SUCCESS);
        })
        .unwrap();
    assert_eq!(engine.world().entity_count(), 0);
}

/// Verify command callbacks reject stale entities and undeclared Commands use.
#[test]
fn managed_command_abi_rejects_stale_generations_and_undeclared_commands() {
    let mut engine = Engine::new();
    let bindings = shared_component_bindings(&mut engine);
    let stale = engine
        .world_mut()
        .create_entity()
        .with(TransformComponent::default())
        .build()
        .unwrap();
    assert!(engine.world_mut().destroy_entity(stale));
    // Reserve the freed slot so it is reissued with a fresh generation; the
    // old handle must then be rejected as stale by every queue callback.
    let _replacement = engine.world_mut().reserve_entity();
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, true);
            assert_eq!(ffi_queue_destroy(&stale), ABI_STALE_ENTITY_GENERATION);
        })
        .unwrap();
    engine
        .run_deferred_commands(|world, queue| {
            let _guard =
                ActiveScopeGuard::install_scope_with_commands(world, queue, &[], &bindings, false);
            assert_eq!(ffi_queue_destroy(&stale), ABI_COMMAND_SCOPE_DENIED);
        })
        .unwrap();
}

/// Verify a managed Commands parameter makes its scheduler access exclusive.
#[test]
fn reflected_managed_commands_access_is_scheduler_exclusive() {
    let mut commands_access = managed_access(&[("TransformComponent", 0)]);
    commands_access.set_uses_commands(true);
    let disjoint_reader = managed_access(&[("MeshRendererComponent", 0)]);
    let scheduler = scheduler_for([commands_access, disjoint_reader]);
    assert_different_batches(&scheduler, 0, 1);
}

/// Assert that all requested system indices occur together in one batch.
fn assert_same_batch(scheduler: &SystemScheduler, systems: &[usize]) {
    assert!(scheduler
        .execution_graph()
        .iter()
        .any(|batch| systems.iter().all(|system| batch.contains(system))));
}

/// Assert that two conflicting systems never occur in the same batch.
fn assert_different_batches(scheduler: &SystemScheduler, first: usize, second: usize) {
    assert!(!scheduler
        .execution_graph()
        .iter()
        .any(|batch| batch.contains(&first) && batch.contains(&second)));
}

/// Construct a zeroed chunk descriptor suitable as an FFI output slot.
fn empty_chunk() -> ComponentChunk {
    ComponentChunk {
        archetype_low: 0,
        archetype_high: 0,
        data: std::ptr::null_mut(),
        entities: std::ptr::null(),
        len: 0,
        element_size: 0,
        ticks: std::ptr::null_mut(),
        change_tick: 0,
        // Zero is the "no scope" token, which is what an unfilled output slot
        // should carry: a chunk the host never wrote is not valid anywhere.
        scope_token: 0,
    }
}

/// Mark one row as changed the same way managed code does, using the chunk's
/// change tick.
///
/// # Safety
///
/// `chunk` must contain the live tick pointer returned by a native query
/// callback, and `row` must address that same chunk invocation.
unsafe fn simulate_managed_write(chunk: &ComponentChunk, row: usize) {
    assert!(row < chunk.len as usize);
    assert!(!chunk.ticks.is_null());
    // SAFETY: the chunk callback returns a tick slice parallel to the
    // component data and `row` was checked against that shared length.
    unsafe {
        (*chunk.ticks.add(row)).set_changed(pill_engine::Tick::new(chunk.change_tick));
    }
}

/// Pin the component-tick fields to the layout consumed by managed code in
/// `EngineApi.cs`, field for field and offset for offset.
#[test]
fn component_chunk_change_tracking_abi_layout_is_stable() {
    assert_eq!(std::mem::size_of::<ComponentTicks>(), 8);
    assert_eq!(std::mem::offset_of!(ComponentTicks, changed), 4);
    assert_eq!(std::mem::size_of::<ComponentChunk>(), 56);
    assert_eq!(std::mem::offset_of!(ComponentChunk, data), 16);
    assert_eq!(std::mem::offset_of!(ComponentChunk, entities), 24);
    assert_eq!(std::mem::offset_of!(ComponentChunk, ticks), 40);
    assert_eq!(std::mem::offset_of!(ComponentChunk, change_tick), 48);
}

/// Verify the archetype-scoped chunk lookup resolves the remaining terms of
/// an archetype without scanning chunk indices.
#[test]
fn archetype_chunk_lookup_resolves_components_and_entities() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    engine
        .world_mut()
        .create_entity()
        .with(TransformComponent {
            translation: [1.0, 2.0, 0.0],
            ..TransformComponent::default()
        })
        .build()
        .unwrap();

    let transform_id = test_stable_id("TransformComponent");
    let renderable_id = test_stable_id("MeshRendererComponent");
    let accesses = [
        native_access("TransformComponent", 1),
        native_access("MeshRendererComponent", 0),
    ];
    let mut chunk = empty_chunk();
    let mut renderable_chunk = empty_chunk();
    let mut entity_chunk = empty_chunk();
    {
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &shared);

        // Step 1: one index-based lookup gives the archetype identity that the
        // managed enumerator keeps in its driver chunk (the chunk it holds
        // while it iterates over the archetype).
        assert_eq!(
            request_component_chunk("TransformComponent", 1, 0, &mut chunk),
            ABI_SUCCESS
        );

        // Step 2: the archetype-scoped variant resolves the same column
        // directly.
        assert_eq!(
            ffi_get_archetype_chunk(
                chunk.archetype_low,
                chunk.archetype_high,
                transform_id.0 as u64,
                (transform_id.0 >> 64) as u64,
                1,
                &mut chunk,
            ),
            ABI_SUCCESS
        );
        assert_eq!(chunk.len, 1);
        assert_eq!(
            chunk.element_size,
            std::mem::size_of::<TransformComponent>() as u32
        );
        // SAFETY: the lookup succeeded and the asserted geometry guarantees
        // the data pointer addresses one valid `TransformComponent` row.
        let first_translation =
            unsafe { *(chunk.data as *const TransformComponent) }.translation[0];
        assert_eq!(first_translation, 1.0);

        // Step 3: an authorized component the archetype does not carry
        // reports absence (`0`), which the enumerator turns into
        // "optional term not present".
        assert_eq!(
            ffi_get_archetype_chunk(
                chunk.archetype_low,
                chunk.archetype_high,
                renderable_id.0 as u64,
                (renderable_id.0 >> 64) as u64,
                0,
                &mut renderable_chunk,
            ),
            0
        );

        // Step 4: mode `2` returns the archetype's entity column without
        // consulting any component binding or access declaration.
        assert_eq!(
            ffi_get_archetype_chunk(
                chunk.archetype_low,
                chunk.archetype_high,
                0,
                0,
                2,
                &mut entity_chunk,
            ),
            ABI_SUCCESS
        );
        assert_eq!(entity_chunk.len, 1);
        assert_eq!(
            entity_chunk.element_size,
            std::mem::size_of::<Entity>() as u32
        );
    }
}

/// A C#-only component's `Handle<T>` field is a typed asset reference. The
/// manifest names `T` by its Rust type path, and the world records the type's
/// registered name, the same one a Rust component's field gets.
#[test]
fn a_managed_handle_field_is_recorded_with_its_registered_asset_type() {
    let mut engine = Engine::new();
    engine
        .world_mut()
        .register_imported_asset::<pill_master_renderer_data::Mesh>();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("Project.MeshReferenceInCSharp");
    let mesh_path = std::any::type_name::<pill_master_renderer_data::Mesh>();
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "Project.MeshReferenceInCSharp",
        "size": 8,
        "alignment": 4,
        "schema_hash": 4242,
        "shared": false,
        "fields": [{
            "name": "Mesh",
            "offset": 0,
            "size": 8,
            "primitive_type": format!("handle:{mesh_path}"),
            "fields": []
        }]
    }]);
    let bindings =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .unwrap();
    let component_id = bindings[&stable_id].component_id();
    assert_eq!(
        engine.world().asset_field_type(component_id, "Mesh"),
        Some("pill_master_renderer::assets::Mesh"),
        "the Rust type path is recorded as the registered name"
    );

    let mut bytes = 3_u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, bytes)])
        .unwrap();
    let value = engine
        .world()
        .read_component_field(entity, "Project.MeshReferenceInCSharp", "Mesh")
        .unwrap();
    assert_eq!(
        value,
        pill_engine::component_field::FieldValue::Asset {
            asset_type: Some("pill_master_renderer::assets::Mesh".to_owned()),
            index: 3,
            generation: 1,
        }
    );
}

/// A handle field that is not the engine's handle shape is refused.
#[test]
fn a_managed_handle_field_of_the_wrong_size_is_refused() {
    let stable_id = stable_component_id("Project.BrokenHandle");
    let manifest = serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "Project.BrokenHandle",
        "size": 4,
        "alignment": 4,
        "schema_hash": 1,
        "shared": false,
        "fields": [{
            "name": "Mesh",
            "offset": 0,
            "size": 4,
            "primitive_type": "handle:some::Mesh",
            "fields": []
        }]
    }]))
    .unwrap();
    let error = match parse_and_validate_manifest(&manifest) {
        Ok(_) => panic!("a four-byte handle is refused"),
        Err(error) => error,
    };
    assert!(error.to_plain_message().contains("handle field"), "{error}");
}

/// Verify a C#-only manifest component can be registered and queried natively.
#[test]
fn managed_manifest_registers_and_queries_a_new_descriptor_component() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("Project.CustomOnlyInCSharp");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "Project.CustomOnlyInCSharp",
        "size": 4,
        "alignment": 4,
        "schema_hash": 12345,
        "shared": false,
        "fields": [{
            "name": "Value",
            "offset": 0,
            "size": 4,
            "primitive_type": "System.UInt32",
            "fields": []
        }]
    }]);
    let bindings =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .unwrap();
    let component_id = bindings[&stable_id].component_id();
    engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, 77_u32.to_ne_bytes().to_vec())])
        .unwrap();

    let accesses = [NativeSystemAccess {
        component_key: stable_id.0 as u64,
        component_key_high: (stable_id.0 >> 64) as u64,
        mode: 1,
        kind: 0,
    }];
    let mut chunk = empty_chunk();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
        assert_eq!(
            ffi_get_component_chunk(
                stable_id.0 as u64,
                (stable_id.0 >> 64) as u64,
                1,
                0,
                &mut chunk,
            ),
            ABI_SUCCESS
        );
        assert_eq!(chunk.len, 1);
        assert_eq!(chunk.element_size, 4);
        // SAFETY: the query returned success and the asserted length and
        // element size guarantee the chunk data pointer addresses one valid
        // `u32` inside the component column.
        assert_eq!(unsafe { *(chunk.data as *const u32) }, 77);
    }
}

/// A newly registered descriptor component exposes its field layout.
///
/// A descriptor component's field layout is what the editor's Inspector reads
/// for it, so registering one through the manifest has to install it.
#[test]
fn descriptor_registration_still_installs_field_layout() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.LayoutProbe");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.LayoutProbe",
        "size": 4,
        "alignment": 4,
        "schema_hash": 9,
        "shared": false,
        "fields": [{
            "name": "Value",
            "offset": 0,
            "size": 4,
            "primitive_type": "System.UInt32",
            "fields": []
        }]
    }]);
    let bindings =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .unwrap();
    let component_id = bindings[&stable_id].component_id();

    let layout = engine
        .world()
        .component_field_layout(component_id)
        .expect("a registered descriptor component keeps its field layout");
    assert_eq!(layout.len(), 1, "one manifest field means one descriptor");
    assert_eq!(layout[0].name, "Value");
    assert_eq!(layout[0].type_tag, "u32");
}

/// The presentation decorators a managed field declares reach the editor's
/// layout unchanged and in declaration order, and an undecorated field carries
/// none.
#[test]
fn managed_field_decorators_reach_the_editor_layout() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.DecoratedProbe");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.DecoratedProbe",
        "size": 12,
        "alignment": 4,
        "schema_hash": 4,
        "shared": false,
        "fields": [
            {
                "name": "Health",
                "offset": 0,
                "size": 4,
                "primitive_type": "System.Single",
                "decorations": [
                    { "kind": "header", "text": "Combat" },
                    { "kind": "tooltip", "text": "Current hit points" },
                    { "kind": "range", "min": 0.0, "max": 100.0 }
                ],
                "fields": []
            },
            {
                "name": "Ammo",
                "offset": 4,
                "size": 4,
                "primitive_type": "System.Int32",
                "decorations": [{ "kind": "min", "number": 0.0 }],
                "fields": []
            },
            {
                "name": "Plain",
                "offset": 8,
                "size": 4,
                "primitive_type": "System.Single",
                "fields": []
            }
        ]
    }]);
    let bindings =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .expect("a decorated manifest registers");
    let layout = engine
        .world()
        .component_field_layout(bindings[&stable_id].component_id())
        .expect("the descriptor component keeps its field layout");

    assert_eq!(
        layout[0].decorators.as_ref(),
        &[
            FieldDecorator::Header("Combat".into()),
            FieldDecorator::Tooltip("Current hit points".into()),
            FieldDecorator::Range {
                min: 0.0,
                max: 100.0,
            },
        ],
        "decorators keep their declaration order"
    );
    assert_eq!(layout[1].decorators.as_ref(), &[FieldDecorator::Min(0.0)]);
    assert!(
        layout[2].decorators.is_empty(),
        "an undecorated field carries no decorators"
    );
}

/// A decorator that cannot mean what it says is refused with the field named,
/// instead of being silently ignored the way Unity ignores a bad range.
#[test]
fn an_impossible_decorator_is_refused() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.BadDecorators");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.BadDecorators",
        "size": 4,
        "alignment": 4,
        "schema_hash": 5,
        "shared": false,
        "fields": [{
            "name": "Health",
            "offset": 0,
            "size": 4,
            "primitive_type": "System.Single",
            "decorations": [{ "kind": "range", "min": 100.0, "max": 0.0 }],
            "fields": []
        }]
    }]);

    // `expect_err` needs `Debug` on the bindings map, which the binding type
    // does not implement, so the refusal is matched out directly.
    let error = match register_component_manifest(
        &mut engine,
        &serde_json::to_vec(&manifest).unwrap(),
        shared,
    ) {
        Ok(_) => panic!("a range with the maximum first is refused"),
        Err(error) => error,
    };
    let message = error.to_plain_message();
    assert!(
        message.contains("Health"),
        "the refusal names the field: {message}"
    );
    assert!(
        message.contains("range"),
        "the refusal names the decorator: {message}"
    );
}

/// A caller that passes a null output pointer gets a status, not a silent
/// truncation.
///
/// `0` means "end of iteration" to every managed caller, so answering a null
/// output buffer with it would turn a binding bug into a query that found no
/// rows. Status `5` names the mistake, and the managed `ValidateStatus` maps it
/// to an `ArgumentException`.
#[test]
fn null_output_pointer_reports_invalid_argument() {
    assert_eq!(
        ffi_get_component_chunk(0, 0, 0, 0, std::ptr::null_mut()),
        5,
        "a component chunk with no output buffer is a caller bug"
    );
    assert_eq!(
        ffi_get_archetype_chunk(0, 0, 0, 0, 0, std::ptr::null_mut()),
        5,
        "an archetype chunk with no output buffer is a caller bug"
    );
    assert_eq!(
        ffi_get_entity_chunk(0, std::ptr::null_mut()),
        5,
        "an entity chunk with no output buffer is a caller bug"
    );
    assert_eq!(
        ffi_entity_count(std::ptr::null_mut()),
        5,
        "an entity count with no output buffer is a caller bug"
    );
}

/// The served stride is the live column's, not the binding's copy.
///
/// Managed row arithmetic trusts `element_size`. If a binding ever described a
/// different layout than the column its pointer addresses, every row after the
/// first would be read at the wrong offset. The chunk callbacks therefore
/// serve the registered layout, so a binding that disagrees cannot change what
/// managed code multiplies by.
#[test]
fn descriptor_chunk_stride_comes_from_the_live_column() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.DriftProbe");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.DriftProbe",
        "size": 4,
        "alignment": 4,
        "schema_hash": 7,
        "shared": false,
        "fields": [{
            "name": "Value",
            "offset": 0,
            "size": 4,
            "primitive_type": "System.UInt32",
            "fields": []
        }]
    }]);
    let mut bindings =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .unwrap();
    let component_id = bindings[&stable_id].component_id();
    engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, 77_u32.to_ne_bytes().to_vec())])
        .unwrap();

    // A store that disagrees with the registered column: the registration
    // path checks that the two agree, and the serving path must keep working
    // either way.
    bindings.insert(
        stable_id,
        ComponentBinding::Managed {
            component_id,
            size: 8,
            align: 8,
            schema_hash: 7,
        },
    );

    let accesses = [NativeSystemAccess {
        component_key: stable_id.0 as u64,
        component_key_high: (stable_id.0 >> 64) as u64,
        mode: 1,
        kind: 0,
    }];
    let mut chunk = empty_chunk();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
        assert_eq!(
            ffi_get_component_chunk(
                stable_id.0 as u64,
                (stable_id.0 >> 64) as u64,
                1,
                0,
                &mut chunk,
            ),
            ABI_SUCCESS
        );
        assert_eq!(chunk.len, 1);
        assert_eq!(
            chunk.element_size, 4,
            "the stride must be the live column's, not the binding's copy"
        );
        // SAFETY: the query returned success and the asserted length and
        // element size guarantee the chunk data pointer addresses one valid
        // `u32` inside the component column.
        assert_eq!(unsafe { *(chunk.data as *const u32) }, 77);
    }
}

/// A module binding whose forwarded layout disagrees with the live column is
/// dropped when the bindings are built, and a stale binding that still reaches
/// a query is refused instead of panicking.
#[test]
fn module_native_binding_rejects_live_layout_mismatch() {
    let mut engine = Engine::new();
    let stable_id = test_stable_id("ModuleThing");
    let module_id = engine
        .world_mut()
        .register_component_descriptor(
            stable_id.0,
            "TracyLive.ModuleThing",
            8,
            4,
            1,
            test_blittability_witness(),
        )
        .expect("the module component registers");
    engine
        .world_mut()
        .create_descriptor_entity(&[(module_id, vec![0_u8; 8])])
        .expect("the entity carries the registered layout");

    // The module forwards a size the live column does not have - a mirror
    // built against a stale generation - and its binding is dropped rather
    // than handed to managed code.
    let exposed_components = [ModuleExposedComponent {
        csharp_name: "TracyLive.ModuleThing".to_string(),
        component_id: module_id,
        size: 16,
        align: 4,
        fields: Vec::new(),
    }];
    let agreed_bindings = module_native_bindings(&mut engine, &exposed_components);
    assert!(
        agreed_bindings.is_empty(),
        "a binding that disagrees with the live column is not handed out"
    );

    // A stale binding already in a store fails the query arm with the
    // `NotRegistered` status instead of being served.
    //
    // `MeshRendererComponent` stands in for the module's component here: it is
    // real native storage, which is exactly what the `ModuleNative` arm serves.
    // The stale binding is compared against its live layout.
    let mut bindings = shared_component_bindings(&mut engine);
    let renderable_stable_id = test_stable_id("MeshRendererComponent");
    bindings.insert(
        renderable_stable_id,
        ComponentBinding::ModuleNative {
            component_id: ComponentId::of::<MeshRendererComponent>(),
            // Deliberately not the live layout, so the arm has to refuse.
            size: 64,
            align: 4,
            field_signature: None,
        },
    );
    engine
        .world_mut()
        .create_entity()
        .with(MeshRendererComponent::default())
        .build()
        .unwrap();
    let accesses = [native_access("MeshRendererComponent", 1)];
    let mut chunk = empty_chunk();
    let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
    assert_eq!(
        ffi_get_component_chunk(
            renderable_stable_id.0 as u64,
            (renderable_stable_id.0 >> 64) as u64,
            1,
            0,
            &mut chunk,
        ),
        2,
        "a wrong-size binding reports failure instead of aborting"
    );
}

/// The archetype-scoped native arm refuses a stale binding the same way the
/// index-based one does.
#[test]
fn archetype_scoped_module_native_binding_rejects_live_layout_mismatch() {
    let mut engine = Engine::new();
    let mut bindings = shared_component_bindings(&mut engine);
    let renderable_stable_id = test_stable_id("MeshRendererComponent");
    bindings.insert(
        renderable_stable_id,
        ComponentBinding::ModuleNative {
            component_id: ComponentId::of::<MeshRendererComponent>(),
            // Deliberately not the live layout, so the arm has to refuse.
            size: 64,
            align: 4,
            field_signature: None,
        },
    );
    engine
        .world_mut()
        .create_entity()
        .with(MeshRendererComponent::default())
        .build()
        .unwrap();
    let accesses = [native_access("MeshRendererComponent", 1)];
    let mut entity_chunk = empty_chunk();
    let mut chunk = empty_chunk();
    let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
    // The archetype is learned from the entity column, exactly as the managed
    // enumerator learns it from the chunk it keeps while iterating.
    assert_eq!(ffi_get_entity_chunk(0, &mut entity_chunk), ABI_SUCCESS);
    assert_eq!(
        ffi_get_archetype_chunk(
            entity_chunk.archetype_low,
            entity_chunk.archetype_high,
            renderable_stable_id.0 as u64,
            (renderable_stable_id.0 >> 64) as u64,
            1,
            &mut chunk,
        ),
        2,
        "the archetype-scoped native arm fails closed on a wrong-size binding"
    );
}

/// Verify a shared managed mirror with a different field schema is rejected.
#[test]
fn managed_shared_component_schema_mismatch_is_rejected() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = test_stable_id("TransformComponent");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": test_full_name("TransformComponent"),
        "size": 40,
        "alignment": 4,
        "schema_hash": 0,
        "shared": true,
        "fields": [
            { "name": "Y", "offset": 0, "size": 4, "primitive_type": "System.Single", "fields": [] },
            { "name": "X", "offset": 4, "size": 4, "primitive_type": "System.Single", "fields": [] }
        ]
    }]);

    let error =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .err()
            .expect("an equal-sized but incompatible shared schema must fail");
    assert!(error.to_string().contains("field schema"));
}

/// Verify managed shared bindings use the renderer's concrete component types.
///
/// The engine's `TransformComponent` and the renderer data's
/// `MeshRendererComponent` are always available, so the managed bridge has one
/// binding path in every build.
#[test]
fn csharp_world_supports_the_renderable_renderer_query() {
    let mut engine = Engine::new();
    setup_test_world(&mut engine);

    let mut query = pill_engine::Query::<(&TransformComponent, &MeshRendererComponent)>::new(
        engine.world_mut(),
    );
    assert_eq!(query.iter_mut().count(), TEST_WORLD_ENTITY_COUNT);
}

/// Verify writers of different components can share one parallel batch.
#[test]
fn disjoint_managed_writers_share_a_parallel_batch() {
    let scheduler = scheduler_for([
        managed_access(&[("PhysicsState", 1)]),
        managed_access(&[("TransformComponent", 1)]),
        managed_access(&[("MeshRendererComponent", 1)]),
    ]);

    assert_eq!(scheduler.execution_graph().len(), 1);
    assert_same_batch(&scheduler, &[0, 1, 2]);
}

/// Verify multiple readers of one component can execute concurrently.
#[test]
fn managed_readers_of_the_same_component_share_a_parallel_batch() {
    let scheduler = scheduler_for([
        managed_access(&[("TransformComponent", 0)]),
        managed_access(&[("TransformComponent", 0)]),
    ]);

    assert_eq!(scheduler.execution_graph().len(), 1);
    assert_same_batch(&scheduler, &[0, 1]);
}

/// Verify a reader and writer of one component are placed in separate batches.
#[test]
fn managed_reader_and_writer_are_scheduled_in_different_batches() {
    let scheduler = scheduler_for([
        managed_access(&[("TransformComponent", 0)]),
        managed_access(&[("TransformComponent", 1)]),
    ]);

    assert_eq!(scheduler.execution_graph().len(), 2);
    assert_different_batches(&scheduler, 0, 1);
}

/// Verify multiple writers of one component are placed in separate batches.
#[test]
fn managed_writers_of_the_same_component_are_scheduled_in_different_batches() {
    let scheduler = scheduler_for([
        managed_access(&[("MeshRendererComponent", 1)]),
        managed_access(&[("MeshRendererComponent", 1)]),
    ]);

    assert_eq!(scheduler.execution_graph().len(), 2);
    assert_different_batches(&scheduler, 0, 1);
}

/// Verify EntityTerm contributes no component conflict to scheduler access.
#[test]
fn entity_only_managed_system_does_not_create_a_scheduler_conflict() {
    // EntityTerm is intentionally omitted from the native component
    // access list exported by IQueryDescriptor.
    let scheduler = scheduler_for([
        managed_access(&[]),
        managed_access(&[
            ("PhysicsState", 1),
            ("TransformComponent", 1),
            ("MeshRendererComponent", 1),
        ]),
    ]);

    assert_eq!(scheduler.execution_graph().len(), 1);
    assert_same_batch(&scheduler, &[0, 1]);
}

/// Verify optional query terms retain their underlying scheduler conflicts.
#[test]
fn optional_managed_access_conflicts_when_the_component_may_be_present() {
    // OptionalWrite<MeshRendererComponent> exports the same scheduler write as Write<MeshRendererComponent>;
    // optionality affects matching, never parallel safety.
    let first = managed_access(&[("PhysicsState", 1), ("MeshRendererComponent", 0)]);
    let second = managed_access(&[("TransformComponent", 1)]);
    let third = managed_access(&[("MeshRendererComponent", 1)]);
    let scheduler = scheduler_for([first.clone(), second.clone(), third.clone()]);

    assert_eq!(scheduler.execution_graph().len(), 2);
    assert_same_batch(&scheduler, &[0, 1]);
    assert_different_batches(&scheduler, 0, 2);
    assert!(!second.conflicts_with(&third));
}

/// Verify marking one managed write is observed by Rust's Changed filter.
#[test]
fn one_managed_row_write_is_visible_to_rust_changed_filter() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let baseline = engine.world().change_tick();
    engine.world_mut().set_system_last_run(baseline);
    engine.world_mut().increment_change_tick();

    let accesses = [native_access("TransformComponent", 1)];
    let mut component_chunk = empty_chunk();
    let mut entity_chunk = empty_chunk();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
        assert_eq!(
            request_component_chunk("TransformComponent", 1, 0, &mut component_chunk),
            ABI_SUCCESS
        );
        assert_eq!(ffi_get_entity_chunk(0, &mut entity_chunk), ABI_SUCCESS);
        // SAFETY: the transform chunk was fetched successfully and row `37`
        // lies within its live length, so the chunk tick pointer is valid to
        // advance by that many rows.
        unsafe { simulate_managed_write(&component_chunk, 37) };
    }

    // SAFETY: the entity chunk was fetched successfully and row `37` indexes
    // a live entity within its declared length.
    let expected = unsafe { *((entity_chunk.entities as *const pill_engine::Entity).add(37)) };
    let mut changed = pill_engine::Query::<
        (pill_engine::Entity,),
        pill_engine::Changed<TransformComponent>,
    >::new(engine.world_mut());
    let hits: Vec<_> = changed.iter_mut().map(|(entity,)| entity).collect();
    assert_eq!(hits, vec![expected]);
}

/// Verify read-only managed iteration leaves component change ticks untouched.
#[test]
fn managed_read_only_chunk_does_not_trigger_changed_filter() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let baseline = engine.world().change_tick();
    engine.world_mut().set_system_last_run(baseline);
    engine.world_mut().increment_change_tick();

    let accesses = [native_access("TransformComponent", 0)];
    let mut chunk = empty_chunk();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
        assert_eq!(
            request_component_chunk("TransformComponent", 0, 0, &mut chunk),
            ABI_SUCCESS
        );
        assert!(!chunk.ticks.is_null());
    }

    let mut changed = pill_engine::Query::<
        (pill_engine::Entity,),
        pill_engine::Changed<TransformComponent>,
    >::new(engine.world_mut());
    assert_eq!(changed.iter_mut().count(), 0);
}

/// Verify disjoint writable columns update only their corresponding row ticks.
#[test]
fn disjoint_managed_writes_mark_the_correct_tick_columns() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let baseline = engine.world().change_tick();
    engine.world_mut().set_system_last_run(baseline);
    engine.world_mut().increment_change_tick();

    let accesses = [
        native_access("TransformComponent", 1),
        native_access("MeshRendererComponent", 1),
    ];
    let mut transforms = empty_chunk();
    let mut renderables = empty_chunk();
    let mut entities = empty_chunk();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
        assert_eq!(
            request_component_chunk("TransformComponent", 1, 0, &mut transforms),
            ABI_SUCCESS
        );
        assert_eq!(
            request_component_chunk("MeshRendererComponent", 1, 0, &mut renderables),
            ABI_SUCCESS
        );
        assert_eq!(ffi_get_entity_chunk(0, &mut entities), ABI_SUCCESS);
        assert_ne!(transforms.ticks, renderables.ticks);
        // SAFETY: both chunks were fetched successfully and rows `3` and `7`
        // fall within their live lengths, so each tick pointer is valid to
        // advance by the requested row.
        unsafe {
            simulate_managed_write(&transforms, 3);
            simulate_managed_write(&renderables, 7);
        }
    }

    // SAFETY: the entity chunk was fetched successfully and each queried row
    // indexes a live entity within the chunk's declared length.
    let entity_at = |row| unsafe { *((entities.entities as *const pill_engine::Entity).add(row)) };
    let mut changed_transforms = pill_engine::Query::<
        (pill_engine::Entity,),
        pill_engine::Changed<TransformComponent>,
    >::new(engine.world_mut());
    let transform_hits: Vec<_> = changed_transforms
        .iter_mut()
        .map(|(entity,)| entity)
        .collect();
    assert_eq!(transform_hits, vec![entity_at(3)]);

    let mut changed_renderables = pill_engine::Query::<
        (pill_engine::Entity,),
        pill_engine::Changed<MeshRendererComponent>,
    >::new(engine.world_mut());
    let renderable_hits: Vec<_> = changed_renderables
        .iter_mut()
        .map(|(entity,)| entity)
        .collect();
    assert_eq!(renderable_hits, vec![entity_at(7)]);
}

/// Verify entity columns are exposed only inside a scheduled managed scope.
#[test]
fn entity_chunks_are_available_only_during_a_managed_system() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let mut chunk = empty_chunk();

    assert_eq!(ffi_get_entity_chunk(0, &mut chunk), ABI_OUT_OF_SCOPE);
    {
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &bindings);
        assert_eq!(ffi_get_entity_chunk(0, &mut chunk), ABI_SUCCESS);
        assert_eq!(chunk.len as usize, TEST_WORLD_ENTITY_COUNT);
        assert_eq!(
            chunk.element_size as usize,
            std::mem::size_of::<pill_engine::Entity>()
        );
        assert!(
            chunk.data.is_null(),
            "entity rows must not be reachable through the writable `data` slot"
        );
        assert!(
            !chunk.entities.is_null(),
            "entity rows arrive in `entities`"
        );
    }
    assert_eq!(ffi_get_entity_chunk(0, &mut chunk), ABI_OUT_OF_SCOPE);
}

/// Entity columns are handed out through the const `entities` slot only.
///
/// The writable `data` slot stays null for them, so an aliasing write into
/// engine-owned entity rows cannot be expressed by accident.
#[test]
fn entity_chunks_expose_a_const_pointer() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let mut chunk = empty_chunk();
    {
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &bindings);
        assert_eq!(ffi_get_entity_chunk(0, &mut chunk), ABI_SUCCESS);
        assert!(
            chunk.data.is_null(),
            "the writable slot must stay null for entity rows"
        );
        assert!(
            !chunk.entities.is_null(),
            "entity rows are served through the const slot"
        );
    }
}

/// The entity path serves only archetypes a validated term already reached.
///
/// Without that rule, `mode == 2` would answer for any archetype id a managed
/// caller guessed, which would tell it about component sets nothing declared.
/// The rule is kept per invocation, so it is gone again for the next system.
/// Outside a managed invocation the out-of-scope answer takes precedence.
#[test]
fn mode_two_serves_only_observed_archetypes() {
    let mut engine = Engine::new();
    let bindings = setup_test_world(&mut engine);
    let accesses = [native_access("TransformComponent", 0)];
    // The id is looked up the way the world itself knows it. The managed path
    // never holds one before a validated term has served it.
    let archetype = engine
        .world()
        .entity_chunk(0)
        .expect("the world has an archetype")
        .0;
    let low = archetype.0 as u64;
    let high = (archetype.0 >> 64) as u64;
    let mut chunk = empty_chunk();

    assert_eq!(
        ffi_get_archetype_chunk(low, high, 0, 0, 2, &mut chunk),
        ABI_OUT_OF_SCOPE,
        "outside a managed invocation the scope answer comes first"
    );
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);

        // Step 1: a fresh id is refused, because nothing declared it.
        assert_eq!(
            ffi_get_archetype_chunk(low, high, 0, 0, 2, &mut chunk),
            ABI_UNDECLARED_ACCESS,
            "an unobserved archetype must not be servable"
        );

        // Step 2: once a validated component term served the archetype, its
        // entity column resolves.
        assert_eq!(
            request_component_chunk("TransformComponent", 0, 0, &mut chunk),
            ABI_SUCCESS
        );
        assert_eq!(
            ffi_get_archetype_chunk(low, high, 0, 0, 2, &mut chunk),
            ABI_SUCCESS
        );
        assert_eq!(chunk.len as usize, TEST_WORLD_ENTITY_COUNT);
        assert!(!chunk.entities.is_null());
    }

    // Step 3: the observation does not survive the invocation.
    let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &accesses, &bindings);
    assert_eq!(
        ffi_get_archetype_chunk(low, high, 0, 0, 2, &mut chunk),
        ABI_UNDECLARED_ACCESS,
        "each invocation starts with no archetype observed"
    );
}

/// Verify a managed system that reports failure is recorded as a drained
/// system failure carrying the managed message.
#[test]
fn failing_managed_system_is_recorded_as_a_system_failure() {
    let mut engine = Engine::new();
    // SAFETY: the closure never touches the world, so the empty access
    // declaration is exact.
    unsafe {
        engine.register_system_with_access(
            "managed_failure",
            SystemAccess::new(),
            move |_world, _queue| -> Result<(), SystemError> {
                Err(SystemError::Managed {
                    message: String::from("index out of range"),
                })
            },
        );
    }
    engine.process_frame().unwrap();
    let failures = engine.drain_system_failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].system, "managed_failure");
    assert!(failures[0]
        .to_plain_message()
        .contains("index out of range"));
}

/// Verify a healthy managed system leaves the frame failure record empty.
#[test]
fn succeeding_managed_system_leaves_no_system_failure() {
    let mut engine = Engine::new();
    // SAFETY: the closure never touches the world, so the empty access
    // declaration is exact.
    unsafe {
        engine.register_system_with_access(
            "managed_success",
            SystemAccess::new(),
            move |_world, _queue| Ok(()),
        );
    }
    engine.process_frame().unwrap();
    assert!(engine.drain_system_failures().is_empty());
}

/// Verifies that an excessively nested field manifest fails validation with a
/// regular error instead of exhausting the validation stack.
#[test]
fn deeply_nested_manifest_is_rejected_without_stack_overflow() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.DeeplyNested");

    // Build a field tree deeper than the validation depth budget without
    // nesting this test's own call stack. It must also stay below
    // serde_json's own recursion limit so the validator, not the parser,
    // produces the rejection.
    let mut field = serde_json::json!({
        "name": "leaf",
        "offset": 0,
        "size": 1,
        "primitive_type": "System.Byte",
        "fields": []
    });
    for _ in 0..40 {
        field = serde_json::json!({
            "name": "wrapper",
            "offset": 0,
            "size": 1,
            "primitive_type": "struct",
            "fields": [field]
        });
    }
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.DeeplyNested",
        "size": 1,
        "alignment": 1,
        "schema_hash": 1,
        "shared": false,
        "fields": [field]
    }]);

    let error =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .err()
            .expect("an excessively nested manifest must be rejected");
    assert!(
        error.to_string().contains("nesting depth"),
        "unexpected error: {error}"
    );
}

/// A descriptor component may only contain blittable value types (types that
/// can be copied byte for byte).
///
/// `ComponentColumn` moves rows with `ptr::copy` and frees its buffer without
/// running drop glue, and the engine shares those columns across threads on an
/// `unsafe impl Send`/`Sync`. A field owning a managed resource would be
/// duplicated on move, leaked on free, and raced on.
#[test]
fn a_non_blittable_field_type_is_rejected() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.HasManagedField");

    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.HasManagedField",
        "size": 8,
        "alignment": 8,
        "schema_hash": 1,
        "shared": false,
        "fields": [{
            "name": "Label",
            "offset": 0,
            "size": 8,
            "primitive_type": "System.String",
            "fields": []
        }]
    }]);

    let error =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .err()
            .expect("a managed reference field must be rejected");
    let message = error.to_string();
    assert!(
        message.contains("non-blittable") && message.contains("Label"),
        "the error should name the field and why it was refused: {message}"
    );
}

/// A non-blittable type nested inside a value type is rejected too, so a
/// nested type cannot hide one from the check.
#[test]
fn a_nested_non_blittable_field_type_is_rejected() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.NestedManaged");

    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.NestedManaged",
        "size": 8,
        "alignment": 8,
        "schema_hash": 1,
        "shared": false,
        "fields": [{
            "name": "Inner",
            "offset": 0,
            "size": 8,
            "primitive_type": "struct",
            "fields": [{
                "name": "Handle",
                "offset": 0,
                "size": 8,
                "primitive_type": "System.Object",
                "fields": []
            }]
        }]
    }]);

    let error =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .err()
            .expect("a nested managed reference must be rejected");
    assert!(
        error.to_string().contains("Handle"),
        "the error should name the nested field: {error}"
    );
}

/// Verifies that overlapping component fields are rejected so conflicting
/// byte interpretations cannot corrupt data silently.
#[test]
fn overlapping_component_fields_are_rejected() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let stable_id = stable_component_id("TracyLive.Overlapping");
    let manifest = serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.Overlapping",
        "size": 8,
        "alignment": 4,
        "schema_hash": 1,
        "shared": false,
        "fields": [
            { "name": "First", "offset": 0, "size": 6, "primitive_type": "struct", "fields": [] },
            { "name": "Second", "offset": 4, "size": 4, "primitive_type": "struct", "fields": [] }
        ]
    }]);
    let error =
        register_component_manifest(&mut engine, &serde_json::to_vec(&manifest).unwrap(), shared)
            .err()
            .expect("overlapping component fields must be rejected");
    assert!(
        error.to_string().contains("overlap"),
        "unexpected error: {error}"
    );
}

/// The four poll codes are the loader's whole vocabulary, and anything else
/// is a typed error naming the code instead of a silent "nothing happened".
#[cfg(feature = "hot_reload")]
#[test]
fn unknown_poll_status_is_a_typed_error() {
    assert!(poll_status_is_known(POLL_NO_CHANGE));
    assert!(poll_status_is_known(POLL_RELOADED));
    assert!(poll_status_is_known(POLL_REJECTED));
    // A held-back version is a status the host answers, not one it rejects.
    // If it were left unknown, the reload loop would stall: the loader would
    // keep holding the version back, and every later poll would report the
    // same unknown status.
    assert!(poll_status_is_known(POLL_MANIFEST_PENDING));
    assert!(!poll_status_is_known(4));
    assert!(!poll_status_is_known(u8::MAX));

    let error = pill_core::error::CSharpError::UnknownPollStatus { status: 9 };
    assert!(
        error.to_string().contains('9'),
        "the message names the code: {error}"
    );
}

/// A loader refusal is named once per attempt, and the recorded answer never
/// goes stale: a parked manifest stays this poll's answer even when the host
/// then aborts the swap, and a refusal after any other answer is a new
/// attempt that is named again.
#[cfg(feature = "hot_reload")]
#[test]
fn a_rejection_is_recorded_once_until_the_next_answer() {
    let mut last_poll_status = POLL_NO_CHANGE;

    // The first refusal of an attempt is named.
    assert!(record_poll_answer(&mut last_poll_status, POLL_REJECTED));
    assert_eq!(last_poll_status, POLL_REJECTED);

    // The same refusal on the next frame is not repeated.
    assert!(!record_poll_answer(&mut last_poll_status, POLL_REJECTED));

    // A parked manifest is what the loader answered, whatever the host
    // decides about it afterwards, so that is what the field keeps.
    assert!(!record_poll_answer(
        &mut last_poll_status,
        POLL_MANIFEST_PENDING
    ));
    assert_eq!(last_poll_status, POLL_MANIFEST_PENDING);

    // A refusal after a parked version is a new attempt and is named.
    assert!(record_poll_answer(&mut last_poll_status, POLL_REJECTED));

    // Any ordinary answer ends the attempt.
    assert!(!record_poll_answer(&mut last_poll_status, POLL_NO_CHANGE));
    assert_eq!(last_poll_status, POLL_NO_CHANGE);
    assert!(!record_poll_answer(&mut last_poll_status, POLL_RELOADED));
    assert_eq!(last_poll_status, POLL_RELOADED);
}

/// The exported phase numbers are the managed `RunPhase` values, and an
/// unknown number is refused instead of folded into the default phase.
#[test]
fn exported_phase_numbers_map_to_the_engine_phases() {
    assert_eq!(run_phase_from_export(0), Some(RunPhase::PreProject));
    assert_eq!(run_phase_from_export(1), Some(RunPhase::Project));
    assert_eq!(run_phase_from_export(2), Some(RunPhase::PostProject));
    assert_eq!(run_phase_from_export(3), None);
    assert_eq!(run_phase_from_export(u8::MAX), None);
}

/// Verifies that managed-reported manifest lengths are bounded before any
/// host allocation happens.
///
/// The generic behaviour is pinned beside the helper; what is pinned here is
/// that the manifest path asks for `MAX_COMPONENT_MANIFEST_BYTES`.
#[test]
fn manifest_length_bounds_reject_empty_and_oversized_values() {
    /// Run the protocol against a reported length, using a copy callback that
    /// always refuses.
    ///
    /// A refused copy is the success case here: reaching it means the length
    /// was accepted, which is what the bound is being tested for.
    fn accepts(length: u32) -> bool {
        matches!(
            fetch_managed_buffer(|| length, |_, _| 0, MAX_COMPONENT_MANIFEST_BYTES),
            Err(ManagedBufferError::CopyFailed)
        )
    }

    assert!(!accepts(0));
    assert!(accepts(1));
    assert!(accepts(MAX_COMPONENT_MANIFEST_BYTES));
    assert!(!accepts(MAX_COMPONENT_MANIFEST_BYTES + 1));
    assert!(!accepts(u32::MAX));
}

/// Verifies that the counts a managed assembly reports are bounded before they
/// size a host allocation, with the offending count and the limit carried in
/// the refusal.
#[test]
fn system_counts_beyond_the_caps_are_refused() {
    assert_eq!(
        checked_system_count(MAX_SYSTEMS_PER_ASSEMBLY).unwrap(),
        MAX_SYSTEMS_PER_ASSEMBLY as usize,
        "the cap itself is accepted"
    );
    assert!(matches!(
        checked_system_count(MAX_SYSTEMS_PER_ASSEMBLY + 1),
        Err(pill_core::error::CSharpError::SystemCountOutOfRange { count, limit })
            if count == MAX_SYSTEMS_PER_ASSEMBLY + 1 && limit == MAX_SYSTEMS_PER_ASSEMBLY
    ));

    assert_eq!(
        checked_access_count(MAX_ACCESSES_PER_SYSTEM).unwrap(),
        MAX_ACCESSES_PER_SYSTEM as usize,
        "the cap itself is accepted"
    );
    assert!(matches!(
        checked_access_count(MAX_ACCESSES_PER_SYSTEM + 1),
        Err(pill_core::error::CSharpError::SystemCountOutOfRange { count, limit })
            if count == MAX_ACCESSES_PER_SYSTEM + 1 && limit == MAX_ACCESSES_PER_SYSTEM
    ));
}

// =============================================================================
// Manifest apply on reload
// =============================================================================

/// Build one top-level field entry for a managed manifest.
fn manifest_field(name: &str, offset: usize, size: usize) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "offset": offset,
        "size": size,
        "primitive_type": "System.Single",
        "fields": [],
    })
}

/// Build one top-level field entry that declares a default literal.
fn manifest_field_with_default(
    name: &str,
    offset: usize,
    size: usize,
    primitive_type: &str,
    default: &str,
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "offset": offset,
        "size": size,
        "primitive_type": primitive_type,
        "default": default,
        "fields": [],
    })
}

/// Build the serialized manifest for one descriptor test component.
fn manifest_bytes(
    name: &str,
    size: usize,
    alignment: usize,
    schema_hash: u64,
    fields: Vec<serde_json::Value>,
) -> Vec<u8> {
    let stable_id = stable_component_id(&format!("TracyLive.{name}"));
    serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": format!("TracyLive.{name}"),
        "size": size,
        "alignment": alignment,
        "schema_hash": schema_hash,
        "shared": false,
        "fields": fields,
    }]))
    .expect("the test manifest serializes")
}

/// Build the serialized manifest for one entry that declares aliases.
fn manifest_bytes_with_aliases(
    name: &str,
    aliases: &[&str],
    size: usize,
    alignment: usize,
    schema_hash: u64,
    fields: Vec<serde_json::Value>,
) -> Vec<u8> {
    let stable_id = stable_component_id(&format!("TracyLive.{name}"));
    serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": format!("TracyLive.{name}"),
        "size": size,
        "alignment": alignment,
        "schema_hash": schema_hash,
        "shared": false,
        "aliases": aliases,
        "fields": fields,
    }]))
    .expect("the test manifest serializes")
}

/// Register one descriptor test component from a manifest and hand back its
/// binding store, the way a managed project start leaves them.
fn store_with_component(
    engine: &mut Engine,
    name: &str,
    size: usize,
    alignment: usize,
    schema_hash: u64,
    fields: Vec<serde_json::Value>,
) -> (BindingStore, Vec<u8>) {
    let manifest = manifest_bytes(name, size, alignment, schema_hash, fields);
    let shared = shared_component_bindings(engine);
    let store = BindingStore::new(
        register_component_manifest(engine, &manifest, shared)
            .expect("the test manifest registers"),
    );
    (store, manifest)
}

/// A manifest that says what the bindings already hold applies nothing.
#[test]
fn an_unchanged_manifest_applies_nothing() {
    let mut engine = Engine::new();
    let (store, manifest) = store_with_component(
        &mut engine,
        "Unchanged",
        8,
        4,
        7,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );

    let report = apply_component_manifest_on_reload(&mut engine, &manifest, &store)
        .expect("the same manifest applies cleanly");

    assert!(report.added.is_empty(), "nothing was added");
    assert!(report.migrated.is_empty(), "nothing was migrated");
    let component_id = store.read()[&test_stable_id("Unchanged")].component_id();
    assert_eq!(engine.world().component_layout(component_id), Some((8, 4)));
}

/// A reshaped descriptor component keeps its entities and moves their bytes.
#[cfg(feature = "hot_reload")]
#[test]
fn a_reshaped_descriptor_component_is_migrated_on_apply() {
    let mut engine = Engine::new();
    let (store, _before) = store_with_component(
        &mut engine,
        "Relayout",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let stable_id = test_stable_id("Relayout");
    let component_id = store.read()[&stable_id].component_id();
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, [1.0_f32, 2.0].map(f32::to_ne_bytes).concat())])
        .expect("the entity carries the registered layout");

    // `b` first, then `a`, then two fields that did not exist.
    let after = manifest_bytes(
        "Relayout",
        16,
        8,
        2,
        vec![
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
            manifest_field("d", 12, 4),
        ],
    );
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("a reshaped layout migrates");

    assert_eq!(report.migrated, vec!["TracyLive.Relayout".to_string()]);
    let mut expected = [2.0_f32, 1.0].map(f32::to_ne_bytes).concat();
    expected.extend_from_slice(&[0_u8; 8]);
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, component_id)
            .expect("the entity still carries the component"),
        expected.as_slice(),
        "values follow their field names and the new fields start zeroed"
    );
    assert_eq!(engine.world().component_layout(component_id), Some((16, 8)));
    // The table a system scope reads is the one that moved, not a copy of it.
    let ComponentBinding::Managed {
        size,
        align,
        schema_hash,
        ..
    } = store.read()[&stable_id]
    else {
        panic!("the binding is still managed");
    };
    assert_eq!((size, align, schema_hash), (16, 8, 2));
}

/// A build without the reload posture has no manifest apply path that can
/// reshape storage: a changed component layout is refused, and the registered
/// column keeps the shape it had.
#[cfg(not(feature = "hot_reload"))]
#[test]
fn a_reshaped_descriptor_component_is_refused_on_apply() {
    let mut engine = Engine::new();
    let (store, _before) = store_with_component(
        &mut engine,
        "Relayout",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let stable_id = test_stable_id("Relayout");
    let component_id = store.read()[&stable_id].component_id();

    // `b` first, then `a`, then two fields that did not exist.
    let after = manifest_bytes(
        "Relayout",
        16,
        8,
        2,
        vec![
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
            manifest_field("d", 12, 4),
        ],
    );
    let error = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect_err("a build without the reload posture refuses to reshape a component");

    assert!(
        error.to_plain_message().contains("cannot reshape storage"),
        "the refusal says why: {}",
        error.to_plain_message()
    );
    assert_eq!(
        engine.world().component_layout(component_id),
        Some((8, 4)),
        "the registered column keeps its shape"
    );
}

/// A rename declared through an alias migrates the predecessor's rows onto
/// the successor instead of being refused as a disappearance.
#[cfg(feature = "hot_reload")]
#[test]
fn a_renamed_component_carries_its_rows_through_the_alias() {
    // The apply path reads the resource table too; clear whatever an earlier
    // test left on this worker thread, or a leftover entry reads as a
    // vanished resource.
    resource_test_scope();
    let mut engine = Engine::new();
    let (store, _before) = store_with_component(
        &mut engine,
        "OldName",
        8,
        4,
        7,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let old_id = store.read()[&test_stable_id("OldName")].component_id();
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(old_id, [1.0_f32, 2.0].map(f32::to_ne_bytes).concat())])
        .expect("the entity carries the registered layout");

    let after = manifest_bytes_with_aliases(
        "NewName",
        &["TracyLive.OldName"],
        8,
        4,
        7,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("an aliased rename migrates");

    assert_eq!(
        report.renamed,
        vec!["TracyLive.OldName -> TracyLive.NewName".to_string()]
    );
    assert!(report.added.is_empty(), "a rename is not an addition");
    assert!(report.migrated.is_empty(), "a rename is not a relayout");
    let new_id = store.read()[&test_stable_id("NewName")].component_id();
    assert!(
        !store.read().contains_key(&test_stable_id("OldName")),
        "the predecessor key is gone from the binding table"
    );
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, new_id)
            .expect("the successor holds the row"),
        [1.0_f32, 2.0].map(f32::to_ne_bytes).concat().as_slice()
    );
    assert!(engine
        .world()
        .descriptor_component_bytes(entity, old_id)
        .is_none());
    assert_eq!(
        engine
            .world()
            .resolve_component_id_by_name_any("TracyLive.OldName")
            .unwrap(),
        None,
        "the old name is free again"
    );
    assert_eq!(
        engine
            .world()
            .resolve_component_id_by_name_any("TracyLive.NewName")
            .unwrap(),
        Some(new_id)
    );

    // Applying the same manifest again changes nothing: the alias no longer
    // resolves to anything, so there is no predecessor left to rename.
    let again = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("the second application settles");
    assert!(again.renamed.is_empty());
}

/// A field added with a default starts from that value, not zero.
#[cfg(feature = "hot_reload")]
#[test]
fn an_added_field_starts_from_its_declared_default() {
    resource_test_scope();
    let mut engine = Engine::new();
    let (store, _before) = store_with_component(
        &mut engine,
        "Defaults",
        4,
        4,
        1,
        vec![manifest_field("a", 0, 4)],
    );
    let stable_id = test_stable_id("Defaults");
    let component_id = store.read()[&stable_id].component_id();
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, 1.0_f32.to_ne_bytes().to_vec())])
        .expect("the entity carries the registered layout");

    let after = manifest_bytes(
        "Defaults",
        8,
        4,
        2,
        vec![
            manifest_field("a", 0, 4),
            manifest_field_with_default("added", 4, 4, "System.Single", "7.5"),
        ],
    );
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("the added field migrates");

    assert_eq!(report.migrated, vec!["TracyLive.Defaults".to_string()]);
    let mut expected = 1.0_f32.to_ne_bytes().to_vec();
    expected.extend_from_slice(&7.5_f32.to_ne_bytes());
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, component_id)
            .unwrap(),
        expected.as_slice(),
        "the carried value survives and the added field takes its default"
    );
}

/// A default declares what an empty field starts from; a field the row already
/// carries keeps its value even when the default for it changed.
#[cfg(feature = "hot_reload")]
#[test]
fn a_changed_default_never_resets_a_carried_field() {
    resource_test_scope();
    let mut engine = Engine::new();
    let (store, _before) = store_with_component(
        &mut engine,
        "Kept",
        4,
        4,
        1,
        vec![manifest_field("a", 0, 4)],
    );
    let stable_id = test_stable_id("Kept");
    let component_id = store.read()[&stable_id].component_id();
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, 1.0_f32.to_ne_bytes().to_vec())])
        .expect("the entity carries the registered layout");

    // Same shape, new schema hash (which is what makes it a migration) and a
    // new default for the same field.
    let after = manifest_bytes(
        "Kept",
        4,
        4,
        2,
        vec![manifest_field_with_default(
            "a",
            0,
            4,
            "System.Single",
            "9.5",
        )],
    );
    apply_component_manifest_on_reload(&mut engine, &after, &store).expect("the reshape migrates");

    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, component_id)
            .unwrap(),
        1.0_f32.to_ne_bytes().as_slice(),
        "a default never overwrites a value that was carried"
    );
}

/// A default that does not parse as its field's type, or that sits on a struct
/// field, is refused where the manifest is parsed.
#[test]
fn field_defaults_are_validated_against_their_field_type() {
    let wrong_type = manifest_bytes(
        "BadDefault",
        4,
        4,
        1,
        vec![manifest_field_with_default(
            "a",
            0,
            4,
            "System.Int32",
            "1.5",
        )],
    );
    let error = match parse_and_validate_manifest(&wrong_type) {
        Ok(_) => panic!("a default that does not parse as the field's type is refused"),
        Err(error) => error,
    };
    assert!(
        error.to_plain_message().contains("is not a valid"),
        "the refusal names the reason: {error}"
    );

    let nested_id = stable_component_id("TracyLive.NestedDefault");
    let nested_default = serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": nested_id.0 as u64,
        "stable_id_high": (nested_id.0 >> 64) as u64,
        "full_name": "TracyLive.NestedDefault",
        "size": 4,
        "alignment": 4,
        "schema_hash": 1,
        "shared": false,
        "fields": [{
            "name": "inner",
            "offset": 0,
            "size": 4,
            "primitive_type": "System.Single",
            "default": "1.0",
            "fields": [manifest_field("leaf", 0, 4)],
        }],
    }]))
    .expect("the test manifest serializes");
    let error = match parse_and_validate_manifest(&nested_default) {
        Ok(_) => panic!("a default on a struct field is refused"),
        Err(error) => error,
    };
    assert!(
        error
            .to_plain_message()
            .contains("only supported on primitive fields"),
        "the refusal names the reason: {error}"
    );
}

/// The same rule for resources: a field added with a default starts from that
/// default instead of zero.
#[cfg(feature = "hot_reload")]
#[test]
fn a_resource_field_added_with_a_default_starts_from_it() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes("Tuned", 4, 4, 11, vec![manifest_field("speed", 0, 4)]);
    register_component_manifest(&mut engine, &before, ComponentBindings::new())
        .expect("the resource registers");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(resource_id_of("Tuned"), &1.0_f32.to_ne_bytes())
        .expect("the payload is the declared width");

    let after = resource_manifest_bytes(
        "Tuned",
        8,
        4,
        12,
        vec![
            manifest_field("speed", 0, 4),
            manifest_field_with_default("gain", 4, 4, "System.Single", "2.5"),
        ],
    );
    let store = BindingStore::new(ComponentBindings::new());
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("the resource reshape migrates");

    assert_eq!(
        report.resources_migrated,
        vec!["TracyLive.Tuned".to_string()]
    );
    let mut expected = 1.0_f32.to_ne_bytes().to_vec();
    expected.extend_from_slice(&2.5_f32.to_ne_bytes());
    assert_eq!(
        engine
            .world()
            .foreign_resource_bytes(resource_id_of("Tuned"))
            .expect("the value is stored"),
        expected.as_slice(),
        "the carried value survives and the added field takes its default"
    );
}

/// A managed component the manifest stopped naming is retired. Its rows and
/// its registration go, so a later declaration of the same name starts clean
/// instead of colliding with storage nothing tracks.
#[test]
fn a_vanished_component_is_retired_on_apply() {
    resource_test_scope();
    let mut engine = Engine::new();
    let (store, _manifest) = store_with_component(
        &mut engine,
        "Vanished",
        4,
        4,
        1,
        vec![manifest_field("a", 0, 4)],
    );
    let stable_id = test_stable_id("Vanished");
    let component_id = store.read()[&stable_id].component_id();
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(component_id, 1.0_f32.to_ne_bytes().to_vec())])
        .expect("the entity carries the registered layout");

    let empty = serde_json::to_vec(&serde_json::json!([])).expect("an empty manifest serializes");
    let report = apply_component_manifest_on_reload(&mut engine, &empty, &store)
        .expect("a vanished component is retired, not refused");

    assert_eq!(report.retired, vec!["TracyLive.Vanished".to_string()]);
    assert!(
        !store.read().contains_key(&stable_id),
        "the binding is gone from the table"
    );
    assert!(
        engine
            .world()
            .descriptor_component_bytes(entity, component_id)
            .is_none(),
        "the row is gone"
    );
    // The entity carried only the retired component, and retiring removes it
    // like any removal, so the entity itself is gone too.
    assert_eq!(engine.world().entity_count(), 0);
    assert_eq!(
        engine
            .world()
            .resolve_component_id_by_name_any("TracyLive.Vanished")
            .unwrap(),
        None,
        "the registration is gone, so the name is free again"
    );

    // Re-declaring the name later is an ordinary addition: retirement left no
    // storage behind for the name to collide with.
    let readded = manifest_bytes("Vanished", 4, 4, 1, vec![manifest_field("a", 0, 4)]);
    let report = apply_component_manifest_on_reload(&mut engine, &readded, &store)
        .expect("the name can be declared again");
    assert_eq!(report.added, vec!["TracyLive.Vanished".to_string()]);
}

/// Alias rules are enforced where the manifest is parsed, so a colliding or
/// empty alias never reaches planning.
#[test]
fn alias_validation_refuses_empty_and_colliding_names() {
    let empty =
        manifest_bytes_with_aliases("Aliased", &["  "], 4, 4, 1, vec![manifest_field("a", 0, 4)]);
    let error = match parse_and_validate_manifest(&empty) {
        Ok(_) => panic!("an empty alias is refused"),
        Err(error) => error,
    };
    assert!(
        error.to_plain_message().contains("empty alias"),
        "the refusal names the reason: {error}"
    );

    let entry = |name: &str, aliases: &[&str]| {
        let stable_id = stable_component_id(&format!("TracyLive.{name}"));
        serde_json::json!({
            "stable_id_low": stable_id.0 as u64,
            "stable_id_high": (stable_id.0 >> 64) as u64,
            "full_name": format!("TracyLive.{name}"),
            "size": 4,
            "alignment": 4,
            "schema_hash": 1,
            "shared": false,
            "aliases": aliases,
            "fields": [manifest_field("a", 0, 4)],
        })
    };

    let live_collision = serde_json::to_vec(&serde_json::json!([
        entry("Live", &[]),
        entry("Other", &["TracyLive.Live"]),
    ]))
    .expect("the test manifest serializes");
    let error = match parse_and_validate_manifest(&live_collision) {
        Ok(_) => panic!("an alias naming a live declaration is refused"),
        Err(error) => error,
    };
    assert!(
        error.to_plain_message().contains("live declaration"),
        "the refusal names the reason: {error}"
    );

    let duplicate_alias = serde_json::to_vec(&serde_json::json!([
        entry("First", &["TracyLive.Gone"]),
        entry("Second", &["TracyLive.Gone"]),
    ]))
    .expect("the test manifest serializes");
    let error = match parse_and_validate_manifest(&duplicate_alias) {
        Ok(_) => panic!("one alias claimed twice is refused"),
        Err(error) => error,
    };
    assert!(
        error.to_plain_message().contains("declared by both"),
        "the refusal names both owners: {error}"
    );
}

/// A manifest whose application fails partway leaves none of it applied. The
/// journal is unwound, so the store, the bindings and the migrated rows are
/// byte-identical to their values before the call.
#[test]
fn a_refused_manifest_leaves_none_of_it_applied() {
    let mut engine = Engine::new();
    let (store, _manifest) = store_with_component(
        &mut engine,
        "Relayout",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let relayout_id = store.read()[&test_stable_id("Relayout")].component_id();
    let mut row = 1.0_f32.to_ne_bytes().to_vec();
    row.extend_from_slice(&2.0_f32.to_ne_bytes());
    let entity = engine
        .world_mut()
        .create_descriptor_entity(&[(relayout_id, row.clone())])
        .expect("the entity carries the registered layout");

    // A name already claimed by a live column: the second entry's registration
    // is refused by the engine, which no amount of validation can foresee.
    let claimed_stable_id = stable_component_id("TracyLive.Claimed");
    engine
        .world_mut()
        .register_component_descriptor(
            claimed_stable_id.0,
            "TracyLive.Claimed",
            4,
            4,
            9,
            test_blittability_witness(),
        )
        .expect("the claiming component registers");
    let claimed_id = engine
        .world()
        .resolve_component_id_by_name_any("TracyLive.Claimed")
        .expect("the name resolves to its registration")
        .expect("the name is claimed");
    engine
        .world_mut()
        .create_descriptor_entity(&[(claimed_id, 9_u32.to_ne_bytes().to_vec())])
        .expect("the claiming component has a live row");

    let store_len_before = store.read().len();

    // Entry 1 reshapes `Relayout` and migrates its row; entry 2 is a second
    // component claiming the taken name, so it fails after entry 1 applied.
    let relayout_entry = serde_json::json!({
        "stable_id_low": test_stable_id("Relayout").0 as u64,
        "stable_id_high": (test_stable_id("Relayout").0 >> 64) as u64,
        "full_name": "TracyLive.Relayout",
        "size": 16,
        "alignment": 8,
        "schema_hash": 2,
        "shared": false,
        "fields": [
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
            manifest_field("d", 12, 4),
        ],
    });
    // A fresh stable id under the claimed *name*: with the same identity the
    // entry would follow the idempotent re-registration path instead of
    // colliding.
    let colliding_stable_id = stable_component_id("TracyLive.ClaimedV2");
    let claimed_entry = serde_json::json!({
        "stable_id_low": colliding_stable_id.0 as u64,
        "stable_id_high": (colliding_stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.Claimed",
        "size": 4,
        "alignment": 4,
        "schema_hash": 9,
        "shared": false,
        "fields": [manifest_field("value", 0, 4)],
    });
    let refused = serde_json::to_vec(&serde_json::json!([relayout_entry, claimed_entry]))
        .expect("the refused manifest serializes");

    let error = apply_component_manifest_on_reload(&mut engine, &refused, &store)
        .expect_err("a name claimed by a live column refuses the entry");
    assert!(
        error.to_string().contains("Claimed"),
        "the refusal names the colliding component: {error}"
    );

    // Everything is as it was: same store size, the binding still describes
    // the old shape, the layout is back and the row is byte-for-byte intact.
    assert_eq!(store.read().len(), store_len_before);
    assert!(
        store.read().get(&colliding_stable_id).is_none(),
        "the refused addition left no binding behind"
    );
    let ComponentBinding::Managed {
        size,
        align,
        schema_hash,
        ..
    } = store.read()[&test_stable_id("Relayout")]
    else {
        panic!("the binding is still managed");
    };
    assert_eq!(
        (size, align, schema_hash),
        (8, 4, 1),
        "the binding rolled back to the shape the world is in now"
    );
    assert_eq!(engine.world().component_layout(relayout_id), Some((8, 4)));
    assert_eq!(
        engine
            .world()
            .descriptor_component_bytes(entity, relayout_id),
        Some(row.as_slice()),
        "the migrated rows were migrated back"
    );
}

/// Two refusals: a module mirror whose layout changed, and a manifest naming
/// one identity twice.
#[test]
fn the_apply_refuses_what_it_cannot_migrate() {
    // A `ModuleNative` binding the manifest disagrees with: the Rust side owns
    // that layout, so the mirror is simply wrong.
    let mut engine = Engine::new();
    let stable_id = test_stable_id("ModuleThing");
    let module_id = engine
        .world_mut()
        .register_component_descriptor(
            stable_id.0,
            "TracyLive.ModuleThing",
            8,
            4,
            1,
            test_blittability_witness(),
        )
        .expect("the module component registers");
    let mut bindings = shared_component_bindings(&mut engine);
    bindings.insert(
        stable_id,
        ComponentBinding::ModuleNative {
            component_id: module_id,
            size: 8,
            align: 4,
            field_signature: None,
        },
    );
    let store = BindingStore::new(bindings);
    let changed = manifest_bytes("ModuleThing", 16, 8, 2, vec![manifest_field("a", 0, 4)]);
    let error = apply_component_manifest_on_reload(&mut engine, &changed, &store)
        .expect_err("a module mirror cannot change");
    assert!(
        error.to_string().contains("native component uses 8/4"),
        "the refusal names both layouts: {error}"
    );

    // A duplicated identity never reaches the world.
    let mut engine = Engine::new();
    let (store, manifest) = store_with_component(
        &mut engine,
        "Twice",
        4,
        4,
        1,
        vec![manifest_field("a", 0, 4)],
    );
    let entry: serde_json::Value = serde_json::from_slice::<serde_json::Value>(&manifest)
        .expect("the manifest parses")
        .as_array()
        .expect("one entry")
        .first()
        .expect("one entry")
        .clone();
    let duplicated = serde_json::to_vec(&serde_json::json!([entry.clone(), entry]))
        .expect("the duplicated manifest serializes");
    let error = apply_component_manifest_on_reload(&mut engine, &duplicated, &store)
        .expect_err("a duplicate is refused");
    assert!(
        error.to_string().contains("duplicate declaration"),
        "the refusal names the cause: {error}"
    );
}

// =============================================================================
// Managed resources
// =============================================================================

/// Start a resource test from an empty table.
///
/// The table is thread-local under `cargo test` (see `resources`), so tests on
/// parallel threads cannot reach each other's entries. Cargo does reuse
/// threads across tests, though, so the reset is what still gives each test a
/// table of its own.
fn resource_test_scope() {
    super::resources::reset_resource_bindings_for_test();
}

/// Build the serialized manifest for one managed resource.
///
/// Same document shape as a component's, with the `kind` tag that tells the
/// two apart, which is the only thing a resource entry adds to the serialized
/// document.
fn resource_manifest_bytes(
    name: &str,
    size: usize,
    alignment: usize,
    schema_hash: u64,
    fields: Vec<serde_json::Value>,
) -> Vec<u8> {
    let stable_id = stable_component_id(&format!("TracyLive.{name}"));
    serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": format!("TracyLive.{name}"),
        "size": size,
        "alignment": alignment,
        "schema_hash": schema_hash,
        "shared": false,
        "kind": "resource",
        "fields": fields,
    }]))
    .expect("the test manifest serializes")
}

/// Build the serialized manifest for one managed resource that declares
/// aliases.
#[cfg(feature = "hot_reload")]
fn resource_manifest_bytes_with_aliases(
    name: &str,
    aliases: &[&str],
    size: usize,
    alignment: usize,
    schema_hash: u64,
    fields: Vec<serde_json::Value>,
) -> Vec<u8> {
    let stable_id = stable_component_id(&format!("TracyLive.{name}"));
    serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": format!("TracyLive.{name}"),
        "size": size,
        "alignment": alignment,
        "schema_hash": schema_hash,
        "shared": false,
        "kind": "resource",
        "aliases": aliases,
        "fields": fields,
    }]))
    .expect("the test manifest serializes")
}

/// The engine id a registered managed resource was bound to.
///
/// Asked of the binding table rather than recomputed, so the test checks the
/// id the host actually uses instead of a second opinion about what it is.
fn resource_id_of(name: &str) -> pill_engine::ResourceId {
    bound_resource_id_and_width(stable_component_id(&format!("TracyLive.{name}")))
        .expect("the resource is registered")
        .0
}

/// A manifest resource entry registers the resource and seeds its value.
///
/// The seed is the point. A managed declaration is all there is: nothing on
/// the C# side inserts a resource the way a Rust `init` does. A resource
/// registered and left empty would fail the first `Res<T>` access in the frame
/// that follows.
#[test]
fn a_manifest_resource_is_registered_and_seeded() {
    resource_test_scope();
    let mut engine = Engine::new();
    let manifest = resource_manifest_bytes(
        "Tuning",
        8,
        4,
        11,
        vec![manifest_field("speed", 0, 4), manifest_field("gain", 4, 4)],
    );

    let bindings = register_component_manifest(&mut engine, &manifest, ComponentBindings::new())
        .expect("a resource-only manifest registers");

    assert!(
        bindings.is_empty(),
        "a resource entry registers no component column"
    );
    assert_eq!(
        engine
            .world()
            .foreign_resource_bytes(resource_id_of("Tuning")),
        Some(&[0_u8; 8][..]),
        "the resource starts as a defined all-zero value"
    );
}

/// A managed resource is snapshot-visible as soon as it is registered.
#[test]
fn a_manifest_resource_joins_the_snapshot() {
    resource_test_scope();
    let mut engine = Engine::new();
    let manifest = resource_manifest_bytes("Saved", 4, 4, 3, vec![manifest_field("count", 0, 4)]);
    register_component_manifest(&mut engine, &manifest, ComponentBindings::new())
        .expect("the manifest registers");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(resource_id_of("Saved"), &7_u32.to_ne_bytes())
        .expect("the payload is the declared width");

    let snapshot = engine.world().snapshot_resources();

    assert!(
        snapshot.payload("TracyLive.Saved").is_some(),
        "a managed resource reaches a snapshot without any Rust type to derive from"
    );
}

/// A resource rename declared through an alias moves the stored value onto
/// the successor instead of being refused as a disappearance.
#[cfg(feature = "hot_reload")]
#[test]
fn a_renamed_resource_carries_its_value_through_the_alias() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes(
        "OldSettings",
        8,
        4,
        11,
        vec![manifest_field("speed", 0, 4), manifest_field("gain", 4, 4)],
    );
    register_component_manifest(&mut engine, &before, ComponentBindings::new())
        .expect("the resource registers");
    let old_id = resource_id_of("OldSettings");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(old_id, &[1.0_f32, 2.0].map(f32::to_ne_bytes).concat())
        .expect("the payload is the declared width");

    let after = resource_manifest_bytes_with_aliases(
        "NewSettings",
        &["TracyLive.OldSettings"],
        8,
        4,
        11,
        vec![manifest_field("speed", 0, 4), manifest_field("gain", 4, 4)],
    );
    let store = BindingStore::new(ComponentBindings::new());
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("an aliased resource rename migrates");

    assert_eq!(
        report.resources_renamed,
        vec!["TracyLive.OldSettings -> TracyLive.NewSettings".to_string()]
    );
    assert!(
        bound_resource_id_and_width(test_stable_id("OldSettings")).is_none(),
        "the old binding is gone from the table"
    );
    assert_eq!(
        engine
            .world()
            .foreign_resource_bytes(resource_id_of("NewSettings")),
        Some(&[1.0_f32, 2.0].map(f32::to_ne_bytes).concat()[..]),
        "the value moved onto the successor"
    );
    assert_eq!(
        engine.world().foreign_resource_layout(old_id),
        None,
        "the predecessor declaration is dropped"
    );

    // Applying the same manifest again changes nothing: the alias no longer
    // names a live binding, so there is no predecessor left to rename.
    let again = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("the second application settles");
    assert!(again.resources_renamed.is_empty());
}

/// A resource entry marked shared is refused where it is declared.
#[test]
fn a_shared_resource_entry_is_refused() {
    resource_test_scope();
    let mut engine = Engine::new();
    let stable_id = stable_component_id("TracyLive.Confused");
    let manifest = serde_json::to_vec(&serde_json::json!([{
        "stable_id_low": stable_id.0 as u64,
        "stable_id_high": (stable_id.0 >> 64) as u64,
        "full_name": "TracyLive.Confused",
        "size": 4,
        "alignment": 4,
        "schema_hash": 1,
        "shared": true,
        "kind": "resource",
        "fields": [manifest_field("value", 0, 4)],
    }]))
    .expect("the test manifest serializes");

    let Err(error) = register_component_manifest(&mut engine, &manifest, ComponentBindings::new())
    else {
        panic!("a shared resource has no native binding to be shared with");
    };

    assert!(
        error.to_plain_message().contains("marked shared"),
        "the refusal names what is wrong: {}",
        error.to_plain_message()
    );
}

/// A reshaped resource keeps its value and moves its bytes by field name.
#[cfg(feature = "hot_reload")]
#[test]
fn a_reshaped_resource_is_migrated_on_reload() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes(
        "Relaid",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let store = BindingStore::new(
        register_component_manifest(&mut engine, &before, ComponentBindings::new())
            .expect("the manifest registers"),
    );
    let id = resource_id_of("Relaid");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(id, &[1.0_f32, 2.0].map(f32::to_ne_bytes).concat())
        .expect("the payload is the declared width");

    // `b` first, then `a`, then a field that did not exist.
    let after = resource_manifest_bytes(
        "Relaid",
        12,
        4,
        2,
        vec![
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
        ],
    );
    let report = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("a reshaped resource migrates");

    assert_eq!(
        report.resources_migrated,
        vec!["TracyLive.Relaid".to_string()]
    );
    let mut expected = [2.0_f32, 1.0].map(f32::to_ne_bytes).concat();
    expected.extend_from_slice(&[0_u8; 4]);
    assert_eq!(
        engine.world().foreign_resource_bytes(id),
        Some(expected.as_slice()),
        "values follow their field names and the new field starts zeroed"
    );
}

/// A build without the reload posture refuses a reshaped resource instead of
/// reshaping its storage; the value keeps the bytes it was written with.
#[cfg(not(feature = "hot_reload"))]
#[test]
fn a_reshaped_resource_is_refused_on_reload() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes(
        "RelaidRes",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let store = BindingStore::new(
        register_component_manifest(&mut engine, &before, ComponentBindings::new())
            .expect("the manifest registers"),
    );
    let id = resource_id_of("RelaidRes");
    let payload = [1.0_f32, 2.0].map(f32::to_ne_bytes).concat();
    engine
        .world_mut()
        .insert_foreign_resource_bytes(id, &payload)
        .expect("the payload is the declared width");

    // `b` first, then `a`, then a field that did not exist.
    let after = resource_manifest_bytes(
        "RelaidRes",
        12,
        4,
        2,
        vec![
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
        ],
    );
    let error = apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect_err("a build without the reload posture refuses to reshape a resource");

    assert!(
        error.to_plain_message().contains("cannot reshape storage"),
        "the refusal says why: {}",
        error.to_plain_message()
    );
    assert_eq!(
        engine.world().foreign_resource_layout(id),
        Some((8, 4, 1)),
        "the stored value keeps the layout it was written with"
    );
    assert_eq!(
        engine.world().foreign_resource_bytes(id),
        Some(payload.as_slice()),
        "the stored bytes are untouched"
    );
}

/// A resource the arriving manifest stops declaring is retired: its value and
/// declaration go, and an unclaimed id is dropped outright.
#[test]
fn a_resource_dropped_from_the_manifest_is_retired() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes("Vanishing", 4, 4, 1, vec![manifest_field("a", 0, 4)]);
    let store = BindingStore::new(
        register_component_manifest(&mut engine, &before, ComponentBindings::new())
            .expect("the manifest registers"),
    );
    let id = resource_id_of("Vanishing");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(id, &7_u32.to_ne_bytes())
        .expect("the payload is the declared width");

    let empty = serde_json::to_vec(&serde_json::json!([])).expect("an empty manifest serializes");
    let report = apply_component_manifest_on_reload(&mut engine, &empty, &store)
        .expect("a dropped resource is retired, not refused");

    assert_eq!(
        report.resources_retired,
        vec!["TracyLive.Vanishing".to_string()]
    );
    assert!(
        bound_resource_id_and_width(stable_component_id("TracyLive.Vanishing")).is_none(),
        "the binding is gone from the table"
    );
    assert_eq!(
        engine.world().foreign_resource_layout(id),
        None,
        "the value and its declaration are gone"
    );
}

/// A shared resource another subject still claims survives the retirement of
/// this subject's declaration: `drop_resources` releases only unclaimed ids.
#[test]
fn a_claimed_resource_survives_a_dropped_declaration() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes("Shared", 4, 4, 1, vec![manifest_field("a", 0, 4)]);
    let store = BindingStore::new(
        register_component_manifest(&mut engine, &before, ComponentBindings::new())
            .expect("the manifest registers"),
    );
    let id = resource_id_of("Shared");
    engine
        .world_mut()
        .insert_foreign_resource_bytes(id, &7_u32.to_ne_bytes())
        .expect("the payload is the declared width");
    // A module declaring the same name holds a claim; the drop must skip it.
    engine.world_mut().retain_resource_claims(&[id]);

    let empty = serde_json::to_vec(&serde_json::json!([])).expect("an empty manifest serializes");
    let report = apply_component_manifest_on_reload(&mut engine, &empty, &store)
        .expect("the declaration is retired");

    assert_eq!(report.resources_retired.len(), 1);
    assert_eq!(
        engine.world().foreign_resource_bytes(id),
        Some(&7_u32.to_ne_bytes()[..]),
        "the claimed value survives this subject's retirement"
    );
}

/// A reflected resource access reaches the scheduler as a resource.
///
/// Component and resource keys are both 128-bit name hashes, so only the kind
/// byte tells them apart. Recorded as a component, two systems writing one
/// resource would be free to run in the same batch.
#[test]
fn a_resource_access_is_derived_as_a_resource() {
    resource_test_scope();
    let mut engine = Engine::new();
    let manifest = resource_manifest_bytes("Scheduled", 4, 4, 1, vec![manifest_field("a", 0, 4)]);
    register_component_manifest(&mut engine, &manifest, ComponentBindings::new())
        .expect("the manifest registers");
    let stable_id = stable_component_id("TracyLive.Scheduled");

    let writer = derive_system_access(
        &[NativeSystemAccess {
            component_key: stable_id.0 as u64,
            component_key_high: (stable_id.0 >> 64) as u64,
            mode: 1,
            kind: 1,
        }],
        &ComponentBindings::new(),
    )
    .expect("the resource is registered");
    let reader = derive_system_access(
        &[NativeSystemAccess {
            component_key: stable_id.0 as u64,
            component_key_high: (stable_id.0 >> 64) as u64,
            mode: 0,
            kind: 1,
        }],
        &ComponentBindings::new(),
    )
    .expect("the resource is registered");

    assert!(
        writer.conflicts_with(&reader),
        "a writer and a reader of one resource cannot share a batch"
    );
    assert!(
        !reader.conflicts_with(&reader),
        "two readers of one resource can"
    );
}

/// A resource access for a key no manifest registered is refused by name.
#[test]
fn an_unregistered_resource_access_is_refused() {
    resource_test_scope();
    let stable_id = stable_component_id("TracyLive.NeverDeclared");

    let error = derive_system_access(
        &[NativeSystemAccess {
            component_key: stable_id.0 as u64,
            component_key_high: (stable_id.0 >> 64) as u64,
            mode: 0,
            kind: 1,
        }],
        &ComponentBindings::new(),
    )
    .expect_err("nothing registered this resource");

    assert!(
        error
            .to_plain_message()
            .contains("unregistered resource key"),
        "the refusal says resource rather than component: {}",
        error.to_plain_message()
    );
}

/// A managed resource's field layout reaches the engine, and survives a
/// reshape.
///
/// The host is the only holder of a foreign resource's field names, so this is
/// what makes one inspectable at all. A reload that moves the fields has to
/// republish them, because the engine drops the layout with the shape it
/// described.
#[cfg(feature = "hot_reload")]
#[test]
fn a_managed_resource_publishes_its_field_layout() {
    resource_test_scope();
    let mut engine = Engine::new();
    let before = resource_manifest_bytes(
        "Inspectable",
        8,
        4,
        1,
        vec![manifest_field("a", 0, 4), manifest_field("b", 4, 4)],
    );
    let store = BindingStore::new(
        register_component_manifest(&mut engine, &before, ComponentBindings::new())
            .expect("the manifest registers"),
    );
    let id = resource_id_of("Inspectable");

    let fields = engine
        .world()
        .resource_field_layout(id)
        .expect("registration published the layout");
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "a");
    assert_eq!(fields[1].offset, 4);

    // A reshape moves the fields; the arriving layout has to replace the one
    // the relayout dropped.
    let after = resource_manifest_bytes(
        "Inspectable",
        12,
        4,
        2,
        vec![
            manifest_field("b", 0, 4),
            manifest_field("a", 4, 4),
            manifest_field("c", 8, 4),
        ],
    );
    apply_component_manifest_on_reload(&mut engine, &after, &store)
        .expect("a reshaped resource migrates");

    let fields = engine
        .world()
        .resource_field_layout(id)
        .expect("the reload republished the layout");
    assert_eq!(fields.len(), 3, "the arriving shape is what is served");
    assert_eq!(fields[0].name, "b", "and it is the arriving order");
}

// =============================================================================
// Mirrored Rust functions (renderer data crate, reached by address)
// =============================================================================

/// The qualified name that `#[pill_mirror_object]` records for the renderer's
/// mesh.
const MESH_TYPE: &str = "pill_master_renderer_data::assets::mesh::Mesh";

/// The qualified name of the renderer's pipeline resource.
const RENDERING_MANAGER_TYPE: &str =
    "pill_master_renderer_data::rendering_manager::RenderingManager";

/// The address of one mirrored function in this binary's own table.
fn mirrored(type_name: &str, method: &str) -> usize {
    super::mirror_calls::statically_linked_mirror_methods()
        .into_iter()
        .find(|row| row.type_name == type_name && row.method_name == method)
        .unwrap_or_else(|| panic!("{type_name}::{method} is not mirrored"))
        .address
}

/// Call a mirrored trampoline with packed slots, returning its status and the
/// return buffer.
fn call_mirrored(address: usize, slots: &[[u8; 16]]) -> (u8, [u8; 64]) {
    type Trampoline = unsafe extern "C" fn(*const u8, *mut u8) -> u8;
    let mut return_buffer = [0u8; 64];
    // SAFETY: the address is a trampoline of the one mirror signature, from
    // this binary's inventory; the slots and the return buffer outlive the call.
    let status = unsafe {
        let trampoline: Trampoline = std::mem::transmute(address);
        trampoline(slots.as_ptr().cast(), return_buffer.as_mut_ptr())
    };
    (status, return_buffer)
}

/// One `str` slot over a borrowed string.
fn str_slot(text: &str) -> [u8; 16] {
    let mut slot = [0u8; 16];
    slot[..8].copy_from_slice(&(text.as_ptr() as usize).to_le_bytes());
    slot[8..12].copy_from_slice(&(text.len() as u32).to_le_bytes());
    slot
}

/// One slot holding an address.
fn pointer_slot(pointer: usize) -> [u8; 16] {
    let mut slot = [0u8; 16];
    slot[..8].copy_from_slice(&pointer.to_le_bytes());
    slot
}

/// The access a system declares for `ResMut<T>` over a Rust-owned resource.
fn native_resource_write(shared_name: &str) -> NativeSystemAccess {
    let identity = pill_engine::component::shared_component_identity(shared_name);
    NativeSystemAccess {
        component_key: identity as u64,
        component_key_high: (identity >> 64) as u64,
        mode: 1,
        kind: super::context::ACCESS_KIND_NATIVE_RESOURCE,
    }
}

/// Ask the bridge for a Rust-owned resource's address, as the managed side does.
fn native_resource(shared_name: &str, mode: u8) -> (u8, usize) {
    let identity = pill_engine::component::shared_component_identity(shared_name);
    let mut pointer = std::ptr::null_mut();
    let status = super::mirror_calls::ffi_get_native_resource(
        identity as u64,
        (identity >> 64) as u64,
        mode,
        &mut pointer,
    );
    (status, pointer as usize)
}

/// Take one of the mirror text channels, as the managed side does.
fn take_text(kind: u8) -> Option<String> {
    let (mut data, mut length) = (std::ptr::null(), 0u32);
    let status = super::mirror_calls::ffi_take_mirror_text(kind, &mut data, &mut length);
    (status == 0).then(|| {
        // SAFETY: the bridge handed out `length` bytes it keeps until the next
        // call on this thread.
        let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) };
        String::from_utf8(bytes.to_vec()).expect("the channels carry UTF-8")
    })
}

/// A binary that links the renderer data crate carries its mirrored functions
/// and types in its own inventory. Each callable one has its address there,
/// which is the table a shipping build hands the managed runtime.
#[test]
fn the_renderer_data_mirrors_reach_the_static_table() {
    let rows = super::mirror_calls::statically_linked_mirror_methods();
    for (type_name, method) in [
        (MESH_TYPE, "from_obj_bytes"),
        (MESH_TYPE, "__type"),
        (MESH_TYPE, "__asset_add_named"),
        (MESH_TYPE, "__asset_import"),
        (RENDERING_MANAGER_TYPE, "clear"),
    ] {
        let row = rows
            .iter()
            .find(|row| row.type_name == type_name && row.method_name == method)
            .unwrap_or_else(|| panic!("{type_name}::{method} is not in the table"));
        assert_ne!(row.address, 0, "{type_name}::{method} has a trampoline");
    }
    let resource = rows
        .iter()
        .find(|row| row.type_name == RENDERING_MANAGER_TYPE && row.method_name == "__type")
        .expect("the resource declares its type row");
    assert_eq!(resource.owner_kind, "resource");
    assert_eq!(
        resource.address, 0,
        "a resource's type row has no trampoline"
    );
}

/// A Rust-owned resource is reached only inside an invocation, and only under
/// the access the system declared - or in a startup, which holds the world.
#[test]
fn a_native_resource_needs_a_declared_access() {
    let mut engine = Engine::new();
    pill_master_renderer_data::register(&mut engine);
    let shared_name = pill_engine::asset::ASSET_MANAGER_SHARED_NAME;
    // Step 1: outside a managed invocation every request is refused.
    assert_eq!(
        native_resource(shared_name, 1).0,
        3,
        "no invocation is running"
    );

    let bindings = ComponentBindings::default();
    {
        // Step 2: inside an invocation, a write nobody declared is refused.
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &bindings);
        assert_eq!(native_resource(shared_name, 1).0, 2, "nothing was declared");
    }
    let declared = [native_resource_write(shared_name)];
    let expected = engine
        .world_mut()
        .get_resource_mut::<pill_engine::AssetManager>()
        .map(|assets| assets as *mut pill_engine::AssetManager as usize)
        .expect("the renderer data registers asset types");
    {
        // Step 3: a declared write reaches the resource itself.
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &declared, &bindings);
        assert_eq!(
            native_resource(shared_name, 1),
            (0, expected),
            "the declared write reaches the store itself"
        );
    }
    let mut queue = pill_engine::commands::CommandQueue::default();
    {
        // Step 4: a startup holds the world, so no declaration is needed.
        let _guard =
            ActiveScopeGuard::install_exclusive_scope(engine.world_mut(), &mut queue, &bindings);
        assert_eq!(
            native_resource(shared_name, 1),
            (0, expected),
            "a startup holds the world"
        );
    }
}

/// A mesh built by its mirrored constructor and added through the mirrored
/// asset store operation ends up in the world's store, with no bridge code
/// that names the renderer.
#[test]
fn a_mesh_loads_through_its_mirrored_trampolines() {
    let mut engine = Engine::new();
    pill_master_renderer_data::register(&mut engine);
    let shared_name = pill_engine::asset::ASSET_MANAGER_SHARED_NAME;
    let declared = [native_resource_write(shared_name)];
    let bindings = ComponentBindings::default();
    let name = "mirrored_triangle";
    let obj_text = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    let (index, generation) = {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &declared, &bindings);
        let (status, return_buffer) = call_mirrored(
            mirrored(MESH_TYPE, "from_obj_bytes"),
            &[str_slot(name), str_slot(obj_text)],
        );
        assert_eq!(status, 0, "the OBJ decodes: {:?}", take_text(0));
        let mesh = usize::from_le_bytes(return_buffer[..8].try_into().unwrap());
        assert_ne!(mesh, 0, "the constructor returns a boxed mesh");

        let (status, assets) = native_resource(shared_name, 1);
        assert_eq!(status, 0);
        let (status, return_buffer) = call_mirrored(
            mirrored(MESH_TYPE, "__asset_add_named"),
            &[pointer_slot(assets), str_slot(name), pointer_slot(mesh)],
        );
        assert_eq!(status, 0, "the store takes the box: {:?}", take_text(0));
        (
            u32::from_le_bytes(return_buffer[..4].try_into().unwrap()),
            u32::from_le_bytes(return_buffer[4..8].try_into().unwrap()),
        )
    };

    let assets = engine
        .world()
        .get_resource::<pill_engine::AssetManager>()
        .expect("the engine inserts the store");
    let handle = assets
        .handle_by_name::<pill_master_renderer_data::Mesh>(name)
        .expect("the mesh is stored under its name");
    assert_eq!((handle.index(), handle.generation()), (index, generation));
}

/// A failed mirrored call reports status 1 and leaves its message - the Rust
/// error's text - in the last-error channel for the managed exception.
#[test]
fn a_failed_mirrored_call_reports_its_message() {
    let name = "not_a_mesh";
    let obj_text = "this is not an OBJ file";
    let (status, _) = call_mirrored(
        mirrored(MESH_TYPE, "from_obj_bytes"),
        &[str_slot(name), str_slot(obj_text)],
    );
    assert_eq!(status, 1, "an undecodable buffer fails the call");
    let message = take_text(0).expect("the failure left its message");
    assert!(
        message.contains(name),
        "the message names the asset: {message}"
    );
    assert_eq!(take_text(0), None, "taking the message clears it");
}

/// The resource's mirrored `clear` runs against the world's own manager,
/// returning the renderer to its built-in frame.
#[test]
fn the_render_pipeline_clears_through_its_mirrored_method() {
    let mut engine = Engine::new();
    pill_master_renderer_data::register(&mut engine);
    let shared_name = "pill_master_renderer::resources::rendering_manager::RenderingManager";
    let manager = engine
        .world()
        .get_resource::<pill_master_renderer_data::RenderingManager>()
        .expect("register inserts the manager");
    assert!(
        manager.pipeline().is_some(),
        "register installs the PBR chain as the default"
    );
    let declared = [native_resource_write(shared_name)];
    let bindings = ComponentBindings::default();
    {
        let _guard =
            ActiveScopeGuard::install_query_scope(engine.world_mut(), &declared, &bindings);
        let (status, manager) = native_resource(shared_name, 1);
        assert_eq!(status, 0);
        let (status, _) = call_mirrored(
            mirrored(RENDERING_MANAGER_TYPE, "clear"),
            &[pointer_slot(manager)],
        );
        assert_eq!(status, 0, "the mirrored clear succeeds");
    }

    let manager = engine
        .world()
        .get_resource::<pill_master_renderer_data::RenderingManager>()
        .expect("register inserts the manager");
    assert!(
        manager.pipeline().is_none(),
        "the pipeline handle is gone and the renderer falls back to its built-in frame"
    );
}

// =============================================================================
// Component registration
// =============================================================================

/// A module that registers a component twice - explicitly and through its
/// entry point, as the renderer data crate does - exposes it once, so its C#
/// mirror struct is emitted once.
#[test]
fn a_component_registered_twice_is_exposed_once() {
    let mut engine = Engine::new();
    let sequence = engine.world().component_registration_sequence();
    pill_master_renderer_data::register(&mut engine);
    pill_master_renderer_data::register(&mut engine);
    let registered_names = engine.world().registered_component_names_since(sequence);

    let exposed_components = exposed_components_from_names(engine.world(), &registered_names);
    let mut csharp_names: Vec<&str> = exposed_components
        .iter()
        .map(|component| component.csharp_name.as_str())
        .collect();
    let exposed_count = csharp_names.len();
    csharp_names.sort_unstable();
    csharp_names.dedup();
    assert_eq!(
        csharp_names.len(),
        exposed_count,
        "no component is exposed twice"
    );
    assert!(
        registered_names.len() > exposed_count,
        "the log itself does repeat names"
    );
}

// =============================================================================
// Parallel dispatch
// =============================================================================

/// Every index of a parallel dispatch reaches the callback, and a large
/// fan-out runs on more than one Rayon thread.
#[test]
fn parallel_dispatch_runs_every_index_across_threads() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &shared);

    let work_item_count = 4096u32;
    let visited_count = AtomicU32::new(0);
    let thread_ids = Mutex::new(HashSet::new());
    let status = parallel_for(work_item_count, |_index| {
        visited_count.fetch_add(1, Ordering::Relaxed);
        thread_ids
            .lock()
            .expect("the thread set is not poisoned")
            .insert(std::thread::current().id());
    });

    assert_eq!(status, PARALLEL_OK);
    assert_eq!(visited_count.load(Ordering::Relaxed), work_item_count);
    if pill_core::rayon::current_num_threads() > 1 {
        let distinct = thread_ids
            .lock()
            .expect("the thread set is not poisoned")
            .len();
        assert!(
            distinct >= 2,
            "expected more than one Rayon thread, saw {distinct}"
        );
    }
}

/// A dispatch without a managed scope is refused before any callback runs,
/// and a dispatch started from inside a callback is refused as nested.
#[test]
fn parallel_dispatch_refuses_off_scope_and_nested_calls() {
    // Step 1: no scope on this thread - the callback must not run at all.
    extern "C" fn count_calls(state: *mut c_void, _index: u32) {
        // SAFETY: the test passes a pointer to a live `AtomicU32` as state,
        // and the dispatch keeps it alive for the call's duration.
        let calls = unsafe { &*(state as *const AtomicU32) };
        calls.fetch_add(1, Ordering::Relaxed);
    }
    let calls = AtomicU32::new(0);
    assert_eq!(
        ffi_parallel_for(count_calls, &calls as *const AtomicU32 as *mut c_void, 4,),
        PARALLEL_NO_SCOPE
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);

    // Step 2: with a scope, a callback that dispatches again records the
    // nested refusal - on whichever thread runs it.
    extern "C" fn inner_must_not_run(_state: *mut c_void, _index: u32) {}
    extern "C" fn nested(state: *mut c_void, _index: u32) {
        // SAFETY: the test passes a pointer to a live `AtomicU8` as state,
        // and the dispatch keeps it alive until every callback finished.
        let recorded = unsafe { &*(state as *const AtomicU8) };
        let nested_status = ffi_parallel_for(inner_must_not_run, std::ptr::null_mut(), 1);
        recorded.store(nested_status, Ordering::Relaxed);
    }

    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let nested_status = AtomicU8::new(255);
    {
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &shared);
        let status = ffi_parallel_for(nested, &nested_status as *const AtomicU8 as *mut c_void, 8);
        assert_eq!(status, PARALLEL_OK);
    }
    assert_eq!(nested_status.load(Ordering::Relaxed), PARALLEL_NESTED);
}

/// Zero work items is a valid no-op dispatch.
#[test]
fn parallel_dispatch_with_zero_items_runs_nothing() {
    let mut engine = Engine::new();
    let shared = shared_component_bindings(&mut engine);
    let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &shared);

    let visited_count = AtomicU32::new(0);
    let status = parallel_for(0, |_index| {
        visited_count.fetch_add(1, Ordering::Relaxed);
    });
    assert_eq!(status, PARALLEL_OK);
    assert_eq!(visited_count.load(Ordering::Relaxed), 0);
}

/// A managed zone that is begun and never ended is closed when the invocation
/// returns. A per-frame system that forgets to dispose one therefore neither
/// grows the guard list without bound nor leaves the Tracy timeline open.
#[test]
fn an_unclosed_managed_zone_does_not_outlive_its_invocation() {
    let mut engine = Engine::new();
    let bindings = ComponentBindings::default();
    {
        let _guard = ActiveScopeGuard::install_query_scope(engine.world_mut(), &[], &bindings);
        let zone_token = begin_managed_zone_for_test("leaked");
        assert_ne!(zone_token, 0, "a zone was opened");
        assert_eq!(
            managed_zone_count_for_test(),
            1,
            "the guard is held until it is ended"
        );
        // Deliberately not ended: this is the managed-side mistake.
    }
    assert_eq!(
        managed_zone_count_for_test(),
        0,
        "the invocation boundary closed the zone it was left holding"
    );
}
