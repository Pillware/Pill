//! # NetworkStateComponent (replication metadata per entity)
//!
//! `NetworkStateComponent` describes **what to replicate** about an entity and
//! **how it participates** in the networking lifecycle. It’s attached to every
//! entity that should be visible to the networking layer (spawn/despawn/updates).
//!
//! See also:
//! - `pill_engine::ecs::components::network_manager_component::NetworkManagerComponent`
//!   (drives the replication pipeline).
//! - `pill_core::networking` (wire format, client/server wrappers).
//! - `pill_engine::ecs::systems::networking_system.rs` (networking systems implementation).

use crate::ecs::{Component, ComponentStorage, TransformComponent};

use pill_core::PillTypeMapKey;
use serde::{Deserialize, Serialize};

/// High-level lifecycle marker for a networked entity.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkEntityState {
    /// Entity should appear on the remote side (first time) with initial data.
    Spawn,
    /// Entity should be removed on the remote side.
    Despawn,
    /// Entity already exists; payload carries state changes (e.g., transform).
    Alive,
}

/// Per-entity replication metadata and latest network-relevant snapshot.
///
/// This component is meant to be **lightweight** and focused on networking.
/// Game-specific data lives in your own components; you can mirror pieces
/// here as needed for replication (e.g., position/orientation via `transform`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStateComponent {
    /// Logical **owner** of the entity (client ID). `0` is reserved for server.
    ///
    /// Ownership is used for authority decisions and for routing inputs/updates.
    pub owner_id: u64,

    /// Lifecycle state for this entity on the network (spawn/despawn/alive).
    pub state: NetworkEntityState,

    /// Unique **network entity ID** for this session/world.
    ///
    /// Semantics are engine-defined; typically allocated by the server and
    /// unique within the current world/epoch. Clients should treat it as an
    /// opaque handle when addressing entities over the wire.
    pub network_entity_id: u64,

    /// Latest transform snapshot to replicate (if applicable).
    ///
    /// Servers usually fill this each tick for moving objects; clients read it
    /// and apply/interpolate. When `None`, no transform update is sent.
    pub transform: Option<TransformComponent>,

    /// Previous transform (client-side cache) for **interpolation**.
    ///
    /// Your interpolation system can use `(last_transform, transform)` to
    /// compute smooth visuals between authoritative snapshots.
    pub last_transform: Option<TransformComponent>,

    /// Game-defined **entity type key** (e.g., `"Player"`, `"Crate"`, …).
    ///
    /// This is used to look up spawn/despawn handlers in the network manager.
    pub entity_type: String,
    // TODO: add more components (Health etc.) — either mirror here or reference by ID.
}

impl Component for NetworkStateComponent {}

impl PillTypeMapKey for NetworkStateComponent {
    type Storage = ComponentStorage<NetworkStateComponent>;
}
