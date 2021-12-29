use std::{collections::HashMap};

use indexmap::IndexMap;
use pill_core::PillSlotMapKey;

use super::Resource;

pub struct ResourceStorage<T: Resource> {
    pub data: pill_core::PillSlotMap<T::Handle, Option<T>>, 
    pub mapping: pill_core::PillTwinMap<String, T::Handle>, // Mapping from resource name to resource handle and vice versa
}

impl<T: Resource> ResourceStorage<T> {
    pub fn new() -> Self {  
        return Self { 
            data: pill_core::PillSlotMap::<T::Handle, Option<T>>::with_key(),
            mapping: pill_core::PillTwinMap::<String, T::Handle>::new(),
        };
    }
}