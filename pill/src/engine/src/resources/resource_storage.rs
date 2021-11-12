use std::{collections::HashMap};

pub struct ResourceStorage<T> {
    pub data: HashMap<String, Box<T>>,
}

impl<T> ResourceStorage<T> {
    pub fn new() -> Self {  
        return Self { 
            data: HashMap::<String, Box<T>>::new(),
        };
    }
}