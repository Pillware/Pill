// zlib License

// Copyright (c) 2021 Orson Peters <orsonpeters@gmail.com>

// This software is provided 'as-is', without any express or implied warranty. In
// no event will the authors be held liable for any damages arising from the use of
// this software.

// Permission is granted to anyone to use this software for any purpose, including
// commercial applications, and to alter it and redistribute it freely, subject to
// the following restrictions:

//  1. The origin of this software must not be misrepresented; you must not claim
//     that you wrote the original software. If you use this software in a product,
//     an acknowledgment in the product documentation would be appreciated but is
//     not required.

//  2. Altered source versions must be plainly marked as such, and must not be
//     misrepresented as being the original software.

//  3. This notice may not be removed or altered from any source distribution.




// This is PillSlotMap with public key variables, removed iterators and with version limit set to 2^8 = 256
// https://crates.io/crates/PillSlotMap


// Needed because assigning to non-Copy union is unsafe in stable but not in nightly.
#![allow(unused_unsafe)]

//! Contains the slot map implementation.

#[cfg(all(nightly, any(doc, feature = "unstable")))]
use alloc::collections::TryReserveError;
use core::fmt;
use core::marker::PhantomData;
#[allow(unused_imports)] // MaybeUninit is only used on nightly at the moment.
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{Index, IndexMut};
use std::fmt::Formatter;
use core::fmt::Debug;
use std::num::{ NonZeroU8};

// Storage inside a slot or metadata for the freelist when vacant.
union SlotUnion<T> {
    value: ManuallyDrop<T>,
    next_free: u32,
}

// A slot, which represents storage for a value and a current version.
// Can be occupied or vacant.
struct Slot<T> {
    u: SlotUnion<T>,
    version: u8, // Even = vacant, odd = occupied.
}

// Safe API to read a slot.
enum SlotContent<'a, T: 'a> {
    Occupied(&'a T),
    Vacant(&'a u32),
}

enum SlotContentMut<'a, T: 'a> {
    OccupiedMut(&'a mut T),
    VacantMut(&'a mut u32),
}

use self::SlotContent::{Occupied, Vacant};
use self::SlotContentMut::{OccupiedMut, VacantMut};

impl<T> Slot<T> {
    // Is this slot occupied?
    #[inline(always)]
    pub fn occupied(&self) -> bool {
        self.version % 2 > 0
    }

    pub fn get(&self) -> SlotContent<T> {
        unsafe {
            if self.occupied() {
                Occupied(&*self.u.value)
            } else {
                Vacant(&self.u.next_free)
            }
        }
    }

    pub fn get_mut(&mut self) -> SlotContentMut<T> {
        unsafe {
            if self.occupied() {
                OccupiedMut(&mut *self.u.value)
            } else {
                VacantMut(&mut self.u.next_free)
            }
        }
    }
}

impl<T> Drop for Slot<T> {
    fn drop(&mut self) {
        if core::mem::needs_drop::<T>() && self.occupied() {
            // This is safe because we checked that we're occupied.
            unsafe {
                ManuallyDrop::drop(&mut self.u.value);
            }
        }
    }
}

impl<T: Clone> Clone for Slot<T> {
    fn clone(&self) -> Self {
        Self {
            u: match self.get() {
                Occupied(value) => SlotUnion {
                    value: ManuallyDrop::new(value.clone()),
                },
                Vacant(&next_free) => SlotUnion { next_free },
            },
            version: self.version,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Slot<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        let mut builder = fmt.debug_struct("Slot");
        builder.field("version", &self.version);
        match self.get() {
            Occupied(value) => builder.field("value", value).finish(),
            Vacant(next_free) => builder.field("next_free", next_free).finish(),
        }
    }
}

// --- PillSlotMap

#[derive(Debug, Clone)]
pub struct PillSlotMap<K: PillSlotMapKey, V> {
    slots: Vec<Slot<V>>,
    free_head: u32,
    num_elems: u32,
    _k: PhantomData<fn(K) -> K>,
}

impl<K: PillSlotMapKey, V> PillSlotMap<K, V> {

    pub fn with_key() -> Self {
        Self::with_capacity_and_key(0)
    }

    pub fn with_capacity_and_key(capacity: usize) -> Self {
        let mut slots = Vec::with_capacity(capacity + 1);
        slots.push(Slot {
            u: SlotUnion { next_free: 0 },
            version: 0,
        });

        Self {
            slots,
            free_head: 1,
            num_elems: 0,
            _k: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.num_elems as usize
    }

    pub fn is_empty(&self) -> bool {
        self.num_elems == 0
    }

    pub fn capacity(&self) -> usize {
        // One slot is reserved for the sentinel.
        self.slots.capacity() - 1
    }

    pub fn reserve(&mut self, additional: usize) {
        // One slot is reserved for the sentinel.
        let needed = (self.len() + additional).saturating_sub(self.slots.len() - 1);
        self.slots.reserve(needed);
    }

    #[cfg(all(nightly, any(doc, feature = "unstable")))]
    #[cfg_attr(all(nightly, doc), doc(cfg(feature = "unstable")))]
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError> {
        // One slot is reserved for the sentinel.
        let needed = (self.len() + additional).saturating_sub(self.slots.len() - 1);
        self.slots.try_reserve(needed)
    }

    pub fn contains_key(&self, key: K) -> bool {
        let kd = key.data();
        self.slots
            .get(kd.index as usize)
            .map_or(false, |slot| slot.version == kd.version.get())
    }

    pub fn insert(&mut self, value: V) -> K {
        self.insert_with_key(|_| value)
    }

    pub fn insert_with_key<F>(&mut self, f: F) -> K
    where
        F: FnOnce(K) -> V,
    {
        // In case f panics, we don't make any changes until we have the value.
        let new_num_elems = self.num_elems + 1;
        if new_num_elems == core::u32::MAX {
            panic!("PillSlotMap number of elements overflow");
        }

        if let Some(slot) = self.slots.get_mut(self.free_head as usize) {
            let occupied_version = slot.version | 1;
            let kd = PillSlotMapKeyData::new(self.free_head, occupied_version);

            // Get value first in case f panics.
            let value = f(kd.into());

            // Update.
            unsafe {
                self.free_head = slot.u.next_free;
                slot.u.value = ManuallyDrop::new(value);
                slot.version = occupied_version;
            }
            self.num_elems = new_num_elems;
            return kd.into();
        }

        let version = 1;
        let kd = PillSlotMapKeyData::new(self.slots.len() as u32, version);

        // Create new slot before adjusting freelist in case f or the allocation panics.
        self.slots.push(Slot {
            u: SlotUnion {
                value: ManuallyDrop::new(f(kd.into())),
            },
            version,
        });

        self.free_head = kd.index + 1;
        self.num_elems = new_num_elems;
        kd.into()
    }

    // Helper function to remove a value from a slot. Safe iff the slot is
    // occupied. Returns the value removed.
    #[inline(always)]
    unsafe fn remove_from_slot(&mut self, index: usize) -> V {
        // Remove value from slot before overwriting union.
        let slot = self.slots.get_unchecked_mut(index);
        let value = ManuallyDrop::take(&mut slot.u.value);

        // Maintain freelist.
        slot.u.next_free = self.free_head;
        self.free_head = index as u32;
        self.num_elems -= 1;
        slot.version = slot.version.wrapping_add(1);

        value
    }

    pub fn remove(&mut self, key: K) -> Option<V> {
        let kd = key.data();
        if self.contains_key(key) {
            // This is safe because we know that the slot is occupied.
            Some(unsafe { self.remove_from_slot(kd.index as usize) })
        } else {
            None
        }
    }

    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(K, &mut V) -> bool,
    {
        for i in 1..self.slots.len() {
            // This is safe because removing elements does not shrink slots.
            let slot = unsafe { self.slots.get_unchecked_mut(i) };
            let version = slot.version;

            let should_remove = if let OccupiedMut(value) = slot.get_mut() {
                let key = PillSlotMapKeyData::new(i as u32, version).into();
                !f(key, value)
            } else {
                false
            };

            if should_remove {
                // This is safe because we know that the slot was occupied.
                unsafe { self.remove_from_slot(i) };
            }
        }
    }

    pub fn get(&self, key: K) -> Option<&V> {
        let kd = key.data();
        self.slots
            .get(kd.index as usize)
            .filter(|slot| slot.version == kd.version.get())
            .map(|slot| unsafe { &*slot.u.value })
    }

    pub unsafe fn get_unchecked(&self, key: K) -> &V {
        debug_assert!(self.contains_key(key));
        &self.slots.get_unchecked(key.data().index as usize).u.value
    }

    pub fn get_mut(&mut self, key: K) -> Option<&mut V> {
        let kd = key.data();
        self.slots
            .get_mut(kd.index as usize)
            .filter(|slot| slot.version == kd.version.get())
            .map(|slot| unsafe { &mut *slot.u.value })
    }

    pub unsafe fn get_unchecked_mut(&mut self, key: K) -> &mut V {
        debug_assert!(self.contains_key(key));
        &mut self
            .slots
            .get_unchecked_mut(key.data().index as usize)
            .u
            .value
    }

    #[cfg(has_min_const_generics)]
    pub fn get_disjoint_mut<const N: usize>(&mut self, keys: [K; N]) -> Option<[&mut V; N]> {
        let mut ptrs: [MaybeUninit<*mut V>; N] = unsafe { MaybeUninit::uninit().assume_init() };

        let mut i = 0;
        while i < N {
            let kd = keys[i].data();
            if !self.contains_key(kd.into()) {
                break;
            }

            unsafe {
                let slot = self.slots.get_unchecked_mut(kd.index as usize);
                slot.version ^= 1;
                ptrs[i] = MaybeUninit::new(&mut *slot.u.value);
            }
            i += 1;
        }

        // Undo temporary unoccupied markings.
        for k in &keys[..i] {
            let index = k.data().index as usize;
            unsafe {
                self.slots.get_unchecked_mut(index).version ^= 1;
            }
        }

        if i == N {
            // All were valid and disjoint.
            Some(unsafe { core::mem::transmute_copy::<_, [&mut V; N]>(&ptrs) })
        } else {
            None
        }
    }

    #[cfg(has_min_const_generics)]
    pub unsafe fn get_disjoint_unchecked_mut<const N: usize>(
        &mut self,
        keys: [K; N],
    ) -> [&mut V; N] {
        // Safe, see get_disjoint_mut.
        let mut ptrs: [MaybeUninit<*mut V>; N] = MaybeUninit::uninit().assume_init();
        for i in 0..N {
            ptrs[i] = MaybeUninit::new(self.get_unchecked_mut(keys[i]));
        }
        core::mem::transmute_copy::<_, [&mut V; N]>(&ptrs)
    }
}

impl<K: PillSlotMapKey, V> Index<K> for PillSlotMap<K, V> {
    type Output = V;

    fn index(&self, key: K) -> &V {
        match self.get(key) {
            Some(r) => r,
            None => panic!("invalid PillSlotMap key used"),
        }
    }
}

impl<K: PillSlotMapKey, V> IndexMut<K> for PillSlotMap<K, V> {
    fn index_mut(&mut self, key: K) -> &mut V {
        match self.get_mut(key) {
            Some(r) => r,
            None => panic!("invalid PillSlotMap key used"),
        }
    }
}

// --- PillSlotMapKeyData

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PillSlotMapKeyData {
    pub index: u32,
    pub version: NonZeroU8,
}

impl PillSlotMapKeyData {
    fn new(index: u32, version: u8) -> Self {
        debug_assert!(version > 0);

        Self {
            index,
            version: unsafe { NonZeroU8::new_unchecked(version | 1) },
        }
    }

    fn null() -> Self {
        Self::new(core::u32::MAX, 1)
    }

    fn is_null(self) -> bool {
        self.index == core::u32::MAX
    }

    /// Returns the key data as a 64-bit integer. No guarantees about its value
    /// are made other than that passing it to [`from_ffi`](Self::from_ffi)
    /// will return a key equal to the original.
    ///
    /// With this you can easily pass slot map keys as opaque handles to foreign
    /// code. After you get them back you can confidently use them in your slot
    /// map without worrying about unsafe behavior as you would with passing and
    /// receiving back references or pointers.
    ///
    /// This is not a substitute for proper serialization, use [`serde`] for
    /// that. If you are not doing FFI, you almost surely do not need this
    /// function.
    ///
    /// [`serde`]: crate#serialization-through-serde-no_std-support-and-unstable-features
    pub fn as_ffi(self) -> u64 {
        (u64::from(self.version.get()) << 32) | u64::from(self.index)
    }

    /// Iff `value` is a value received from `k.as_ffi()`, returns a key equal
    /// to `k`. Otherwise the behavior is safe but unspecified.
    pub fn from_ffi(value: u64) -> Self {
        let index = value & 0xffff_ffff;
        let version = (value >> 32) | 1; // Ensure version is odd.
        Self::new(index as u32, version as u8)
    }
}

impl Debug for PillSlotMapKeyData {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}v{}", self.index, self.version.get())
    }
}

impl Default for PillSlotMapKeyData {
    fn default() -> Self {
        Self::null()
    }
}

pub unsafe trait PillSlotMapKey:
    From<PillSlotMapKeyData>
    + Copy
    + Clone
    + Default
    + Eq
    + PartialEq
    + Ord
    + PartialOrd
    + core::hash::Hash
    + core::fmt::Debug
{

    fn null() -> Self {
        PillSlotMapKeyData::null().into()
    }

    fn is_null(&self) -> bool {
        self.data().is_null()
    }

    fn data(&self) -> PillSlotMapKeyData;
}

//#[macro_export(local_inner_macros)]
#[macro_export]
macro_rules! define_new_pill_slotmap_key {
    ( $(#[$outer:meta])* $vis:vis struct $name:ident; $($rest:tt)* ) => {
        $(#[$outer])*
        #[derive(Copy, Clone, Default,
                 Eq, PartialEq, Ord, PartialOrd,
                 Hash, Debug)]
        #[repr(transparent)]
        $vis struct $name($crate::PillSlotMapKeyData);

        impl From<$crate::PillSlotMapKeyData> for $name {
            fn from(k: $crate::PillSlotMapKeyData) -> Self {
                $name(k)
            }
        }

        unsafe impl $crate::PillSlotMapKey for $name {
            fn data(&self) -> $crate::PillSlotMapKeyData {
                self.0
            }
        }
    };

    () => {}
}
