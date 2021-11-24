use pill_core::na::Storage;
use super::{BitmaskMap, bitmask_map, Component};
pub struct BitmaskController {
    mapping: BitmaskMap,
    count: u32
}

impl BitmaskController {
    pub fn new() -> Self {
        Self {
            mapping: BitmaskMap::new(),
            count: 0b0000_0000_0000_0000_0000_0000_0000_0001
        }
    }

    pub fn set_bitmap<T: Component>(&mut self) {
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
}


#[cfg(test)]
mod test {
    use crate::ecs::{HealthComponent, NameComponent, MeshRenderingComponent};

    use super::BitmaskController;

    #[test]
    fn set_bitmask() {

        let mut controller = BitmaskController::new();
        controller.set_bitmap::<HealthComponent>();
        controller.set_bitmap::<NameComponent>();

        assert_eq!(controller.get_bitmap::<NameComponent>(), &0b0010);
        assert_eq!(controller.get_bitmap::<HealthComponent>(), &0b0001);

        controller.set_bitmap::<MeshRenderingComponent>();
        assert_eq!(controller.get_bitmap::<MeshRenderingComponent>(), &0b0100);
        assert_eq!(controller.get_bitmap::<HealthComponent>(), &0b0001);
    }
}