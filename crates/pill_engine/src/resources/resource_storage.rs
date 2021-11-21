use std::{collections::HashMap};

use indexmap::IndexMap;

pub struct ResourceStorage<T> {
    pub data: IndexMap<String, T>, // HashMap<String, Box<T>>,
}

impl<T> ResourceStorage<T> {
    pub fn new() -> Self {  
        return Self { 
            data: IndexMap::<String, T>::new(), // HashMap::<String, Box<T>>::new(),
        };
    }
}