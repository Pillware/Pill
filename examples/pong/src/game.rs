use std::{path::Component, borrow::BorrowMut};

use cgmath::Transform;
use pill_engine::internal::ComponentStorage;
#[allow(unused_imports, dead_code, unused_variables)]
use pill_engine::{game::*, internal::{Material, MaterialHandle, MeshHandle, Mesh, CameraComponent, Texture, TextureHandle, TextureType, ResourceLoadType, InputComponent}};

pub struct Game { }   

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) {
        println!("Let's play pong"); 

        // Create scene
        let scene = engine.create_scene("Default").unwrap();
        engine.set_active_scene(scene).unwrap();

        // Add systems
        engine.add_system("Paddle movement", paddle_movement_system).unwrap();

        // Register components - NO NEED TO DO THAT ANYMORE
        // engine.register_component::<TransformComponent>(scene).unwrap();
        // engine.register_component::<MeshRenderingComponent>(scene).unwrap();
        // engine.register_component::<CameraComponent>(scene).unwrap();

        let active_scene = engine.get_active_scene_handle().expect("Scene not found");//.unwrap();

        // Add texture
        let texture_1_path = std::env::current_dir().unwrap().join("../examples/pong/res/textures/Camouflage.png");
        println!("{:?}", texture_1_path);
        let texture_1 = Texture::new("TestTexture", TextureType::Color, ResourceLoadType::Path(texture_1_path));
        let texture_1_handle = engine.add_resource::<TextureHandle, Texture>(texture_1).unwrap(); 

        // Add material
        let mut material_1 = Material::new("TestMaterial");
        //material_1.assign_texture(engine, texture_1_handle, TextureType::Color); // [TODO] We cannot assign texture to material if it is not registered??
        let material_1_handle = engine.add_resource::<MaterialHandle, Material>(material_1).unwrap(); // [TODO] Remove requirement of name here, and assure that resource always has name and take it from there (using trait)
        
        // Add mesh
        let mesh_1_path = std::env::current_dir().unwrap().join("../examples/pong/res/models/Cube.obj"); // examples/pong/res/models/Monkey.obj
        let mesh_1 = Mesh::new("TestMesh", mesh_1_path);
        let mesh_1_handle = engine.add_resource::<MeshHandle, Mesh>(mesh_1).unwrap();




        // --- Create camera entity
        let camera_holder = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let camera_transform = TransformComponent::new(
            cgmath::Vector3::<f32>::new(0.0,5.0,10.0), 
            cgmath::Vector3::<f32>::new(-20.0,-90.0,0.0),
               cgmath::Vector3::<f32>::new(1.0,1.0,1.0),
        );
        engine.add_component_to_entity::<TransformComponent>(active_scene, camera_holder, camera_transform).unwrap();
        // Add camera component
        let mut camera = CameraComponent::new(engine).unwrap();
        camera.enabled = true;
        engine.add_component_to_entity::<CameraComponent>(active_scene, camera_holder, camera).unwrap();
        
    


        // --- Create entity
        let paddle_1 = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let transform_1 = TransformComponent::new(
            cgmath::Vector3::<f32>::new(0.0,0.0,0.0), 
            cgmath::Vector3::<f32>::new(0.0, 45.0,0.0),
               cgmath::Vector3::<f32>::new(1.0,3.0,1.0),
        );
        engine.add_component_to_entity::<TransformComponent>(active_scene, paddle_1, transform_1).unwrap();  
        // Add mesh rendering component
        let mut mesh_rendering_1 = MeshRenderingComponent::default();
        mesh_rendering_1.assign_material(engine, &material_1_handle).unwrap();
        mesh_rendering_1.assign_mesh(engine, &mesh_1_handle).unwrap();

        let mut mesh_rendering_2 = MeshRenderingComponent::default();
        mesh_rendering_2.assign_material(engine, &material_1_handle).unwrap();
        mesh_rendering_2.assign_mesh(engine, &mesh_1_handle).unwrap();
       
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, paddle_1, mesh_rendering_1).unwrap();

        // Add another entity through entity builder
        let transform2 = TransformComponent::new(cgmath::Vector3::<f32>::new(5.0,1.0,3.0),
        cgmath::Vector3::<f32>::new(0.0, 1.0,0.0),
           cgmath::Vector3::<f32>::new(1.0,1.0,1.0),);
        engine.build_entity(active_scene).with_component(transform2)
                                         .with_component(mesh_rendering_2)
                                         .build();

        // Engine::build_entity(engine, scene).with_component(transform2)
        //                                         .with_component(mesh_rendering_2)
        //                                         .build();

        //println!("{} .... {}", std::env::current_dir().unwrap().display(), PathBuf::from("../res/models/Monkey.obj").display());
        //PathBuf::from("../res/models/Monkey.obj")
        //println!("{}", mesh_1_path.display());

       
    }
}


fn paddle_movement_system(engine: &mut Engine) -> Result<()> {
    println!("Moving paddles"); 
    // is A key in global component pressed, if yes the do
    for transform in engine.fetch_one_component_storage::<TransformComponent>()? {
            transform.borrow_mut().as_mut().unwrap().rotation.y += 0.05;
            
    }
    Ok(())   
}

