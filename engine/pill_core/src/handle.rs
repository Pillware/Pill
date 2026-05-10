// WHY over PillSlotMapKey: the renderer has 7 resource types — Handle<Tag> gives all 7 for free; PillSlotMapKey needs a macro block + unsafe impl written out per type.

use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;

#[repr(transparent)]
pub struct Handle<T> {
    raw: u64,
    _marker: PhantomData<T>,
}

impl<T> Handle<T> {
    /// Constructs a handle from a raw slot index and generation counter.
    #[inline]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        Self {
            raw: ((generation as u64) << 32) | (index as u64),
            _marker: PhantomData,
        }
    }

    /// Returns the slot index encoded in this handle.
    #[inline]
    pub const fn index(&self) -> u32 {
        self.raw as u32
    }

    /// Returns the generation counter encoded in this handle.
    #[inline]
    pub const fn generation(&self) -> u32 {
        (self.raw >> 32) as u32
    }

    pub const INVALID: Self = Self {
        raw: u64::MAX,
        _marker: PhantomData,
    };

    /// Returns `true` if this handle has not been set to the sentinel invalid value.
    #[inline]
    pub const fn is_valid(&self) -> bool {
        self.raw != u64::MAX
    }
}

impl<T> Copy for Handle<T> {}
impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<T> Eq for Handle<T> {}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state)
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({:#x})", self.raw)
    }
}

struct Entry<V> {
    generation: u32,
    resource: Option<V>,
}

pub struct ResourcePool<K, V> {
    entries: Vec<Entry<V>>,
    free_list: Vec<u32>,
    _marker: PhantomData<K>,
}

impl<K, V> Default for ResourcePool<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K, V> ResourcePool<K, V> {
    /// Creates an empty pool with no allocated slots.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free_list: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Inserts a resource and returns a handle that uniquely identifies this slot generation.
    #[inline]
    pub fn insert(&mut self, resource: V) -> Handle<K> {
        if let Some(idx) = self.free_list.pop() {
            let entry = &mut self.entries[idx as usize];
            entry.generation = entry.generation.wrapping_add(1);
            entry.resource = Some(resource);
            Handle::from_parts(idx, entry.generation)
        } else {
            let idx = self.entries.len() as u32;
            self.entries.push(Entry {
                generation: 0,
                resource: Some(resource),
            });
            Handle::from_parts(idx, 0)
        }
    }

    /// Removes the resource at the given handle; returns `None` if the handle is stale or out of bounds.
    #[inline]
    pub fn remove(&mut self, h: Handle<K>) -> Option<V> {
        let idx = h.index() as usize;
        if idx >= self.entries.len() {
            return None;
        }
        let entry = &mut self.entries[idx];
        if entry.generation != h.generation() {
            return None;
        }
        let res = entry.resource.take();
        entry.generation = entry.generation.wrapping_add(1);
        self.free_list.push(h.index());
        res
    }

    /// Returns a shared reference to the resource at the given handle, or `None` if stale or absent.
    #[inline]
    pub fn get(&self, h: Handle<K>) -> Option<&V> {
        let idx = h.index() as usize;
        if idx >= self.entries.len() {
            return None;
        }
        let entry = &self.entries[idx];
        if entry.generation != h.generation() {
            return None;
        }
        entry.resource.as_ref()
    }

    /// Returns an exclusive reference to the resource at the given handle, or `None` if stale or absent.
    #[inline]
    pub fn get_mut(&mut self, h: Handle<K>) -> Option<&mut V> {
        let idx = h.index() as usize;
        if idx >= self.entries.len() {
            return None;
        }
        let entry = &mut self.entries[idx];
        if entry.generation != h.generation() {
            return None;
        }
        entry.resource.as_mut()
    }
}
