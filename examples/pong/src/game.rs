use pill_engine::game::*;

pub struct Game { 

}   

impl Pill_Game for Game {
    fn initialize(&self, engine: &mut Engine) {
        println!("Let's play pong"); 

        let scene = engine.create_scene("Default").unwrap();
        engine.set_current_scene(scene).unwrap();
        engine.register_component::<TransformComponent>(scene).unwrap();
        engine.register_component::<MeshRenderingComponent>(scene).unwrap();

        
        let current_scene = engine.get_current_scene().expect("Scene not found");//.unwrap();
        println!("[Engine] Creating testing gameobjects in scene {}", current_scene.index);

        let entity_1 = engine.create_entity(current_scene).unwrap();
        let transform_1 = TransformComponent::default();
        engine.add_component_to_entity::<TransformComponent>(current_scene, entity_1, transform_1).unwrap();

        let mesh_rendering_1 = MeshRenderingComponent::default();
        engine.add_component_to_entity::<MeshRenderingComponent>(current_scene, entity_1, mesh_rendering_1).unwrap();


        engine.print_debug_message();
    }

    fn update(&self, engine: &mut Engine) {
        println!("Updating pong"); 
    }
}