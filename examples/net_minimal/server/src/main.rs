use log::info;
use pill_core::Result;
use pill_core::{server_broadcast_exit, server_dying_grasp};
use pill_engine::internal::{
    networking_system_server, Engine, EngineConfig, NetworkEntityState, NetworkSide,
    NetworkStateComponent, PillProject, TransformComponent,
};
use pill_engine::internal::{EngineProcessInfo, NetworkManagerComponent};
use std::io::Write;
use std::time::{Duration, Instant};

fn spawn_player(
    engine: &mut Engine,
    network_state_component: &NetworkStateComponent,
    transform: &TransformComponent,
) -> Result<()> {
    let my_id = engine
        .get_global_component_mut::<NetworkManagerComponent>()?
        .my_id;
    let scene = engine.get_active_scene_handle()?;
    println!(
        "[SERVER] Spawning PLAYER with nid{ } for cid {} with transform {:?}",
        network_state_component.network_entity_id, my_id, transform
    );

    let entity = engine.create_entity(scene)?;

    let mut network_state = network_state_component.clone();
    network_state.state = NetworkEntityState::Alive;

    engine.add_component_to_entity(scene, entity, network_state)?;

    engine.add_component_to_entity(scene, entity, *transform)?;

    // TODO: missing playerTag and targetTransform components

    println!(
        "[SERVER] Spawn finished with nid{ } for cid {} with transform {:?}",
        network_state_component.network_entity_id, my_id, transform
    );
    Ok(())
}

struct HeadlessProject; // TODO: placeholder for the actual project struct
                        //
impl PillProject for HeadlessProject {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        println!("Starting HeadlessProject...");

        let scene = engine.create_scene("ServerWorld")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<NetworkStateComponent>(scene)?;

        let mut network_manager = NetworkManagerComponent::new_server("0.0.0.0:5000", 8)?;

        network_manager
            .spawn_handlers
            .insert("player".into(), spawn_player);
        engine.add_global_component(network_manager)?;

        engine.add_system("NetworkingSystemServer", networking_system_server)?;

        log::info!("Server listening on 0.0.0.0:5000");

        Ok(())
    }
}

fn main() -> Result<()> {
    #[cfg(debug_assertions)]
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format(|buf, record| {
            writeln!(
                buf,
                "[{}] {} {}:{}: {}",
                record.level(),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        })
        .filter_level(log::LevelFilter::Info)
        .init();

    let project: Box<dyn PillProject> = Box::new(HeadlessProject);
    let compile_mode =
        std::env::var("PILL_COMPILE_MODE").map_err(|_| "PILL_COMPILE_MODE is not set")?;
    let process = EngineProcessInfo::new(&compile_mode, pill_engine::internal::BuildTarget::Native);
    let mut engine = Engine::new(project, EngineConfig::from_ini(""), process);

    engine.initialize(None)?;

    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = tx.send(());
    })
    .expect("Error setting Ctrl-C handler");

    let tick = Duration::from_millis(1000 / 60); // 60 FPS

    let mut last = Instant::now();

    info!("Starting headless pill project loop...");

    loop {
        // graceful shutdown on Ctrl-C
        if rx.try_recv().is_ok() {
            info!("Shutdown requested, broadcasting Exit");
            if let Ok(network_manager) =
                engine.get_global_component_mut::<NetworkManagerComponent>()
            {
                if let NetworkSide::Server(state) = &mut network_manager.side {
                    let _ = server_broadcast_exit(&mut state.net, "Server shutting down");
                    let _ =
                        server_dying_grasp(&mut state.net, std::time::Duration::from_millis(500));
                }
            }
            break Ok(());
        }

        let now = Instant::now();
        if now.duration_since(last) >= tick {
            last += tick;

            // drive networking, simulation
            engine.update(tick);
            //println!("Pill project updated at {:?}", last);
        } else {
            // sleep to avoid busy waiting
            std::thread::sleep(tick - now.duration_since(last));
        }
    }
}
