use crate::{Engine, Scene};
use super::{entity::{Entity, EntityHandle}, transform_component::TransformComponent};

pub trait Component {
    fn get_component_type(&self) -> String;
    fn new<'a>(scene: &'a mut Scene, entity_handle: EntityHandle) -> &'a mut Self;
}