use crate::{resources::Resource, config::RESOURCE_VERSION_LIMIT};

use pill_core::{ PillSlotMap, PillSlotMapKey, PillTwinMap };

use std::collections::HashMap;


pub struct ResourceStorage<T: Resource> {
    pub data: PillSlotMap<T::Handle, Option<T>>, 
    pub mapping: PillTwinMap<String, T::Handle>,
}

impl<T: Resource> ResourceStorage<T> {
    pub fn new(max_resource_count: usize) -> Self {  
        Self { 
            data: PillSlotMap::<T::Handle, Option<T>>::with_capacity_and_key_and_version_limit(max_resource_count, RESOURCE_VERSION_LIMIT as u32).unwrap(),
            mapping: PillTwinMap::<String, T::Handle>::new(),
        }
    }
}