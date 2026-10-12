// Per-game wasm shim — minimal, only the parts that genuinely need to be
// per-game:
//   - the `WebGame` struct from this game's `pill_game` crate
//   - the embedded `config.ini` (wasm has no filesystem at runtime, so the
//     launcher copies the game's res/config.ini next to this lib.rs and we
//     pull the bytes in via include_str!)
// Everything else (panic hook, console_log, canvas wiring, event loop) lives
// in `pill_web::run`. This keeps the per-game cdylib tiny and the runtime
// shape symmetric with `pill_native` (which is also a workspace crate).

use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
// SAFETY: wasm32 is single-threaded, so assuming single-threaded allocator access is sound.
static ALLOC: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

#[wasm_bindgen(start)]
pub fn wasm_main() {
    pill_web::run(pill_game::create_pill_game(), include_str!("../config.ini"));
}
