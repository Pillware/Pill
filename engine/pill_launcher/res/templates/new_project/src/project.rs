//! Welcome to Pill!
//!
//! This is the default template for new Pill projects. It sets up a simple
//! 3D scene with a camera, a floating/rotating pill model, and a basic
//! lighting setup. Modify or replace this file to build your own project.

use pill_engine::project::*;

// Declare the project struct and register it with the engine macro.
pub struct Project {}
create_project!(Project {}, PillProject);

impl PillProject for Project {
    // Called once at startup. Set up scenes, register components, load resources, spawn entities, and register systems here.
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Create a default scene and set it as active.
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register the component types this project uses.
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;

        // Load resources: a 3D mesh and a material with a tint color.
        let mesh_handle = engine.add_resource(Mesh::new("pill", "models/pill.obj".into()))?;

        let material_handle = engine.add_resource(
            Material::builder("default")
                .color_parameter("tint", Color::new(0.8, 0.8, 0.82))?
                .build(),
        )?;

        // Spawn a camera entity — positioned back from the origin.
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, -5.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.1, 0.1, 0.11))
                    .build(),
            )
            .build();

        // Spawn the pill model — floating slightly above the origin.
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.5, 0.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&mesh_handle)
                    .material(&material_handle)
                    .build(),
            )
            .build();

        // Register the float-and-rotate system to animate the pill.
        engine.add_system("float_and_rotate", float_and_rotate_system)?;

        Ok(())
    }
}

// ----- Systems ----------------------------------------------------------------

// Makes all entities with both TransformComponent and MeshRenderingComponent
// float up and down while spinning — a simple visual demo.
fn float_and_rotate_system(engine: &mut Engine) -> Result<()> {
    let elapsed = engine.get_global_component::<TimeComponent>()?.time;

    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        // Vertical bobbing: sine wave with 2.5 Hz frequency, 0.3 unit amplitude.
        let float_offset = (elapsed * 2.5).sin() * 0.3;
        transform.set_position(Vector3f::new(
            transform.position.x,
            0.5 + float_offset,
            transform.position.z,
        ));

        // Rotation: gentle tilt on X, continuous spin on Y.
        transform.set_rotation(Vector3f::new(
            (elapsed * 2.0).sin() * 5.0,
            elapsed * 60.0,
            0.0,
        ));
    }

    Ok(())
}
