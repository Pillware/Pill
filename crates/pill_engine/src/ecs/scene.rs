
use crate::engine::{ EngineError };
use crate::ecs::*;

// --------- SceneHandle

#[derive(Clone, Copy)]
pub struct SceneHandle {
    pub index: usize,
}


impl SceneHandle {
    pub fn new(index: usize) -> Self {
	    Self { 
            index,
        }
    }
}

// --------- Scene

pub struct Scene {

    // General
    pub name: String,

    // ECS
    entity_counter: usize,
    entities: Vec<Entity>,
    pub(crate) components: ComponentMap,
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: Vec::<Entity>::new(),
            components: ComponentMap::new(),
        };
    }

    pub fn create_entity(&mut self) -> Result<EntityHandle, EngineError> {
        let entity = Entity { 
            name: String::from("Hello"),
            index: self.entity_counter,   
        };
        self.entities.insert( self.entity_counter, entity);
        self.entity_counter += 1;

        let entity = EntityHandle { index: self.entity_counter - 1 };
        Ok(entity)
    }
    
    #[cfg(feature = "game")]
    pub fn get_counter(&mut self) -> &usize {
        &self.entity_counter
    }

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self) {
        let storage = ComponentStorage::<T>::new();
        self.components.insert::<T>(storage);
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, entity: EntityHandle, component: T)
    {
        let storage = self.get_component_storage_mut::<T>();
        storage.data.insert(entity.index, component);
    }

    pub fn get_component_storage<T: Component<Storage = ComponentStorage::<T>>>(&self) -> &ComponentStorage<T> {
        self.components.get::<T>().unwrap()
    }

    fn get_component_storage_mut<T: Component<Storage = ComponentStorage::<T>>>(&mut self) -> &mut ComponentStorage<T> {
        self.components.get_mut::<T>().unwrap()
    }

  
}
