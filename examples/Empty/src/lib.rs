mod game;
use pill_engine::game::create_game;

create_game!(crate::game::Game {}, pill_engine::game::PillGame);