use pill_engine::game::*;

pub struct Game { 

}   

impl Pill_Game for Game {
    fn initialize(&self, engine: &mut Engine) {
        println!("Let's play pong"); 
        engine.print_debug_message();
    }

    fn update(&self, engine: &mut Engine) {
        println!("Updating pong"); 
    }
}