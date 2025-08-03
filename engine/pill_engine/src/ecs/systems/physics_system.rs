use crate::{
    engine::Engine,
    ecs::{
        TransformComponent, 
        PhysicsWorldComponent, 
        RigidBodyComponent, 
        ColliderComponent,
        SceneHandle
    }
};

use rapier3d::prelude::*;
use cgmath::{Vector3, Matrix4, SquareMatrix};
use anyhow::{Result, Context, Error};

/// The main physics system that handles Rapier physics simulation.
/// 
/// This system performs the following operations each frame:
/// 1. Syncs transform data to physics bodies (for kinematic/static bodies)
/// 2. Creates new rigid bodies and colliders for entities that need them
/// 3. Steps the physics world simulation
/// 4. Syncs physics body positions back to transforms (for dynamic bodies)
/// 
/// # Setup
/// 
/// To use physics in your game:
/// 1. Register the PhysicsWorldComponent as a global component
/// 2. Register RigidBodyComponent and ColliderComponent for entities
/// 3. Add the physics_system to your system manager
/// 4. Add RigidBodyComponent and/or ColliderComponent to entities
/// 
/// # Example
/// 
/// ```rust
/// // Register components and global physics world
/// engine.register_global_component::<PhysicsWorldComponent>(PhysicsWorldComponent::new())?;
/// engine.register_component::<RigidBodyComponent>(scene)?;
/// engine.register_component::<ColliderComponent>(scene)?;
/// 
/// // Add physics system to update loop
/// engine.system_manager.add_system(UpdatePhase::Update, "physics", physics_system)?;
/// 
/// // Create a physics entity
/// let entity = engine.create_entity(scene)?;
/// engine.add_component_to_entity(scene, entity, TransformComponent::new())?;
/// engine.add_component_to_entity(scene, entity, RigidBodyComponent::dynamic())?;
/// engine.add_component_to_entity(scene, entity, ColliderComponent::ball(1.0))?;
/// ```
pub fn physics_system(engine: &mut Engine) -> Result<()> {
    // Get the active scene handle
    let active_scene_handle = engine.scene_manager.get_active_scene_handle()?;
    
    // Sync transforms to physics bodies before stepping
    sync_transforms_to_physics(engine, active_scene_handle)?;
    
    // Create new rigid bodies and colliders for entities that don't have them yet
    //create_physics_bodies(engine, active_scene_handle)?;
    
    // Step the physics world
    {
        let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
        physics_world.step();
    }
    
    // Sync physics bodies back to transforms
    sync_physics_to_transforms(engine, active_scene_handle)?;
    
    Ok(())
}

fn sync_transforms_to_physics(
    engine: &mut Engine, 
    scene_handle: SceneHandle
) -> Result<()> {
    // Get entity handles with both transform and rigid body components
    let entities_with_physics: Vec<_> = {
        let scene = engine.scene_manager.get_scene(scene_handle)?;
        let mut entities = Vec::new();
        
        for (entity_handle, _) in scene.get_one_component_iterator::<TransformComponent>()? {
            if scene.entity_has_component::<RigidBodyComponent>(entity_handle)? {
                entities.push(entity_handle);
            }
        }
        entities
    };
    
    // Process each entity
    for entity_handle in entities_with_physics {
        let (position, rotation) = {
            let transform = engine.scene_manager.get_entity_component::<TransformComponent>(entity_handle, scene_handle)?;
            (transform.position, transform.rotation)
        };
        
        let rb_handle = {
            let rigid_body_comp = engine.scene_manager.get_entity_component::<RigidBodyComponent>(entity_handle, scene_handle)?;
            rigid_body_comp.get_rigid_body_handle()
        };
        
        if let Some(rb_handle) = rb_handle {
            let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
            if let Some(rigid_body) = physics_world.rigid_body_set.get_mut(rb_handle) {
                // Convert cgmath Vector3 to nalgebra Vector3
                let position = nalgebra::Vector3::new(
                    position.x,
                    position.y,
                    position.z,
                );
                
                // Convert Euler angles to quaternion
                let rotation = nalgebra::UnitQuaternion::from_euler_angles(
                    rotation.x.to_radians(),
                    rotation.y.to_radians(),
                    rotation.z.to_radians(),
                );
                
                // Update rigid body position and rotation
                rigid_body.set_position(Isometry::from_parts(position.into(), rotation), true);
            }
        }
    }
    
    Ok(())
}

// fn create_physics_bodies(
//     engine: &mut Engine, 
//     scene_handle: SceneHandle
// ) -> Result<()> {
//     // Get entities that need rigid bodies created
//     let entities_needing_rigidbodies: Vec<_> = {
//         let scene = engine.scene_manager.get_scene(scene_handle)?;
//         let mut entities = Vec::new();
        
//         for (entity_handle, rigid_body_comp) in scene.get_one_component_iterator::<RigidBodyComponent>()? {
//             if !rigid_body_comp.is_created() {
//                 entities.push(entity_handle);
//             }
//         }
//         entities
//     };
    
//     // Create rigid bodies
//     for entity_handle in entities_needing_rigidbodies {
//         let rigid_body_builder = {
//             let rigid_body_comp = engine.scene_manager.get_entity_component::<RigidBodyComponent>(entity_handle, scene_handle)?;
//             rigid_body_comp.rigid_body_builder.clone()
//         };
        
//         // Get the transform for initial position
//         let initial_transform = if let Ok(transform) = engine.scene_manager.get_entity_component::<TransformComponent>(entity_handle, scene_handle) {
//             let position = nalgebra::Vector3::new(
//                 transform.position.x,
//                 transform.position.y,
//                 transform.position.z,
//             );
            
//             let rotation = nalgebra::UnitQuaternion::from_euler_angles(
//                 transform.rotation.x.to_radians(),
//                 transform.rotation.y.to_radians(),
//                 transform.rotation.z.to_radians(),
//             );
            
//             Isometry::from_parts(position.into(), rotation)
//         } else {
//             Isometry::identity()
//         };
        
//         // Create the rigid body with initial transform
//         let rigid_body = rigid_body_builder.position(initial_transform).build();
        
//         // Insert the rigid body and store the handle
//         let rb_handle = {
//             let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
//             physics_world.rigid_body_set.insert(rigid_body)
//         };
        
//         // Store the handle in the component
//         let rigid_body_comp = engine.scene_manager.get_entity_component::<RigidBodyComponent>(entity_handle, scene_handle)?;
//         rigid_body_comp.rigid_body_handle = Some(rb_handle);
//     }
    
//     // Get entities that need colliders created
//     let entities_needing_colliders: Vec<_> = {
//         let scene = engine.scene_manager.get_scene(scene_handle)?;
//         let mut entities = Vec::new();
        
//         for (entity_handle, collider_comp) in scene.get_one_component_iterator::<ColliderComponent>()? {
//             if !collider_comp.is_created() {
//                 entities.push(entity_handle);
//             }
//         }
//         entities
//     };
    
//     // Create colliders
//     for entity_handle in entities_needing_colliders {
//         let collider_builder = {
//             let collider_comp = engine.scene_manager.get_entity_component::<ColliderComponent>(entity_handle, scene_handle)?;
//             collider_comp.collider_builder.clone()
//         };
        
//         // Create the collider
//         let collider = collider_builder.build();
        
//         // Try to attach to a rigid body if one exists for this entity
//         let parent_handle = if let Ok(rigid_body_comp) = engine.scene_manager.get_entity_component::<RigidBodyComponent>(entity_handle, scene_handle) {
//             rigid_body_comp.get_handle()
//         } else {
//             None
//         };
        
//         // Insert the collider and store the handle
//         let collider_handle = {
//             let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
//             physics_world.collider_set.insert_with_parent(
//                 collider,
//                 parent_handle,
//                 &mut physics_world.rigid_body_set,
//             )
//         };
        
//         // Store the handle in the component
//         let collider_comp = engine.scene_manager.get_entity_component::<ColliderComponent>(entity_handle, scene_handle)?;
//         collider_comp.collider_handle = Some(collider_handle);
//     }
    
//     Ok(())
// }

fn sync_physics_to_transforms(
    engine: &mut Engine, 
    scene_handle: SceneHandle
) -> Result<()> {
    // Get entity handles with both transform and rigid body components
    let entities_with_physics: Vec<_> = {
        let scene = engine.scene_manager.get_scene(scene_handle)?;
        let mut entities = Vec::new();
        
        for (entity_handle, _) in scene.get_one_component_iterator::<TransformComponent>()? {
            if scene.entity_has_component::<RigidBodyComponent>(entity_handle)? {
                entities.push(entity_handle);
            }
        }
        entities
    };
    
    // Process each entity
    for entity_handle in entities_with_physics {
        let rb_handle = {
            let rigid_body_comp = engine.scene_manager.get_entity_component::<RigidBodyComponent>(entity_handle, scene_handle)?;
            rigid_body_comp.get_rigid_body_handle()
        };
        
        if let Some(rb_handle) = rb_handle {
            let (position, rotation, body_type) = {
                let physics_world = engine.get_global_component::<PhysicsWorldComponent>()?;
                if let Some(rigid_body) = physics_world.rigid_body_set.get(rb_handle) {
                    let pos = rigid_body.translation();
                    let rot = rigid_body.rotation();
                    (
                        cgmath::Vector3::new(pos.x, pos.y, pos.z),
                        rot.euler_angles(),
                        rigid_body.body_type(),
                    )
                } else {
                    continue;
                }
            };
            
            // Only update dynamic bodies (kinematic and static bodies control their own position)
            if body_type == RigidBodyType::Dynamic {
                let transform = engine.scene_manager.get_entity_component::<TransformComponent>(entity_handle, scene_handle)?;
                
                transform.set_position(position);
                
                // Convert quaternion to Euler angles
                transform.set_rotation(cgmath::Vector3::new(
                    rotation.0.to_degrees(),
                    rotation.1.to_degrees(),
                    rotation.2.to_degrees(),
                ));

                transform.matrix_update_required = true;
            }
        }
    }
    
    Ok(())
}
