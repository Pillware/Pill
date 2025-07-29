mod game;

pub use game::Game;

use std::ffi::c_void;
use pill_engine::game::PillGame;

#[no_mangle]
pub extern "C" fn create_game() -> *mut c_void {
    let game: Box<dyn PillGame> = Box::new(Game {});
    Box::into_raw(Box::new(game)) as *mut c_void
}