
use anyhow::Result;
use pill_engine::internal::{Engine, PillGame, TransformComponent, NetworkStateComponent, NetworkSide, NetworkEntityState, networking_system_server};
use pill_core::{server_broacast_exit, server_dying_grasp};
use log::info;
use std::time::{Duration, Instant};
use env_logger;
use std::io::Write;

#[cfg(feature = "networking")]
use pill_engine::internal::{NetworkManagerComponent};

fn spawn_player(engine: &mut Engine, network_state_component: &NetworkStateComponent, transform: &TransformComponent) -> Result<()> {
    let my_id = engine.get_global_component_mut::<NetworkManagerComponent>()?.my_id;
    let scene = engine.get_active_scene_handle()?;
    println!("[SERVER] Spawning PLAYER with nid{ } for cid {} with transform {:?}", network_state_component.network_entity_id, my_id, transform);

    let network_entity_id = network_state_component.network_entity_id;

    let entity = engine.create_entity(scene)?;

	let mut network_state = network_state_component.clone();
	network_state.state = NetworkEntityState::Alive;

    engine.add_component_to_entity(scene, entity, network_state)?;

    engine.add_component_to_entity(scene, entity, *transform)?;

    // TODO: missing playerTag and targetTransform components

    println!("[SERVER] Spawn finished with nid{ } for cid {} with transform {:?}", network_state_component.network_entity_id, my_id, transform);
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

        #[cfg(feature = "networking")]
        {
            let mut network_manager = NetworkManagerComponent::new_server("0.0.0.0:5000", 8)?;

            network_manager.spawn_handlers.insert("player".into(), spawn_player);
            engine.add_global_component(network_manager)?;

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

    let mut config = config::Config::default();

    let game: Box<dyn PillGame> = Box::new(HeadlessGame);
    let mut engine = Engine::new(game, config);

    engine.initialize(None)?;

    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || { let _ = tx.send(()); }).expect("Error setting Ctrl-C handler");

    let tick = Duration::from_millis(1000 / 60); // 60 FPS

    let mut last = Instant::now();

    info!("Starting headless game loop...");

    loop {
        // graceful shutdown on Ctrl-C
        if rx.try_recv().is_ok() {
            info!("Shutdown requested, broadcasting Exit");
            if let Ok(mut network_manager) = engine.get_global_component_mut::<NetworkManagerComponent>() {
                if let NetworkSide::Server(state) = &mut network_manager.side {
                    let _ = server_broacast_exit(&mut state.server, "Server shutting down");
                    let _ = server_dying_grasp(&mut state.server, std::time::Duration::from_millis(500));
                }
            }
            break Ok(());
        }

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
