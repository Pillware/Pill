use std::collections::VecDeque;

use pill_core::PillSlotMapKey;

use crate::ecs::{ Component, SceneManager, scene_manager, SceneHandle, EntityHandle};

pub struct EntityFetcher<'a> {
    pub scene_manager: &'a SceneManager,
    pub scene_handle: SceneHandle,
    pub filter_bitmask: u32
}

impl<'a> EntityFetcher<'a> {

    pub fn new(scene_manager: &'a SceneManager, scene_handle: SceneHandle) -> Self {

        EntityFetcher {
            scene_manager,
            scene_handle,
            filter_bitmask: 0
        }
    } 

    pub fn filter_by_component<T: Component>(&mut self) -> &mut Self {
        let target_scene = self.scene_manager.get_scene(self.scene_handle).unwrap();
        self.filter_bitmask = self.filter_bitmask | target_scene.get_bitmask_controller().get_bitmap::<T>();
        self
    }

    pub fn fetch_indexes(&self) -> Vec<usize> 
    {
        let mut indexes = Vec::<usize>::new();
        for (entity_handle, entity) in self.scene_manager.get_scene(self.scene_handle).unwrap().entities.iter() {
            if (entity.bitmask & self.filter_bitmask) == self.filter_bitmask {
                indexes.push(entity_handle.data().index as usize);
            }
        }
        indexes    
    }

    pub fn fetch_entities(&self) ->  VecDeque<EntityHandle> {
        let mut entities = VecDeque::<EntityHandle>::new();
        for (entity_handle, entity) in self.scene_manager.get_scene(self.scene_handle).unwrap().entities.iter() {
            if (entity.bitmask & self.filter_bitmask) == self.filter_bitmask {
                entities.push_back(entity_handle.clone());
             }
        }
        entities    
        // let mut indexes = Vec::<usize>::new();
        // for i in 0..self.bitmask_controller.bitmasks.len() {
        //     match &self.bitmask_controller.bitmasks[i] {
        //         Some(bitmask) => {
        //             if (bitmask.get_bitmask() & self.filter_bitmask) == self.filter_bitmask {
        //                 indexes.push(i as usize);
        //             }
        //         }
        //         None => continue   
        //     }
        // }
        // indexes
    }

    pub fn fetch_entities_and_indexes(&self) -> (VecDeque<EntityHandle>, Vec<usize>) {
        unsafe 
        {
            let mut indexes = Vec::<usize>::new();
            let mut entities = VecDeque::<EntityHandle>::new();
            for (entity_handle, entity) in self.scene_manager.get_scene(self.scene_handle).unwrap().entities.iter() {
                if (entity.bitmask & self.filter_bitmask) == self.filter_bitmask {
                    indexes.push(entity_handle.clone().get_data().index as usize);
                    entities.push_back(entity_handle.clone());
                }
            }
            (entities, indexes)    
        }
    }
}