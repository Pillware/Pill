use std::any::{Any, TypeId};
use std::collections::HashMap;

// ---------- Upcast adapter (user supplies per T) ----------
pub struct TraitAccessor<T, Trait: ?Sized> {
    pub up_ref: fn(&T) -> &Trait,
    pub up_mut: fn(&mut T) -> &mut Trait,
    pub up_box: fn(T) -> Box<Trait>,
}

pub trait TraitAccessible<Trait: ?Sized>: Send {
    fn get_accessor() -> TraitAccessor<Self, Trait>
    where
        Self: Sized;
}

#[macro_export]
macro_rules! impl_trait_accessible {
    (dyn $dyn:path; $($ty:ty),+ $(,)?) => {$(
        impl $crate::TraitAccessible<dyn $dyn> for $ty {
            fn get_accessor() -> $crate::TraitAccessor<Self, dyn $dyn> {
                $crate::TraitAccessor { up_ref: |v| v, up_mut: |v| v, up_box: |v| Box::new(v) }
            }
        }
    )+};
}

/* ---------- Options Vector Storage ---------- */

pub struct VectorStorage;

pub struct VectorOptionStorage<T, Trait: ?Sized> {
    pub data: Vec<Option<T>>,
    trait_accessor: TraitAccessor<T, Trait>,
}
impl<T, Trait: ?Sized> VectorOptionStorage<T, Trait> {
    pub fn new(trait_accessor: TraitAccessor<T, Trait>) -> Self { Self { data: Vec::new(), trait_accessor } }
    pub fn push(&mut self, v: T) -> usize {
        let idx = self.data.len();
        self.data.push(Some(v));
        idx
    }
    pub fn iter(&self) -> impl Iterator<Item = &T> { self.data.iter().filter_map(|o| o.as_ref()) }
    pub fn get(&self, i: usize) -> Option<&T> { self.data.get(i).and_then(|o| o.as_ref()) }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> { self.data.get_mut(i).and_then(|o| o.as_mut()) }
    pub fn take(&mut self, i: usize) -> Option<T> { self.data.get_mut(i).and_then(|o| o.take()) }
    pub fn get_dyn(&self, i: usize) -> Option<&Trait> { self.get(i).map(|v| (self.trait_accessor.up_ref)(v)) }
    pub fn get_dyn_mut(&mut self, i: usize) -> Option<&mut Trait> { 
        let up_mut = self.trait_accessor.up_mut;
        self.get_mut(i).map(|v| up_mut(v)) 
    }
    pub fn take_boxed(&mut self, i: usize) -> Option<Box<Trait>> { self.take(i).map(|v| (self.trait_accessor.up_box)(v)) }
}

pub trait TraitVectorStorage<Trait: ?Sized>: Any {
    fn len(&self) -> usize;
    fn get_dyn(&self, idx: usize) -> Option<&Trait>;
    fn get_dyn_mut(&mut self, idx: usize) -> Option<&mut Trait>;
    fn take_boxed(&mut self, idx: usize) -> Option<Box<Trait>>;
    fn as_storage_any(&self) -> &dyn Any;
    fn as_storage_any_mut(&mut self) -> &mut dyn Any;
}
impl<T: 'static, Trait: ?Sized + 'static> TraitVectorStorage<Trait> for VectorOptionStorage<T, Trait> {
    fn len(&self) -> usize { self.data.iter().filter(|o| o.is_some()).count() }
    fn get_dyn(&self, idx: usize) -> Option<&Trait> { VectorOptionStorage::<T, Trait>::get_dyn(self, idx) }
    fn get_dyn_mut(&mut self, idx: usize) -> Option<&mut Trait> { VectorOptionStorage::<T, Trait>::get_dyn_mut(self, idx) }
    fn take_boxed(&mut self, idx: usize) -> Option<Box<Trait>> { VectorOptionStorage::<T, Trait>::take_boxed(self, idx) }
    fn as_storage_any(&self) -> &dyn Any { self }
    fn as_storage_any_mut(&mut self) -> &mut dyn Any { self }
}


/* ---------- Single Option Storage ---------- */
pub struct SingleStorage;

pub struct OptionStorage<T, Trait: ?Sized> {
    pub data: Option<T>,
    trait_accessor: TraitAccessor<T, Trait>,
}
impl<T, Trait: ?Sized> OptionStorage<T, Trait> {
    pub fn new(trait_accessor: TraitAccessor<T, Trait>) -> Self { Self { data: None, trait_accessor } }
    pub fn set(&mut self, v: T) { self.data = Some(v); }
    pub fn get(&self) -> Option<&T> { self.data.as_ref() }
    pub fn get_mut(&mut self) -> Option<&mut T> { self.data.as_mut() }
    pub fn take(&mut self) -> Option<T> { self.data.take() }
    pub fn is_some(&self) -> bool { self.data.is_some() }
    pub fn get_dyn(&self) -> Option<&Trait> { self.get().map(|v| (self.trait_accessor.up_ref)(v)) }
    pub fn get_dyn_mut(&mut self) -> Option<&mut Trait> { 
        let up_mut = self.trait_accessor.up_mut;
        self.get_mut().map(|v| up_mut(v)) 
    }
    pub fn take_boxed(&mut self) -> Option<Box<Trait>> { self.take().map(|v| (self.trait_accessor.up_box)(v)) }
}

pub trait TraitSingleStorage<Trait: ?Sized>: Any + Send {
    fn is_some(&self) -> bool;
    fn get_dyn(&self) -> Option<&Trait>;
    fn get_dyn_mut(&mut self) -> Option<&mut Trait>;
    fn take_boxed(&mut self) -> Option<Box<Trait>>;
    fn as_storage_any(&self) -> &dyn Any;
    fn as_storage_any_mut(&mut self) -> &mut dyn Any;
}
impl<T: Send + 'static, Trait: ?Sized + 'static> TraitSingleStorage<Trait> for OptionStorage<T, Trait> {
    fn is_some(&self) -> bool { self.is_some() }
    fn get_dyn(&self) -> Option<&Trait> { OptionStorage::<T, Trait>::get_dyn(self) }
    fn get_dyn_mut(&mut self) -> Option<&mut Trait> { OptionStorage::<T, Trait>::get_dyn_mut(self) }
    fn take_boxed(&mut self) -> Option<Box<Trait>> { OptionStorage::<T, Trait>::take_boxed(self) }
    fn as_storage_any(&self) -> &dyn Any { self }
    fn as_storage_any_mut(&mut self) -> &mut dyn Any { self }
}


/* ---------- Storage family ---------- */


/// The family trait is generic over the **trait object** `Trait`.
/// Each impl chooses its trait trait (`dyn TraitVectorStorage<Trait>` or `dyn TraitSingleStorage<Trait>`)
/// and its typed storage (`VecOptionStorage<T, Trait>` or `OptionStorage<T, Trait>`).
pub trait StorageFamily<Trait: ?Sized + 'static> {
    type Trait: ?Sized + 'static;
    type Storage<T: Send + 'static>: 'static;

    fn make<T: Send + 'static>(trait_accessor: TraitAccessor<T, Trait>) -> Box<Self::Trait>;
    fn storage_ref<T: Send + 'static>(e: &Self::Trait) -> &Self::Storage<T>;
    fn storage_mut<T: Send + 'static>(e: &mut Self::Trait) -> &mut Self::Storage<T>;
}

impl<D: ?Sized + 'static> StorageFamily<D> for VectorStorage {
    type Trait = dyn TraitVectorStorage<D>;
    type Storage<T: Send + 'static> = VectorOptionStorage<T, D>;

    fn make<T: Send + 'static>(trait_accessor: TraitAccessor<T, D>) -> Box<Self::Trait> {
        Box::new(VectorOptionStorage::<T, D>::new(trait_accessor))
    }
    fn storage_ref<T: Send + 'static>(e: &Self::Trait) -> &Self::Storage<T> {
        e.as_storage_any().downcast_ref::<VectorOptionStorage<T, D>>().expect("wrong T for VecFamily")
    }
    fn storage_mut<T: Send + 'static>(e: &mut Self::Trait) -> &mut Self::Storage<T> {
        e.as_storage_any_mut().downcast_mut::<VectorOptionStorage<T, D>>().expect("wrong T for VecFamily")
    }
}

impl<D: ?Sized + 'static> StorageFamily<D> for SingleStorage {
    type Trait = dyn TraitSingleStorage<D>;
    type Storage<T: Send + 'static> = OptionStorage<T, D>;

    fn make<T: Send + 'static>(trait_accessor: TraitAccessor<T, D>) -> Box<Self::Trait> {
        Box::new(OptionStorage::<T, D>::new(trait_accessor))
    }
    fn storage_ref<T: Send + 'static>(e: &Self::Trait) -> &Self::Storage<T> {
        e.as_storage_any().downcast_ref::<OptionStorage<T, D>>().expect("wrong T for SingleStorage")
    }
    fn storage_mut<T: Send + 'static>(e: &mut Self::Trait) -> &mut Self::Storage<T> {
        e.as_storage_any_mut().downcast_mut::<OptionStorage<T, D>>().expect("wrong T for SingleStorage")
    }
}

/* ---------- Map ---------- */

/// A type-erased storage map that associates types with their corresponding storage instances.
/// 
/// `PillTraitTypeMap` provides a way to store and retrieve different storage types using their
/// `TypeId` as keys. The storage family `F` determines the specific storage implementation
/// (e.g., vector-based or single-value storage).
/// 
/// # Type Parameters
/// 
/// * `Trait` - The trait object type that stored items must implement
/// * `F` - The storage family that defines how items are stored
/// 
/// # Examples
/// 
/// ```rust
/// // Create a new type map
/// let mut type_map = PillTraitTypeMap::new();
/// 
/// // Register a type with its storage
/// type_map.register_type_storage::<MyComponent>();
/// 
/// // Check if a type is registered
/// assert!(type_map.is_type_storage_registered::<MyComponent>());
/// 
/// // Get typed storage reference
/// let storage = type_map.get_storage::<MyComponent>().unwrap();
/// 
/// // Get trait storage by TypeId
/// let tid = std::any::TypeId::of::<MyComponent>();
/// let trait_storage = type_map.get_trait_storage(tid).unwrap();
/// let dyn_ref = trait_storage.get_dyn().unwrap();
/// dyn_ref.some_trait_method();
/// 
/// // Get mutable access to storage
/// let mut_storage = type_map.get_storage_mut::<MyComponent>().unwrap();
/// ```
pub struct PillTraitTypeMap<Trait: ?Sized + 'static, F: StorageFamily<Trait>> {
    entries: HashMap<TypeId, Box<F::Trait>>,
}

impl<Trait: ?Sized + 'static, F: StorageFamily<Trait>> PillTraitTypeMap<Trait, F> {
    pub fn new() -> Self { Self { entries: HashMap::new() } }

    pub fn register_type_storage<T>(&mut self) -> Result<(), &'static str>
        where T: 'static + TraitAccessible<Trait>,
    {
        if self.is_type_storage_registered::<T>() {
            return Err("Storage for this type is already registered");
        }

        self.entries.insert(TypeId::of::<T>(), F::make::<T>(T::get_accessor()));
        Ok(())
    }

    pub fn is_type_storage_registered<T>(&self) -> bool
        where T: 'static + TraitAccessible<Trait>,
    {
        self.entries.contains_key(&TypeId::of::<T>())
    }

    pub fn unregister_type_storage<T>(&mut self) -> Result<(), &'static str>
        where T: 'static + TraitAccessible<Trait>,
    {
        if !self.is_type_storage_registered::<T>() {
            return Err("Storage for this type is not registered");
        }
        
        self.entries.remove(&TypeId::of::<T>());
        Ok(())
    }

    pub fn get_storage<T>(&self) -> Result<&F::Storage<T>, &'static str>
        where T: 'static + TraitAccessible<Trait>,
    {
        self.entries
            .get(&TypeId::of::<T>())
            .map(|storage| F::storage_ref::<T>(&**storage))
            .ok_or("Storage for this type is not registered")
    }

    pub fn get_storage_mut<T>(&mut self) -> Result<&mut F::Storage<T>, &'static str>
        where T: 'static + TraitAccessible<Trait>,
    {
        self.entries
            .get_mut(&TypeId::of::<T>())
            .map(|storage| F::storage_mut::<T>(&mut **storage))
            .ok_or("Storage for this type is not registered")
    }

    pub fn get_trait_storage(&self, type_id: TypeId) -> Result<&F::Trait, &'static str> {
        self.entries.get(&type_id).map(|b| &**b)
        .ok_or("Storage for this type is not registered")
    }

    pub fn get_trait_storage_mut(&mut self, type_id: TypeId) -> Result<&mut F::Trait, &'static str> {
        self.entries.get_mut(&type_id).map(|b| &mut **b)
        .ok_or("Storage for this type is not registered")
    }
}
