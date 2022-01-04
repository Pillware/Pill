#![allow(unused_imports, dead_code, unused_variables, unused_mut)]
use pill_engine::game::*;

struct NonCameraComponent {} 

impl PillTypeMapKey for NonCameraComponent {
    type Storage = ComponentStorage<NonCameraComponent>; 
}

impl Component for NonCameraComponent { }



struct RemovableComponent {} 

impl PillTypeMapKey for RemovableComponent {
    type Storage = ComponentStorage<RemovableComponent>; 
}

impl Component for RemovableComponent {
   
}

pub struct Game { }   

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) {
        println!("Let's play pong"); 

        // Create scene
        let scene = engine.create_scene("Default").unwrap();
        engine.set_active_scene(scene).unwrap();

        // Add systems
        engine.add_system("PaddleMovement", paddle_movement_system).unwrap();
        engine.add_system("DeleteEntitySystem", delete_entity_system).unwrap();

        // Register components
        engine.register_component::<TransformComponent>(scene).unwrap();
        engine.register_component::<MeshRenderingComponent>(scene).unwrap();
        engine.register_component::<RemovableComponent>(scene).unwrap();
        engine.register_component::<CameraComponent>(scene).unwrap();
        engine.register_component::<NonCameraComponent>(scene).unwrap();
        
        let active_scene = engine.get_active_scene_handle().unwrap();

        // --- Create camera entity
        let camera_holder = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let camera_transform = TransformComponent::builder()
            .position(Vector3f::new(0.0,5.0,7.0))
            .rotation(Vector3f::new(-20.0,-90.0,0.0))
            .build();

        engine.add_component_to_entity::<TransformComponent>(active_scene, camera_holder, camera_transform).unwrap();
        // Add camera component
        let mut camera = CameraComponent::new();
        camera.enabled = true;
        engine.add_component_to_entity::<CameraComponent>(active_scene, camera_holder, camera).unwrap();



        // Add texture
        let camo_texture_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/Camouflage.png");
        let camo_texture = Texture::new("Camo", TextureType::Color, ResourceLoadType::Path(camo_texture_path));
        let camo_texture_handle = engine.add_resource::<Texture>(camo_texture).unwrap();

        // Add texture
        let wut_texture_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/WUT.png");
        let wut_texture = Texture::new("WUT", TextureType::Color, ResourceLoadType::Path(wut_texture_path));
        let wut_texture_handle = engine.add_resource::<Texture>(wut_texture).unwrap();

        // Add texture
        let quilted_texture_path = std::env::current_dir().unwrap().join("examples/pong/res/textures/Quilted.png");
        let quilted_texture = Texture::new("Quilted", TextureType::Normal, ResourceLoadType::Path(quilted_texture_path));
        let quilted_texture_handle = engine.add_resource::<Texture>(quilted_texture).unwrap();


        // Add material
        let mut material_alpha = Material::new("Alpha");
        material_alpha.set_texture("Color", camo_texture_handle).unwrap();
        material_alpha.set_color("Tint", Color::new( 1.0, 1.0, 1.0)).unwrap();
        let material_alpha_handle = engine.add_resource::<Material>(material_alpha).unwrap(); 
       
        // Add material
        let mut material_beta = Material::new("Beta");
        material_beta.set_texture("Color", wut_texture_handle).unwrap();
        material_beta.set_texture("Normal", quilted_texture_handle).unwrap();
        material_beta.set_color("Tint", Color::new( 1.0, 0.7, 0.0)).unwrap();
        let material_beta_handle = engine.add_resource::<Material>(material_beta).unwrap();

        // Add mesh
        let monkey_mesh_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Monkey.obj"); 
        let monkey_mesh = Mesh::new("Monkey", monkey_mesh_path);
        let monkey_mesh_handle = engine.add_resource::<Mesh>(monkey_mesh).unwrap();

        let cube_mesh_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Cube.obj");
        let cube_mesh = Mesh::new("Cube", cube_mesh_path);
        let cube_mesh_handle = engine.add_resource::<Mesh>(cube_mesh).unwrap();
      
        let airplane_mesh_path = std::env::current_dir().unwrap().join("examples/pong/res/models/Airplane.obj");
        let airplane_mesh = Mesh::new("Airplane", airplane_mesh_path);
        let airplane_mesh_handle = engine.add_resource::<Mesh>(airplane_mesh).unwrap();

        // --- Create entity
        let monkey_entity = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let transform_1 = TransformComponent::builder()
            .position(Vector3f::new(2.0,0.0,0.0))
            .rotation(Vector3f::new(0.0, 45.0,0.0))
            .scale(Vector3f::new(1.0,1.0,1.0))
            .build();


        engine.add_component_to_entity::<TransformComponent>(active_scene, monkey_entity, transform_1).unwrap();  
        // Add mesh rendering component  
        let mut mesh_rendering_1 = MeshRenderingComponent::builder()
            .mesh(&monkey_mesh_handle)
            .material(&material_alpha_handle)
            .build();
        //mesh_rendering_1.set_material(engine, &material_alpha_handle).unwrap();
        //mesh_rendering_1.set_mesh(engine, &monkey_mesh_handle).unwrap();
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, monkey_entity, mesh_rendering_1).unwrap();


        // --- Create entity
        let cube_entity = engine.create_entity(active_scene).unwrap();
        // Add transform component
        let transform_2 = TransformComponent::builder()
            .position(Vector3f::new(0.0,0.0,-2.0))
            .rotation(Vector3f::new(35.0, 35.0,35.0))
            .scale(Vector3f::new(3.0,3.0,3.0))
            .build();
        
        engine.add_component_to_entity::<TransformComponent>(active_scene, cube_entity, transform_2).unwrap();  
        // Add mesh rendering component
        let mut mesh_rendering_2 = MeshRenderingComponent::builder()
            .mesh(&cube_mesh_handle)
            .material(&material_beta_handle)
            .build();
            
        //mesh_rendering_2.set_material(engine, &material_beta_handle).unwrap();
        //mesh_rendering_2.set_mesh(engine, &cube_mesh_handle).unwrap();
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, cube_entity, mesh_rendering_2).unwrap();


        // --- Create entity
        let transform_3 = TransformComponent::builder()
            .position(Vector3f::new(-20.0,-15.0,-1.0))
            .rotation(Vector3f::new(15.0, 15.0,15.0))
            .scale(Vector3f::new(0.5, 0.5,0.5))
            .build();

        // Add mesh rendering component
        let mut mesh_rendering_3 = MeshRenderingComponent::builder()
            .mesh(&airplane_mesh_handle)
            .material(&material_alpha_handle)
            .build();

        //mesh_rendering_3.set_material(engine, &material_alpha_handle).unwrap();
        //mesh_rendering_3.set_mesh(engine, &airplane_mesh_handle).unwrap();
        let airplane_entity = engine.build_entity(active_scene)
            .with_component(transform_3)
            .with_component(mesh_rendering_3)
            .with_component(RemovableComponent{})
            .with_component(NonCameraComponent{})
            .build();


        // --- Tests
        let material = engine.get_resource_mut::<Material>(&material_alpha_handle).unwrap();
        //material.set_color("Tint", Color::new( 0.0, 0.0, 1.0));
        //material.set_texture("Color", wut_texture_handle).unwrap();
        //material.set_texture("Normal", wut_texture_handle).unwrap();
        //engine.remove_resource_by_name::<Texture>("WUT").unwrap();
        //engine.remove_resource_by_name::<Texture>("Quilted").unwrap();
        //engine.remove_resource_by_name::<Material>("Alpha").unwrap();
        //engine.remove_resource_by_name::<Mesh>("Monkey").unwrap();


        //println!("{} .... {}", std::env::current_dir().unwrap().display(), PathBuf::from("../res/models/Monkey.obj").display());
        //PathBuf::from("../res/models/Monkey.obj")
        //println!("{}", mesh_1_path.display());
       
    }
}

fn delete_entity_system(engine: &mut Engine) -> Result<()> {
    let new_eng = &*engine;
    let mut removable_entities = Vec::<EntityHandle>::new();
    
    for (entity, removable) in new_eng.fetch_one_component_storage_with_entity_handles::<RemovableComponent>()? {
        let input_component = new_eng.get_global_component::<InputComponent>()?;
        if input_component.is_key_clicked(Key::Delete) {
            removable_entities.push(entity.clone());
        }
    }
    for entity in removable_entities.iter() {
        let scene_handle = engine.get_active_scene_handle().unwrap();
        engine.remove_entity(*entity, scene_handle).unwrap();
    }

    Ok(())
}

fn paddle_movement_system(engine: &mut Engine) -> Result<()> {
    let new_eng = &*engine;
    
    for (transform, camera) in new_eng.fetch_two_component_storages::<TransformComponent, NonCameraComponent>()? {
        let input_component = new_eng.get_global_component::<InputComponent>()?;
        if input_component.is_key_pressed(Key::S) {
        transform.borrow_mut().as_mut().unwrap().rotation.y += 0.05; }

        if input_component.is_key_pressed(Key::W) {
            transform.borrow_mut().as_mut().unwrap().rotation.y -= 0.05; }

        for transform_z in new_eng.fetch_one_component_storage::<TransformComponent>()? {
            if input_component.is_key_clicked(Key::Z) {
                transform_z.borrow_mut().as_mut().unwrap().rotation.y += 0.05; }
        }

        if input_component.is_mouse_button_clicked(Mouse::Left) {
            transform.borrow_mut().as_mut().unwrap().rotation.x -= 0.05;
        }
    }
    Ok(())   
}