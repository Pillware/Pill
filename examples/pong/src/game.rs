#[allow(unused_imports, dead_code, unused_variables)]
use pill_engine::game::*;

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
        engine.register_component::<CameraComponent>(scene).unwrap();

        let active_scene = engine.get_active_scene_handle().unwrap();

        


      
        // --- Create camera entity
        let camera_holder = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let camera_transform = TransformComponent::new(
            Vector3f::new(0.0,5.0,7.0), 
            Vector3f::new(-20.0,-90.0,0.0),
            Vector3f::new(1.0,1.0,1.0),
        );
        engine.add_component_to_entity::<TransformComponent>(active_scene, camera_holder, camera_transform).unwrap();
        // Add camera component
        let mut camera = CameraComponent::new(engine).unwrap();
        camera.enabled = true;
        engine.add_component_to_entity::<CameraComponent>(active_scene, camera_holder, camera).unwrap();










        // Add texture
        let texture_1_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/Camouflage.png");
        let texture_1 = Texture::new("TestTexture", TextureType::Color, ResourceLoadType::Path(texture_1_path));
        let texture_1_handle = engine.add_resource::<Texture>(texture_1).unwrap();

        // Add texture
        let texture_2_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/WUT.png");
        let texture_2 = Texture::new("TestTexture2", TextureType::Color, ResourceLoadType::Path(texture_2_path));
        let texture_2_handle = engine.add_resource::<Texture>(texture_2).unwrap();

        // Add texture Normal
        let texture_3_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/Quilted.png");
        let texture_3 = Texture::new("TestTextureNormal", TextureType::Normal, ResourceLoadType::Path(texture_3_path));
        let texture_3_handle = engine.add_resource::<Texture>(texture_3).unwrap();





        // Add material
        let mut material_1 = Material::new("TestMaterial");
        material_1.set_texture(engine,"Color", texture_1_handle).unwrap();
        material_1.set_texture(engine,"Normal", texture_3_handle).unwrap();
        material_1.set_color(engine, "Tint", Color::new( 1.0, 1.0, 1.0)).unwrap();


        let material_1_handle = engine.add_resource::<Material>(material_1).unwrap(); // [TODO] Remove requirement of name here, and assure that resource always has name and take it from there (using trait)
       
       
        //let m = engine.get_resource_mut::<Material>(&material_1_handle).unwrap();
        //m.set_color(engine, "Tint", cgmath::Vector3::<f32>::new( 1.0, 0.0, 0.0)).unwrap();

        //let aa = engine.get_resource_mut::<Material>(&material_1_handle).unwrap();


        // Add material
        let mut material_2 = Material::new("TestMaterial2");
        material_2.set_texture(engine,"Color", texture_2_handle).unwrap();
        material_2.set_color(engine, "Tint", Color::new( 1.0, 0.7, 0.0)).unwrap();
        let material_2_handle = engine.add_resource::<Material>(material_2).unwrap();

        // Add mesh
        let mesh_1_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Monkey.obj"); // examples/pong/res/models/Monkey.obj
        let mesh_1 = Mesh::new("TestMesh", mesh_1_path);
        let mesh_1_handle = engine.add_resource::<Mesh>(mesh_1).unwrap();

        let mesh_2_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Cube.obj"); // examples/pong/res/models/Monkey.obj
        let mesh_2 = Mesh::new("TestMesh2", mesh_2_path);
        let mesh_2_handle = engine.add_resource::<Mesh>(mesh_2).unwrap();
        //engine.remove_resource_by_name::<Texture>("TestTexture").unwrap();
        

    


        // --- Create entity
        let paddle_1 = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let transform_1 = TransformComponent::new(
            Vector3f::new(2.0,0.0,0.0), 
            Vector3f::new(0.0, 45.0,0.0),
            Vector3f::new(1.0,1.0,1.0),
        );
        engine.add_component_to_entity::<TransformComponent>(active_scene, paddle_1, transform_1).unwrap();  
        // Add mesh rendering component
        let mut mesh_rendering_1 = MeshRenderingComponent::default();
        mesh_rendering_1.set_material(engine, &material_1_handle).unwrap();
        mesh_rendering_1.set_mesh(engine, &mesh_1_handle).unwrap();
       
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, paddle_1, mesh_rendering_1).unwrap();












        
        // --- Create entity 2
        

      

        let paddle_2 = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let transform_2 = TransformComponent::new(
            Vector3f::new(0.0,0.0,-2.0), 
            Vector3f::new(35.0, 35.0,35.0),
            Vector3f::new(3.0,3.0,3.0),
        );
        engine.add_component_to_entity::<TransformComponent>(active_scene, paddle_2, transform_2).unwrap();  
        // Add mesh rendering component
        let mut mesh_rendering_2 = MeshRenderingComponent::default();
        mesh_rendering_2.set_material(engine, &material_2_handle).unwrap();
        mesh_rendering_2.set_mesh(engine, &mesh_2_handle).unwrap();
       
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, paddle_2, mesh_rendering_2).unwrap();





        //engine.remove_resource_by_name::<Material>("TestMaterial").unwrap();

        //println!("{} .... {}", std::env::current_dir().unwrap().display(), PathBuf::from("../res/models/Monkey.obj").display());
        //PathBuf::from("../res/models/Monkey.obj")
        //println!("{}", mesh_1_path.display());

       
    }
}

fn paddle_movement_system(_engine: &mut Engine) -> Result<()> {
    //println!("Moving paddles"); 

    Ok(())   
}