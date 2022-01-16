pub use crate::ecs::{ Component, ComponentStorage };

use std::any::{Any, TypeId};
use std::collections::HashMap;

pub struct BitmaskMap(pub(crate) HashMap<TypeId, u32>, pub(crate) u32);

impl BitmaskMap {
    pub fn new() -> Self {
        Self(HashMap::new(), 0b0000_0000_0000_0000_0000_0000_0000_0001)
    }

    pub fn contains_component<T: Component>(&self) -> bool {
        self.0.contains_key(&TypeId::of::<T>())
    }

    pub fn insert<T: Component>(&mut self, bitmask: u32) {
        self.0.insert(TypeId::of::<T>(), bitmask);
    }

    pub fn get_bitmask<T: Component>(&self) -> u32 {
        match self.contains_component::<T>() {
            true => self.0.get(&TypeId::of::<T>()).unwrap().clone(),
            false => 0,
        }
    }

    pub fn add_bitmask<T: Component>(&mut self) {
        if !self.contains_component::<T>() {
            self.insert::<T>(self.1.clone());
            self.1 = self.1 << 1;
        }
    }
}