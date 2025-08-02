#![cfg(feature = "net")]

use anyhow::Result;
use crate::engine::Engine;
use crate::TransformComponent;
use crate::ecs::components::{
    net_components::{NetState, NetSide},
    spawn_despawn_queue_component::SpawnDespawnQueueComponent,
    network_state_component::{NetworkStateComponent, NetEntityState},
};

use cgmath::Vector3;
use rand::{rng, Rng};

#[cfg(feature = "rendering")]
use crate::{
    ecs::{MeshRenderingComponent},
    resources::{Material, MaterialHandle, Mesh, MeshHandle},
};

#[cfg(feature = "net")]
pub fn spawn_network_entities_system(engine: &mut Engine) -> Result<()> {
    let scene = engine.get_active_scene_handle()?;

    let mut spawn_requests = {
        let q = engine.get_global_component_mut::<SpawnDespawnQueueComponent>()?;
        std::mem::take(&mut q.spawn)
    };
    if spawn_requests.is_empty() {
        log::debug!("spawn system tick – queue empty");
        return Ok(()); }

    log::info!("{} items in spawn queue", spawn_requests.len());

    let state = engine.get_global_component_mut::<NetState>();

    // randomness for capsules tint and transforms
    let mut rng = rng();

    #[cfg(feature = "rendering")]
    let (mesh, mat) = {
        // load once
        let mesh: MeshHandle = match engine.get_resource_handle::<Mesh>("Pill") {
            Ok(h) => h,
            Err(_) => engine.add_resource(Mesh::new("Pill", "./res/models/Pill.obj".into()))?,
        };
        let mat: MaterialHandle = match engine.get_resource_handle::<Material>("PillMat") {
            Ok(h) => h,
            Err(_) => {
                let mut m = Material::new("PillMat");
                m.set_color("Tint", Vector3::new(rng.random_range(0.0..=1.0), rng.random_range(0.0..=1.0), rng.random_range(0.0..=1.0)));
                engine.add_resource(m)?
            }
        };
        (mesh, mat)
    };

    for (cid, eh, mut transform) in spawn_requests.drain(..) {

        log::info!("Spawning entity for cid={cid} pkt={:?}", transform);
        let ent = engine.create_entity(scene)?; // TODO: do we need to preserve the
                                                              // entity handles?
        transform.position.x += rng.random_range(-2.0..=2.0); // TODO: is this necessary?
        transform.position.z += rng.random_range(-2.0..=2.0);
        transform.net_dirty = false; // reset net dirty flag

        // add the network state component
        engine.add_component_to_entity(scene, ent, NetworkStateComponent {
            state: NetEntityState::Alive,
            transform: None,
        })?;

        engine.add_component_to_entity(scene, ent,transform)?;

        #[cfg(feature = "rendering")]
        {
            engine.add_component_to_entity(scene, ent, MeshRenderingComponent::builder().mesh(&mesh).material(&mat).build())?;
        }

        log::info!("Spawn complete  cid={cid}  ent={:?}", ent);
    }
    Ok(())
}

