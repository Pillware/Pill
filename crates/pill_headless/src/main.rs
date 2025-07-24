
use anyhow::Result;
use pill_engine::{Engine, PillGame};
use log::info;
use std::time::{Duration, Instant};
use env_logger;

struct HeadlessGame; // TODO: placeholder for the actual game struct
                     //
impl PillGame for HeadlessGame {
    fn start(&self, _engine: &mut Engine) -> Result<()> {
        // Placeholder for the game start logic
        println!("Starting HeadlessGame...");
        Ok(())
    }
}

fn main() -> Result<()> {
    env_logger::init();

    let mut cfg = config::Config::default();

    let game: Box<dyn PillGame> = Box::new(HeadlessGame);
    let mut engine = Engine::new(game, cfg);

    // TODO: do I need to set the runtime run mode?
    engine.initialize(None)?;

    let tick = Duration::from_millis(1000 / 60); // 60 FPS
    let mut last = Instant::now();

    info!("Starting headless game loop...");

    loop {
        let now = Instant::now();
        if now.duration_since(last) >= tick {
            last += tick;

            // drive networking, simulation
            engine.update(tick);
        } else {
            // sleep to avoid busy waiting
            std::thread::sleep(tick - now.duration_since(last));
        }
    }
}
