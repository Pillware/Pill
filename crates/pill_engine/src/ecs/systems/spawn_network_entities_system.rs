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
    if spawn_requests.is_empty() { return Ok(()); }

    let state = engine.get_global_component_mut::<NetState>();

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
                m.set_color("Tint", Vector3::new(1., 1., 1.));
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

        let ent = engine.create_entity(scene)?;

        engine.add_component_to_entity(scene, ent, TransformComponent::from(&pkt))?;

        #[cfg(feature = "rendering")]
        {
            engine.add_component_to_entity(scene, ent, MeshRenderingComponent::builder().mesh(&mesh).material(&mat).build())?;
        }

        {
            let state = engine.get_global_component_mut::<NetState>()?;
            state.entity_by_client.insert(cid, ent);
        }
        log::info!("Spawned entity for client {cid}");
    }
    Ok(())
}

