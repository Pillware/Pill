mod shared;

#[cfg(all(not(feature = "benchmark_window"), not(feature = "benchmark_headless")))]
#[path = "game.rs"]
mod game;

#[cfg(any(feature = "benchmark_window", feature = "benchmark_headless"))]
#[path = "benchmark.rs"]
mod game;

use pill_engine::game::create_game;

create_game!(crate::game::Game {}, pill_engine::game::PillGame);
