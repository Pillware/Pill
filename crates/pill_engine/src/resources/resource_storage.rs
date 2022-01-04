use crate::resources::Resource;

use pill_core::PillSlotMapKey;

use std::collections::HashMap;


pub struct ResourceStorage<T: Resource> {
    pub data: pill_core::PillSlotMap<T::Handle, Option<T>>, 
    pub mapping: pill_core::PillTwinMap<String, T::Handle>, // Mapping from resource name to resource handle and vice versa
}

impl<T: Resource> ResourceStorage<T> {
    pub fn new() -> Self {  
        let capacity = 100;
        let version_limit = 255;
        Self { 
            data: pill_core::PillSlotMap::<T::Handle, Option<T>>::with_capacity_and_key_and_version_limit(capacity, version_limit).unwrap(),
            mapping: pill_core::PillTwinMap::<String, T::Handle>::new(),
        }
    }
}