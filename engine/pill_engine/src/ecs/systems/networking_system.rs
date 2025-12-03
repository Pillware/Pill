//! # Networking System (engine runtime)
//!
//! This module drives PillEngine’s **replication loop**, **connection lifecycle**,
//! and basic **conflict handling** on both server and client. It glues the
//! engine’s ECS world to the thin networking façade in `pill_core::networking`.
//!
//! ## Responsibilities
//! - Pump the underlying transports every frame (client/server).
//! - Gate network work to a fixed cadence (tick) independent of frame rate.
//! - Encode/decode **entity updates** (spawn/despawn/transform).
//! - Broadcast server updates; apply local/world changes; run spawn/despawn hooks.
//! - Handle **connection state**, graceful exit, and **reconnect backoff**.
//!
//! ## Data Model
//! - **Wire:** [`pill_core::NetworkPacket`] with 1-byte [`pill_core::NetworkAction`]
//!   tag: `Update`, `Join`, `Exit`.
//! - **Payload:** [`NetworkUpdatePayload`] → a batch of [`EntityUpdate`] entries.
//! - **EntityUpdate:** action + latest [`NetworkStateComponent`] + optional
//!   authoritative [`TransformComponent`] (extend this as needed with components to replicate).
//!
//! ## Diagrams
//! ### Connect / message flow:
//!
#![cfg_attr(
    doc,
    doc = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/uml_out/connection_operation.svg"
    ))
)]
//!
//! ### Disconnect / reconnect flow:
//!
#![cfg_attr(
    doc,
    doc = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/docs/uml_out/dis_reconnection.svg"
    ))
)]
//!
//! ## Usage (high level)
//! - **Server loop:** call [`networking_system_server`] once per frame.
//! - **Client loop:** call [`networking_system_client`] once per frame.
//! Both pump the transport every frame, but only perform update work when a
//! fixed timeout elapses (see [`timeout_elapsed`]).
//!
//! See also:
//! - [`crate::ecs::components::network_manager_component::NetworkManagerComponent`]
//!   for global networking state and hooks.
//! - [`crate::ecs::components::network_state_component::NetworkStateComponent`] for entity
//! replication metadata.
//! - `pill_core::networking` for transports and wire helpers.

use anyhow::Result;
use log::debug;
use pill_core::{
    client_connect, client_disconnect, client_flush, client_get_events, client_send, client_update,
    is_not_ready, server_broadcast, server_broadcast_except, server_disconnect_client,
    server_flush, server_get_events, server_send_one, server_update, ExitNotice, NetworkAction,
    NetworkClient, NetworkPacket, Vector3f,
};

use crate::ecs::components::network_manager_component::OfflinePolicy;
use crate::ecs::components::transform_component;
use crate::ecs::{
    ClientState, ConnectionState, EntityHandle, NetworkEntityState, NetworkManagerComponent,
    NetworkSide, NetworkStateComponent, TimeComponent, TransformComponent,
};
use crate::engine::Engine;

use rand::{rng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

/// High-level action for an entity update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEntityAction {
    /// Create the entity on the receiver if missing; initialize state.
    Spawn,
    /// Remove the entity on the receiver.
    Despawn,
    /// Update existing entity state (e.g. transform).
    Update,
}

/// One entity change bundled into a network payload.
///
/// The **authoritative** transform is included on server → client updates and
/// on client → server updates for client-owned entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityUpdate {
    /// Which kind of change this is.
    pub action: NetworkEntityAction,
    /// Latest replication metadata.
    pub net_state: NetworkStateComponent,
    /// Optional snapshot (position/rotation, etc.).
    ///
    /// TODO: after Hist0r merge, convert to a vector of nettable components and
    /// inject conflict resolution/arbiter.
    pub transform: Option<TransformComponent>,
}

/// Batch of updates exchanged as the body of `NetworkAction::Update`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUpdatePayload {
    /// Origin client ID (`0` means server).
    pub client_id: u64,
    /// All entity changes bundled into this tick’s message.
    pub updates: Vec<EntityUpdate>,
    /// Sender’s time (seconds); used for logging/ordering only.
    pub timestamp: f32,
}

// ------------- CLIENT HOOKS -------------
//
/// Base smoothing rate used by [`exp_blend`].
const LERP_RATE: f32 = 0.001;

/// Exponential smoothing toward target, parameterized by [`LERP_RATE`].
///
/// For small `delta_time`, behaves ~ `LERP_RATE * delta_time`.
#[inline(always)]
fn exp_blend(delta_time: f32) -> f32 {
    1.0 - (-LERP_RATE * delta_time).exp()
}

/// Linear interpolation for `Vector3f`.
fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}

/// Heuristic: did the transform change enough to warrant sending an update?
///
/// Uses squared thresholds (≈ 0.1 units) on position/rotation.
fn changed_enough(current: &TransformComponent, previous: &TransformComponent) -> bool {
    let pos_diff = (current.position - previous.position).length();
    let rot_diff = (current.rotation - previous.rotation).length();
    pos_diff > 0.01 || rot_diff > 0.01
}

/// Default client-side interpolation for **non-owned** entities.
///
/// Reads authoritative snapshots in [`NetworkStateComponent::transform`] and
/// eases the local `TransformComponent` toward them.
fn run_client_interpolation(engine: &mut Engine) -> Result<()> {
    let my_id = engine
        .get_global_component::<NetworkManagerComponent>()?
        .my_id;
    let delta_time = engine.frame_delta_time;

    for (_, transform, net_state) in
        engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
    {
        if let Some(tr) = &net_state.transform {
            if net_state.owner_id != my_id {
                let t = exp_blend(delta_time);
                transform.set_position(lerp_vec3(transform.position, tr.position, t));
                transform.set_rotation(lerp_vec3(transform.rotation, tr.rotation, t));
                // TODO: scale?
            }
        }
    }
    Ok(())
}

/// Run a custom interpolation hook if registered; otherwise fallback to
/// [`run_client_interpolation`].
fn run_client_interpolation_hook(engine: &mut Engine) -> Result<()> {
    let transform_interpolation_hook = {
        let global_net_state = engine.get_global_component::<NetworkManagerComponent>()?;
        global_net_state.client_interpolation_hook
    };
    if let Some(hook) = transform_interpolation_hook {
        hook(engine)?;
    } else {
        run_client_interpolation(engine)?;
    }
    Ok(())
}

/// Default despawn behavior when no entity-specific handler is registered.
///
/// Removes any entity whose `NetworkStateComponent.network_entity_id` matches.
fn default_despawn_hook(engine: &mut Engine, net_state: &NetworkStateComponent) -> Result<()> {
    let mut to_despawn: Vec<EntityHandle> = Vec::new();
    for (handle, net_state_comp) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
        if net_state_comp.network_entity_id == net_state.network_entity_id {
            to_despawn.push(handle);
        }
    }
    for handle in to_despawn {
        engine.remove_entity_default_scene(handle)?;
    }
    Ok(())
}

// ------------- NETWORKING HELPERS -------------

/// Mark client as disconnected and schedule a reconnect with exponential backoff.
fn mark_disconnected(engine: &mut Engine, reason: &str) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    if network_manager.is_client() {
        if let Some(client) = network_manager.client_mut() {
            if client.connection_state == ConnectionState::Disconnected {
                return Ok(()); // already disconnected
            }
            println!("[Client] Disconnected: {reason}");
            client.connection_state = ConnectionState::Disconnected;
            client.want_reconnect = true;
            client.backoff_ms = (client.backoff_ms.saturating_mul(2)).min(2_000);
            client.next_try_s = time + (client.backoff_ms as f32 / 1000.0);
        }
    }
    Ok(())
}

/// Mark client as connected and clear backoff/reconnect intent.
fn mark_connected(engine: &mut Engine) -> Result<()> {
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    if network_manager.is_client() {
        if let Some(client) = network_manager.client_mut() {
            if client.connection_state == ConnectionState::Connected {
                return Ok(());
            }
            println!("[Client] Connected to server at {}", client.server_address);
            client.connection_state = ConnectionState::Connected;
            client.want_reconnect = false;
            client.backoff_ms = 500;
            client.next_try_s = 0.0;
        }
    }
    Ok(())
}

/// Attempt a reconnect if backoff permits; re-announce owned entities as `Spawn`.
fn client_try_reconnect(engine: &mut Engine) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    let my_id = network_manager.my_id;

    if let Some(client) = network_manager.client_mut() {
        if !(client.connection_state == ConnectionState::Disconnected && client.want_reconnect) {
            return Ok(());
        }
        if time < client.next_try_s {
            return Ok(());
        }

        println!(
            "[Client] Attempting to reconnect to server at {}...",
            client.server_address
        );
        if let Ok(new_client) = client_connect(&client.server_address, my_id) {
            println!(
                "[Client] Reconnected to server at {}",
                client.server_address
            );
            network_manager.side = NetworkSide::Client(ClientState {
                net: new_client,
                server_address: client.server_address.clone(),
                connection_state: ConnectionState::Connecting,
                want_reconnect: false,
                backoff_ms: 500,
                next_try_s: 0.0,
                world_epoch: client.world_epoch,
                connection_seq: client.connection_seq + 1,
            });
        } else {
            return Ok(());
        }
    }

    // Re-announce locally owned entities
    for (_, net_state) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
        if net_state.owner_id == my_id {
            net_state.state = NetworkEntityState::Spawn;
        }
    }

    Ok(())
}

/// Client-initiated graceful disconnect (sends `Exit`, updates state, closes).
///
/// Safe to call from UI or game logic wanting to go offline.
pub fn client_go_offline(engine: &mut Engine, reason: &str) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;

    // Notify the server
    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let Some(client) = network_manager.client_mut() {
            let msg = NetworkPacket {
                tag: NetworkAction::Exit,
                data: bincode::serialize(&ExitNotice {
                    reason: reason.into(),
                    when_ms: (time * 1000.0) as u64,
                })?,
            };
            let _ = client_send(&mut client.net, &msg);
        }
    }

    mark_disconnected(engine, reason)?;

    // Close the underlying connection
    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let Some(client) = network_manager.client_mut() {
            client_disconnect(&mut client.net)?;
        }
    }

    Ok(())
}

/// Pump the transport every frame (client or server), flush, and map transient
/// errors to a disconnect mark instead of failing hard.
///
/// Always call this once per frame to avoid timeouts even when no work is due.
fn pump_transport(engine: &mut Engine) -> Result<()> {
    let delta_time = {
        let frame_delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
        Duration::from_secs_f32(frame_delta_time.max(0.0))
    };

    let mut disconnect_reason: Option<String> = None;

    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        match &mut network_manager.side {
            NetworkSide::Client(ref mut state) => {
                if let Err(e) = client_update(&mut state.net, delta_time) {
                    if is_not_ready(&e) {
                        disconnect_reason = Some(e.to_string());
                    } else {
                        return Err(e);
                    }
                }
                if let Err(e) = client_flush(&mut state.net) {
                    if is_not_ready(&e) {
                        disconnect_reason.get_or_insert(e.to_string());
                    } else {
                        return Err(e);
                    }
                }
            }
            NetworkSide::Server(ref mut state) => {
                if let Err(e) = server_update(&mut state.net, delta_time) {
                    if !is_not_ready(&e) {
                        return Err(e);
                    }
                }
                if let Err(e) = server_flush(&mut state.net) {
                    if !is_not_ready(&e) {
                        return Err(e);
                    }
                }
            }
        }
    }

    if let Some(reason) = disconnect_reason {
        mark_disconnected(engine, &reason)?;
    }
    Ok(())
}

/// Drain network events/messages and return decoded **Update** payloads.
///
/// Also handles `Join`/`Exit` book-keeping:
/// - Server: sends existing entities to newcomers, applies exit policy.
/// - Client: marks server exit and transitions to disconnected.
///
/// Returns **only** `Update` batches; side effects handle other tags.
fn receive_updates(engine: &mut Engine) -> Result<Vec<NetworkUpdatePayload>> {
    let mut updates: Vec<NetworkUpdatePayload> = Vec::new();
    let mut join_cids: Vec<u64> = Vec::new();
    let mut exit_cids: Vec<u64> = Vec::new();

    {
        let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;

        match &mut state.side {
            NetworkSide::Client(state) => match client_get_events(&mut state.net) {
                Ok(msgs) => {
                    for msg in &msgs {
                        if msg.tag == NetworkAction::Update {
                            let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                            debug!("[Client] ◂ received pkt from srv at time {}", pkt.timestamp);
                            updates.push(pkt);
                        } else if msg.tag == NetworkAction::Exit {
                            let notice: ExitNotice =
                                bincode::deserialize(&msg.data).unwrap_or(ExitNotice {
                                    reason: "Server exit".into(),
                                    when_ms: 0,
                                });
                            println!(
                                "[Client] Server Exit: {} (t={})",
                                notice.reason, notice.when_ms
                            );
                            mark_disconnected(engine, &notice.reason)?;
                        }
                    }
                }
                Err(e) if is_not_ready(&e) => {
                    println!("[Client] ▸ not connected yet – update skipped");
                    return Ok(updates);
                }
                Err(e) => return Err(e),
            },

            NetworkSide::Server(state) => {
                for (cid, msg) in server_get_events(&mut state.net)? {
                    println!(
                        "[Server] ◂ received msg from cid={cid} with tag {:?}",
                        msg.tag
                    );
                    if msg.tag == NetworkAction::Update {
                        let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                        println!(
                            "[Server] ◂ received pkt from cid={cid} at time {}",
                            pkt.timestamp
                        );

                        // Drop the client's connection if caught cheating (ID spoofing)
                        if pkt.client_id != cid {
                            println!(
                                "[Server] Client {cid} sent mismatched client_id {}, dropping",
                                pkt.client_id
                            );
                            server_disconnect_client(&mut state.net, cid)?;
                            continue;
                        }

                        updates.push(pkt);
                    } else if msg.tag == NetworkAction::Join {
                        println!("[Server] Client {cid} JOIN");
                        join_cids.push(cid);
                    } else if msg.tag == NetworkAction::Exit {
                        println!("[Server] Client {cid} EXIT");
                        exit_cids.push(cid);
                    }
                }
            }
        }
    }

    // Server-side post-processing
    if !join_cids.is_empty() {
        send_existing_entities(engine, join_cids)?;
    }
    if !exit_cids.is_empty() {
        handle_exit_policy(engine, exit_cids)?;
    }

    Ok(updates)
}

/// For each newly joined client, send a snapshot of **all existing entities**.
///
/// Skips entities owned by the joining client (no echo).
fn send_existing_entities(engine: &mut Engine, join_cids: Vec<u64>) -> Result<()> {
    // Only if a client is fully connected (server path always true)
    let connected = {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        network_manager
            .client_mut()
            .map(|c| c.connection_state == ConnectionState::Connected)
            .unwrap_or(true)
    };
    if !connected {
        return Ok(());
    }

    for cid in join_cids {
        let mut entity_updates: Vec<EntityUpdate> = Vec::new();
        for (_, transform, net_state) in
            engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
        {
            if net_state.owner_id == cid {
                continue; // do not reannounce entities owned by the client itself
            }
            entity_updates.push(EntityUpdate {
                action: NetworkEntityAction::Spawn,
                net_state: net_state.clone(),
                transform: Some(*transform),
            });
        }

        let snapshot = NetworkUpdatePayload {
            client_id: 0, // 0 = "server"
            updates: entity_updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut network_manager.side {
            server_send_one(
                &mut state.net,
                cid,
                &NetworkPacket {
                    tag: NetworkAction::Update,
                    data: bincode::serialize(&snapshot)?,
                },
            )?;
        }

        println!(
            "[Server] → sent {} existing entities to cid={cid}",
            snapshot.updates.len()
        );
    }
    Ok(())
}

/// Apply the configured offline policy for disconnected clients.
fn handle_exit_policy(engine: &mut Engine, exit_cids: Vec<u64>) -> Result<()> {
    let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    let policy = state
        .server_mut()
        .map(|s| s.offline_policy)
        .unwrap_or(OfflinePolicy::Despawn);
    match policy {
        OfflinePolicy::Despawn => despawn_entities(engine, exit_cids)?,
        OfflinePolicy::Freeze => {
            println!("Freezing entities from disconnected clients is not implemented yet")
        }
        OfflinePolicy::AI => {
            println!("AI takeover for entities from disconnected clients is not implemented yet")
        }
    }
    Ok(())
}

/// Despawn all entities owned by `despawn_cids` and broadcast that to clients.
fn despawn_entities(engine: &mut Engine, despawn_cids: Vec<u64>) -> Result<()> {
    let set: HashSet<u64> = despawn_cids.iter().copied().collect();

    // Build the update batch
    let entity_updates: Vec<EntityUpdate> = {
        let mut entity_updates: Vec<EntityUpdate> = Vec::new();
        for (_, net_state) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
            if set.contains(&net_state.owner_id) {
                entity_updates.push(EntityUpdate {
                    action: NetworkEntityAction::Despawn,
                    net_state: net_state.clone(),
                    transform: None,
                });
            }
        }
        entity_updates
    };

    if !entity_updates.is_empty() {
        let snapshot = NetworkUpdatePayload {
            client_id: 0, // 0 = "server"
            updates: entity_updates.clone(),
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut network_manager.side {
            server_broadcast(
                &mut state.net,
                &NetworkPacket {
                    tag: NetworkAction::Update,
                    data: bincode::serialize(&snapshot)?,
                },
            )?;
        }
    }

    // Apply locally too
    for entity_update in &entity_updates {
        run_despawn_hooks(engine, entity_update)?;
    }

    println!(
        "[Server] → despawned {} entities from disconnected clients",
        entity_updates.len()
    );
    Ok(())
}

/// If the entity exists locally, apply the update and return `true`.
///
/// On server, also writes the authoritative transform into the scene.
fn run_update_for_existing_entity(
    engine: &mut Engine,
    entity_update: &EntityUpdate,
) -> Result<bool> {
    let nid = entity_update.net_state.network_entity_id;
    for (_, transform, net_state) in
        engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
    {
        if nid == net_state.network_entity_id {
            net_state.transform = entity_update.transform;
            if let Some(tr) = &entity_update.transform {
                *transform = *tr; // authoritative change
            }
            *net_state = entity_update.net_state.clone();
            return Ok(true);
        }
    }
    Ok(false)
}

/// Look up and run the **spawn handler** for this entity type (if present).
fn run_spawn_hooks(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<()> {
    let tr = entity_update.transform.unwrap_or_default();
    let spawn_fn = {
        let network_manager = engine.get_global_component::<NetworkManagerComponent>()?;
        network_manager
            .spawn_handlers
            .get(&entity_update.net_state.entity_type)
            .copied()
    };

    if let Some(spawn_fn) = spawn_fn {
        spawn_fn(engine, &entity_update.net_state, &tr)?;
    } else {
        println!(
            "No spawn handler for entity_type '{}'",
            entity_update.net_state.entity_type
        );
    }
    Ok(())
}

/// Look up and run the **despawn handler** for this entity type (if present);
/// otherwise fallback to [`default_despawn_hook`].
fn run_despawn_hooks(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<()> {
    let despawn_fn = {
        let network_manager = engine.get_global_component::<NetworkManagerComponent>()?;
        network_manager
            .despawn_handlers
            .get(&entity_update.net_state.entity_type)
            .copied()
    };

    if let Some(despawn_fn) = despawn_fn {
        despawn_fn(engine, &entity_update.net_state)?;
    } else {
        println!(
            "No despawn handler for entity_type '{}'; running default",
            entity_update.net_state.entity_type
        );
        default_despawn_hook(engine, &entity_update.net_state)?;
    }
    Ok(())
}

/// Gather client-owned entity changes and send an `Update` payload to the server.
///
/// - Sends `Spawn` once per entity when it first appears locally, then `Alive`.
/// - Sends `Update` when transform changed sufficiently since the last send.
/// - Skips work if not connected (transient errors are ignored).
fn send_client_updates(engine: &mut Engine) -> Result<()> {
    let mut updates: Vec<EntityUpdate> = Vec::new();
    let my_id = engine
        .get_global_component::<NetworkManagerComponent>()?
        .my_id;

    for (_, transform, net_state) in
        engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
    {
        if net_state.state == NetworkEntityState::Spawn {
            if net_state.owner_id != my_id {
                continue; // only announce entities we own
            }
            println!(
                "▸ Queuing for spawn: nid={:?} from cid={my_id}",
                net_state.network_entity_id
            );
            updates.push(EntityUpdate {
                action: NetworkEntityAction::Spawn,
                net_state: net_state.clone(),
                transform: Some(*transform),
            });
            net_state.state = NetworkEntityState::Alive;
            net_state.last_transform = Some(*transform);
            continue;
        }

        let need_send = match &net_state.last_transform {
            Some(previous) => changed_enough(transform, previous),
            None => true,
        };

        if need_send {
            updates.push(EntityUpdate {
                action: NetworkEntityAction::Update,
                net_state: net_state.clone(),
                transform: Some(*transform),
            });
            net_state.last_transform = Some(*transform);
        }
    }

    if updates.is_empty() {
        return Ok(());
    }

    let payload = NetworkUpdatePayload {
        client_id: my_id,
        updates,
        timestamp: engine.get_global_component::<TimeComponent>()?.time,
    };

    if let NetworkSide::Client(state) = &mut engine
        .get_global_component_mut::<NetworkManagerComponent>()?
        .side
    {
        let msg = NetworkPacket {
            tag: NetworkAction::Update,
            data: bincode::serialize(&payload)?,
        };
        match client_send(&mut state.net, &msg) {
            Ok(_) => {}
            Err(e) if is_not_ready(&e) => {
                println!("[Client] ▸ not connected yet – send skipped");
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }

    Ok(())
}

/// Fixed-rate gate for network work (independent of frame rate).
///
/// Accumulates frame time and returns `true` when at least one **network step**
/// interval has elapsed (resets the accumulator).
fn timeout_elapsed(engine: &mut Engine) -> bool {
    let delta_time = engine
        .get_global_component::<TimeComponent>()
        .map(|c| c.delta_time)
        .unwrap_or(0.0);
    let network_manager = engine
        .get_global_component_mut::<NetworkManagerComponent>()
        .unwrap();
    network_manager.accumulator += delta_time;
    if network_manager.accumulator >= network_manager.timeout {
        network_manager.accumulator = 0.0;
        return true;
    }
    false
}

// ------------- NETWORKING PUBLIC SYSTEMS -------------

/// **Server-side** networking system; call once per frame.
///
/// Steps:
/// 1. Always pump transports (avoid timeouts).
/// 2. If not time for a network tick yet → return.
/// 3. Read client updates, apply to server world, **broadcast** to other clients.
///
/// # Errors
/// Propagates transport/serialization errors that aren’t transient.
pub fn networking_system_server(engine: &mut Engine) -> Result<()> {
    // Run transport pump every tick to avoid timeouts
    pump_transport(engine)?;

    if !timeout_elapsed(engine) {
        return Ok(());
    }

    // Step 1: Receive network updates from clients and broadcast them to all clients
    match receive_updates(engine) {
        Ok(updates) => {
            println!("Got {} updates from clients", updates.len());
            // Step 2: Apply to authoritative world and rebroadcast
            for update in &updates {
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetworkEntityAction::Spawn => {
                            println!(
                                "Spawn ◂ from cid={}  nid={:?}",
                                update.client_id, entity_update.net_state.network_entity_id
                            );
                            // TODO: optional duplicate suppression on server
                            run_spawn_hooks(engine, entity_update)?;
                        }
                        NetworkEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        }
                        NetworkEntityAction::Update => {
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
                                {
                                    if entity_update.net_state.network_entity_id == net_state.network_entity_id {
                                        net_state.transform = Some(*tr);
                                        *transform = *tr; // authoritative
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                let network_manager =
                    engine.get_global_component_mut::<NetworkManagerComponent>()?;
                if let NetworkSide::Server(state) = &mut network_manager.side {
                    // Broadcast to everyone except the sender
                    server_broadcast_except(
                        &mut state.net,
                        update.client_id,
                        &NetworkPacket {
                            tag: NetworkAction::Update,
                            data: bincode::serialize(&update)?,
                        },
                    )?;
                };
            }
        }
        Err(e) => {
            println!("Failed to receive or broadcast network updates: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

/// **Client-side** networking system; call once per frame.
///
/// Steps:
/// 1. Attempt reconnects if needed.
/// 2. Always pump transports; run interpolation hook.
/// 3. Gate on fixed network cadence; then send local updates.
/// 4. Read server updates and apply locally; mark connected when any update arrives.
///
/// # Errors
/// Propagates non-transient transport/serialization errors.
pub fn networking_system_client(engine: &mut Engine) -> Result<()> {
    client_try_reconnect(engine)?;
    pump_transport(engine)?;

    run_client_interpolation_hook(engine)?;

    if !timeout_elapsed(engine) {
        return Ok(());
    }

    send_client_updates(engine)?;

    match receive_updates(engine) {
        Ok(updates) => {
            let got_updates = !updates.is_empty();

            for update in &updates {
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetworkEntityAction::Spawn => {
                            println!(
                                "Spawn ◂ from cid={}  nid={:?}",
                                update.client_id, entity_update.net_state.network_entity_id
                            );
                            if !run_update_for_existing_entity(engine, entity_update)? {
                                run_spawn_hooks(engine, entity_update)?;
                            }
                        }
                        NetworkEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        }
                        NetworkEntityAction::Update => {
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()?
                                {
                                    if entity_update.net_state.network_entity_id == net_state.network_entity_id {
                                        net_state.transform = Some(*tr);
                                        debug!(
                                            "▸ Updating nid={:?} for cid={} net_state={:?} tr={:?}",
                                            net_state.network_entity_id, update.client_id, net_state, tr
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if got_updates {
                mark_connected(engine)?;
            }
        }
        Err(e) => {
            println!("Failed to receive network updates: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
