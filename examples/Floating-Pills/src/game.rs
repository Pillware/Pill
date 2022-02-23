use pill_engine::game::*;
use rand::{thread_rng, Rng};

pub const FLOATING_OBJECT_SPAWN_BATCH_COUNT: usize = 10;
pub const FLOATING_OBJECT_REMOVE_BATCH_COUNT: usize = 10;
pub const SPAWN_FLOATING_OBJECTS_BUTTON: KeyboardKey = KeyboardKey::O;
pub const REMOVE_FLOATING_OBJECTS_BUTTON: KeyboardKey = KeyboardKey::L;
pub const TOGGLE_FLOATING_OBJECTS_SYSTEM: KeyboardKey = KeyboardKey::I;
pub const FLOATING_OBJECTS_CHANGE_MESH_BUTTON: KeyboardKey = KeyboardKey::N;
pub const FLOATING_OBJECTS_CHANGE_MATERIAL_BUTTON: KeyboardKey = KeyboardKey::M;
pub const INCREASE_CAMERA_FOV_BUTTON: KeyboardKey = KeyboardKey::T;
pub const DECREASE_CAMERA_FOV_BUTTON: KeyboardKey = KeyboardKey::G;

pub struct FloatingObjectComponent {
    pub angle: f32,
    pub radius_factor: f32,
    pub scale_factor: f32,
    pub y_axis_factor: f32,

    pub orbital_movement_speed: f32,
    pub y_axis_movement_speed: f32,
    pub rotation_speed: f32,
    pub scale_speed: f32,
    pub radius_speed: f32,
}

impl Component for FloatingObjectComponent { }

impl PillTypeMapKey for FloatingObjectComponent {
    type Storage = ComponentStorage<Self>;
}

struct DemoStateComponent {
    pub floating_objects_movemement_enabled: bool,
    pub current_mesh: usize,
    pub mesh_handles: Vec::<MeshHandle>,
    pub current_material_set: usize,
    pub textured_material_handles: Vec::<MaterialHandle>,
    pub plain_color_material_handles: Vec::<MaterialHandle>,
}
impl PillTypeMapKey for DemoStateComponent { type Storage = GlobalComponentStorage<DemoStateComponent>; }
impl GlobalComponent for DemoStateComponent { }

struct CameraMovementComponent {
    pub orbit_speed: f32,
    pub zoom_speed: f32,
    pub angle: f32,
    pub radius: f32,
    pub delta_y: f32,
    pub delta_z: f32,
}
impl PillTypeMapKey for CameraMovementComponent { type Storage = ComponentStorage<CameraMovementComponent>; }
impl Component for CameraMovementComponent { }

pub struct Game { } 

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {

        // --- Basic setup ---

        // Create scene
        let active_scene = engine.create_scene("Default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<CameraMovementComponent>(active_scene)?;
        engine.register_component::<FloatingObjectComponent>(active_scene)?;
        
        // Add systems
        engine.add_system("SpawnFloatingObjects", floating_objects_spawn_system)?;
        engine.add_system("DeleteFloatingObjects", floating_objects_remove_system)?;
        engine.add_system("ObjectsMovement", floating_objects_movement_system)?;
        engine.add_system("CameraMovement", camera_movement_system)?;
        engine.add_system("CameraFov", camera_fov_changing_system)?;
        engine.add_system("MeshChanging", object_appearance_changing_system)?;
        engine.add_system("DemoControl", demo_control_system)?;

        // --- Create resources ---

        // Add meshes
        let pill_mesh = Mesh::new("Pill", "./res/models/Pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;
        let cube_mesh = Mesh::new("Cube", "./res/models/Cube.obj".into());
        let cube_mesh_handle = engine.add_resource(cube_mesh)?;
        let torus_mesh = Mesh::new("Torus", "./res/models/Torus.obj".into());
        let torus_mesh_handle = engine.add_resource(torus_mesh)?;

        // Add sounds
        let ambient_music = Sound::new("Ambient", "./res/audio/TestMusic.mp3".into());
        let ambient_music_handle = engine.add_resource(ambient_music)?;

        // Add textures
        let fabric_color_texture = Texture::new("FabricColor", TextureType::Color, ResourceLoadType::Path("./res/textures/FabricColor.jpg".into()));
        let fabric_color_texture_handle = engine.add_resource::<Texture>(fabric_color_texture)?;
       
        let fabric_normal_texture = Texture::new("FabricNormal", TextureType::Normal, ResourceLoadType::Path("./res/textures/FabricNormal.jpg".into()));
        let fabric_normal_texture_handle = engine.add_resource::<Texture>(fabric_normal_texture)?;
        
        let stones_color_texture = Texture::new("StonesColor", TextureType::Color, ResourceLoadType::Path("./res/textures/StonesColor.jpg".into()));
        let stones_color_texture_handle = engine.add_resource::<Texture>(stones_color_texture)?;
        
        let stones_normal_texture = Texture::new("StonesNormal", TextureType::Normal, ResourceLoadType::Path("./res/textures/StonesNormal.jpg".into()));
        let stones_normal_texture_handle = engine.add_resource::<Texture>(stones_normal_texture)?;
        
        let organic_color_texture = Texture::new("OrganicColor", TextureType::Color, ResourceLoadType::Path("./res/textures/OrganicColor.jpg".into()));
        let organic_color_texture_handle = engine.add_resource::<Texture>(organic_color_texture)?;

        let organic_normal_texture = Texture::new("OrganicNormal", TextureType::Normal, ResourceLoadType::Path("./res/textures/OrganicNormal.jpg".into()));
        let organic_normal_texture_handle = engine.add_resource::<Texture>(organic_normal_texture)?;

        // Add materials
        let mut fabric_material = Material::new("Fabric");
        fabric_material.set_texture("Color", fabric_color_texture_handle)?;
        fabric_material.set_texture("Normal", fabric_normal_texture_handle)?;
        fabric_material.set_color("Tint", Color::new( 1.0, 0.1, 0.1))?;
        let fabric_material_handle = engine.add_resource::<Material>(fabric_material)?; 

        let mut stones_material = Material::new("Stones");
        stones_material.set_texture("Color", stones_color_texture_handle)?;
        stones_material.set_texture("Normal", stones_normal_texture_handle)?;
        let stones_material_handle = engine.add_resource::<Material>(stones_material)?; 

        let mut organic_material = Material::new("Organic");
        organic_material.set_texture("Color", organic_color_texture_handle)?;
        organic_material.set_texture("Normal", organic_normal_texture_handle)?;
        organic_material.set_color("Tint", Color::new( 0.26, 0.87, 0.9))?;
        organic_material.set_scalar("Specularity", 3.0)?;
        let organic_material_handle = engine.add_resource::<Material>(organic_material)?; 


        let mut yellow_material = Material::new("Yellow");
        yellow_material.set_color("Tint", Color::new( 1.0, 0.88, 0.0))?;
        let yellow_material_handle = engine.add_resource::<Material>(yellow_material)?; 

        let mut blue_material = Material::new("Blue");
        blue_material.set_color("Tint", Color::new( 0.26, 0.87, 0.9))?;
        let blue_material_handle = engine.add_resource::<Material>(blue_material)?; 

        let white_material = Material::new("White");
        let white_material_handle = engine.add_resource::<Material>(white_material)?; 

        // --- Create entities ---

        // Create ambient music player entity
        let ambient_music_player_entity = engine.create_entity(active_scene)?;

        let audio_source_component = AudioSourceComponent::builder().sound_type(SoundType::Sound2D).sound(ambient_music_handle).volume(0.05).play_on_awake(false).build();
        engine.add_component_to_entity(active_scene, ambient_music_player_entity, audio_source_component)?;

        // Create origin point entity
        let origin_entity = engine.create_entity(active_scene)?;

        let origin_transform = TransformComponent::builder().build();
        engine.add_component_to_entity(active_scene, origin_entity, origin_transform)?;      

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;

        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(-30.0,3.0,0.0))
            .rotation(Vector3f::new(0.0,0.0,-20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;

        let camera_component = CameraComponent::builder().enabled(true).fov(60.0).clear_color(Color::new(0.25, 0.40, 0.80)).build();
        engine.add_component_to_entity::<CameraComponent>(active_scene, camera, camera_component)?;

        let camera_movement_component = CameraMovementComponent {
            orbit_speed: 60.0,
            zoom_speed: 5.0,
            angle: 0.0,
            radius: 30.0,
            delta_y: 0.0,
            delta_z: 0.0
        };
        engine.add_component_to_entity(active_scene, camera, camera_movement_component)?;

        let audio_listener_component = AudioListenerComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, audio_listener_component)?;

        
        // Setup demo state component
        let demo_state = DemoStateComponent {
            floating_objects_movemement_enabled: true,
            current_mesh: 0,
            mesh_handles: vec!(pill_mesh_handle, cube_mesh_handle, torus_mesh_handle),
            current_material_set: 0,
            textured_material_handles: vec!(fabric_material_handle, stones_material_handle, organic_material_handle),
            plain_color_material_handles: vec!(yellow_material_handle, blue_material_handle, white_material_handle),
        };
        engine.add_global_component(demo_state)?;

        // Spawn certain number of floating objects
        spawn_floating_objects(engine, FLOATING_OBJECT_SPAWN_BATCH_COUNT)?;

        Ok(())
    }
}

// --- Systems ---

fn demo_control_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let system_toggle_key = input_component.get_key_pressed(TOGGLE_FLOATING_OBJECTS_SYSTEM);

    let demo_state =  engine.get_global_component_mut::<DemoStateComponent>()?;
    if system_toggle_key {
        demo_state.floating_objects_movemement_enabled = !demo_state.floating_objects_movemement_enabled;
        let enabled = demo_state.floating_objects_movemement_enabled;
        engine.toggle_system("ObjectsMovement", enabled)?;
    }

    Ok(())
}

fn floating_objects_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    for (_, floating_object_transform, floating_object_component) in engine.iterate_two_components_mut::<TransformComponent, FloatingObjectComponent>()? {

        // Local rotation
        let rotation_speed = floating_object_component.rotation_speed.clone();
        floating_object_transform.rotation += Vector3f::new(1.0,1.0,1.0) * rotation_speed * delta_time;

        // Local scale
        let scale_speed = floating_object_component.scale_speed.clone();
        floating_object_component.scale_factor += scale_speed * delta_time;
        let scale_factor = floating_object_component.scale_factor.clone();
        floating_object_transform.scale = Vector3f::new(0.4,0.4,0.4) * (scale_factor.sin() / 1.5 + 1.5);

        // Radius
        let radius_speed = floating_object_component.radius_speed.clone();
        floating_object_component.radius_factor += radius_speed * delta_time;

        // Movement
        let orbital_movement_speed = floating_object_component.orbital_movement_speed.clone();
        floating_object_component.angle += orbital_movement_speed * delta_time;

        let angle = floating_object_component.angle.clone();
        let radius = floating_object_component.radius_factor.clone().sin() * 6.0 + 10.0;
        floating_object_transform.position.x = angle.to_radians().cos() * radius;
        floating_object_transform.position.z = angle.to_radians().sin() * radius;
       
        let y_axis_movement_speed = floating_object_component.y_axis_movement_speed.clone();
        floating_object_component.y_axis_factor += y_axis_movement_speed * delta_time;
        let y_axis_factor = floating_object_component.y_axis_factor.clone();
        floating_object_transform.position.y = y_axis_factor.sin() * 0.8 * radius;
    }  

    Ok(())
}

fn object_appearance_changing_system(engine: &mut Engine) -> Result<()> {
    let mut rng = thread_rng();
    
    let input_component = engine.get_global_component::<InputComponent>()?;
    let mesh_key = input_component.get_key_pressed(FLOATING_OBJECTS_CHANGE_MESH_BUTTON);
    let material_key = input_component.get_key_pressed(FLOATING_OBJECTS_CHANGE_MATERIAL_BUTTON);

    // Set same mesh 
    if mesh_key {
        let demo_state =  engine.get_global_component_mut::<DemoStateComponent>()?;
        demo_state.current_mesh = (demo_state.current_mesh + 1) % 3;
        let mesh_handle = demo_state.mesh_handles.get(demo_state.current_mesh).unwrap().clone();
        for (_, mesh_rendering_component) in engine.iterate_one_component_mut::<MeshRenderingComponent>()? {
            mesh_rendering_component.set_mesh(&mesh_handle);
        }
    }

    // Set random material from set
    if material_key {
        let demo_state =  engine.get_global_component_mut::<DemoStateComponent>()?;
        demo_state.current_material_set = (demo_state.current_material_set + 1) % 2;
        
        let current_material_set = match demo_state.current_material_set == 0 {
            true => demo_state.textured_material_handles.clone(),
            false => demo_state.plain_color_material_handles.clone(),
        };
        
        for (_, mesh_rendering_component) in engine.iterate_one_component_mut::<MeshRenderingComponent>()? {
            let material_handle = current_material_set[rng.gen_range(0..=2)];
            mesh_rendering_component.set_material(&material_handle);
        }
    }

    Ok(())
}

fn camera_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Get input
    let a_key = input_component.get_key(KeyboardKey::A);
    let d_key = input_component.get_key(KeyboardKey::D);
    let right_mouse_button = input_component.get_mouse_button(MouseButton::Right);
    let mouse_scroll_delta = input_component.get_mouse_scroll_delta();
    let mouse_delta = input_component.get_mouse_delta();

    for (_, transform_transform, camera_movement_component) in engine.iterate_two_components_mut::<TransformComponent, CameraMovementComponent>()?
    {   
        // Zoom
        let zoom_speed = camera_movement_component.zoom_speed;
        camera_movement_component.radius -= mouse_scroll_delta.y * zoom_speed;

        // Orbit
        let mut change_value: f32 = 0.0;
        if d_key { change_value -= 1.0; }
        if a_key { change_value += 1.0; }
        let orbit_speed = camera_movement_component.orbit_speed;
        camera_movement_component.angle += change_value * orbit_speed * delta_time;
        let angle = camera_movement_component.angle;
        let radius = camera_movement_component.radius;

        let x_position = angle.to_radians().cos() * radius;
        let z_position = angle.to_radians().sin() * radius;

        // Mouse movement
        let mut z_change_value = 0.0;
        if mouse_delta.x > 0.0 { z_change_value -= 0.2; }
        if mouse_delta.x < 0.0 { z_change_value += 0.2; }

        let mut y_change_value = 0.0;
        if mouse_delta.y > 0.0 { y_change_value -= 0.2; }
        if mouse_delta.y < 0.0 { y_change_value += 0.2; }

        if right_mouse_button {
            camera_movement_component.delta_z += z_change_value;
            camera_movement_component.delta_y += y_change_value;
        }

        let delta_y = camera_movement_component.delta_y;
        let delta_z = camera_movement_component.delta_z;

        // Set position
        transform_transform.position = Vector3f::new(x_position, delta_y, z_position + delta_z);

        // Set rotation
        transform_transform.rotation = Vector3f::new(0.0, -angle - 90.0, 0.0);
    }

    Ok(())
}

fn camera_fov_changing_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Get input
    let t_key = input_component.get_key(INCREASE_CAMERA_FOV_BUTTON);
    let g_key = input_component.get_key(DECREASE_CAMERA_FOV_BUTTON);

    for (_, camera_component) in engine.iterate_one_component_mut::<CameraComponent>()?
    {   
        let mut change_value: f32 = 0.0;
        if t_key { change_value += 1.0; }
        if g_key { change_value -= 1.0; }

        let new_fov = camera_component.fov + change_value * 100.0 * delta_time;
        if new_fov > 10.0 && new_fov < 120.0 {
            camera_component.fov = new_fov;
        }
    }

    Ok(())
}

fn floating_objects_spawn_system(engine: &mut Engine) -> Result<()> {
    // Get input component
    let input_component = (&*engine).get_global_component::<InputComponent>()?;

    // Create new objects
    if input_component.get_key_pressed(SPAWN_FLOATING_OBJECTS_BUTTON) {
        spawn_floating_objects(engine, FLOATING_OBJECT_SPAWN_BATCH_COUNT)?;
    }

    Ok(())
}

fn floating_objects_remove_system(engine: &mut Engine) -> Result<()> {
    let mut count = FLOATING_OBJECT_REMOVE_BATCH_COUNT;

    // Get active scene handle
    let scene_handle = engine.get_active_scene_handle()?;

    // Get input component
    let input_component = (&*engine).get_global_component::<InputComponent>()?;

    // Remove objects
    if input_component.get_key_pressed(REMOVE_FLOATING_OBJECTS_BUTTON) {
        let mut entities_for_deletion = Vec::<EntityHandle>::new();
        
        for (entity_handle, _) in engine.iterate_one_component::<FloatingObjectComponent>()? {
            if count == 0 {
                break;
            }
            entities_for_deletion.push(entity_handle);
            count -= 1;
        }

        for entity_handle in entities_for_deletion.iter() {
            engine.remove_entity(*entity_handle, scene_handle)?;
        }
    }

    Ok(())
}

// --- Functions ---

fn spawn_floating_objects(engine: &mut Engine, object_count: usize) -> Result<()> {
    let mut rng = thread_rng();

    // Get active scene handle
    let active_scene = engine.get_active_scene_handle()?;

    // Get resources
    let demo_state = (&*engine).get_global_component::<DemoStateComponent>()?;
    let mesh_handle = demo_state.mesh_handles[demo_state.current_mesh];
    
    let material_handle = match demo_state.current_material_set == 0 {
        true => demo_state.textured_material_handles[rng.gen_range(0..=2)],
        false => demo_state.plain_color_material_handles[rng.gen_range(0..=2)],
    };

    for _ in 0..object_count {
        let new_entity = engine.create_entity(active_scene).unwrap();

        // Creatine FloatingObject component with randomized initial data
        let float_object_component = FloatingObjectComponent {
            angle: rng.gen_range(0.0..359.0),
            radius_factor: rng.gen_range(20.0..180.0),
            scale_factor: rng.gen_range(0.5..1.5),
            y_axis_factor: rng.gen_range(0.0..6.0),

            orbital_movement_speed: rng.gen_range(40.0..120.0),
            y_axis_movement_speed: rng.gen_range(-0.6..0.6),
            rotation_speed: rng.gen_range(-90.0..90.0),
            scale_speed: rng.gen_range(0.06..1.2),
            radius_speed: rng.gen_range(0.1..1.2),
        };

        // Create transform component 
        let transform_component = TransformComponent::builder().build();

        // Create mesh component
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .material(&material_handle)
            .mesh(&mesh_handle)
            .build();

        // Add components
        engine.add_component_to_entity(active_scene, new_entity, float_object_component)?;
        engine.add_component_to_entity(active_scene, new_entity, transform_component)?;
        engine.add_component_to_entity(active_scene, new_entity, mesh_rendering_component)?;       
    } 
    
    // Update initial positions once (in case movement system is disabled)
    floating_objects_movement_system(engine)?;

    Ok(())
}