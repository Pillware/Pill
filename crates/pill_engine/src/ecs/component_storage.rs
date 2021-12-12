pub struct ComponentStorage<T> {
    pub data: Vec<Option<T>>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  

        // Create vector and initialize it with empty values
        let capacity = 100;
        let mut data = Vec::<Option<T>>::with_capacity(capacity);
        for _i in 0..capacity {
            data.push(None);
        }

        return Self { 
            data,
        };
    }
}