// ISC License (ISC)

// Copyright (c) 2016, Alex M. M. <acdenissk69@gmail.com>

// Permission to use, copy, modify, and/or distribute this software for any purpose
// with or without fee is hereby granted, provided that the above copyright notice
// and this permission notice appear in all copies.

// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH
// REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND
// FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT,
// INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS
// OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER
// TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
// THIS SOFTWARE.




// This is just typemap_rev with modified names (Value->Storage, TypeMap->ComponentMap, TypeMapKey->Component)
// https://crates.io/crates/typemap_rev




//! A hashmap whose keys are defined by types.

#![allow(dead_code)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::collections::hash_map::{
    Entry as HashMapEntry,
    OccupiedEntry as HashMapOccupiedEntry,
    VacantEntry as HashMapVacantEntry,
};
use std::marker::PhantomData;


/// Component is used to declare key types that are eligible for use
/// with [`ComponentMap`].
///
/// [`ComponentMap`]: struct.ComponentMap.html
pub trait Component: Any {
    /// Defines the value type that corresponds to this `Component`.
    type Storage: Send + Sync;
}

/// ComponentMap is a simple abstraction around the standard library's [`HashMap`]
/// type, where types are its keys. This allows for statically-checked value
/// retrieval.
///
/// [`HashMap`]: std::collections::HashMap
pub struct ComponentMap(HashMap<TypeId, Box<(dyn Any + Send + Sync)>>);

impl ComponentMap {
    /// Creates a new instance of `ComponentMap`.
    #[inline]
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    /// Returns `true` if the map contains a value for the specified [`Component`].
    ///
    /// ```rust
    /// use typemap_rev::{ComponentMap, Component};
    ///
    /// struct Number;
    ///
    /// impl Component for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = ComponentMap::new();
    /// assert!(!map.contains_key::<Number>());
    /// map.insert::<Number>(42);
    /// assert!(map.contains_key::<Number>());
    /// ```
    #[inline]
    pub fn contains_key<T>(&self) -> bool
    where
        T: Component
    {
        self.0.contains_key(&TypeId::of::<T>())
    }

    /// Inserts a new value based on its [`Component`].
    /// If the value has been already inserted, it will be overwritten
    /// with the new value.
    ///
    /// ```rust
    /// use typemap_rev::{ComponentMap, Component};
    ///
    /// struct Number;
    ///
    /// impl Component for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = ComponentMap::new();
    /// map.insert::<Number>(42);
    /// // Overwrite the value of `Number` with -42.
    /// map.insert::<Number>(-42);
    /// ```
    ///
    /// [`Component`]: trait.Component.html
    #[inline]
    pub fn insert<T>(&mut self, value: T::Storage)
    where
        T: Component
    {
        self.0.insert(TypeId::of::<T>(), Box::new(value));
    }

    /// Retrieve the entry based on its [`Component`]
    ///
    /// [`Component`]: trait.Component.html
    #[inline]
    pub fn entry<T>(&mut self) -> Entry<'_, T>
    where
        T: Component
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

    /// Retrieve a reference to a value based on its [`Component`].
    /// Returns `None` if it couldn't be found.
    ///
    /// ```rust
    /// use typemap_rev::{ComponentMap, Component};
    ///
    /// struct Number;
    ///
    /// impl Component for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = ComponentMap::new();
    /// map.insert::<Number>(42);
    ///
    /// assert_eq!(*map.get::<Number>().unwrap(), 42);
    /// ```
    ///
    /// [`Component`]: trait.Component.html
    #[inline]
    pub fn get<T>(&self) -> Option<&T::Storage>
    where
        T: Component
    {
        self.0
            .get(&TypeId::of::<T>())
            .and_then(|b| b.downcast_ref::<T::Storage>())
    }

    /// Retrieve a mutable reference to a value based on its [`Component`].
    /// Returns `None` if it couldn't be found.
    ///
    /// ```rust
    /// use typemap_rev::{ComponentMap, Component};
    ///
    /// struct Number;
    ///
    /// impl Component for Number {
    ///     type Storage = i32;
    /// }
    ///
    /// let mut map = ComponentMap::new();
    /// map.insert::<Number>(42);
    ///
    /// assert_eq!(*map.get::<Number>().unwrap(), 42);
    /// *map.get_mut::<Number>().unwrap() -= 42;
    /// assert_eq!(*map.get::<Number>().unwrap(), 0);
    /// ```
    ///
    /// [`Component`]: trait.Component.html
    #[inline]
    pub fn get_mut<T>(&mut self) -> Option<&mut T::Storage>
    where
        T: Component
    {
        self.0
            .get_mut(&TypeId::of::<T>())
            .and_then(|b| b.downcast_mut::<T::Storage>())
    }

    /// Removes a value from the map based on its [`Component`], returning the value or `None` if
    /// the key has not been in the map.
    ///
    /// ```rust
    /// use typemap_rev::{ComponentMap, Component};
    ///
    /// struct Text;
    ///
    /// impl Component for Text {
    ///     type Storage = String;
    /// }
    ///
    /// let mut map = ComponentMap::new();
    /// map.insert::<Text>(String::from("Hello ComponentMap!"));
    /// assert!(map.remove::<Text>().is_some());
    /// assert!(map.get::<Text>().is_none());
    /// ```
    #[inline]
    pub fn remove<T>(&mut self) -> Option<T::Storage>
    where
        T: Component
    {
        self.0
            .remove(&TypeId::of::<T>())
            .and_then(|b| (b as Box<dyn Any>).downcast::<T::Storage>().ok())
            .map(|b| *b)
    }
}

impl Default for ComponentMap {
    fn default() -> Self {
        Self(HashMap::default())
    }
}

/// A view into a single entry in the [`ComponentMap`],
/// which may either be vacant or occupied.
///
/// This heavily mirrors the official [`Entry`] API in the standard library,
/// but not all of it is provided due to implementation restrictions. Please
/// refer to its documentations.
///
/// [`ComponentMap`]: struct.ComponentMap.html
/// [`Entry`]: std::collections::hash_map::Entry
pub enum Entry<'a, K>
where
    K: Component,
{
    Occupied(OccupiedEntry<'a, K>),
    Vacant(VacantEntry<'a, K>),
}

impl<'a, K> Entry<'a, K>
where
    K: Component,
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
    K: Component,
    K::Storage: Default
{
    #[inline]
    pub fn or_default(self) -> &'a mut K::Storage {
        self.or_insert_with(<K::Storage as Default>::default)
    }
}

pub struct OccupiedEntry<'a, K>
where
    K: Component,
{
    entry: HashMapOccupiedEntry<'a, TypeId, Box<(dyn Any + Send + Sync)>>,
    _marker: PhantomData<&'a K::Storage>,
}

impl<'a, K> OccupiedEntry<'a, K>
where
    K: Component,
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
    K: Component,
{
    entry: HashMapVacantEntry<'a, TypeId, Box<(dyn Any + Send + Sync)>>,
    _marker: PhantomData<&'a K::Storage>,
}

impl<'a, K> VacantEntry<'a, K>
where
    K: Component,
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

    impl Component for Counter {
        type Storage = u64;
    }

    #[test]
    fn typemap_counter() {
        let mut map = ComponentMap::new();

        map.insert::<Counter>(0);

        assert_eq!(*map.get::<Counter>().unwrap(), 0);

        for _ in 0..100 {
            *map.get_mut::<Counter>().unwrap() += 1;
        }

        assert_eq!(*map.get::<Counter>().unwrap(), 100);
    }

    #[test]
    fn typemap_entry() {
        let mut map = ComponentMap::new();

        assert_eq!(map.get::<Counter>(), None);
        *map.entry::<Counter>().or_insert(0) += 42;
        assert_eq!(*map.get::<Counter>().unwrap(), 42);
    }

    struct Text;

    impl Component for Text {
        type Storage = String;
    }

    #[test]
    fn typemap_remove() {
        let mut map = ComponentMap::new();

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
    fn typemap_default() {
        fn ensure_default<T: Default>() {}

        ensure_default::<ComponentMap>();

        let map = ComponentMap::default();
        assert!(map.get::<Text>().is_none());
    }
}
