
// --- PillTypeMap
// This is typemap_rev crate modified by changing names of types
// Original crate: https://crates.io/crates/typemap_rev

//! A hashmap whose keys are defined by types.


use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::{
    Entry as HashMapEntry,
    OccupiedEntry as HashMapOccupiedEntry,
    VacantEntry as HashMapVacantEntry,
};
use std::marker::PhantomData;

/// PillTypeMapKey is used to declare key types that are eligible for use
/// with [`PillTypeMap`].
///
/// [`PillTypeMap`]: struct.PillTypeMap.html
pub trait PillTypeMapKey: Any {
    /// Defines the value type that corresponds to this `PillTypeMapKey`.
    type Storage: Send; //+ Sync;
}

/// PillTypeMap is a simple abstraction around the standard library's [`HashMap`]
/// type, where types are its keys. This allows for statically-checked value
/// retrieval.
///
/// [`HashMap`]: std::collections::HashMap
//pub struct PillTypeMap(HashMap<TypeId, Box<(dyn Any + Send + Sync)>>);
pub struct PillTypeMap(HashMap<TypeId, Box<(dyn Any + Send)>>);


impl PillTypeMap {
    /// Creates a new instance of `PillTypeMap`.
    #[inline]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Returns `true` if the map contains a value for the specified [`PillTypeMapKey`].
    ///
    /// ```rust
    /// use pill_typemap::{PillTypeMap, PillTypeMapKey};
    ///
    /// struct Number;
    ///
    /// impl PillTypeMapKey for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = PillTypeMap::new();
    /// assert!(!map.contains_key::<Number>());
    /// map.insert::<Number>(42);
    /// assert!(map.contains_key::<Number>());
    /// ```
    #[inline]
    pub fn contains_key<T>(&self) -> bool
    where
        T: PillTypeMapKey
    {
        self.0.contains_key(&TypeId::of::<T>())
    }

    /// Inserts a new value based on its [`PillTypeMapKey`].
    /// If the value has been already inserted, it will be overwritten
    /// with the new value.
    ///
    /// ```rust
    /// use pill_typemap::{PillTypeMap, PillTypeMapKey};
    ///
    /// struct Number;
    ///
    /// impl PillTypeMapKey for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = PillTypeMap::new();
    /// map.insert::<Number>(42);
    /// // Overwrite the value of `Number` with -42.
    /// map.insert::<Number>(-42);
    /// ```
    ///
    /// [`PillTypeMapKey`]: trait.PillTypeMapKey.html
    #[inline]
    pub fn insert<T>(&mut self, value: T::Storage)
    where
        T: PillTypeMapKey
    {
        self.0.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Retrieve the entry based on its [`PillTypeMapKey`]
    ///
    /// [`PillTypeMapKey`]: trait.PillTypeMapKey.html
    #[inline]
    pub fn entry<T>(&mut self) -> Entry<'_, T>
    where
        T: PillTypeMapKey
    {
        match self.0.entry(TypeId::of::<T>()) {
            HashMapEntry::Occupied(entry) => Entry::Occupied(OccupiedEntry {
                entry,
                _marker: PhantomData,
            }),
            HashMapEntry::Vacant(entry) => Entry::Vacant(VacantEntry {
                entry,
                _marker: PhantomData,
            })
        }
    }

    /// Retrieve a reference to a value based on its [`PillTypeMapKey`].
    /// Returns `None` if it couldn't be found.
    ///
    /// ```rust
    /// use pill_typemap::{PillTypeMap, PillTypeMapKey};
    ///
    /// struct Number;
    ///
    /// impl PillTypeMapKey for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = PillTypeMap::new();
    /// map.insert::<Number>(42);
    ///
    /// assert_eq!(*map.get::<Number>().unwrap(), 42);
    /// ```
    ///
    /// [`PillTypeMapKey`]: trait.PillTypeMapKey.html
    #[inline]
    pub fn get<T>(&self) -> Option<&T::Storage>
    where
        T: PillTypeMapKey
    {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T::Storage>())
    }

    /// Retrieve a mutable reference to a value based on its [`PillTypeMapKey`].
    /// Returns `None` if it couldn't be found.
    ///
    /// ```rust
    /// use pill_typemap::{PillTypeMap, PillTypeMapKey};
    ///
    /// struct Number;
    ///
    /// impl PillTypeMapKey for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = PillTypeMap::new();
    /// map.insert::<Number>(42);
    ///
    /// assert_eq!(*map.get::<Number>().unwrap(), 42);
    /// *map.get_mut::<Number>().unwrap() -= 42;
    /// assert_eq!(*map.get::<Number>().unwrap(), 0);
    /// ```
    ///
    /// [`PillTypeMapKey`]: trait.PillTypeMapKey.html
    #[inline]
    pub fn get_mut<T>(&mut self) -> Option<&mut T::Storage>
    where
        T: PillTypeMapKey
    {
        self.0
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T::Storage>())
    }

    /// Removes a value from the map based on its [`PillTypeMapKey`], returning the value or `None` if
    /// the key has not been in the map.
    ///
    /// ```rust
    /// use pill_typemap::{PillTypeMap, PillTypeMapKey};
    ///
    /// struct Text;
    ///
    /// impl PillTypeMapKey for Text {
    ///     type Storage = String;
    /// }
    ///
    /// let mut map = PillTypeMap::new();
    /// map.insert::<Text>(String::from("Hello PillTypeMap!"));
    /// assert!(map.remove::<Text>().is_some());
    /// assert!(map.get::<Text>().is_none());
    /// ```
    #[inline]
    pub fn remove<T>(&mut self) -> Option<T::Storage>
    where
        T: PillTypeMapKey
    {
        self.0
            .remove(&TypeId::of::<T>())
            .and_then(|b| (b as Box<dyn Any>).downcast::<T::Storage>().ok())
            .map(|b| *b)
    }
}

impl Default for PillTypeMap {
    fn default() -> Self {
        Self(HashMap::default())
    }
}

/// A view into a single entry in the [`PillTypeMap`],
/// which may either be vacant or occupied.
///
/// This heavily mirrors the official [`Entry`] API in the standard library,
/// but not all of it is provided due to implementation restrictions. Please
/// refer to its documentations.
///
/// [`PillTypeMap`]: struct.PillTypeMap.html
/// [`Entry`]: std::collections::hash_map::Entry
pub enum Entry<'a, K>
where
    K: PillTypeMapKey,
{
    Occupied(OccupiedEntry<'a, K>),
    Vacant(VacantEntry<'a, K>),
}

impl<'a, K> Entry<'a, K>
where
    K: PillTypeMapKey,
{
    #[inline]
    pub fn or_insert(self, value: K::Storage) -> &'a mut K::Storage {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(value),
        }
    }

    #[inline]
    pub fn or_insert_with<F>(self, f: F) -> &'a mut K::Storage
    where
        F: FnOnce() -> K::Storage
    {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(f()),
        }
    }

    #[inline]
    pub fn and_modify<F>(self, f: F) -> Self
    where
        F: FnOnce(&mut K::Storage)
    {
        match self {
            Entry::Occupied(mut entry) => {
                f(entry.get_mut());
                Entry::Occupied(entry)
            },
            Entry::Vacant(entry) => Entry::Vacant(entry),
        }
    }
}

impl<'a, K> Entry<'a, K>
where
    K: PillTypeMapKey,
    K::Storage: Default
{
    #[inline]
    pub fn or_default(self) -> &'a mut K::Storage {
        self.or_insert_with(<K::Storage as Default>::default)
    }
}

pub struct OccupiedEntry<'a, K>
where
    K: PillTypeMapKey,
{
    //entry: HashMapOccupiedEntry<'a, TypeId, Box<(dyn Any + Send + Sync)>>,
    entry: HashMapOccupiedEntry<'a, TypeId, Box<(dyn Any + Send)>>,
    _marker: PhantomData<&'a K::Storage>,
}

impl<'a, K> OccupiedEntry<'a, K>
where
    K: PillTypeMapKey,
{
    #[inline]
    pub fn get(&self) -> &K::Storage {
        self.entry.get().downcast_ref().unwrap()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut K::Storage {
        self.entry.get_mut().downcast_mut().unwrap()
    }

    #[inline]
    pub fn into_mut(self) -> &'a mut K::Storage {
        self.entry.into_mut().downcast_mut().unwrap()
    }

    #[inline]
    pub fn insert(&mut self, value: K::Storage) {
        self.entry.insert(Box::new(value));
    }

    #[inline]
    pub fn remove(self) {
        self.entry.remove();
    }
}

pub struct VacantEntry<'a, K>
where
    K: PillTypeMapKey,
{
    //entry: HashMapVacantEntry<'a, TypeId, Box<(dyn Any + Send + Sync)>>,
    entry: HashMapVacantEntry<'a, TypeId, Box<(dyn Any + Send)>>,
    _marker: PhantomData<&'a K::Storage>,
}

impl<'a, K> VacantEntry<'a, K>
where
    K: PillTypeMapKey,
{
    #[inline]
    pub fn insert(self, value: K::Storage) -> &'a mut K::Storage {
        self.entry.insert(Box::new(value)).downcast_mut().unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    struct Counter;

    impl PillTypeMapKey for Counter {
        type Storage = u64;
    }

    #[test]
    fn PillTypeMap_counter() {
        let mut map = PillTypeMap::new();

        map.insert::<Counter>(0);

        assert_eq!(*map.get::<Counter>().unwrap(), 0);

        for _ in 0..100 {
            *map.get_mut::<Counter>().unwrap() += 1;
        }

        assert_eq!(*map.get::<Counter>().unwrap(), 100);
    }

    #[test]
    fn PillTypeMap_entry() {
        let mut map = PillTypeMap::new();

        assert_eq!(map.get::<Counter>(), None);
        *map.entry::<Counter>().or_insert(0) += 42;
        assert_eq!(*map.get::<Counter>().unwrap(), 42);
    }

    struct Text;

    impl PillTypeMapKey for Text {
        type Storage = String;
    }

    #[test]
    fn PillTypeMap_remove() {
        let mut map = PillTypeMap::new();

        map.insert::<Text>(String::from("foobar"));

        // This will give a &String
        assert_eq!(map.get::<Text>().unwrap(), "foobar");

        // Ensure we get an owned String back.
        let original: String = map.remove::<Text>().unwrap();
        assert_eq!(original, "foobar");

        // Ensure our String is gone from the map.
        assert!(map.get::<Text>().is_none());
    }

    #[test]
    fn PillTypeMap_default() {
        fn ensure_default<T: Default>() {}

        ensure_default::<PillTypeMap>();

        let map = PillTypeMap::default();
        assert!(map.get::<Text>().is_none());
    }
}
