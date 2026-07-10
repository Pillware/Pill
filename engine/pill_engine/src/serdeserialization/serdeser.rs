use std::{
    any::{type_name, Any, TypeId},
    collections::HashMap,
    path::Path,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    ecs::{EntityHandle, TransformComponent, UuidComponent},
    serdeserialization::SerdeserBackend,
    Component,
};
use pill_core::{get_type_name, EngineError, ErrorContext, Result};
use uuid::Uuid;
// Here we register everything that is worthy being serialized, then appropriate backend is chosen
// for the actual serialization

pub type ComponentPayload = serde_json::Value;

#[derive(Serialize, Deserialize)]
pub struct SerializedScene {
    pub entities: Vec<SerializedEntity>,
}

#[derive(Serialize, Deserialize)]
pub struct SerializedEntity {
    pub uuid: u128,
    pub components: Vec<SerializedComponent>,
}

#[derive(Serialize, Deserialize)]
pub struct SerializedComponent {
    pub key: String,
    pub payload: ComponentPayload,
}

pub struct DecodedComponent {
    pub type_id: TypeId,
    pub value: Box<dyn Any>,
}

pub struct DecodedEntity {
    pub uuid: u128, // redundant unless be spawn it here
    pub components: Vec<DecodedComponent>,
}

pub type ComponentSerializeFn = fn(component: &dyn Any) -> Result<ComponentPayload>;
pub type ComponentDeserializeFn = fn(payload: ComponentPayload) -> Result<Box<dyn Any>>;

pub struct ComponentSerdeDescriptor {
    pub key: String,
    pub serialize: ComponentSerializeFn,
    pub deserialize: ComponentDeserializeFn,
}

pub struct Serdeser {
    pub by_type_id: HashMap<TypeId, ComponentSerdeDescriptor>,
    pub type_id_by_key: HashMap<String, TypeId>,
}

fn serialize_with_serde<C>(component: &dyn Any) -> Result<ComponentPayload>
where
    C: Component + Serialize + 'static,
{
    let component = component.downcast_ref::<C>().unwrap(); // TODO: error handling?
    let value = serde_json::to_value(component)?;
    Ok(value)
}

fn deserialize_with_serde<C>(payload: ComponentPayload) -> Result<Box<dyn Any>>
where
    C: Component + DeserializeOwned + 'static,
{
    Ok(Box::new(serde_json::from_value::<C>(payload)?))
}

impl Serdeser {
    pub fn new() -> Serdeser {
        Serdeser {
            by_type_id: HashMap::new(),
            type_id_by_key: HashMap::new(),
        }
    }

    pub fn register_component<C>(&mut self) -> Result<()>
    where
        C: Component + Serialize + DeserializeOwned + 'static,
    {
        let type_id = TypeId::of::<C>();

        let descriptor = ComponentSerdeDescriptor {
            key: get_type_name::<C>()
                .strip_suffix("Component")
                .unwrap()
                .to_string(),
            serialize: serialize_with_serde::<C>,
            deserialize: deserialize_with_serde::<C>,
        };

        self.type_id_by_key
            .insert(descriptor.key.to_string(), type_id); // TODO: maybe use
                                                          // &'static str?
        self.by_type_id.insert(type_id, descriptor);
        Ok(())
    }

    // TODO: hardcoded entities - in the final implementation we need to have the handle to the world snapshot
    pub fn serialize<'a>(
        &self,
        entities: impl Iterator<Item = (EntityHandle, &'a UuidComponent, &'a TransformComponent)>,
        backend: &mut dyn SerdeserBackend,
        file: &Path,
    ) -> Result<()> {
        // dump engine/scene config options
        // go through all entities in scene, dump them, mapping them on the fly into serdes types
        // TODO: how to do that efficiently?
        // in current ECS iterate all entities that have transform components for now
        // TODO: we later want to serialize the knowledge that an entity contains some components
        // and recreate them (this will be the archetype in the ECS - for now ignore)

        let mut scene = SerializedScene {
            entities: Vec::new(),
        };
        for (eh, uuid, transform) in entities {
            // handle the componentId specially - write it as the entities ID
            let mut entity = SerializedEntity {
                uuid: uuid.uuid,
                components: Vec::new(),
            };

            if let Some(transform_descriptor) =
                self.by_type_id.get(&TypeId::of::<TransformComponent>())
            {
                let serialized_transform = SerializedComponent {
                    key: transform_descriptor.key.clone(),
                    payload: (transform_descriptor.serialize)(transform)?,
                };

                entity.components.push(serialized_transform);
            };

            scene.entities.push(entity);
        }
        backend.write_scene(file, &scene)?;
        Ok(())
    }

    // TODO: temporarily we have a spawn function injected that we will use to spawn a decoded
    // entity
    pub fn deserialize(
        &self,
        backend: &mut dyn SerdeserBackend,
        file: &Path,
    ) -> Result<Vec<DecodedEntity>> {
        let scene = backend.read_scene(file)?;
        let mut decoded = Vec::with_capacity(scene.entities.len());

        for entity in scene.entities {
            let mut components = Vec::with_capacity(entity.components.len());
            for component in entity.components {
                let type_id = *self.type_id_by_key.get(component.key.as_str()).ok_or_else(
                    || -> pill_core::PillError {
                        EngineError::Other(format!(
                            "Unknown serialized component key: {}",
                            component.key
                        ))
                        .into()
                    },
                )?;
                let descriptor =
                    self.by_type_id
                        .get(&type_id)
                        .ok_or_else(|| -> pill_core::PillError {
                            EngineError::Other(format!(
                                "Deserializer registry is inconsistent for component key: {}",
                                component.key
                            ))
                            .into()
                        })?;

                let decoded_component = DecodedComponent {
                    type_id,
                    value: (descriptor.deserialize)(component.payload)?,
                };
                components.push(decoded_component);
            }

            decoded.push(DecodedEntity {
                uuid: entity.uuid,
                components,
            });
        }

        Ok(decoded)
    }
}
