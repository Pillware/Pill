#![cfg(feature = "net")]

use anyhow::Result;

use crate::engine::Engine;
use crate::ecs::{EntityHandle, TransformComponent, NetworkStateComponent};

pub enum Action {
    Spawn,
    Despawn,
    Update,
}

pub struct EntityUpdate {
    pub action: Action,
    pub entity_handle: EntityHandle,
    pub transform: Option<TransformComponent>,
}

pub struct NetworkUpdatePayload {
    pub client_id: u64,
    pub updates: Vec<EntityUpdate>,
}

// TODO: implement
pub fn network_component_update_system(engine: &mut Engine) -> Result<()> {
    // TODO: optimization -> can have a global array with indices of entities that have changed
    // the ids are removed when the interpolation has finished etc
    // 3 operations:
    // // this is a first iterator
    // 1. Iterate over all entities that have changed (E.g. Transform + Health +
    //    NetworkStateComponent (only these that are networking)
    // creates network payloads with if dirty (flag that the component has changed)
    // 2. Send the payloads to the server
    //   Flags the components as not dirty
    // 3. Receives the payloads with EntityHandles
    //   Finds NetworkStateComponent by the entity handle
    //   updates the target fields (e.g. TransformComponent)
    //  // this is the second iterator
    // 4. Now it does the interpolation of the entity it received
    //  checks the current transform and the target transform and steps

    let mut entity_updates: Vec<EntityUpdate> = Vec::new();


    // TODO: this will be different -> we will have a global array of pairs of (EntityHandle,
    // componentType) from which we will get the components to update over the network
    for (eh, transform, network_state) in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {

        if network_state.dirty {
            entity_updates.push(EntityUpdate {
                action: Action::Update,
                entity_handle: eh.clone(),
                transform: Some(transform.clone()),
            });
            network_state.dirty = false; // Reset dirty flag after processing
        }
    }

    Ok(())
}
