#![allow(dead_code)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::{
    Entry as HashMapEntry,
    OccupiedEntry as HashMapOccupiedEntry,
    VacantEntry as HashMapVacantEntry,
};
use std::marker::PhantomData;
pub use crate::ecs::{Component, ComponentStorage};

pub struct BitmaskMap(HashMap<TypeId, u32>);

impl BitmaskMap {

    #[inline]
    pub fn new() -> Self {
        Self(HashMap::new())
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
    pub fn get_bitmask<T>(&self) -> u32
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
}

#[cfg(test)]
mod test {
    use super::*;
    struct FirstStruct(u32);
    struct SecondStruct(bool, String);
    struct ThirdStruct(FirstStruct, SecondStruct, usize, bool);
    struct FourthStruct(Option<bool>);

    impl Component for FirstStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for SecondStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for ThirdStruct { type Storage = ComponentStorage<Self>; }
    impl Component for FourthStruct {type Storage = ComponentStorage<Self>; }

    #[test]
    fn test_insert() {
        let mut bitmap = BitmaskMap::new();

        bitmap.insert::<FirstStruct>(1);
        bitmap.insert::<SecondStruct>(2);
        bitmap.insert::<ThirdStruct>(0b1111);

        assert_eq!(bitmap.0.len(), 3);
        assert_eq!(bitmap.contains_component::<FourthStruct>(), false);
        assert_eq!(bitmap.contains_component::<FirstStruct>(), true);

        bitmap.insert::<FourthStruct>(0b0000_0000_0000_0000_0000_0000_0000_0001);

        assert_eq!(bitmap.0.len(), 4);
        assert_eq!(bitmap.contains_component::<FourthStruct>(), true);
        assert_eq!(bitmap.contains_component::<FirstStruct>(), true);
    }

    #[test]
    fn test_bitmask_fetch() {
        let mut bitmap = BitmaskMap::new();

        bitmap.insert::<FirstStruct>(0b0001);
        bitmap.insert::<SecondStruct>(0b0010);
        bitmap.insert::<ThirdStruct>(0b0011);
        bitmap.insert::<FourthStruct>(0b0100);

        assert_eq!(bitmap.0.len(), 4);
        // assert_eq!(bitmap.get_bitmask::<FirstStruct>(), &0b001);
        // assert_eq!(bitmap.get_bitmask::<SecondStruct>(), &2);
        // assert_eq!(bitmap.get_bitmask::<ThirdStruct>(), &3);
        // assert_eq!(bitmap.get_bitmask::<FourthStruct>(), &4);
    }
}