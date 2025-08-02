use pill_engine::game::*;
use pill_engine::{define_component};

use crate::free_camera::free_camera_system;

define_component!(PlayerTagComponent { });

pub struct TargetTransformComponent(pub TransformComponent);

impl PillTypeMapKey for TargetTransformComponent {
    type Storage = ComponentStorage<Self>;
}

impl TargetTransformComponent {
	pub fn new() -> Self {
		Self(TransformComponent::new())
	}
}

impl Component for TargetTransformComponent {}


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
		engine.register_component::<PlayerTagComponent>(active_scene)?;

		engine.register_component::<TargetTransformComponent>(active_scene)?;
		// Add systems
		//engine.add_system("PlayerMovementSystem", player_movement_system)?;
		engine.add_system("FreeCameraSystem", free_camera_system)?;


		// Add meshes
        let truck_mesh_handle = engine.add_resource(
			Mesh::new("Truck", "models/Truck.obj".into())
		)?;

		let ground_mesh_handle = engine.add_resource(
			Mesh::new("Ground", "models/Plane.obj".into())
		)?;

		let cube_mesh_handle = engine.add_resource(
			Mesh::new("Cube", "models/Cube.obj".into())
		)?;

		let axis_gizmo_mesh_handle = engine.add_resource(
			Mesh::new("AxisGizmo", "models/AxisGizmo.obj".into())
		)?;

		// Add textures
		let dirt_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"TruckColor", 
				TextureType::Color, 
				ResourceLoadType::Path("textures/Dirt.png".into())
			)
		)?;

		let axis_gizmo_colors_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"AxisGizmoColors", 
				TextureType::Color, 
				ResourceLoadType::Path("textures/AxisGizmoColors.png".into())
			)
		)?;

		// Add materials
		let truck_material_handle = engine.add_resource::<Material>(
			Material::builder("Truck")
				.color("Tint", Color::new(0.97, 0.72, 0.09))?
				.scalar("Specularity", 0.5)?
				.build()
		)?;

		let ground_material_handle = engine.add_resource::<Material>(
			Material::builder("Ground")
				.texture("Color", dirt_texture_handle)?
				.color("Tint", Color::new(1.0, 1.0, 1.0))?
				.scalar("Specularity", 0.1)?
				.build()
		)?;

		let axis_gizmo_material_handle = engine.add_resource::<Material>(
			Material::builder("AxisGizmo")
				.texture("Color", axis_gizmo_colors_texture_handle)?
				.scalar("Specularity", 0.0)?
				.build()
		)?;

		// --- Create entities ---

		// Create camera entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 3.0, -3.0))
				.build())
			.with_component(CameraComponent::builder()
				.enabled(true)
				.fov(60.0)
				.clear_color(Color::new(0.9, 0.9, 0.3))
				.build())
				.with_component(TargetTransformComponent::new())
			.build();

		// Create ground entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 0.0, 0.0))
				.scale(Vector3f::new(10.0, 1.0, 10.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&ground_material_handle)
				.mesh(&ground_mesh_handle)
				.build())
			.build();

		// Create player truck entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::new())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&truck_mesh_handle)
				.build())
			.with_component(PlayerTagComponent {})
			.build();

		// Create cube entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 13.0, 0.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&cube_mesh_handle)
				.build())
			.build();

		// Create axis gizmo entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&axis_gizmo_material_handle)
				.mesh(&axis_gizmo_mesh_handle)
				.build())
			.build();

		Ok(())
	}
}

fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

	let w_key = input_component.get_key(KeyboardKey::KeyW);
	let s_key = input_component.get_key(KeyboardKey::KeyS);
	let a_key = input_component.get_key(KeyboardKey::KeyA);
	let d_key = input_component.get_key(KeyboardKey::KeyD);

    // Tweakable constants
    let move_speed = 5.0; // units per second
    let rotation_speed = 90.0; // degrees per second

    for (_, transform, _) in engine.iterate_two_components_mut::<TransformComponent, PlayerTagComponent>()? {
        // Handle rotation
        if a_key {
            transform.rotate_around_axis(rotation_speed * delta_time, Vector3f::unit_y() * -1.0);
        }

        if d_key {
            transform.rotate_around_axis(rotation_speed * delta_time, Vector3f::unit_y());
        }

        // Handle movement
        if w_key {
            transform.translate(move_speed * delta_time, Direction::Forward);
        }

        if s_key {
            transform.translate(move_speed * delta_time, Direction::Backward);
        }
    }

    Ok(())
}