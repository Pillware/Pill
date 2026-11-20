//! Read-only frame state for the managed runtime: input and time.
//!
//! # Responsibilities
//!
//! - Answer the managed runtime's input queries (keys and mouse buttons held,
//!   mouse motion since the previous frame) from the active invocation's
//!   `Input` resource.
//! - Report the engine clock's frame delta and elapsed time from the active
//!   invocation's `Time` resource.
//!
//! # Design
//!
//! Each function reads the world of the active managed invocation, which the
//! host owns. It returns the same values the frame's systems see, so a managed
//! system and a native one agree about the frame they process. With no
//! invocation active, the answers are "nothing held" and zero. Nothing is
//! cached between calls, so an answer cannot go stale.
//!
//! The managed runtime passes keys and mouse buttons as `u8` numbers. Each
//! number is an index into [`KeyCode::ALL`] or [`MouseButton::ALL`], and the
//! C# mirror lists the same items in the same order. A number that names no
//! entry reports "not held".

// External crates
use pill_core::math::Vector2f;
use pill_engine::{Input, KeyCode, MouseButton, Time};

// Current crate
use crate::context::with_active_world;

// =============================================================================
// Free Functions
// =============================================================================

/// Reports whether the key with discriminant `key_discriminant` is held this
/// frame.
///
/// Returns `1` while the key is held and `0` otherwise, including outside an
/// active invocation and for a number no [`KeyCode`] has.
pub(super) extern "C" fn ffi_input_key_held(key_discriminant: u8) -> u8 {
    let Some(key_code) = KeyCode::ALL.get(key_discriminant as usize).copied() else {
        return 0;
    };
    let held = with_active_world(|world| {
        world
            .get_resource::<Input>()
            .is_some_and(|input| input.key_held(key_code))
    })
    .unwrap_or(false);
    u8::from(held)
}

/// Reports whether the mouse button with discriminant
/// `button_discriminant` is held this frame. Works the same way as
/// [`ffi_input_key_held`].
pub(super) extern "C" fn ffi_input_mouse_button_held(button_discriminant: u8) -> u8 {
    let Some(mouse_button) = MouseButton::ALL.get(button_discriminant as usize).copied() else {
        return 0;
    };
    let held = with_active_world(|world| {
        world
            .get_resource::<Input>()
            .is_some_and(|input| input.mouse_button_held(mouse_button))
    })
    .unwrap_or(false);
    u8::from(held)
}

/// Writes the mouse movement since the previous frame to `out_x` and `out_y`
/// and returns `1`.
///
/// The values are raw device motion with no pixel unit: the platform reports
/// them in its own units and does not scale them. Outside an active invocation
/// the values written are zero. Returns `0` and writes nothing when either
/// output is null.
///
/// # Safety
///
/// `out_x` and `out_y` must each be null or point at a writable `f32` for the
/// whole call.
pub(super) extern "C" fn ffi_input_mouse_delta(out_x: *mut f32, out_y: *mut f32) -> u8 {
    if out_x.is_null() || out_y.is_null() {
        return 0;
    }
    let mouse_delta = with_active_world(|world| {
        world
            .get_resource::<Input>()
            .map_or(Vector2f::ZERO, |input| input.mouse_delta())
    })
    .unwrap_or(Vector2f::ZERO);
    // SAFETY: both pointers were checked non-null above, and the caller's
    // contract makes them point at writable `f32` values.
    unsafe {
        *out_x = mouse_delta.x;
        *out_y = mouse_delta.y;
    }
    1
}

/// Returns the frame's time step in seconds, capped at the engine's maximum,
/// or `0.0` with no active invocation.
///
/// Gameplay advances by this value instead of by wall-clock time, and the cap
/// keeps a stalled frame from moving a managed simulation too far.
pub(super) extern "C" fn ffi_time_delta_seconds() -> f32 {
    with_active_world(|world| world.get_resource::<Time>().map(Time::delta_seconds))
        .flatten()
        .unwrap_or(0.0)
}

/// Returns the seconds since the project started, or `0.0` with no active
/// invocation.
///
/// This is wall-clock time, for animation that must not advance with the
/// frame delta.
pub(super) extern "C" fn ffi_time_elapsed_seconds() -> f32 {
    with_active_world(|world| world.get_resource::<Time>().map(Time::elapsed_seconds))
        .flatten()
        .unwrap_or(0.0)
}
