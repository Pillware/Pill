pub use crate::ecs::{Component, ComponentStorage};

use std::any::{Any, TypeId};
use std::collections::HashMap;

pub struct BitmaskMap(pub(crate) HashMap<TypeId, u32>, pub(crate) u32);

impl BitmaskMap {

    #[inline]
    pub fn new() -> Self {
        Self(HashMap::new(), 0b0000_0000_0000_0000_0000_0000_0000_0001)
    }

    #[inline]
    pub fn contains_component<T>(&self) -> bool 
    where
        T: Component
    {
        self.0.contains_key(&TypeId::of::<T>())
    }

    #[inline]
    pub fn insert<T>(&mut self, bitmask: u32)
    where
        T: Component
    {
        self.0.insert(TypeId::of::<T>(), bitmask);
    }

    #[inline]
    pub fn get_bitmap<T>(&self) -> u32
    where
        T : Component 
        {
            if self.contains_component::<T>() {
                self.0.get(&TypeId::of::<T>()).unwrap().clone()
            }
            else {
                0
            }
        }

    #[inline]
    pub fn add_bitmap<T: Component>(&mut self) {
        if self.contains_component::<T>() == false {
            self.insert::<T>(self.1.clone());
            self.1 = self.1 << 1;
        }
    }

}