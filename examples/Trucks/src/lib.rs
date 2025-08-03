mod game;
mod free_camera;
mod player_movement;
mod player_physics_movement;
use pill_engine::game::create_game;

create_game!(crate::game::Game {}, pill_engine::game::PillGame);