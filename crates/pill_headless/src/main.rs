
use anyhow::Result;
use pill_engine::{Engine, PillGame, TransformComponent};
use log::info;
use std::time::{Duration, Instant};
use env_logger;
use std::io::Write;

#[cfg(feature = "net")]
use pill_engine::{NetState, SpawnQueueComponent};

struct HeadlessGame; // TODO: placeholder for the actual game struct
                     //
impl PillGame for HeadlessGame {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        println!("Starting HeadlessGame...");

        let scene = engine.create_scene("ServerWorld")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;

        #[cfg(feature = "net")]
        {
            engine.add_global_component(NetState::new_server("0.0.0.0:5000", 8)?)?;
            engine.add_global_component(SpawnQueueComponent::default())?;

            log::info!("Server listening on 0.0.0.0:5000");
        }

        Ok(())
    }
}

fn main() -> Result<()> {
    let log_level = log::LevelFilter::Info; // TODO: always use Info filter

    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            writeln!(buf, "[{}] {} {}:{}: {}",
                record.level(),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        })
        .filter_level(log::LevelFilter::Info)
        .init();

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
            //println!("Game updated at {:?}", last);
        } else {
            // sleep to avoid busy waiting
            std::thread::sleep(tick - now.duration_since(last));
        }
    }
}
