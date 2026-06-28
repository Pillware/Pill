mod shared;

#[cfg(all(not(feature = "benchmark_window"), not(feature = "benchmark_headless")))]
#[path = "game.rs"]
mod project;

#[cfg(any(feature = "benchmark_window", feature = "benchmark_headless"))]
#[path = "benchmark.rs"]
mod project;

use pill_engine::project::*;

create_project!(crate::project::Project {}, PillProject);
