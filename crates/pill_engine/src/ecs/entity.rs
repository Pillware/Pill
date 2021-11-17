#[derive(Clone, Copy)]
pub struct EntityHandle {
    //pub name: String,
    pub index: usize,
    pub generation: u64,
    //pub parent: Option<EntityHandle>, // [TODO] Implement
    //pub children: Vec<EntityHandle>, // [TODO] Implement
}

impl EntityHandle {
    pub fn new(id: usize, gen: u64) -> Self {
	    Self { 
            index: id,
            generation: gen
        }
    }

    pub fn get_generation(&self) -> u64 {
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