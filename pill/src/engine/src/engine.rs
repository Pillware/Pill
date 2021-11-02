use pill_core::*;



use crate::ecs::component::Component;
use crate::ecs::entity::Entity;
//use pill_graphics::{Renderer, RendererError};
use crate::{graphics::renderer::Pill_Renderer, scene::Scene, graphics::renderer::Renderer};
use crate::gameobject::GameObject;
//use crate::resource_manager::ResourceManager;
use crate::input::input_event::InputEvent;
use crate::ecs::mesh_rendering_component::MeshRenderingComponent;

use std::collections::VecDeque;
use std::path::Path;
use winit::{ // Import dependencies
    event::*, // Bring all public items into scope
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use std::rc::Rc;
use std::cell::RefCell;

// ---------------------------------------------------------------------

pub type Game = Box<dyn Pill_Game>;

pub trait Pill_Game { 
    fn initialize(&self);
    fn update(&self);
}

pub struct Engine {
    
    // General
    game : Game,
    pub renderer: Renderer,// Rc<dyn Pill_Renderer>, 
    scene: Option<Scene>, // [TODO: What will happen with objects registered in renderer if we change the scene for which they were registered?]
    //resource_manager: Box<ResourceManager>

    // Input
    input_queue: VecDeque<InputEvent>,

    // Resources

    

}

impl Engine {

    // Functions for Standalone
    #[cfg(feature = "standalone")]
    pub fn new(game: Box<dyn Pill_Game>, renderer: Box<dyn Pill_Renderer>) -> Self { 
        println!("[Engine] Creating...");
        return Engine { 
            game,
            renderer,
            scene: None,
            //resource_manager: ResourceManager::new(renderer),
            input_queue: VecDeque::new(),
            
        }; 
    }

    #[cfg(feature = "standalone")]
    pub fn initialize(&mut self) {
        use crate::ecs::transform_component::TransformComponent;


        println!("[Engine] Init");
        self.renderer.initialize();
        self.game.initialize();

        // https://stackoverflow.com/questions/36936221/pass-self-reference-to-contained-objects-function
        // https://users.rust-lang.org/t/cannot-move-out-of-self-which-is-behind-a-mutable-reference/56447

        //self.scene = Some(Box::new(Scene::new(Box::new(self.renderer), String::from("TestScene"),)));
        //let renderer = &mut *self.renderer;

        //let ren = Rc::new(self.renderer);
        self.scene = Some(Scene::new(String::from("TestScene")));
        //self.scene = Some(Box::new(Scene::new(Rc::clone(&self.renderer), String::from("TestScene"))));



        println!("[Engine] Creating testing gameobjects in scene {}", self.scene.as_ref().unwrap().name);

        //self.create_entity(self.scene.as_mut().unwrap());

        
        let object1 = create_entity(self.scene.as_mut().unwrap()); // Returns entity which is bound to scene so scene is still borrowed after this function ends!
        
        add_component_to_entity::<TransformComponent>(self.scene.as_mut().unwrap(), &object1); 

        let object2 = create_entity(self.scene.as_mut().unwrap());


    //     let object1: Rc<RefCell<GameObject>> = self.scene.unwrap().create_gameobject(
    //         &mut self.renderer,
    //     String::from("TestGameObject_1"), 
    // Box::new(Path::new("D:\\Programming\\Rust\\pill_project\\pill_engine\\pill\\src\\graphics\\res\\models\\cube.obj")),
    //     );

    //     object1.borrow_mut().set_position(cgmath::Vector3 { x: 0.0, y: 1.0, z: 0.0 });




        let object2: Rc<RefCell<GameObject>> = self.scene.as_mut().unwrap().create_gameobject(
            &mut self.renderer,
        String::from("TestGameObject_2"), 
    Box::new(Path::new("D:\\Programming\\Rust\\pill_project\\pill_engine\\pill\\src\\graphics\\res\\models\\cube.obj")),
        );

        object2.borrow_mut().set_position(cgmath::Vector3 { x: 2.5, y: -0.3, z: 0.0 });

        println!("[Engine] Init done");
    }


    // ------------------------------ GAME ------------------------------

    

    // --------------------------- STANDALONE ---------------------------

    #[cfg(feature = "standalone")]
    pub fn update(&mut self, dt: std::time::Duration) {
        println!("[Engine] Starting new frame");

        self.game.update();


        match self.renderer.render(self.scene.as_ref().unwrap(), dt) {
            Ok(_) => {}
            // Recreate the swap_chain if lost
            //Err(RendererError::SwapChainLost) => self.renderer.resize(self.renderer.state.window_size),
            // The system is out of memory, we should probably quit
            //Err(RendererError::SwapChainOutOfMemory) => *control_flow = ControlFlow::Exit,
            // All other errors (Outdated, Timeout) should be resolved by the next frame
            Err(e) => eprintln!("{:?}", e),
        }

        println!("[Engine] Update (frame duration {:?})", dt);
    }

    #[cfg(feature = "standalone")]
    pub fn shutdown(&mut self) {
        println!("[Engine] Shutting down");
    }

    #[cfg(feature = "standalone")]
    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        println!("[Engine] Resizing");
        self.renderer.resize(new_size);
    }

    #[cfg(feature = "standalone")]
    pub fn pass_keyboard_key_input(&mut self, keyboard_input: &KeyboardInput) {
        let key: VirtualKeyCode = keyboard_input.virtual_keycode.unwrap();
        let state: ElementState = keyboard_input.state;

        let input_event = InputEvent::KeyboardKey { key: key, state: state };
        self.input_queue.push_back(input_event);

        println!("[Engine] Got new keyboard key input: {:?} {:?}", key, state);
    }

    #[cfg(feature = "standalone")]
    pub fn pass_mouse_key_input(&mut self, key: &MouseButton, state: &ElementState) {
        let input_event = InputEvent::MouseKey { key: *key, state: *state }; // Here using * we actually are copying the value of key because MouseButton implements a Copy trait
        self.input_queue.push_back(input_event);

        println!("[Engine] Got new mouse key input");
    }

    #[cfg(feature = "standalone")]
    pub fn pass_mouse_wheel_input(&mut self, delta: &MouseScrollDelta) {
        let input_event = InputEvent::MouseWheel { delta: *delta };
        self.input_queue.push_back(input_event);


        println!("[Engine] Got new mouse wheel input");
    }

    #[cfg(feature = "standalone")]
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

pub fn create_entity(scene: &mut Scene) -> &Entity  { // Instead returning reference to entity and handling it in game (which may cause problems) return EntityHandle storing index of entity in vector
    //let x = Entity {name:"aa".to_string(), index: 0 };
    
    scene.create_entity()
}

// pub fn create_entity<'a>(scene: &'a mut Scene)-> Entity { // Instead returning reference to entity and handling it in game (which may cause problems) return EntityHandle storing index of entity in vector
//     let x = Entity {name:"aa".to_string(), index: 0 };
//     x
//     //scene.create_entity()
// }

pub fn register_system() {

}

pub fn xxxa<'a>(scene: &'a mut Scene) -> &'a String {
    &scene.test
}

pub fn add_component_to_entity<'a, T: Component>(scene: &'a mut Scene, entity: &Entity) -> &'a T {
    // We need to specify to which collection of components new component should be added
    // The problem is that in this function we don't know it because we need to get proper collection first
    // To do this we may use match but problem with it is that when component is added in game code there is no way to create new match arm in code
    // We can define trait function and implement it for all types of components, but this will require from game developer to do this also it game components

    // IMPORTANT DESIGN TOPIC: 
    // How to design component storing? 
    // On engine side we can precreate collection (vector) for each of built-in component type, but what if game developer creates game-side component?
    // We need something dynamic, like list of vectors to which we can add new vector for new component when registering it in the engine (but is such data structure effective?)
    // Maybe try register pattern? - hash map where type is a key and vector is value? (In C++ type as key and pointer to value as vector would be good, but in Rust pointers should be avoided)

    println!("[Scene] Adding component {:?} to entity {} in scene {}", std::any::type_name::<T>(), entity.index, scene.name);
    let component: &T = T::new(scene, entity);
    component
}
