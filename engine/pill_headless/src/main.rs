
use anyhow::Result;
use pill_engine::internal::{Engine, PillGame, TransformComponent, NetworkStateComponent, NetEntityState, networking_system_server};
use log::info;
use std::time::{Duration, Instant};
use env_logger;
use std::io::Write;

#[cfg(feature = "net")]
use pill_engine::internal::{GlobalNetState};

fn spawn_player(engine: &mut Engine, net_state_component: &NetworkStateComponent, transform: &TransformComponent) -> Result<()> {
    let my_id = engine.get_global_component_mut::<GlobalNetState>()?.my_id;
    let scene = engine.get_active_scene_handle()?;
    println!("[SERVER] Spawning PLAYER with nid{ } for cid {} with transform {:?}", net_state_component.net_entity_id, my_id, transform);

    let net_entity_id = net_state_component.net_entity_id;

    let ent = engine.create_entity(scene)?;

	let mut ns = net_state_component.clone();
	ns.state = NetEntityState::Alive;

    engine.add_component_to_entity(scene, ent, ns)?;

    engine.add_component_to_entity(scene, ent,*transform)?;

    // TODO: missing playerTag and targetTransform components

    println!("[SERVER] Spawn finished with nid{ } for cid {} with transform {:?}", net_state_component.net_entity_id, my_id, transform);
    Ok(())
}


struct HeadlessGame; // TODO: placeholder for the actual game struct
                     //
impl PillGame for HeadlessGame {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        println!("Starting HeadlessGame...");

        let scene = engine.create_scene("ServerWorld")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<NetworkStateComponent>(scene)?;

        #[cfg(feature = "net")]
        {
            let mut net_state = GlobalNetState::new_server("0.0.0.0:5000", 8)?;

            net_state.spawn_handlers.insert("player".into(), spawn_player);
            engine.add_global_component(net_state)?;

            engine.add_system(
                "NetworkingSystemServer",
                networking_system_server,
            )?;

            log::info!("Server listening on 0.0.0.0:5000");
        }

        Ok(())
    }
}

fn main() -> Result<()> {
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
