use pill_engine::game::*;
use pill_engine::{define_component};
use crate::player_movement::player_movement_system;
use crate::free_camera::free_camera_system;
use crate::player_physics_movement;
use crate::player_physics_movement::player_physics_movement_system;

define_component!(PlayerTagComponent { });

pub struct TargetTransformComponent(pub TransformComponent);

impl PillTypeMapKey for TargetTransformComponent {
    type Storage = ComponentStorage<Self>;
}

impl TargetTransformComponent {
	pub fn new(transform_component: TransformComponent) -> Self {
		Self(transform_component)
	}
}

impl Component for TargetTransformComponent {}

const USE_PHYSICS: bool = false;

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
		engine.register_component::<RigidBodyComponent>(active_scene)?;
		engine.register_component::<ColliderComponent>(active_scene)?;

		// Add systems
		if USE_PHYSICS {
			engine.add_system("PlayerPhysicsMovementSystem", player_physics_movement_system)?;
		} else {
			engine.add_system("PlayerMovementSystem", player_movement_system)?;
		}
		
		//engine.add_system("FreeCameraSystem", free_camera_system)?;


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
		let initial_camera_transform = TransformComponent::builder()
			.position(Vector3f::new(0.0, 6.0, 10.0))
			.build();
		engine.build_entity(active_scene)
			.with_component(initial_camera_transform.clone())
			.with_component(CameraComponent::builder()
				.enabled(true)
				.fov(60.0)
				.clear_color(Color::new(0.3, 0.3, 0.3))
				.build())
				.with_component(TargetTransformComponent::new(initial_camera_transform))
			.build();

		// Create ground entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 0.0, 0.0))
				.scale(Vector3f::new(70.0, 1.0, 70.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&ground_material_handle)
				.mesh(&ground_mesh_handle)
				.build())
			.with_component(RigidBodyComponent::builder().body_type(RigidBodyType::Fixed)
				.build())
			.with_component(ColliderComponent::builder().shape(SharedShape::cuboid(135.0, 0.5, 135.0))
				.build())
			.build();

		// Create player truck entity
		let initial_player_transform = TransformComponent::builder()
			.position(Vector3f::new(0.0, 2.0, 0.0))
			.build();

		let mut player_entity_builder = engine.build_entity(active_scene)
			.with_component(initial_player_transform.clone())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&truck_mesh_handle)
				.build())
			.with_component(PlayerTagComponent {})
			.with_component(TargetTransformComponent::new(initial_player_transform.clone()));

		if USE_PHYSICS {
			player_entity_builder = player_entity_builder
			.with_component(RigidBodyComponent::builder().body_type(RigidBodyType::Dynamic)
				.build())
			.with_component(ColliderComponent::builder().shape(SharedShape::cuboid(3.0, 2.0, 3.0))
				.build());
		}

		player_entity_builder.build();


		// Create cube entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 13.0, 0.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&cube_mesh_handle)
				.build())
			.with_component(RigidBodyComponent::builder()
				.build())
			.with_component(ColliderComponent::builder()
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
