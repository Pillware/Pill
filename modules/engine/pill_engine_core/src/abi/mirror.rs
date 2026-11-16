//! The C ABI every mirrored Rust function crosses to reach C#.
//!
//! # Responsibilities
//!
//! - Define the one trampoline shape every `#[pill_mirror_*]` attribute emits:
//!   `extern "C" fn(packed_arguments: *const u8, return_buffer: *mut u8) -> u8`.
//!   Each argument travels in its own 16-byte slot, and the result is written
//!   to `return_buffer`.
//! - Carry what a status byte cannot: the error message of a failed call and a
//!   returned string. Both travel through thread-local channels that the C#
//!   bridge reads.
//! - Read argument slots and write results for every type the mirror
//!   vocabulary supports, through [`MirrorValue`] and [`MirrorReturn`].
//! - Implement the asset store operations every mirrored asset type shares
//!   ([`assets`]), so each generated per-type trampoline is one call.
//!
//! # Design
//!
//! Every mirrored function has the same trampoline signature, whatever the Rust
//! function looks like. That is what lets the managed side call all of them
//! through one C-ABI function pointer type: no delegate per signature, no
//! marshalling stub, and nothing for NativeAOT to generate at build time. The
//! cost is that the caller packs the arguments and this module unpacks them,
//! which takes a handful of unaligned reads per call.
//!
//! Slot layout, the contract with `TracyLive.MirrorCall` in the C# runtime:
//!
//! | Tag | Slot contents |
//! | --- | --- |
//! | primitive | the value at offset 0, little endian; `bool` is one byte |
//! | `str` | UTF-8 pointer at 0, byte length (`u32`) at 8 |
//! | `slice:<element>` | element pointer at 0, element count (`u32`) at 8 |
//! | `ref:`/`mut:` | the address of the value at 0 |
//! | named value type | see its [`MirrorValue`] implementation |
//! | `option:<inner>` | the inner encoding, presence byte at 15 |
//!
//! A returned value is written to `return_buffer` in the same layout, with two
//! differences. An `option:` puts its presence byte at offset 0 and the value
//! at offset 16. A returned string travels through the return channel instead
//! of `return_buffer`.
//!
//! The thread-local channels live in this crate, and this crate is the engine
//! dylib that the host and every module share: one copy per process. So a
//! module's trampoline and the bridge that reads a channel see the same
//! thread-local.

// Standard library
use std::cell::RefCell;
use std::path::Path;

// External crates
use trait_type_map::TraitAccessible;

// Current crate
use crate::asset::{Asset, AssetGuid, AssetLoader, AssetManager, Handle};
use crate::asset_metadata::{
    AssetImport, ImportOutcome, ImportedAsset, MetadataPolicy, MetadataSource,
};
use crate::asset_standalone::StandaloneAsset;

// =============================================================================
// Constants
// =============================================================================

/// Width of one argument slot, in bytes.
pub const SLOT_SIZE: usize = 16;

/// The call succeeded and wrote its result.
pub const STATUS_OK: u8 = 0;

/// The call failed. Its message is in the last-error channel.
pub const STATUS_ERROR: u8 = 1;

/// Offset of an `option:` argument's presence byte inside its slot.
pub const OPTION_ARGUMENT_FLAG_OFFSET: usize = 15;

/// Offset of an `option:` return's value inside the return buffer. Its
/// presence byte sits at offset 0.
pub const OPTION_RETURN_VALUE_OFFSET: usize = 16;

/// Offset of an `assets.import` return's "already loaded" byte.
pub const IMPORT_ALREADY_LOADED_OFFSET: usize = 8;

/// Offset of an `assets.import` return's metadata-source byte.
pub const IMPORT_METADATA_SOURCE_OFFSET: usize = 9;

/// The result of a mirrored call: the value, or the message that explains the
/// failure.
pub type MirrorResult<T> = Result<T, String>;

// =============================================================================
// Channels
// =============================================================================

thread_local! {
    /// The message of the last failed mirrored call on this thread.
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The string the last mirrored call on this thread returned.
    static RETURN_STRING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record why the current mirrored call failed.
pub fn set_last_error(message: impl Into<String>) {
    let message = message.into();
    LAST_ERROR.with(|slot| *slot.borrow_mut() = Some(message));
}

/// Take the message the last failed call recorded on this thread.
pub fn take_last_error() -> Option<String> {
    LAST_ERROR.with(|slot| slot.borrow_mut().take())
}

/// Record the string the current mirrored call returns.
pub fn set_return_string(value: String) {
    RETURN_STRING.with(|slot| *slot.borrow_mut() = Some(value));
}

/// Take the string the last mirrored call on this thread returned.
pub fn take_return_string() -> Option<String> {
    RETURN_STRING.with(|slot| slot.borrow_mut().take())
}

// =============================================================================
// Invocation
// =============================================================================

/// Run one trampoline body and turn its outcome into a status byte.
///
/// An `Err` is recorded in the last-error channel. A panic is caught and
/// recorded the same way, because a panic that unwinds out of an `extern "C"`
/// function aborts the process, and a message lets the managed caller raise its
/// own exception instead.
pub fn invoke(trampoline_body: impl FnOnce() -> MirrorResult<()>) -> u8 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(trampoline_body)) {
        Ok(Ok(())) => STATUS_OK,
        Ok(Err(message)) => {
            set_last_error(message);
            STATUS_ERROR
        }
        Err(panic_payload) => {
            let panic_message = panic_payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_string())
                .or_else(|| panic_payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a non-string panic payload".to_string());
            set_last_error(format!("the Rust function panicked: {panic_message}"));
            STATUS_ERROR
        }
    }
}

/// Turn an error into the message text a mirrored failure carries.
pub fn error_message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

// =============================================================================
// Slots
// =============================================================================

/// Address of argument slot `slot_index`.
///
/// # Safety
///
/// `packed_arguments` must point at a buffer of at least `slot_index + 1`
/// slots.
pub unsafe fn slot(packed_arguments: *const u8, slot_index: usize) -> *const u8 {
    // SAFETY: the caller's contract.
    unsafe { packed_arguments.add(slot_index * SLOT_SIZE) }
}

/// Read a plain value from the start of a slot or element.
///
/// # Safety
///
/// `address` must reference `size_of::<T>()` readable bytes holding a valid `T`.
pub unsafe fn read<T: Copy>(address: *const u8) -> T {
    // SAFETY: the caller's contract. The read is unaligned because slots are
    // packed.
    unsafe { std::ptr::read_unaligned(address.cast::<T>()) }
}

/// Read a `bool` (one byte, nonzero is true).
///
/// # Safety
///
/// `address` must reference one readable byte.
pub unsafe fn read_bool(address: *const u8) -> bool {
    // SAFETY: the caller's contract.
    unsafe { *address != 0 }
}

/// Read the address a slot carries.
///
/// # Safety
///
/// `address` must reference a readable pointer-sized value.
pub unsafe fn read_pointer(address: *const u8) -> *mut u8 {
    // SAFETY: the caller's contract.
    unsafe { read::<usize>(address) as *mut u8 }
}

/// Read the `(pointer, length)` pair a `str` or `slice:` slot carries.
///
/// # Safety
///
/// `address` must reference twelve readable bytes.
pub unsafe fn read_span(address: *const u8) -> (*const u8, usize) {
    // SAFETY: the caller's contract.
    unsafe {
        (
            read_pointer(address) as *const u8,
            read::<u32>(address.add(8)) as usize,
        )
    }
}

/// Borrow the value a `ref:` slot addresses.
///
/// `value_label` names the value in the error a null slot produces.
///
/// # Errors
///
/// Returns an error that names the value when the slot holds a null pointer.
///
/// # Safety
///
/// The slot must hold null or the address of a live `T` that nothing else
/// mutates for `'a`.
pub unsafe fn read_ref<'a, T>(address: *const u8, value_label: &str) -> MirrorResult<&'a T> {
    // SAFETY: the caller's contract.
    let pointer = unsafe { read_pointer(address) } as *const T;
    // SAFETY: the caller's contract.
    unsafe { pointer.as_ref() }.ok_or_else(|| format!("{value_label} was null"))
}

/// Borrow the value a `mut:` slot addresses, mutably.
///
/// `value_label` names the value in the error a null slot produces.
///
/// # Errors
///
/// Returns an error that names the value when the slot holds a null pointer.
///
/// # Safety
///
/// The slot must hold null or the address of a live `T` no one else uses for
/// `'a`.
pub unsafe fn read_mut<'a, T>(address: *const u8, value_label: &str) -> MirrorResult<&'a mut T> {
    // SAFETY: the caller's contract.
    let pointer = unsafe { read_pointer(address) } as *mut T;
    // SAFETY: the caller's contract.
    unsafe { pointer.as_mut() }.ok_or_else(|| format!("{value_label} was null"))
}

/// Read a `str` slot as a borrowed string, checking it is UTF-8.
///
/// # Errors
///
/// Returns an error when the slot names bytes but points at nothing, and when
/// the bytes are not valid UTF-8.
///
/// # Safety
///
/// The slot must describe `length` readable bytes that stay valid for `'a`.
pub unsafe fn read_str<'a>(address: *const u8) -> MirrorResult<&'a str> {
    // SAFETY: the caller's contract.
    let (pointer, length) = unsafe { read_span(address) };
    if length == 0 {
        return Ok("");
    }
    if pointer.is_null() {
        return Err("a string argument was null".to_string());
    }
    // SAFETY: the caller's contract.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, length) };
    std::str::from_utf8(bytes).map_err(|error| format!("a string argument is not UTF-8: {error}"))
}

/// Read a `slice:` slot of plain elements into an owned vector.
///
/// The elements are copied rather than borrowed. The managed caller packs them
/// without promising any alignment, and a vector is what most Rust APIs take
/// anyway.
///
/// # Errors
///
/// Returns an error when the slot names elements but points at nothing.
///
/// # Safety
///
/// The slot must describe `element_count` readable, valid `T` values.
pub unsafe fn read_plain_vec<T: Copy>(address: *const u8) -> MirrorResult<Vec<T>> {
    // SAFETY: the caller's contract.
    let (pointer, element_count) = unsafe { read_span(address) };
    if element_count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err("a slice argument was null".to_string());
    }
    let element_size = std::mem::size_of::<T>();
    // SAFETY: the caller's contract; every element is read unaligned.
    Ok((0..element_count)
        .map(|index| unsafe { read::<T>(pointer.add(index * element_size)) })
        .collect())
}

/// Read a `slice:str` slot into owned strings.
///
/// # Errors
///
/// Returns an error when the slot names strings but points at nothing, and the
/// errors [`read_str`] reports for one of them.
///
/// # Safety
///
/// The slot must describe `string_count` readable `(pointer, length)` pairs,
/// each describing readable bytes.
pub unsafe fn read_string_vec(address: *const u8) -> MirrorResult<Vec<String>> {
    // SAFETY: the caller's contract.
    let (pointer, string_count) = unsafe { read_span(address) };
    if string_count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err("a slice argument was null".to_string());
    }
    (0..string_count)
        // SAFETY: each element is one slot-shaped `(pointer, length)` pair.
        .map(|index| unsafe { read_str(pointer.add(index * SLOT_SIZE)) }.map(str::to_owned))
        .collect()
}

/// Read a `slice:` slot of named values into an owned vector.
///
/// # Errors
///
/// Returns an error when the slot names elements but points at nothing, and the
/// error [`MirrorValue::read_element`] reports for one of them.
///
/// # Safety
///
/// The slot must describe `value_count` elements laid out as `T`'s
/// [`MirrorValue::ELEMENT_SIZE`] promises.
pub unsafe fn read_value_vec<T: MirrorValue>(address: *const u8) -> MirrorResult<Vec<T>> {
    // SAFETY: the caller's contract.
    let (pointer, value_count) = unsafe { read_span(address) };
    if value_count == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err("a slice argument was null".to_string());
    }
    (0..value_count)
        // SAFETY: the caller's contract.
        .map(|index| unsafe { T::read_element(pointer.add(index * T::ELEMENT_SIZE)) })
        .collect()
}

/// Read an `option:` slot, decoding the inner value only when present.
///
/// # Errors
///
/// Returns the error the inner reader reports when the value is present.
///
/// # Safety
///
/// The slot must hold the inner encoding and a presence byte at
/// [`OPTION_ARGUMENT_FLAG_OFFSET`].
pub unsafe fn read_option<T>(
    address: *const u8,
    read_inner_value: impl FnOnce(*const u8) -> MirrorResult<T>,
) -> MirrorResult<Option<T>> {
    // SAFETY: the caller's contract.
    if unsafe { *address.add(OPTION_ARGUMENT_FLAG_OFFSET) } == 0 {
        return Ok(None);
    }
    read_inner_value(address).map(Some)
}

/// Write a plain value to the start of a return buffer.
///
/// # Safety
///
/// `return_buffer` must reference `size_of::<T>()` writable bytes.
pub unsafe fn write<T: Copy>(return_buffer: *mut u8, value: T) {
    // SAFETY: the caller's contract. The write is unaligned for the same reason
    // as `read`.
    unsafe { std::ptr::write_unaligned(return_buffer.cast::<T>(), value) }
}

/// Write an `option:` result: the presence byte, and the value when present.
///
/// # Safety
///
/// `return_buffer` must reference at least [`OPTION_RETURN_VALUE_OFFSET`]
/// writable bytes plus what `write_inner_value` writes.
pub unsafe fn write_option<T>(
    return_buffer: *mut u8,
    value: Option<T>,
    write_inner_value: impl FnOnce(*mut u8, T),
) {
    match value {
        // SAFETY: the caller's contract.
        Some(value) => unsafe {
            *return_buffer = 1;
            write_inner_value(return_buffer.add(OPTION_RETURN_VALUE_OFFSET), value);
        },
        // SAFETY: the caller's contract.
        None => unsafe { *return_buffer = 0 },
    }
}

// =============================================================================
// Named Values
// =============================================================================

/// A type a mirrored call takes by value, read from a slot or a slice element.
///
/// Implemented by the mirror attributes for the types they declare -
/// `#[pill_mirror_object]` boxes, `#[derive(PillMirror)]` value types and
/// enums - and here for the engine types the vocabulary names directly.
///
/// # Safety
///
/// An implementation must read exactly the encoding the C# runtime writes for
/// the type. The tag the code generator gives the type is what selects that
/// encoding.
pub unsafe trait MirrorValue: Sized {
    /// Stride of one element in a `slice:` of this type.
    const ELEMENT_SIZE: usize;

    /// Read the value an argument slot carries.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes do not hold a value of this type.
    ///
    /// # Safety
    ///
    /// `slot` must reference a full slot written for this type.
    unsafe fn read_slot(slot: *const u8) -> MirrorResult<Self>;

    /// Read one element of a `slice:` of this type.
    ///
    /// # Errors
    ///
    /// Returns an error when the bytes do not hold a value of this type.
    ///
    /// # Safety
    ///
    /// `element` must reference [`Self::ELEMENT_SIZE`] bytes written for this
    /// type.
    unsafe fn read_element(element: *const u8) -> MirrorResult<Self>;
}

/// A type a mirrored call returns by value.
///
/// # Safety
///
/// An implementation must write exactly the encoding the C# runtime reads for
/// the type. It must not write past the bytes the managed side reserved for it.
pub unsafe trait MirrorReturn: Sized {
    /// Write the value to a return buffer.
    ///
    /// # Safety
    ///
    /// `return_buffer` must reference the bytes the managed side reserved for
    /// this type.
    unsafe fn write_return(self, return_buffer: *mut u8);
}

/// A Rust value C# holds by pointer: a boxed `T` it creates, passes and drops.
///
/// `#[pill_mirror_object]` implements this trait and also generates the type's
/// drop trampoline and its [`MirrorValue`] and [`MirrorReturn`] implementations.
/// The helpers here are the one place the box crosses the boundary.
pub trait MirrorObject: Sized + Send + 'static {}

/// Move an object out of the box a slot or element addresses.
///
/// # Errors
///
/// Returns an error when the slot holds a null pointer, which means the value
/// was missing or had already been moved.
///
/// # Safety
///
/// `address` must hold null or a pointer from [`object_into_raw`] that the
/// managed side has given up, so nothing reads it again.
pub unsafe fn object_from_raw<T: MirrorObject>(address: *const u8) -> MirrorResult<T> {
    // SAFETY: the caller's contract.
    let pointer = unsafe { read_pointer(address) } as *mut T;
    if pointer.is_null() {
        return Err(format!(
            "a {} argument was null or already moved",
            std::any::type_name::<T>()
        ));
    }
    // SAFETY: the pointer came from `Box::into_raw` and is owned by this call.
    Ok(*unsafe { Box::from_raw(pointer) })
}

/// Box an object for the managed side and return its address.
pub fn object_into_raw<T: MirrorObject>(value: T) -> *mut T {
    Box::into_raw(Box::new(value))
}

/// Drop the boxed object a slot addresses. This is the body of every drop
/// trampoline.
///
/// # Safety
///
/// As [`object_from_raw`].
pub unsafe fn drop_object<T: MirrorObject>(packed_arguments: *const u8) -> u8 {
    invoke(|| {
        // SAFETY: the caller's contract.
        drop(unsafe { object_from_raw::<T>(slot(packed_arguments, 0)) }?);
        Ok(())
    })
}

// SAFETY: two `u32`s, the layout `TracyLive.Handle<T>` declares.
unsafe impl<T: Asset> MirrorValue for Handle<T> {
    const ELEMENT_SIZE: usize = 8;

    unsafe fn read_slot(slot: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        unsafe { Self::read_element(slot) }
    }

    unsafe fn read_element(element: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        let (index, generation) = unsafe { (read::<u32>(element), read::<u32>(element.add(4))) };
        Ok(Handle::from_raw(index, generation))
    }
}

// SAFETY: as the `MirrorValue` implementation.
unsafe impl<T: Asset> MirrorReturn for Handle<T> {
    unsafe fn write_return(self, return_buffer: *mut u8) {
        // SAFETY: the caller's contract.
        unsafe {
            write(return_buffer, self.index());
            write(return_buffer.add(4), self.generation());
        }
    }
}

/// `TracyLive.AssetLoader`'s encoding: data pointer at 0, length at 8, and the
/// kind at 12 (`0` a path below `res`, `1` the bytes themselves).
// SAFETY: the layout `MirrorCall.PushLoader` writes.
unsafe impl MirrorValue for AssetLoader {
    const ELEMENT_SIZE: usize = SLOT_SIZE;

    unsafe fn read_slot(slot: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        unsafe { Self::read_element(slot) }
    }

    unsafe fn read_element(element: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        let kind = unsafe { *element.add(12) };
        match kind {
            // SAFETY: the caller's contract.
            0 => Ok(AssetLoader::Path(unsafe { read_str(element) }?.into())),
            1 => {
                // SAFETY: the caller's contract.
                let (pointer, length) = unsafe { read_span(element) };
                if length > 0 && pointer.is_null() {
                    return Err("an asset loader's bytes were null".to_string());
                }
                let bytes = if length == 0 {
                    Box::default()
                } else {
                    // SAFETY: the caller's contract.
                    unsafe { std::slice::from_raw_parts(pointer, length) }.into()
                };
                Ok(AssetLoader::Bytes(bytes))
            }
            other => Err(format!("unknown asset loader kind {other}")),
        }
    }
}

macro_rules! plain_mirror_value {
    ($($ty:ty),*) => {$(
        // SAFETY: the managed side writes the primitive with the same type this
        // reads back, so the bytes match.
        unsafe impl MirrorValue for $ty {
            const ELEMENT_SIZE: usize = std::mem::size_of::<$ty>();

            unsafe fn read_slot(slot: *const u8) -> MirrorResult<Self> {
                // SAFETY: the caller's contract.
                Ok(unsafe { read::<$ty>(slot) })
            }

            unsafe fn read_element(element: *const u8) -> MirrorResult<Self> {
                // SAFETY: the caller's contract.
                Ok(unsafe { read::<$ty>(element) })
            }
        }

        // SAFETY: as above.
        unsafe impl MirrorReturn for $ty {
            unsafe fn write_return(self, return_buffer: *mut u8) {
                // SAFETY: the caller's contract.
                unsafe { write(return_buffer, self) }
            }
        }
    )*};
}

plain_mirror_value!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64, usize, isize);

// SAFETY: one byte, nonzero is true.
unsafe impl MirrorValue for bool {
    const ELEMENT_SIZE: usize = 1;

    unsafe fn read_slot(slot: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        Ok(unsafe { read_bool(slot) })
    }

    unsafe fn read_element(element: *const u8) -> MirrorResult<Self> {
        // SAFETY: the caller's contract.
        Ok(unsafe { read_bool(element) })
    }
}

// SAFETY: one byte.
unsafe impl MirrorReturn for bool {
    unsafe fn write_return(self, return_buffer: *mut u8) {
        // SAFETY: the caller's contract.
        unsafe { *return_buffer = u8::from(self) }
    }
}

// =============================================================================
// Asset Store Operations
// =============================================================================

/// The asset store operations every mirrored asset type shares.
///
/// `#[pill_mirror_object(asset)]` emits one trampoline per operation per type,
/// each a single call into here. The C# runtime's `AssetManagerExtensions`
/// methods reach them by name (`__asset_add`, ...). Slot 0 always holds the
/// address of the world's `AssetManager`, which the managed side fetched under
/// the running system's declared access.
///
/// Every operation runs through [`invoke`](crate::mirror::invoke), so a failure
/// returns [`STATUS_ERROR`](crate::mirror::STATUS_ERROR) and leaves its message
/// in the last-error channel.
pub mod assets {
    use super::*;

    /// The `AssetManager` a slot addresses.
    ///
    /// # Safety
    ///
    /// The slot must hold null or the address of the world's live store, used
    /// by nothing else for the call.
    unsafe fn asset_manager_from_slot<'a>(
        packed_arguments: *const u8,
    ) -> MirrorResult<&'a mut AssetManager> {
        // SAFETY: the caller's contract.
        unsafe { read_mut::<AssetManager>(slot(packed_arguments, 0), "the AssetManager") }
    }

    /// `assets.add(value)`: slot 1 the value. Returns its handle.
    ///
    /// # Safety
    ///
    /// The trampoline contract: `packed_arguments` holds the slots described
    /// above, and `return_buffer` is writable for 16 bytes.
    pub unsafe fn add<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset> + MirrorValue,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let value = T::read_slot(slot(packed_arguments, 1))?;
                asset_manager.add(value).write_return(return_buffer);
            }
            Ok(())
        })
    }

    /// `assets.add_named(name, value)`: slot 1 the name, slot 2 the value.
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn add_named<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset> + MirrorValue,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let name = read_str(slot(packed_arguments, 1))?;
                let value = T::read_slot(slot(packed_arguments, 2))?;
                asset_manager
                    .add_named(name, value)
                    .map_err(error_message)?
                    .write_return(return_buffer);
            }
            Ok(())
        })
    }

    /// `assets.add_named_with_guid(name, guid, value)`: slot 1 the name, slot 2
    /// the guid as 32 hexadecimal digits, slot 3 the value.
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn add_named_with_guid<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset> + MirrorValue,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let name = read_str(slot(packed_arguments, 1))?;
                let guid = parse_guid(read_str(slot(packed_arguments, 2))?)?;
                let value = T::read_slot(slot(packed_arguments, 3))?;
                asset_manager
                    .add_named_with_guid(name, guid, value)
                    .map_err(error_message)?
                    .write_return(return_buffer);
            }
            Ok(())
        })
    }

    /// `assets.remove(handle)`: slot 1 the handle. Returns the removed value's
    /// box, or null when the handle was stale.
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn remove<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset> + MirrorObject,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let handle = Handle::<T>::read_slot(slot(packed_arguments, 1))?;
                let removed = asset_manager
                    .remove(handle)
                    .map_or(std::ptr::null_mut(), object_into_raw);
                write(return_buffer, removed as usize);
            }
            Ok(())
        })
    }

    /// `assets.contains(handle)`: slot 1 the handle. Returns a `bool`.
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn contains<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let handle = Handle::<T>::read_slot(slot(packed_arguments, 1))?;
                asset_manager.contains(handle).write_return(return_buffer);
            }
            Ok(())
        })
    }

    /// `assets.handle_by_name(name)`: slot 1 the name. Returns an
    /// `option:handle`.
    ///
    /// # Safety
    ///
    /// As [`add`], with `return_buffer` holding an `option:` result.
    pub unsafe fn handle_by_name<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let name = read_str(slot(packed_arguments, 1))?;
                write_option(
                    return_buffer,
                    asset_manager.handle_by_name::<T>(name),
                    |value_address, handle| handle.write_return(value_address),
                );
            }
            Ok(())
        })
    }

    /// `assets.handle_by_guid(guid)`: slot 1 the guid as hexadecimal. Returns
    /// an `option:handle`.
    ///
    /// # Safety
    ///
    /// As [`handle_by_name`].
    pub unsafe fn handle_by_guid<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let guid = parse_guid(read_str(slot(packed_arguments, 1))?)?;
                write_option(
                    return_buffer,
                    asset_manager.handle_by_guid::<T>(guid),
                    |value_address, handle| handle.write_return(value_address),
                );
            }
            Ok(())
        })
    }

    /// `assets.guid_of(handle)`: slot 1 the handle. Returns the guid as a
    /// string (empty when the asset has none).
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn guid_of<T>(packed_arguments: *const u8, _return_buffer: *mut u8) -> u8
    where
        T: Asset + TraitAccessible<dyn Asset>,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let handle = Handle::<T>::read_slot(slot(packed_arguments, 1))?;
                set_return_string(guid_text(asset_manager.guid_of(handle)));
            }
            Ok(())
        })
    }

    /// `assets.import(path, policy, settings)`: slot 1 the path below `res`,
    /// slot 2 the policy (`0` read if present, `1` create if missing), slot 3
    /// the initial settings as JSON (empty for the type's defaults).
    ///
    /// Writes the handle at offset 0, an "already loaded" byte at
    /// [`IMPORT_ALREADY_LOADED_OFFSET`] and a metadata-source byte at
    /// [`IMPORT_METADATA_SOURCE_OFFSET`] (`0` a file, `1` written now, `2`
    /// memory only). Returns the guid through the string channel, empty when
    /// the asset carries none. That is the encoding [`guid_of`] already uses.
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn import<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: ImportedAsset,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                // Step 1: Read the store and the three arguments from the slots.
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let path = read_str(slot(packed_arguments, 1))?;
                let policy = match read::<u8>(slot(packed_arguments, 2)) {
                    0 => MetadataPolicy::ReadIfPresent,
                    1 => MetadataPolicy::CreateIfMissing,
                    other => return Err(format!("unknown metadata policy {other}")),
                };
                let settings = read_str(slot(packed_arguments, 3))?;
                // Step 2: Build the request, with the settings only when the
                // managed side gave any.
                let mut request = AssetImport::<T>::new(path, policy);
                if !settings.trim().is_empty() {
                    let initial_settings = serde_json::from_str(settings).map_err(|error| {
                        format!(
                            "the initial settings do not fit {}: {error}",
                            T::metadata_type_name()
                        )
                    })?;
                    request = request.with_initial_settings(initial_settings);
                }
                // Step 3: Import, then write the result into the return buffer.
                let outcome = asset_manager
                    .import(request)
                    .map_err(|error| format!("import of `{path}` failed: {error}"))?;
                // SAFETY: `return_buffer` addresses the return buffer the
                // caller promised for the duration of this call.
                write_import_outcome(&outcome, return_buffer);
            }
            Ok(())
        })
    }

    /// `assets.import_standalone(path)`: slot 1 the path below `res`. Writes
    /// the same return buffer as [`import`].
    ///
    /// # Safety
    ///
    /// As [`add`].
    pub unsafe fn import_standalone<T>(packed_arguments: *const u8, return_buffer: *mut u8) -> u8
    where
        T: StandaloneAsset,
    {
        invoke(|| {
            // SAFETY: the caller's contract.
            unsafe {
                let asset_manager = asset_manager_from_slot(packed_arguments)?;
                let path = read_str(slot(packed_arguments, 1))?;
                let outcome = asset_manager
                    .import_standalone::<T>(Path::new(path))
                    .map_err(|error| format!("import of `{path}` failed: {error}"))?;
                // SAFETY: `return_buffer` addresses the return buffer the
                // caller promised for the duration of this call.
                write_import_outcome(&outcome, return_buffer);
            }
            Ok(())
        })
    }

    /// Parse a guid the managed side wrote as 32 hexadecimal digits.
    fn parse_guid(text: &str) -> MirrorResult<AssetGuid> {
        AssetGuid::parse(text).ok_or_else(|| format!("`{text}` is not a 32-digit hexadecimal guid"))
    }

    /// A guid as the string channel carries it: 32 hexadecimal digits, or the
    /// empty string when the asset has none.
    fn guid_text(guid: Option<AssetGuid>) -> String {
        guid.map(|guid| guid.to_string()).unwrap_or_default()
    }

    /// Write an import result into the return buffer. The handle goes at offset
    /// 0, the two flag bytes at their named offsets, and the guid through the
    /// string channel.
    ///
    /// Both import trampolines answer with this shape, so the layout the
    /// managed side reads is written once.
    ///
    /// # Safety
    ///
    /// `return_buffer` must address the return buffer the caller promised,
    /// writable for the whole call.
    unsafe fn write_import_outcome<T: Asset>(outcome: &ImportOutcome<T>, return_buffer: *mut u8) {
        // SAFETY: `return_buffer` addresses the return buffer, writable for the
        // whole call, which is the caller's promise.
        unsafe { outcome.handle.write_return(return_buffer) };
        // SAFETY: `return_buffer` addresses the return buffer, which is at
        // least a slot header long - the two flag bytes sit inside it.
        unsafe {
            *return_buffer.add(IMPORT_ALREADY_LOADED_OFFSET) = u8::from(outcome.already_loaded);
            *return_buffer.add(IMPORT_METADATA_SOURCE_OFFSET) =
                metadata_source_code(outcome.metadata);
        }
        set_return_string(guid_text(outcome.guid));
    }

    /// The byte the return buffer carries for a metadata source.
    fn metadata_source_code(metadata_source: MetadataSource) -> u8 {
        match metadata_source {
            MetadataSource::ReadFromFile => 0,
            MetadataSource::CreatedOnDisk => 1,
            MetadataSource::InMemoryOnly => 2,
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A test object: a heap-owning value the managed side would hold by box.
    #[derive(Debug, PartialEq)]
    struct Label(String);
    impl MirrorObject for Label {}

    /// A zeroed buffer of `slot_count` slots, the shape the managed runtime
    /// passes.
    fn zeroed_slots(slot_count: usize) -> Vec<u8> {
        vec![0; slot_count * SLOT_SIZE]
    }

    #[test]
    fn an_error_and_a_panic_both_become_status_one_with_a_message() {
        assert_eq!(invoke(|| Ok(())), STATUS_OK);
        assert_eq!(take_last_error(), None);

        assert_eq!(invoke(|| Err("no such mesh".to_string())), STATUS_ERROR);
        assert_eq!(take_last_error().as_deref(), Some("no such mesh"));

        assert_eq!(invoke(|| panic!("boom")), STATUS_ERROR);
        let message = take_last_error().expect("a panic records a message");
        assert!(message.contains("boom"), "{message}");
        assert_eq!(take_last_error(), None, "taking the message clears it");
    }

    #[test]
    fn strings_and_plain_slices_read_from_their_slots() {
        let text = "pill";
        let numbers = [1.5f32, -2.0, 4.25];
        let mut packed_arguments = zeroed_slots(2);
        // SAFETY: two slots, each written within its sixteen bytes.
        unsafe {
            write(packed_arguments.as_mut_ptr(), text.as_ptr() as usize);
            write(packed_arguments.as_mut_ptr().add(8), text.len() as u32);
            write(
                packed_arguments.as_mut_ptr().add(16),
                numbers.as_ptr() as usize,
            );
            write(packed_arguments.as_mut_ptr().add(24), numbers.len() as u32);
            assert_eq!(read_str(slot(packed_arguments.as_ptr(), 0)), Ok("pill"));
            assert_eq!(
                read_plain_vec::<f32>(slot(packed_arguments.as_ptr(), 1)),
                Ok(numbers.to_vec())
            );
        }
    }

    #[test]
    fn invalid_utf8_is_refused_with_a_message() {
        let bytes = [0xff_u8, 0xfe];
        let mut packed_arguments = zeroed_slots(1);
        // SAFETY: one slot.
        unsafe {
            write(packed_arguments.as_mut_ptr(), bytes.as_ptr() as usize);
            write(packed_arguments.as_mut_ptr().add(8), bytes.len() as u32);
            assert!(read_str(slot(packed_arguments.as_ptr(), 0)).is_err());
        }
    }

    #[test]
    fn an_object_round_trips_through_its_box_and_cannot_be_read_twice() {
        let object_pointer = object_into_raw(Label("cube".to_string()));
        let mut packed_arguments = zeroed_slots(1);
        // SAFETY: one slot holds the box and is read once. The null written
        // after it is refused rather than dereferenced.
        unsafe {
            write(packed_arguments.as_mut_ptr(), object_pointer as usize);
            assert_eq!(
                object_from_raw::<Label>(slot(packed_arguments.as_ptr(), 0)),
                Ok(Label("cube".to_string()))
            );
            write(packed_arguments.as_mut_ptr(), 0usize);
            assert!(object_from_raw::<Label>(slot(packed_arguments.as_ptr(), 0)).is_err());
        }
    }

    #[test]
    fn handles_and_loaders_decode_their_encodings() {
        struct Mesh;
        impl Asset for Mesh {}
        trait_type_map::impl_trait_accessible!(dyn Asset; Mesh);

        let path = "models/cube.obj";
        let mut packed_arguments = zeroed_slots(2);
        // SAFETY: two slots, each written within its sixteen bytes.
        unsafe {
            write(packed_arguments.as_mut_ptr(), 7u32);
            write(packed_arguments.as_mut_ptr().add(4), 2u32);
            write(
                packed_arguments.as_mut_ptr().add(16),
                path.as_ptr() as usize,
            );
            write(packed_arguments.as_mut_ptr().add(24), path.len() as u32);
            *packed_arguments.as_mut_ptr().add(28) = 0;
            let handle = Handle::<Mesh>::read_slot(slot(packed_arguments.as_ptr(), 0)).unwrap();
            assert_eq!((handle.index(), handle.generation()), (7, 2));
            let loader = AssetLoader::read_slot(slot(packed_arguments.as_ptr(), 1)).unwrap();
            assert!(matches!(loader, AssetLoader::Path(found) if found == Path::new(path)));
        }
    }

    #[test]
    fn an_absent_option_skips_the_inner_read() {
        let mut packed_arguments = zeroed_slots(1);
        // SAFETY: the buffer is one slot long, and the presence byte is inside
        // it.
        unsafe {
            write(packed_arguments.as_mut_ptr(), 9u32);
            assert_eq!(
                read_option(
                    slot(packed_arguments.as_ptr(), 0),
                    |address| u32::read_slot(address),
                ),
                Ok(None)
            );
            *packed_arguments
                .as_mut_ptr()
                .add(OPTION_ARGUMENT_FLAG_OFFSET) = 1;
            assert_eq!(
                read_option(
                    slot(packed_arguments.as_ptr(), 0),
                    |address| u32::read_slot(address),
                ),
                Ok(Some(9))
            );
        }
    }
}
