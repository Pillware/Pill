use crate::ecs::{BitmaskMap, Component, EntityHandle, bitmask_map, EntityFetcher};

pub struct Bitmask {
    bitmask: u32
}

impl Bitmask {
    pub fn new(bits: u32) -> Self {
        Self {
            bitmask: bits
        }
    }

    pub fn get_bitmask(&self) -> &u32 {
        &self.bitmask
    }

    pub fn get_mut_bitmask(&mut self) -> &mut u32 {
        &mut self.bitmask
    }

    pub fn apply_and_operator(&mut self, bits: u32) {
        self.bitmask = self.bitmask & bits;
    }

    pub fn apply_or_operator(&mut self, bits: u32) {
        self.bitmask = self.bitmask | bits;
    }

    pub fn apply_minus_operator(&mut self, bits: u32) {
        self.bitmask = self.bitmask - bits;
    }

}

pub struct BitmaskController {
    pub(crate) mapping: BitmaskMap,
    pub(crate) count: u32
}

impl BitmaskController {
    pub fn new() -> Self {
        Self {
            mapping: BitmaskMap::new(),
            count: 0b0000_0000_0000_0000_0000_0000_0000_0001,
        }
    }

    pub fn add_bitmap<T: Component>(&mut self) {
        if self.mapping.contains_component::<T>() == false {
            self.mapping.insert::<T>(self.count.clone());
            self.count = self.count << 1;
        }
    }

    pub fn get_bitmap<T: Component>(&self) -> u32 {
        if self.mapping.contains_component::<T>() {
            self.mapping.get_bitmask::<T>()
        }
        else {
            0
        }
    }
}