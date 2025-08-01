#![cfg(feature = "net")]

use anyhow::Result;
use crate::engine::Engine;
use crate::TransformComponent;
use crate::ecs::components::{
    net_components::{NetState, NetSide},
    spawn_queue_component::SpawnQueueComponent,
};

use pill_net::TrPacket;
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
        let q = engine.get_global_component_mut::<SpawnQueueComponent>()?;
        std::mem::take(&mut q.requests)
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

    for (cid, pkt) in spawn_requests.drain(..) {
        // if already known - just update transform
        let already_present = {
            let state = engine.get_global_component::<NetState>()?;
            state.entity_by_client.contains_key(&cid)
        };
        if already_present {
            continue;
        }

        log::info!("Spawning entity for cid={cid} pkt={:?}", pkt);
        let ent = engine.create_entity(scene)?;
        let mut tr = TransformComponent::from(&pkt);
        tr.position.x += rng.random_range(-2.0..=2.0);
        tr.position.z += rng.random_range(-2.0..=2.0);

        engine.add_component_to_entity(scene, ent, tr)?;

        #[cfg(feature = "rendering")]
        {
            engine.add_component_to_entity(scene, ent, MeshRenderingComponent::builder().mesh(&mesh).material(&mat).build())?;
        }

        {
            let state = engine.get_global_component_mut::<NetState>()?;
            state.entity_by_client.insert(cid, ent);
            log::info!("Spawn complete  cid={cid}  ent={:?}", ent);
        }
    }
    Ok(())
}

