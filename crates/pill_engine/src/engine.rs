use std::{any::type_name, collections::VecDeque};
use anyhow::{Context, Result, Error};
use boolinator::Boolinator;
use log::{debug, info, error};
use typemap_rev::TypeMapKey;
use winit::{ event::*, dpi::PhysicalPosition,};

use pill_core::{EngineError, get_type_name};
use crate::{ 
    resources::*,
    ecs::*,
    graphics::*,
    input::*,
};

// ---------------------------------------------------------------------

pub type Game = Box<dyn PillGame>;
pub trait PillGame { 
    fn start(&self, engine: &mut Engine);
}
pub struct Engine { 
    game: Option<Game>,
    renderer: Renderer,
    scene_manager: SceneManager, // [TODO: What will happen with objects registered in renderer if we change the scene for which they were registered?]
    system_manager: SystemManager,
    resource_manager: ResourceManager,
    input_queue: VecDeque<InputEvent>,
}

// ---- INTERNAL -----------------------------------------------------------------

impl Engine {

    #[cfg(feature = "internal")]
    pub fn new(game: Box<dyn PillGame>, renderer: Box<dyn PillRenderer>) -> Self {
        Self { 
            game: Some(game),
            renderer,
            scene_manager: SceneManager::new(),
            system_manager: SystemManager::new(),
            resource_manager: ResourceManager::new(),
            input_queue: VecDeque::new(),
        }
    }

    #[cfg(feature = "internal")]
    pub fn initialize(&mut self) {
        info!("Pill Engine initializing");

        self.renderer.initialize(); // [TODO] Needed? Initialization should happen in constructor?
        
        // Add built-in systems


        // Initialize game
        let game = self.game.take().unwrap(); 
        game.start(self); 
        self.game = Some(game);
    }

    #[cfg(feature = "internal")]
    pub fn update(&mut self, dt: std::time::Duration) {

        // Run systems
        let update_phase_count = self.system_manager.update_phases.len();
        for i in (0..update_phase_count).rev() {
            let systems_count = self.system_manager.update_phases[i].len();
            for j in (0..systems_count).rev() {
                let system = self.system_manager.update_phases[i][j];
                system(self);
            }
        }
 
        // [TODO] Move render to rendering system 
        let scene_handle = self.scene_manager.get_active_scene().unwrap();
        let scene = self.scene_manager.get_scene_mut(scene_handle).unwrap();

        match self.renderer.render(scene, dt) {
            Ok(_) => {}
            // Recreate the swap_chain if lost
            //Err(RendererError::SwapChainLost) => self.renderer.resize(self.renderer.state.window_size),
            // The system is out of memory, we should probably quit
            //Err(RendererError::SwapChainOutOfMemory) => *control_flow = ControlFlow::Exit,
            // All other errors (Outdated, Timeout) should be resolved by the next frame
            Err(e) => error!("{:?}", e),
        }

        info!("Frame finished (duration: {:?})", dt);
    }

    #[cfg(feature = "internal")]
    pub fn shutdown(&mut self) {
        info!("Shutting down");
    }

    #[cfg(feature = "internal")]
    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        info!("Resizing");
        self.renderer.resize(new_size);
    }

    #[cfg(feature = "internal")]
    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        let key: VirtualKeyCode = keyboard_input.virtual_keycode.unwrap();
        let state: ElementState = keyboard_input.state;

        let input_event = InputEvent::KeyboardKey { key: key, state: state };
        self.input_queue.push_back(input_event);
        info!("Got new keyboard key input: {:?} {:?}", key, state);
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseKey { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);
        info!("Got new mouse key input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);
        info!("Got new mouse wheel input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_motion_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MouseMotion { position: *position };
        self.input_queue.push_back(input_event);
        info!("Got new mouse motion input");
    }

    #[cfg(feature = "internal")]
    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }
}

// ---------------------------------------------------------------------

impl Engine { 

    // --- ECS

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        info!("Creating scene: {}", name);
        self.scene_manager.create_scene(name).context("Scene creation failed")
    }

    pub fn get_active_scene(&mut self) -> Result<SceneHandle> {
        self.scene_manager.get_active_scene().context("Getting active scene failed")
    }

    pub fn create_entity(&mut self, scene: SceneHandle) -> Result<EntityHandle> {
        self.scene_manager.create_entity(scene).context("Creating entity failed")
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {
        info!("Adding component {} to entity {} in scene {}", get_type_name::<T>(), entity.index, scene.index);
        self.scene_manager.add_component_to_entity::<T>(scene, entity, component).context("Adding component to entity failed")
    }

    pub fn set_active_scene(&mut self, scene: SceneHandle) -> Result<()> {
        self.scene_manager.set_active_scene(scene).context("Setting active scene failed")
    }

    pub fn register_component<T: TypeMapKey<Value = u32> + Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle) -> Result<()> {
        self.scene_manager.register_component::<T>(scene).context("Registering component failed")
    }

    pub fn add_system(&mut self, name: &str, system_function: fn(engine: &mut Engine)) -> Result<()> {
        self.system_manager.add_system(name, system_function, UpdatePhase::Game).context("Adding system failed")
    }
    
    // --- RESOURCES

    // pub fn load_resource<T: Resource>(&mut self, t: T, path: String, source: ResourceSource) {
    //     self.resource_manager.load_resource(t, path, source)
    // }

}