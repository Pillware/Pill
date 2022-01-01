pill_core::define_new_pill_slotmap_key! { 
    pub struct EntityHandle;
}

pub struct Entity {
    pub(crate) bitmask: u32
}

impl Entity {
     
    pub fn new(bitmask: u32) -> Self {

        Self {
            bitmask,
        }
    }

}

impl Default for Entity {

    fn default() -> Self {
        
        Self {
            bitmask: 0
        }
    }
}
