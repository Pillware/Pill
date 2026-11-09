mod falling_pills;
mod game;
mod mesh;
mod shots;
mod tex_gen;

use pill_engine::game::create_game;

create_game!(crate::game::Game {}, pill_engine::game::PillGame);
