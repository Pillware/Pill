use pill_engine::game::*;

pub struct Game { }   

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) {
        println!("Let's play pong"); 

        let scene = engine.create_scene("Default").unwrap();
        engine.set_active_scene(scene).unwrap();

        engine.add_system("Paddle movement", paddle_movement_system).unwrap();


        engine.register_component::<TransformComponent>(scene).unwrap();
        engine.register_component::<MeshRenderingComponent>(scene).unwrap();

        
        let active_scene = engine.get_active_scene().expect("Scene not found");//.unwrap();

        let entity_1 = engine.create_entity(active_scene).unwrap();
        let transform_1 = TransformComponent::default();
        engine.add_component_to_entity::<TransformComponent>(active_scene, entity_1, transform_1).unwrap();

        let mesh_rendering_1 = MeshRenderingComponent::default();
        engine.add_component_to_entity::<MeshRenderingComponent>(active_scene, entity_1, mesh_rendering_1).unwrap();
    }
}

fn paddle_movement_system(engine: &mut Engine) {
    println!("Moving paddles"); 
}