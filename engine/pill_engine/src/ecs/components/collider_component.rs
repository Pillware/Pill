use crate::{
    ecs::{
        components::rigid_body_component, Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, PhysicsWorldComponent, RigidBodyComponent, SceneHandle
    }, engine::Engine
};

use pill_core::{get_type_name, PillStyle, PillTypeMapKey, Vector3f};
use rapier3d::{na::Vector3, prelude::*};
use anyhow::{ Result, Error, Context };

const DEFERRED_REQUEST_VARIANT_ADD: usize = 0;

#[derive(Clone)]
#[readonly::make]
pub struct ColliderComponent {
    #[readonly]
    pub shape: SharedShape,
    #[readonly]
    pub position: Isometry<Real>,
    #[readonly]
    pub friction: Real,
    #[readonly]
    pub restitution: Real,
    #[readonly]
    pub mass: Real,
    #[readonly]
    pub is_sensor: bool,
    #[readonly]
    pub collision_groups: InteractionGroups,
    #[readonly]
    pub solver_groups: InteractionGroups,

    collider_handle: Option<ColliderHandle>,
    scene_handle: Option<SceneHandle>,
    entity_handle: Option<EntityHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl ColliderComponent {
    pub fn new() -> Self {
        Self {
            shape: SharedShape::cuboid(0.5, 0.5, 0.5),
            position: Isometry::identity(),
            friction: 0.5,
            restitution: 0.0,
            mass: 1.0,
            is_sensor: false,
            collision_groups: InteractionGroups::all(),
            solver_groups: InteractionGroups::all(),

            collider_handle: None,
            scene_handle: None,
            entity_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn builder() -> ColliderComponentBuilder {
        ColliderComponentBuilder::default()
    }

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if self.deferred_update_manager.is_some() {
            let entity_handle = self.entity_handle.expect("Critical: Cannot post deferred update request. No EntityHandle set in Component");
            let scene_handle = self.scene_handle.expect("Critical: Cannot post deferred update request. No SceneHandle set in Component");
            let request = DeferredUpdateComponentRequest::<ColliderComponent>::new(entity_handle, scene_handle, request_variant);
            self.deferred_update_manager.as_mut().expect("Critical: No DeferredUpdateManager").post_update_request(request);
        }
    }

    fn register_in_physics_world(&mut self, engine: &mut Engine) -> Result<()> {
        // Try to get associated rigid body handle
        let rigid_body_component_storage = engine.scene_manager.get_active_scene().unwrap().get_component_storage::<RigidBodyComponent>()?;
        let rigid_body_handle: Option<RigidBodyHandle> = rigid_body_component_storage.data.get(self.entity_handle.unwrap().0.index as usize)
            .context("Failed to get rigid body handle, no corresponding rigidbody component found in the entity")?.as_ref().and_then(|rb| rb.get_rigid_body_handle().clone());
        let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;

        match rigid_body_handle {
            Some(rigid_body_handle) => {
                // If we have a rigid body handle, we can create the collider with it
                let collider: Collider = self.clone().into();
                let rigid_body_set = &mut physics_world.rigid_body_set;
                self.collider_handle = Some(physics_world.collider_set.insert_with_parent(collider, rigid_body_handle, rigid_body_set));
            },
            None => {
                // If we don't have a rigid body handle, we can still create the collider
                let collider: Collider = self.clone().into();
                self.collider_handle = Some(physics_world.collider_set.insert(collider));
            }
        }

        Ok(())
    }
}

pub struct ColliderComponentBuilder {
    component: ColliderComponent,
}

impl ColliderComponentBuilder {
    pub fn new() -> Self {
        Self {
            component: ColliderComponent::new(),
        }
    }

    pub fn default() -> Self {
        Self {
            component: ColliderComponent::new(),
        }
    }

    pub fn shape(mut self, shape: SharedShape) -> Self {
        self.component.shape = shape;
        self
    }

    pub fn friction(mut self, friction: Real) -> Self {
        self.component.friction = friction;
        self
    }

    pub fn restitution(mut self, restitution: Real) -> Self {
        self.component.restitution = restitution;
        self
    }

    pub fn mass(mut self, mass: Real) -> Self {
        self.component.mass = mass;
        self
    }

    pub fn sensor(mut self, is_sensor: bool) -> Self {
        self.component.is_sensor = is_sensor;
        self
    }

    pub fn collision_groups(mut self, collision_groups: InteractionGroups) -> Self {
        self.component.collision_groups = collision_groups;
        self
    }

    pub fn solver_groups(mut self, solver_groups: InteractionGroups) -> Self {
        self.component.solver_groups = solver_groups;
        self
    }

    pub fn position(mut self, position: Isometry<Real>) -> Self {
        self.component.position = position;
        self
    }

    pub fn translation(mut self, translation: Vector3f) -> Self {
        self.component.position = Isometry::translation(translation.x, translation.y, translation.z);
        self
    }

    pub fn build(self) -> ColliderComponent {
        self.component
    }
}

impl PillTypeMapKey for ColliderComponent {
    type Storage = ComponentStorage<ColliderComponent>;
}

// Implement Into<rapier3d::dynamics::RigidBody> for RigidBodyComponent
impl Into<Collider> for ColliderComponent {
    fn into(self) -> Collider {
        let mut builder = ColliderBuilder::new(self.shape)
            .position(self.position)
            .friction(self.friction)
            .restitution(self.restitution)
            .mass(self.mass)
            .sensor(self.is_sensor)
            .collision_groups(self.collision_groups)
            .solver_groups(self.solver_groups);

        builder.build()
    }
}



impl Component for ColliderComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

    
        // self.register_in_physics_world(engine)
        //     .context(format!("Registering {} {} in physics world", "Component".gobj_style(), get_type_name::<Self>().sobj_style()))?;

        Ok(())
    }

    fn pass_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {
        self.scene_handle = Some(self_scene_handle);
        self.entity_handle = Some(self_entity_handle);

            self.post_deferred_update_request(
            DEFERRED_REQUEST_VARIANT_ADD
        );
    }

    

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { 
        match request {
            DEFERRED_REQUEST_VARIANT_ADD => 
            {
                // Try to get associated rigid body handle
                let rigid_body_component_storage = engine.scene_manager.get_active_scene().unwrap().get_component_storage::<RigidBodyComponent>()?;
                let rigid_body_handle: Option<RigidBodyHandle> = rigid_body_component_storage.data.get(self.entity_handle.unwrap().0.index as usize).unwrap().as_ref().and_then(|rb| rb.get_rigid_body_handle().clone());
                let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;

                match rigid_body_handle {
                    Some(rigid_body_handle) => {
                        // If we have a rigid body handle, we can create the collider with it
                        let collider: Collider = self.clone().into();
                        let rigid_body_set = &mut physics_world.rigid_body_set;
                        self.collider_handle = Some(physics_world.collider_set.insert_with_parent(collider, rigid_body_handle, rigid_body_set));
                    },
                    None => {
                        // If we don't have a rigid body handle, we can still create the collider
                        let collider: Collider = self.clone().into();
                        self.collider_handle = Some(physics_world.collider_set.insert(collider));
                    }
                }
            },
            _ => 
            {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().sobj_style());
            }
        }

        Ok(()) 
    }
}