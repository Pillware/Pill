use super::{ComponentMap, EntityHandle};

pub struct Generation {
    alive: bool,
    generation: u32
}

impl Generation {
    pub fn new(gen: u32) -> Self {
        Self {
            alive: true,
            generation: gen
        }
    }

    pub fn is_alive(&self) -> bool {
        self.alive
    }

    pub fn get_generation(&self) -> u32 {
        self.generation
    }

    pub fn get_killed(&mut self) {
        self.alive = false;
    }
}

pub struct Allocator {
    free_indexes: Vec<usize>,
    generations: Vec<Generation>,
    current_gen: u32,
    current_index: usize,
}

impl Allocator {
    pub fn new() -> Self {
        Self {
            free_indexes: Vec::<usize>::new(),
            generations: Vec::<Generation>::new(),
            current_gen: 1,
            current_index: 0
        }
    }

    pub fn get_generations_count(&self) -> usize {
        self.generations.len()
    }

    pub fn get_max_index(&self) -> &usize {
        &self.current_index
    }

    pub fn has_free_indexes(&self) -> bool {
        !self.free_indexes.is_empty()
    }

    // pub fn allocate_new_entity(&mut self) -> EntityHandle {
    //     let new_generation = Generation::new(self.current_gen);
    //     self.generations.push(new_generation);
    //     self.current_gen += 1;
    //     if self.has_free_indexes() {
    //         let new_index = self.free_indexes.pop().unwrap();
    //         EntityHandle {
    //             index: new_index,
    //             generation: self.current_gen - 1
    //         }
    //     }
    //     else {
    //         self.current_index += 1;
    //         EntityHandle {
    //             index: self.current_index - 1,
    //             generation: self.current_gen - 1
    //         }
    //     }
    // }

    // pub fn is_entity_alive(&mut self, entity: EntityHandle) -> bool {
    //     let gen_index = entity.clone().get_generation();
    //     self.generations[(gen_index - 1) as usize].is_alive()
    // }

    // pub fn deallocate_entity(&mut self, entity: EntityHandle) -> bool {
    //     self.free_indexes.push(entity.get_index());
    //     let gen_index = entity.clone().get_generation();
    //     if (gen_index as usize) < self.generations.len() {
    //         if self.generations[gen_index as usize].is_alive() {
    //             self.generations[gen_index as usize].get_killed();
    //             true
    //         }
    //         else {
    //             false
    //         }
    //     }
    //     else {
    //         false
    //     }
    // }
    
}

#[cfg(test)]
mod test {

    use super::{Generation, Allocator};

    #[test]
    fn generation_properties() {
        let mut gen = Generation::new(0);
        assert_eq!(gen.alive, true);
        assert_eq!(gen.get_generation(), 0);

        gen.get_killed();
        assert_eq!(gen.alive, false);
    }

    // #[test]
    // fn allocator_properties() {
    //     let mut allocator = Allocator::new();
    //     let first = allocator.allocate_new_entity();
    //     let second = allocator.allocate_new_entity();
    //     let third = allocator.allocate_new_entity();

    //     assert_eq!(allocator.get_generations_count(), 3 as usize);
    //     assert_eq!(allocator.free_indexes.len(), 0);

    //     allocator.deallocate_entity(first);
    //     allocator.deallocate_entity(third);

    //     assert_eq!(allocator.get_generations_count(), 3 as usize);
    //     assert_eq!(allocator.generations[0].is_alive(), false);
    //     assert_eq!(allocator.generations[1].is_alive(), true);
    //     assert_eq!(allocator.free_indexes.len(), 2);

    //     let fourth = allocator.allocate_new_entity();
    //     assert_eq!(allocator.free_indexes.len(), 1);
    //     assert_eq!(fourth.generation, 3);
    //     assert_eq!(fourth.index, 2);

    //     let fifth = allocator.allocate_new_entity();
    //     let sixth = allocator.allocate_new_entity();
    //     assert_eq!(allocator.free_indexes.len(), 0);
    //     for entry in vec![first, second, third, fourth, fifth, sixth] {
    //         println!("Entity: gen {} index {}", entry.get_generation(), entry.get_index());
    //     }
    // }
}