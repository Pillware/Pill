use crate::ecs::GlobalComponent;
use pill_core::{get_type_name, EngineError, PillTypeMap, PillTypeMapKey, Result};

// --- Component storage ---

pub struct ComponentStorage<T> {
    pub data: Vec<Option<T>>,
}

impl<T> ComponentStorage<T> {
    pub fn new(max_entity_count: usize) -> Self {
        // Create vector
        let mut data = Vec::<Option<T>>::with_capacity(max_entity_count);

        // Initialize it with empty values
        for _i in 0..max_entity_count {
            data.push(None);
        }

        Self { data }
    }
}

// --- Global component storage ---

pub struct GlobalComponentStorage<T> {
    pub data: Option<T>,
}

impl<T> GlobalComponentStorage<T> {
    pub fn new(data: T) -> Self {
        Self { data: Some(data) }
    }
}

/// Borrows global component `T` out of a `PillTypeMap`, erroring if absent.
/// Single source of truth for `Engine::get_global_component` and `WorldQuery::get_global` —
/// lives here because it depends on `GlobalComponentStorage`'s `data` layout.
pub fn get_global_component_from<T>(globals: &PillTypeMap) -> Result<&T>
where
    T: GlobalComponent<Storage = GlobalComponentStorage<T>> + PillTypeMapKey,
{
    globals
        .get::<T>()
        .and_then(|storage| storage.data.as_ref())
        .ok_or_else(|| -> pill_core::PillError {
            EngineError::GlobalComponentNotFound(get_type_name::<T>()).into()
        })
}
