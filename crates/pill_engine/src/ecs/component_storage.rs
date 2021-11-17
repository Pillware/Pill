pub struct StorageEntry<T> {
    pub component: T,
    pub generation: u64
}

impl<T> StorageEntry<T> {
    pub fn new(comp: T, gen: u64) -> Self {
        Self {
            component: comp,
            generation: gen
        }
    }
}

pub struct ComponentStorage<T> {
    pub data: Vec<Option<StorageEntry<T>>>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        Self { 
            data: Vec::<Option<StorageEntry<T>>>::new(),
        }
    }

}