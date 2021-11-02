use crate::Scene;
use super::{entity::Entity, transform_component::TransformComponent};

pub trait Component: Default {
    fn get_component_type(&self) -> String;
    fn new<'a>(scene: &'a mut Scene, entity: &Entity) -> &'a Self;
}