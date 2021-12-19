use pill_core::na::Storage;
use super::{BitmaskMap, Component, EntityHandle, bitmask_map};

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

}

pub struct BitmaskController {
    pub bitmasks: Vec<Bitmask>,
    filter_bitmask: u32,
    mapping: BitmaskMap,
    count: u32
}

impl BitmaskController {
    pub fn new() -> Self {
        Self {
            mapping: BitmaskMap::new(),
            count: 0b0000_0000_0000_0000_0000_0000_0000_0001,
            bitmasks: Vec::<Bitmask>::new(),
            filter_bitmask: 0
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter_bitmask = 0;
    }

    pub fn get_bitmasks(&self) -> &Vec::<Bitmask> {
        &self.bitmasks
    }

    pub fn get_mut_bitmasks(&mut self) -> &mut Vec::<Bitmask> {
        &mut self.bitmasks
    }

    pub fn add_bitmap<T: Component>(&mut self) {
        if self.mapping.contains_component::<T>() == false {
            self.mapping.insert::<T>(self.count.clone());
            self.count = self.count << 1;
        }
    }

    pub fn get_bitmap<T: Component>(&self) -> &u32 {
        if self.mapping.contains_component::<T>() {
            &self.mapping.get_bitmask::<T>()
        }
        else {
            &0
        }
    }

    pub fn get_entity_bitmask(&self, index: usize) -> &u32 {
        &self.bitmasks[index].get_bitmask()
    }

    pub fn get_mut_entity_bitmask(&mut self, index: usize) -> &mut u32 {
        self.bitmasks[index].get_mut_bitmask()
    }

    pub fn add_new_entity_bitmask(&mut self, bits: u32, index: usize) {
        let new_bitmask = Bitmask::new(bits);
        if index >= self.bitmasks.len() {
            self.bitmasks.insert(index, new_bitmask)
        }
        else {
            self.bitmasks[index] = new_bitmask;
        }
    }

    pub fn update_after_component_insertion<T : Component>(&mut self, id: usize) {
        self.bitmasks[id].apply_or_operator(*self.mapping.get_bitmask::<T>());
    }

    pub fn apply_or_operator_to_binary<T: Component>(&self, bits: u32) -> u32 {
        let new_bits = bits | self.get_bitmap::<T>();
        new_bits
    }

    pub fn add_bitmask_filter_to_binary<T: Component>(&self, bits: u32) -> u32 {
        let new_bits = self.apply_or_operator_to_binary::<T>(bits);
        new_bits
    }

    pub fn filter_by_component<T: Component>(&mut self) -> &mut Self {
        self.filter_bitmask = self.filter_bitmask | self.get_bitmap::<T>();
        self
    }

    pub fn get_filtering_bitmask(&mut self) -> u32 {
        let bitmask = self.filter_bitmask.clone();
        self.clear_filter();
        bitmask
    }

    pub fn fetch_indexes(&mut self) -> Vec<usize> {
        let mut indexes = Vec::<usize>::new();
        for i in 0..self.bitmasks.len() {
            if (self.bitmasks[i].get_bitmask() & self.filter_bitmask) == self.filter_bitmask {
                indexes.push(i as usize);
            }
        }
        self.clear_filter();
        indexes
    }

}


#[cfg(test)]
mod test {
    use crate::ecs::{Component, ComponentStorage, MeshRenderingComponent};

    use super::BitmaskController;

    struct HealthComponent(u32);
    impl Component for HealthComponent {type Storage = ComponentStorage<Self> ;}
    struct NameComponent(String);
    impl Component for NameComponent {type Storage = ComponentStorage<Self> ;}

    #[test]
    fn test_bitmask_set() {

        let mut controller = BitmaskController::new();
        controller.add_bitmap::<HealthComponent>();
        controller.add_bitmap::<NameComponent>();

        assert_eq!(controller.get_bitmap::<NameComponent>(), &0b0010);
        assert_eq!(controller.get_bitmap::<HealthComponent>(), &0b0001);

        controller.add_bitmap::<MeshRenderingComponent>();
        assert_eq!(controller.get_bitmap::<MeshRenderingComponent>(), &0b0100);
        assert_eq!(controller.get_bitmap::<HealthComponent>(), &0b0001);
    }

    struct FirstStruct(u32);
    struct SecondStruct(bool, String);
    struct ThirdStruct(u32);
    struct FourthStruct(Option<bool>);

    impl Component for FirstStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for SecondStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for ThirdStruct { type Storage = ComponentStorage<Self>; }
    impl Component for FourthStruct {type Storage = ComponentStorage<Self>; }

    #[test]
    fn test_index_fetch() {
        let mut controller = BitmaskController::new();

        controller.add_bitmap::<FirstStruct>();
        controller.add_bitmap::<SecondStruct>();
        controller.add_bitmap::<ThirdStruct>();

        controller.add_new_entity_bitmask(0b0101, 0);
        controller.add_new_entity_bitmask(0b0001, 1);
        controller.add_new_entity_bitmask(0b0000, 2);
        controller.add_new_entity_bitmask(0b0101, 3);

        let filtered_indexes = controller.filter_by_component::<FirstStruct>().
                                                filter_by_component::<ThirdStruct>().
                                                fetch_indexes();

        assert_eq!(filtered_indexes.len(), 2);
        assert_eq!(filtered_indexes[0], 0);
        assert_eq!(filtered_indexes[1], 3);
    }
}