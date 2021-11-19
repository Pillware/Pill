use std::{io::Error, ops::Index};

use pill_core::na::Storage;

use super::EntityHandle;

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

    pub fn set_generation(&mut self, gen: u64) {
        self.generation = gen;
    }

    pub fn set_component(&mut self, comp: T) {
        self.component = comp;
    }
}

pub struct ComponentStorage<T> {
    pub data: Vec<StorageEntry<T>>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        Self { 
            data: Vec::<StorageEntry<T>>::new(),
        }
    }

    pub fn set(&mut self, handle: EntityHandle, comp: T) {
        if handle.index >= self.data.len() {
            self.data.push(StorageEntry::new(comp, handle.generation.clone()))
        }
        else {
            self.data[handle.index].generation = handle.generation;
            self.data[handle.index].component = comp;
        }
    }

    pub fn get(&self, handle: EntityHandle) -> Option<&T> {
        if self.data[handle.index].generation == handle.generation {
            return Some(&self.data[handle.index].component)
        }
        else {
            return None
        }
    }

    pub fn get_mut(&mut self, handle: EntityHandle) -> Option<&mut T> {
        if self.data[handle.index].generation == handle.generation {
            return Some(&mut self.data[handle.index].component)
        }
        else {
            return None
        }
    }

    /*
    pub fn contains_entry(&self, index: usize) -> bool {
        match &self.data[index] {
            None => false,
            Some(entry) => true
        }
    }

    pub fn set(&mut self, index: usize, value: T)  {
        if self.data.len() <= index {
            let new_entry = StorageEntry::<T>::new(value, *self.get_gen(index).unwrap());
            self.data[index].replace(new_entry);
        }
    }

    pub fn set_entry(&mut self, index: usize, entry: StorageEntry<T>) {
        if self.data.len() <= index {
            self.data[index].replace(entry);
        }
    }
    
    pub fn get(&self, index: usize) -> Option<&T> {
        match self.data.get(index) {
            Some(Some(StorageEntry {
                component,
                generation
            })
            ) => Some(component),
            _ => None
        }
    }

    fn get_gen(&self, index: usize) -> Option<&u64> {
        match self.data.get(index) {
            Some(Some(StorageEntry {
                component,
                generation
            })
            ) => Some(generation),
            _ => None
        }
    }
    */
}

#[cfg(test)]
mod test {
    use std::slice::SliceIndex;

    use crate::ecs::EntityHandle;

    use super::{ComponentStorage, StorageEntry};

    #[test]
    fn basic_component_insertion() {
        let mut components = ComponentStorage::<u64>::new();

        let number: u64 = 10;
        let handle = EntityHandle::new(0, 0);

        components.set(handle, number);

        assert_eq!(Some(&10), components.get(handle));

        components.set(handle, 20);
        assert_eq!(Some(&20), components.get(handle));

        let second_handle = EntityHandle::new(0, 1);
        components.set(second_handle, 30);
        assert_eq!(None, components.get(handle));
        assert_eq!(Some(&30), components.get(second_handle));
    }

    #[test]
    fn mutable_component_test() {
        let mut components = ComponentStorage::<String>::new();

        let first = EntityHandle::new(0, 0);
        let second = EntityHandle::new(1, 1);

        components.set(first, String::from("TEST STRING"));
        assert_eq!(components.get(first), Some(&String::from("TEST STRING")));

        let new_string = components.get(first).unwrap();
        components.set(first, new_string.to_string());
    }
}