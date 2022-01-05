use std::cell::RefCell;

// --- Component storage ---

pub struct ComponentStorage<T> {
    pub data: Vec<RefCell<Option<T>>>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        // Create vector
        let capacity = 100;
        let mut data = Vec::<RefCell<Option<T>>>::with_capacity(capacity);

        // Initialize it with empty values
        for _i in 0..capacity {
            data.push(RefCell::new(None));
        }

        Self { 
            data,
        }
    }
}

// --- Global component storage ---

pub struct GlobalComponentStorage<T> {
    pub data: T,
}

impl<T> GlobalComponentStorage<T> {
    pub fn new(data: T) -> Self {  
        Self { 
            data,
        }
    }
}