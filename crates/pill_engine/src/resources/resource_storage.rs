use crate::resources::Resource;

use pill_core::{ PillSlotMap, PillSlotMapKey, PillTwinMap };

use std::collections::HashMap;


pub struct ResourceStorage<T: Resource> {
    pub data: PillSlotMap<T::Handle, Option<T>>, 
    pub mapping: PillTwinMap<String, T::Handle>,
}

impl<T: Resource> ResourceStorage<T> {
    pub fn new() -> Self {  
        let capacity = 100;
        let version_limit = 255;
        Self { 
            data: PillSlotMap::<T::Handle, Option<T>>::with_capacity_and_key_and_version_limit(capacity, version_limit).unwrap(),
            mapping: PillTwinMap::<String, T::Handle>::new(),
        }
    }
}