use std::{collections::HashMap};

use indexmap::IndexMap;
use pill_core::PillSlotMapKey;

pub struct ResourceStorage<H: PillSlotMapKey, T> {
    pub data: pill_core::PillSlotMap<H, T>, // IndexMap<String, T>, // HashMap<String, Box<T>>,
}

impl<H: PillSlotMapKey, T> ResourceStorage<H, T> {
    pub fn new() -> Self {  
        return Self { 
            data: pill_core::PillSlotMap::<H, T>::with_key(), // IndexMap::<String, T>::new(), // HashMap::<String, Box<T>>::new(),
        };
    }
}