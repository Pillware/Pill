// --- Component storage ---

pub struct ComponentStorage<T> {
    pub data: Vec<Option<T>>,
}

impl<T> ComponentStorage<T> {
    pub fn new(max_entity_count: usize) -> Self {  
        // Create vector
        let mut data = Vec::<Option<T>>::with_capacity(max_entity_count);

        // Initialize it with empty values
        for _i in 0..max_entity_count {
            data.push(None);
        }

        Self { 
            data,
        }
    }
}

// --- Global component storage ---

pub struct GlobalComponentStorage<T> {
    pub data: Option<T>,
}

impl<T> GlobalComponentStorage<T> {
    pub fn new(data: T) -> Self {  
        Self { 
            data: Some(data),
        }
    }
}