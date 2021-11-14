use crate::{ 
    resources::*,
    ecs::*,
    graphics::*,
    input::*,
};

use std::collections::VecDeque;
use anyhow::{Context, Result, Error};

use pill_core::EngineError;
use winit::{ // Import dependencies
    event::*, // Bring all public items into scope
    dpi::PhysicalPosition,
};

// ---------------------------------------------------------------------




pub type Game = Box<dyn Pill_Game>;
pub trait Pill_Game { 
    fn initialize(&self, engine: &mut Engine);
    fn update(&self, engine: &mut Engine);
}





pub struct Engine {
    
    // General
    game: Option<Game>,
    renderer: Renderer,

    // Scenes
    scene_manager: SceneManager, // [TODO: What will happen with objects registered in renderer if we change the scene for which they were registered?]
    
    // Input
    input_queue: VecDeque<InputEvent>,

    // Resources
    resource_manager: ResourceManager,
}

impl Engine {

    // Functions for Standalone
    #[cfg(feature = "internal")]
    pub fn new(game: Box<dyn Pill_Game>, renderer: Box<dyn Pill_Renderer>) -> Self {
        Self { 
            game: Some(game),
            renderer,
            scene_manager: SceneManager::new(),

            input_queue: VecDeque::new(),
            resource_manager: ResourceManager::new(),
        }
    }

    #[cfg(feature = "internal")]
    pub fn initialize(&mut self) {

        self.renderer.initialize(); // [TODO] Needed? Initialization should happen in constructor?
        
        self.initialize_game();



        


        //entity_1_transform_component.position = cgmath::Vector3 { x: 0.0, y: 1.0, z: 0.0 };

        // let entity_1_mesh_rendering_component = add_component_to_entity::<MeshRenderingComponent>(self.scene.as_mut().unwrap(), entity_1);
        



        // let entity_2 = create_entity(self.scene.as_mut().unwrap());
        
        // let entity_2_transform_component = add_component_to_entity::<TransformComponent>(self.scene.as_mut().unwrap(), entity_2); 
        // entity_2_transform_component.position = cgmath::Vector3 { x: 2.5, y: -0.3, z: 0.0 };

        // let entity_2_mesh_rendering_component = add_component_to_entity::<MeshRenderingComponent>(self.scene.as_mut().unwrap(), entity_2);

        println!("[Engine] Initialization completed");
    }


    // ------------------------------ GAME ------------------------------

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        println!("[Engine] Creating scene: {}", name);
        self.scene_manager.create_scene(name).context("[Engine] Scene creation failed")
    }

    pub fn print_debug_message(&self) {
        println!("Engine here!");
    }

    pub fn get_current_scene(&mut self) -> Result<SceneHandle> {
        self.scene_manager.get_current_scene()
    }

    pub fn create_entity(&mut self, scene: SceneHandle) -> Result<EntityHandle> {
        self.scene_manager.create_entity(scene)
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {
        println!("[Scene] Adding component {:?} to entity {} in scene {}", std::any::type_name::<T>(), entity.index, scene.index);
        self.scene_manager.add_component_to_entity::<T>(scene, entity, component)
    }

    pub fn set_current_scene(&mut self, scene: SceneHandle) -> Result<()> {
        self.scene_manager.set_current_scene(scene)
    }

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle) -> Result<()> {
        self.scene_manager.register_component::<T>(scene)
    }

    // ----------------------------- ENGINE INTERNAL -----------------------------

    // pub fn load_resource<T: Resource>(&mut self, t: T, path: String, source: ResourceSource) {
    //     self.resource_manager.load_resource(t, path, source)
    // }

    pub fn initialize_game(&mut self) {
        let game = self.game.take().unwrap(); // Take game memory out of Engine, we can do this because game is an Option  
        game.initialize(self); // Run initialize function on Game, Engine passed in parameter will contain Option<Game> with None value   
        self.game = Some(game);  // After update is finished, return memory of Game to the Engine's game variable 
    }

    // --------------------------- STANDALONE ---------------------------

    #[cfg(feature = "internal")]
    pub fn update(&mut self, dt: std::time::Duration) {
        //self.game.update(self);
       // self.game_manager.update_game(self);
       //Engine::get_game(&mut self.game_manager).update(self);//.update_game(self);
        //Self::get_game(&mut self.game_manager).update(self);

        //let game = &self.game_manager.game;
        //let manager = Engine::get_game_manager(self);
        //manager.game.update(self);

        //let game = &self.game_manager.get_game() .game;
        //game.update(self);
      //  .update(self);

        let scene_handle = self.scene_manager.get_current_scene().unwrap();
        let scene = self.scene_manager.get_scene_mut(scene_handle).unwrap();

        match self.renderer.render(scene, dt) {
            Ok(_) => {}
            // Recreate the swap_chain if lost
            //Err(RendererError::SwapChainLost) => self.renderer.resize(self.renderer.state.window_size),
            // The system is out of memory, we should probably quit
            //Err(RendererError::SwapChainOutOfMemory) => *control_flow = ControlFlow::Exit,
            // All other errors (Outdated, Timeout) should be resolved by the next frame
            Err(e) => eprintln!("{:?}", e),
        }

        println!("[Engine] Frame finished (duration: {:?})", dt);
    }

    #[cfg(feature = "internal")]
    pub fn shutdown(&mut self) {
        println!("[Engine] Shutting down");
    }

    #[cfg(feature = "internal")]
    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        println!("[Engine] Resizing");
        self.renderer.resize(new_size);
    }

    #[cfg(feature = "internal")]
    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        let key: VirtualKeyCode = keyboard_input.virtual_keycode.unwrap();
        let state: ElementState = keyboard_input.state;

        let input_event = InputEvent::KeyboardKey { key: key, state: state };
        self.input_queue.push_back(input_event);

        println!("[Engine] Got new keyboard key input: {:?} {:?}", key, state);
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseKey { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);

        println!("[Engine] Got new mouse key input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);


        println!("[Engine] Got new mouse wheel input");
    }

    #[cfg(feature = "internal")]
    pub fn pass_mouse_motion_input(&mut self, position: &PhysicalPosition<f64>) {
        let input_event = InputEvent::MouseMotion { position: *position };
        self.input_queue.push_back(input_event);

        println!("[Engine] Got new mouse motion input");
    }

    // Functions for Engine's built-in systems
    pub fn get_input_queue(&self) -> &VecDeque<InputEvent> {
        &self.input_queue
    }
}







// pub fn create_entity(scene: &mut Scene) -> &Entity {
//     let entity = Entity { 
//         name: String::from("Hello"),
//         index: scene.entity_counter,
//     };
//     scene.entities.insert( scene.entity_counter, entity);
//     scene.entity_counter += 1;

//     scene.entities.last().unwrap()
// }


// pub fn create_entity(scene: &mut Scene) -> &Entity {
//     let entity = Entity { 
//         name: String::from("Hello"),
//         index: scene.entity_counter,
//     };

//     self.entities.insert( self.entity_counter, entity);
//     self.entity_counter += 1;

//     self.entities.last().unwrap()
// }



// pub fn create_entity<'a>(scene: &'a mut Scene)-> Entity { // Instead returning reference to entity and handling it in game (which may cause problems) return EntityHandle storing index of entity in vector
//     let x = Entity {name:"aa".to_string(), index: 0 };
//     x
//     //scene.create_entity()
// }

// pub fn register_system() {

// }





// pub fn add_component_to_entity<'a, T: Component>(scene: &'a mut Scene, entity_handle: EntityHandle) -> &'a mut T {
//     // We need to specify to which collection of components new component should be added
//     // The problem is that in this function we don't know it because we need to get proper collection first
//     // To do this we may use match but problem with it is that when component is added in game code there is no way to create new match arm in code
//     // We can define trait function and implement it for all types of components, but this will require from game developer to do this also it game components

//     // IMPORTANT DESIGN TOPIC: 
//     // How to design component storing? 
//     // On engine side we can precreate collection (vector) for each of built-in component type, but what if game developer creates game-side component?
//     // We need something dynamic, like list of vectors to which we can add new vector for new component when registering it in the engine (but is such data structure effective?)
//     // Maybe try register pattern? - hash map where type is a key and vector is value? (In C++ type as key and pointer to value as vector would be good, but in Rust pointers should be avoided)

//     println!("[Scene] Adding component {:?} to entity {} in scene {}", std::any::type_name::<T>(), entity_handle.index, scene.name);
//     let component: &mut T = T::new(scene, entity_handle);
//     component
// }