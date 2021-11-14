pub struct Entity {
    pub name: String,
    pub index: usize,
    //pub generational_index: usize, // [TODO] Implement
    //pub parent: Option<EntityHandle>, // [TODO] Implement
    //pub children: Vec<EntityHandle>, // [TODO] Implement
}

#[derive(Clone, Copy)]
pub struct EntityHandle {
    pub index: usize,
}

impl EntityHandle {
    pub fn new(index: usize) -> Self {
	    Self { 
            index,
        }
    }
}