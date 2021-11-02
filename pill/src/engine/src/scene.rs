
//use pill_graphics::model::Model;
use crate::gameobject::GameObject;
use crate::engine::Engine;
use crate::graphics::renderer::Pill_Renderer;
use std::any::TypeId;
use std::convert::TryInto;
use std::path::Path;
use std::collections::LinkedList;
use std::rc::Rc;
use std::cell::RefCell;
use crate::ecs::mesh_rendering_component::MeshRenderingComponent;
use crate::ecs::transform_component::TransformComponent;
use crate::ecs::entity::Entity;
use crate::ecs::component::{self, Component};


pub struct Scene {
    //renderer: Rc<dyn Pill_Renderer>,
    pub name: String,
    pub gameobjectCollection: LinkedList<Rc<RefCell<GameObject>>>,
    //gameobjectCollection: LinkedList<Box<GameObject>>,

    pub test: String,

    // ECS
    pub entity_counter: usize,
    pub entities:  Vec<Entity>,
    pub transform_components: Vec<TransformComponent>,
    mesh_rendering_components: Vec<MeshRenderingComponent>,
}

impl Scene {
    //pub fn new(renderer: Box<Pill_Renderer>, name: String) -> Self {
    //pub fn new(renderer: Pill_Renderer, name: String) -> Self {  
    pub fn new(name: String) -> Self {  
        return Scene { 
            //renderer,
            name,
            gameobjectCollection: LinkedList::new(),

            test: "xcw".to_string(),

            entity_counter: 0,
            entities: Vec::<Entity>::new(),
            transform_components: Vec::<TransformComponent>::new(),
            mesh_rendering_components: Vec::<MeshRenderingComponent>::new(),
        };
    }

    pub fn create_entity(&mut self) -> &Entity {
        let entity = Entity { 
            name: String::from("Hello"),
            index: self.entity_counter,
        };
        self.entities.insert( self.entity_counter, entity);
        self.entity_counter += 1;

        self.entities.last().unwrap()
    }

    // pub fn add_component_to_entity<T: Component>(&mut self, entity: &Entity) -> &T {
    //     println!("[Scene] Adding component {:?} to entity {} in scene {}", std::any::type_name::<T>(), entity.index, self.name);
        

    //     // We need to specify to which collection of components new component should be added
    //     // The problem is that in this function we don't know it because we need to get proper collection first
    //     // To do this we may use match but problem with it is that when component is added in game code there is no way to create new match arm in code
    //     // We can define trait function and implement it for all types of components, but this will require from game developer to do this also it game components

    //     // IMPORTANT DESIGN TOPIC: 
    //     // How to design component storing? 
    //     // On engine side we can precreate collection (vector) for each of built-in component type, but what if game developer creates game-side component?
    //     // We need something dynamic, like list of vectors to which we can add new vector for new component when registering it in the engine (but is such data structure effective?)
    //     // Maybe try register pattern? - hash map where type is a key and vector is value? (In C++ type as key and pointer to value as vector would be good, but in Rust pointers should be avoided)

    //     let component: &T = T::new(self, entity);
    //     component
    // }
    
    pub fn create_gameobject(&mut self, renderer: &mut Box<dyn Pill_Renderer>, name: String, file_path: Box<&Path>) -> Rc<RefCell<GameObject>> {
        println!("[Scene] Creating GameObject from path: {:?}", file_path);
        let new_gameobject = Rc::new(RefCell::new(GameObject::new(renderer, name, file_path)));
        self.gameobjectCollection.push_back(Rc::clone(&new_gameobject));

        //let new_gameobject = Rc::new(GameObject::new(renderer, name, file_path));
        //self.gameobjectCollection.push_back(new_gameobject);


       
        return Rc::clone(&new_gameobject);
    }
}

pub fn c_g(scene: &mut Scene, renderer: &mut Box<dyn Pill_Renderer>, name: String, file_path: Box<&Path>) {
    println!("[Scene] Creating GameObject from path: {:?}", file_path);
    


}