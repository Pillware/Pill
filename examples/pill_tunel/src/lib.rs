mod game;
pub use game::WebGame;

use pill_engine::game::create_game;
create_game!(crate::game::WebGame {}, pill_engine::game::PillGame);
