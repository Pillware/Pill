use pill_core::*;
//use pill_graphics::{Renderer, RendererError};
use crate::{graphics::renderer::Pill_Renderer, scene::Scene};
use crate::gameobject::GameObject;
//use crate::resource_manager::ResourceManager;
use crate::input::input_event::InputEvent;


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

pub trait Pill_Game { 
    fn initialize(&self);
    fn update(&self);
}

pub struct Engine {
    
    // General
    game : Box<dyn Pill_Game>,
    pub renderer: Box<dyn Pill_Renderer>,// Rc<dyn Pill_Renderer>, 
    scene:  Option<Box<Scene>>, // Box<Scene>  //
    //resource_manager: Box<ResourceManager>

    // Input
    input_queue: VecDeque<InputEvent>,

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

        println!("[Engine] Init");
        self.renderer.initialize();
        self.game.initialize();

        // https://stackoverflow.com/questions/36936221/pass-self-reference-to-contained-objects-function
        // https://users.rust-lang.org/t/cannot-move-out-of-self-which-is-behind-a-mutable-reference/56447

        //self.scene = Some(Box::new(Scene::new(Box::new(self.renderer), String::from("TestScene"),)));
        //let renderer = &mut *self.renderer;

        //let ren = Rc::new(self.renderer);
        self.scene = Some(Box::new(Scene::new(String::from("TestScene"))));
        //self.scene = Some(Box::new(Scene::new(Rc::clone(&self.renderer), String::from("TestScene"))));

        println!("[Engine] Creating testing gameobjects in scene {}", self.scene.as_ref().unwrap().name);




        let object1: Rc<RefCell<GameObject>> = self.scene.as_mut().unwrap().create_gameobject(
            &mut self.renderer,
        String::from("TestGameObject_1"), 
    Box::new(Path::new("D:\\Programming\\Rust\\pill_project\\pill_engine\\pill\\src\\graphics\\res\\models\\cube.obj")),
        );

        object1.borrow_mut().set_position(cgmath::Vector3 { x: 0.0, y: 1.0, z: 0.0 });




        let object2: Rc<RefCell<GameObject>> = self.scene.as_mut().unwrap().create_gameobject(
            &mut self.renderer,
        String::from("TestGameObject_2"), 
    Box::new(Path::new("D:\\Programming\\Rust\\pill_project\\pill_engine\\pill\\src\\graphics\\res\\models\\cube.obj")),
        );

        object2.borrow_mut().set_position(cgmath::Vector3 { x: 2.5, y: -0.3, z: 0.0 });

        println!("[Engine] Init done");
    }

    #[cfg(feature = "standalone")]
    pub fn update(&mut self, dt: std::time::Duration) {
        println!("[Engine] Starting new frame");

        self.game.update();


        match self.renderer.render(self.scene.as_ref().unwrap().as_ref(), dt) {
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
