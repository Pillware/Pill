#[derive(Clone, Copy)]
pub struct EntityHandle {
    pub index: usize,
    pub generation: u32,
}

impl EntityHandle {
    pub fn new(id: usize, gen: u32) -> Self {
	    Self { 
            index: id,
            generation: gen
        }
    }

    pub fn get_generation(&self) -> u32 {
        self.generation
    }

    pub fn get_index(&self) -> usize {
        self.index
    }
}

#[cfg(test)] 
mod test {
    use super::EntityHandle;

    #[test]
    fn entity_properties() {
        let gen = 2;
        let index = 2;
        let entity = EntityHandle::new(index as usize, gen);

        assert_eq!(entity.get_generation(), 2);
        assert_eq!(entity.get_index(), 2);
    }
}