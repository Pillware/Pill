use crate::{
    ecs::{ Entity, ComponentStorage, Component, EntityHandle, ComponentDestroyer, ConcreteComponentDestroyer }
};

use indexmap::IndexMap;
use pill_core::{
    EngineError,
    PillTypeMap,
    PillTypeMapKey,
    PillSlotMap,
    get_type_name,
    create_bitmask_with_one,
    get_indices_of_set_elements
};

use anyhow::{Result, Context, Error};
use std::{ cell::RefCell, any::TypeId, slice::Iter, iter::Zip, collections::HashMap };
use log::{debug, info};
use rayon::prelude::*;

// Newtypes for sending raw ptr as safe to other threads
#[derive(Clone, Copy)]
struct SendSyncPtr<T>(*mut T);
unsafe impl<T> Send for SendSyncPtr<T> {}
unsafe impl<T> Sync for SendSyncPtr<T> {}

#[derive(Clone, Copy)]
struct SendSyncConstPtr<T>(*const T);
unsafe impl<T> Send for SendSyncConstPtr<T> {}
unsafe impl<T> Sync for SendSyncConstPtr<T> {}

pub const NEW_COMPONENT_BIT: u16 = 0b0000_0000_0000_0001;

// --- Scene ---

pub struct Scene {
    pub name: String,
    pub entities: PillSlotMap<EntityHandle, Entity>,
    pub components: PillTypeMap,

    pub scene_bitmask: u16, // Total bitmask of all components registered in scene
    pub component_bitmasks: IndexMap<TypeId, u16>, // Bitmasks for each component type

    pub component_destroyers: HashMap::<TypeId, Box::<dyn ComponentDestroyer>>,
}

impl Scene {
    pub fn new(name: String) -> Self {
        return Self {
            name,
            entities: PillSlotMap::<EntityHandle, Entity>::with_key(),
            components: PillTypeMap::new(),

            scene_bitmask: 0b0000_0000_0000_0000,
            component_bitmasks: IndexMap::new(),

            component_destroyers: HashMap::new(),
        };
    }

    // --- Components ---

    pub fn is_component_registered<T>(&self) -> bool
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.component_bitmasks.contains_key(&TypeId::of::<T>())
    }

    pub fn entity_exists(&self, entity_handle: EntityHandle) -> bool {
        self.entities.contains_key(entity_handle)
    }

    pub fn entity_has_component<T>(&self, entity_handle: EntityHandle) -> Result<bool>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        let error = Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone()));
        let entity = self.entities.get(entity_handle).ok_or(Error::new(EngineError::InvalidEntityHandle))?;
        let component_bitmask = self.component_bitmasks.get(&TypeId::of::<T>()).ok_or(error)?;

        Ok(entity.bitmask & component_bitmask > 0)
    }

    // Add component destroyer for this component type only if it is not already added
    // Component destroyer can destroy component even if its type is not known
    // (for example when removing whole entity using remove_entity function which does not take and generic parameters that will allow for determine components)
    pub fn add_component_destroyer<T>(&mut self)
        where T: Component<Storage = ComponentStorage::<T>>
    {
        let component_typeid = TypeId::of::<T>();
        let component_destroyer = ConcreteComponentDestroyer::<T>::new();
        if !self.component_destroyers.contains_key(&component_typeid) {
            self.component_destroyers.insert(component_typeid, Box::new(component_destroyer));
        }
    }

    pub fn get_component_destoyer(&self, type_id: &TypeId) -> Result<Box::<dyn ComponentDestroyer>> {
        let component_destroyer = self.component_destroyers.get(type_id).unwrap();
        Ok((*component_destroyer).clone())
    }

    // --- Storages ---

    pub fn get_component_storage<T>(&self) -> Result<&ComponentStorage<T>>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    pub fn get_component_storage_mut<T>(&mut self) -> Result<&mut ComponentStorage<T>>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get_mut::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    // --- Bitmasks ---

    pub fn add_component_bitmask<T>(&mut self)
        where T: Component<Storage = ComponentStorage::<T>>
    {
        if !self.is_component_registered::<T>() {
            // Add new component bitmask
            let component_index = self.component_bitmasks.len();
            let component_bitmask = create_bitmask_with_one(component_index as u16);
            self.component_bitmasks.insert(TypeId::of::<T>(), component_bitmask);

            // Update scene bitmask
            self.scene_bitmask = self.scene_bitmask | component_bitmask;
        }
    }

    pub fn get_component_bitmask<T>(&self) -> Result<u16>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        match self.component_bitmasks.get(&TypeId::of::<T>()) {
            Some(v) => Ok(v.clone()),
            None => Err(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone()))),
        }
    }

    pub fn get_components_typeids_from_bitmask(&self, bitmask: u16) -> Vec::<TypeId> {
        // Iterate through each entry in bitmask and get typeid of component related to it
        let mut component_typeids = Vec::<TypeId>::new();
        let component_indices = get_indices_of_set_elements(bitmask);
        for index in component_indices {
            let (typeid, bitmask) = self.component_bitmasks.get_index(index).unwrap();
            component_typeids.push(typeid.clone());
        }
        component_typeids
    }

    // -- Iterators ---
    #[inline]
    fn unsafe_mut_cast<T>(reference: &T) -> &mut T {
        #![allow(invalid_reference_casting)]
        unsafe {
            let const_ptr = reference as *const T;
            let mut_ptr = const_ptr as *mut T;
            &mut *mut_ptr
        }
    }

    pub fn get_one_component_iterator<A>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A)>>
        where
        A: Component<Storage = ComponentStorage::<A>>
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                storage_a.get(h.0.index as usize).unwrap().as_ref().unwrap(),
            )});

        Ok(iterator)
    }

    pub fn get_one_component_iterator_mut<A>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A)>>
        where
        A: Component<Storage = ComponentStorage::<A>>
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                Self::unsafe_mut_cast(storage_a).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
            )});

        Ok(iterator)
    }

    pub fn get_two_component_iterator<A, B>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A, &B)>>
        where
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()? | self.get_component_bitmask::<B>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();
        let storage_b: &Vec<Option<B>> = self.components.get::<B>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                storage_a.get(h.0.index as usize).unwrap().as_ref().unwrap(),
                storage_b.get(h.0.index as usize).unwrap().as_ref().unwrap(),
            )});

        Ok(iterator)
    }

    pub fn get_two_component_iterator_mut<A, B>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B)>>
        where
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()? | self.get_component_bitmask::<B>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();
        let storage_b: &Vec<Option<B>> = self.components.get::<B>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                Self::unsafe_mut_cast(storage_a).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
                Self::unsafe_mut_cast(storage_b).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
            )});

        Ok(iterator)
    }

    pub fn get_three_component_iterator<A, B, C>(&self) -> Result<impl Iterator<Item = (EntityHandle, &A, &B, &C)>>
        where
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>,
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()? | self.get_component_bitmask::<B>()? | self.get_component_bitmask::<C>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();
        let storage_b: &Vec<Option<B>> = self.components.get::<B>().unwrap().data.as_ref();
        let storage_c: &Vec<Option<C>> = self.components.get::<C>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                storage_a.get(h.0.index as usize).unwrap().as_ref().unwrap(),
                storage_b.get(h.0.index as usize).unwrap().as_ref().unwrap(),
                storage_c.get(h.0.index as usize).unwrap().as_ref().unwrap(),
            )});

        Ok(iterator)
    }

    pub fn get_three_component_iterator_mut<A, B, C>(&mut self) -> Result<impl Iterator<Item = (EntityHandle, &mut A, &mut B, &mut C)>>
        where
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>,
    {
        // Generate filter bitmask
        let filter_bitmask = self.get_component_bitmask::<A>()? | self.get_component_bitmask::<B>()? | self.get_component_bitmask::<C>()?;

        // Get storages
        let entities = &self.entities;
        let storage_a: &Vec<Option<A>> = self.components.get::<A>().unwrap().data.as_ref();
        let storage_b: &Vec<Option<B>> = self.components.get::<B>().unwrap().data.as_ref();
        let storage_c: &Vec<Option<C>> = self.components.get::<C>().unwrap().data.as_ref();

        // Create iterator
        let iterator = entities.iter()
            .filter(move |(_, e)| e.bitmask & filter_bitmask == filter_bitmask)
            .map(move |(h, _)|
            {(
                h,
                Self::unsafe_mut_cast(storage_a).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
                Self::unsafe_mut_cast(storage_b).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
                Self::unsafe_mut_cast(storage_c).get_mut(h.0.index as usize).unwrap().as_mut().unwrap(),
            )});

        Ok(iterator)
    }

    /// Parallel iterate (&mut A, &mut B) filtered by presence of C at same index.
    /// SAFETY: we split into non-overlapping index ranges; no aliasing across tasks.
    pub fn par_for_each2_with<A, B, C, F>(&mut self, chunk: usize, f: F) -> Result<()>
        where
            A: Component<Storage = ComponentStorage<A>> + Send,
            B: Component<Storage = ComponentStorage<B>> + Send,
            C: Component<Storage = ComponentStorage<C>> + Send + Sync,
            F: Fn(&mut A, &mut B) + Send + Sync,
    {
        let a = self.get_component_storage_mut::<A>()?;
        let b = self.get_component_storage_mut::<B>()?;
        let c = self.get_component_storage::<C>()?;

        let a_slice = a.as_mut_slice();
        let b_slice = b.as_mut_slice();
        let c_slice = c.as_slice();

        let len = a_slice.len().min(b_slice.len()).min(c_slice.len());

        // Base element pointers (not slice fat-pointers)
        let a_base = SendSyncPtr::<Option<A>>(a_slice.as_mut_ptr()); // *mut Option<A>
        let b_base = SendSyncPtr::<Option<B>>(b_slice.as_mut_ptr()); // *mut Option<B>
        let c_base = SendSyncConstPtr::<Option<C>>(c_slice.as_ptr()); // *const Option<C

        // chunk index ranges
        let ranges: Vec<(usize, usize)> = (0..len).step_by(chunk).map(|s| (s, (s + chunk).min(len))).collect();

        ranges.into_par_iter().for_each(move |(s, e)| {
            unsafe {
                let a_seg = std::slice::from_raw_parts_mut(a_base.0.add(s), e - s);
                let b_seg = std::slice::from_raw_parts_mut(b_base.0.add(s), e - s);
                let c_seg = std::slice::from_raw_parts(c_base.0.add(s), e - s);

                // Iterate aligned indices, only run when all 3 are present
                for i in 0..(e - s) {
                    if let (Some(a), Some(b), Some(_)) =
                        (a_seg[i].as_mut(), b_seg[i].as_mut(), c_seg[i].as_ref())
                    {
                        f(a, b);
                    }
                }
            }
        });

        Ok(())
    }
}
