pub struct Entity {
    pub name: String,
    pub index: usize,
    //pub generational_index: usize, // [TODO] Implement
    //pub parent: Option<EntityHandle>, // [TODO] Implement
    //pub children: Vec<EntityHandle>, // [TODO] Implement
}

pub struct EntityHandle {
    pub index: usize,
}