use super::{BitmaskController, bitmask_controller};
use crate::ecs::Component;

pub struct EntityFetcher<'a> {
    pub bitmask_controller: &'a BitmaskController,
    pub filter_bitmask: u32
}

impl<'a> EntityFetcher<'a> {

    pub fn new(controller: &'a BitmaskController) -> Self {

        EntityFetcher {
            bitmask_controller: controller,
            filter_bitmask: 0
        }
    } 

    pub fn filter_by_component<T: Component>(&mut self) -> &mut Self {
        self.filter_bitmask = self.filter_bitmask | self.bitmask_controller.get_bitmap::<T>();
        self
    }

    pub fn fetch(&self) ->  Vec<usize> {
        let mut indexes = Vec::<usize>::new();
        for i in 0..self.bitmask_controller.bitmasks.len() {
            match &self.bitmask_controller.bitmasks[i] {
                Some(bitmask) => {
                    if (bitmask.get_bitmask() & self.filter_bitmask) == self.filter_bitmask {
                        indexes.push(i as usize);
                    }
                }
                None => continue   
            }
        }
        indexes
    }
}