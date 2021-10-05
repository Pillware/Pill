//use pill_graphics::model::Model;
use cgmath::Rotation3;
use cgmath::Zero;
use cgmath::prelude::*;
use std::path::Path;
//use crate::graphics::renderer::Pill_Renderer;




use crate::Engine;
use crate::graphics::renderer::Pill_Renderer;

use std::rc::Rc;

pub struct GameObject {
    pub name: String,
    pub position: cgmath::Vector3<f32>,
    pub rotation: cgmath::Quaternion<f32>,
    pub resource_id: Option<usize>,
    //model: &Model,
}

impl GameObject {
    pub fn new(renderer: &mut Box<dyn Pill_Renderer>, name: String, file_path: Box<&Path>) -> Self { // "../res/models/cube.obj"

        //let res_dir = std::path::Path::new("D:\\Programming\\Rust\\pill_project\\pill\\src\\graphics\\res\\models\\cube.obj"); 
        //let res_dir = std::path::Path::new(env!("OUT_DIR")).join("res"); // Create path from build directory to res directory
        //let model_path = res_dir.join(model_path);


        // Create model in renderer and get reference to it and store it in GameObject
        return GameObject { 
            name,
            position: cgmath::Vector3 { x: 0.0, y: 0.0, z: 0.0 },
            rotation: cgmath::Quaternion::zero(),
            resource_id: Some(renderer.create_model(file_path)),
            //model
        };
    }

    pub fn set_position(&mut self, position: cgmath::Vector3<f32>) {
        self.position = position;
    }

    pub fn print_name(&self) {
        println!("{}", self.name);
    }


}