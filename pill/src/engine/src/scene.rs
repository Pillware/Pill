
//use pill_graphics::model::Model;
use crate::gameobject::GameObject;
use crate::engine::Engine;
use crate::graphics::renderer::Pill_Renderer;
use std::path::Path;
use std::collections::LinkedList;
use std::rc::Rc;
use std::cell::RefCell;

pub struct Scene {
    //renderer: Rc<dyn Pill_Renderer>,
    pub name: String,
    pub gameobjectCollection: LinkedList<Rc<RefCell<GameObject>>>,
    //gameobjectCollection: LinkedList<Box<GameObject>>,
}

impl Scene {
    //pub fn new(renderer: Box<Pill_Renderer>, name: String) -> Self {
    //pub fn new(renderer: Pill_Renderer, name: String) -> Self {  
    pub fn new(name: String) -> Self {  
        return Scene { 
            //renderer,
            name,
            gameobjectCollection: LinkedList::new(),
        };
    }

    // Without mut self scene cannot modify itself, eg add objects to list. Rc do not allow for mutable variables inside, try refCell !!!!!!
    // Or check adding &'mut to Option<Box<Scene>>, how these lifetimes work
    pub fn create_gameobject(&mut self, renderer: &mut Box<dyn Pill_Renderer>, name: String, file_path: Box<&Path>) -> Rc<RefCell<GameObject>> {
        println!("[Scene] Creating GameObject from path: {:?}", file_path);
        let new_gameobject = Rc::new(RefCell::new(GameObject::new(renderer, name, file_path)));
        self.gameobjectCollection.push_back(Rc::clone(&new_gameobject));

        //let new_gameobject = Rc::new(GameObject::new(renderer, name, file_path));
        //self.gameobjectCollection.push_back(new_gameobject);


       
        return Rc::clone(&new_gameobject);
    }
}
