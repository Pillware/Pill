pub struct ComponentStorage<T> {
    pub data: Vec<T>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        return Self { 
            data: Vec::<T>::new(),
        };
    }

    
}