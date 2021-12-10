use std::path::PathBuf;

use pill_engine::{game::*, internal::{Material, MaterialHandle, MeshHandle, Mesh}};

pub struct Game { }   

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) {
        println!("Let's play pong"); 

        // Create scene
        let scene = engine.create_scene("Default").unwrap();
        engine.set_active_scene(scene).unwrap();

        // Add systems
        engine.add_system("Paddle movement", paddle_movement_system).unwrap();

        // Register components
        engine.register_component::<TransformComponent>(scene).unwrap();
        engine.register_component::<MeshRenderingComponent>(scene).unwrap();

        
        let active_scene = engine.get_active_scene().expect("Scene not found");//.unwrap();


        //engine.l


        // Create entity
        let paddle_1 = engine.create_entity(active_scene).unwrap();

        // Add transform component
        let transform_1 = TransformComponent::default();
        engine.add_component_to_entity::<TransformComponent>(active_scene, paddle_1, transform_1).unwrap();

        // Add mesh rendering component
        let material_1 = Material::default(engine, "TestMaterial").unwrap();
        let material_1_handle = engine.add_resource::<MaterialHandle, Material>("TestMaterial", material_1).unwrap(); // [TODO] Remove requirement of name here, and assure that resource always has name and take it from there (using trait)
        
        
        //println!("{} .... {}", std::env::current_dir().unwrap().display(), PathBuf::from("../res/models/Monkey.obj").display());
        let mesh_1_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Monkey.obj");// PathBuf::from("../res/models/Monkey.obj")
        println!("{}", mesh_1_path.display());


        let mesh_1 = Mesh::new(engine, "TestMesh", mesh_1_path).unwrap();
        let mesh_1_handle = engine.add_resource::<MeshHandle, Mesh>("TestMesh", mesh_1).unwrap();

        let mut mesh_rendering_1 = MeshRenderingComponent::default();
        mesh_rendering_1.assign_material(engine, &material_1_handle).unwrap();
        mesh_rendering_1.assign_mesh(engine, &mesh_1_handle).unwrap();


        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, paddle_1, mesh_rendering_1).unwrap();
    }
}

fn paddle_movement_system(engine: &mut Engine) {
    println!("Moving paddles"); 


   
}