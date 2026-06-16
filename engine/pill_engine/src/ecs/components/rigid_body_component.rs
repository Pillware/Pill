use crate::{
    ecs::{
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest,
        DeferredUpdateManagerPointer, EntityHandle, PhysicsWorldComponent, SceneHandle,
        TransformComponent,
    },
    engine::Engine,
};

use pill_core::{get_type_name, PillStyle, PillTypeMapKey};
use pill_core::{ErrorContext, Result};
use rapier3d::{glamx::EulerRot, prelude::*};

const DEFERRED_REQUEST_VARIANT_ADD: usize = 0;

/// A component that wraps a Rapier rigid body for physics simulation.
///
/// This component stores a RigidBodyBuilder that defines the properties of the rigid body
/// and a handle to the actual rigid body in the physics world once it's created.
///
/// # Usage Examples
///
/// ```rust
/// // Create a simple dynamic rigid body
/// let rigid_body = RigidBodyComponent::dynamic();
///
/// // Create a kinematic rigid body with custom properties
/// let rigid_body = RigidBodyComponent::builder()
///     .body_type(RigidBodyType::KinematicPositionBased)
///     .linear_damping(0.5)
///     .angular_damping(0.1)
///     .build();
///
/// // Create a fixed (static) rigid body
/// let rigid_body = RigidBodyComponent::fixed();
/// ```
///
///

// rapier3d::dynamics::rigid_body
#[derive(Clone)]
#[readonly::make]
pub struct RigidBodyComponent {
    #[readonly]
    pub body_type: RigidBodyType,
    #[readonly]
    pub locked_axes: LockedAxes,
    #[readonly]
    pub linear_damping: Real,
    #[readonly]
    pub angular_damping: Real,
    #[readonly]
    pub gravity_scale: Real,
    #[readonly]
    pub additional_mass: Option<RigidBodyAdditionalMassProps>,
    #[readonly]
    pub initial_position: Pose,
    #[readonly]
    pub ccd_enabled: bool,
    #[readonly]
    pub dominance_group: i8,
    #[readonly]
    pub can_sleep: bool,
    #[readonly]
    pub enabled: bool,

    rigid_body_handle: Option<RigidBodyHandle>,
    scene_handle: Option<SceneHandle>,
    entity_handle: Option<EntityHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl RigidBodyComponent {
    pub fn new() -> Self {
        let r = rapier3d::dynamics::RigidBodyBuilder::dynamic();
        Self {
            body_type: RigidBodyType::Dynamic,
            locked_axes: LockedAxes::empty(),
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 1.0,
            additional_mass: None,
            initial_position: Pose::identity(),
            ccd_enabled: false,
            dominance_group: 0,
            can_sleep: true,
            enabled: true,

            rigid_body_handle: None,
            scene_handle: None,
            entity_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn builder() -> RigidBodyComponentBuilder {
        RigidBodyComponentBuilder::new()
    }

    pub fn get_rigid_body_handle(&self) -> Option<RigidBodyHandle> {
        self.rigid_body_handle
    }

    pub fn register_in_physics_world(&mut self, engine: &mut Engine) -> Result<()> {
        // Try to get associated rigid body handle
        let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;

        let rigid_body: RigidBody = self.clone().into();
        self.rigid_body_handle = Some(physics_world.rigid_body_set.insert(rigid_body));

        Ok(())
    }

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if let Some(manager) = &mut self.deferred_update_manager {
            let entity_handle = self.entity_handle.expect(
                "Critical: Cannot post deferred update request. No EntityHandle set in Component",
            );
            let scene_handle = self.scene_handle.expect(
                "Critical: Cannot post deferred update request. No SceneHandle set in Component",
            );
            let request = DeferredUpdateComponentRequest::<RigidBodyComponent>::new(
                entity_handle,
                scene_handle,
                request_variant,
            );
            manager.post_update_request(request);
        }
    }
}

pub struct RigidBodyComponentBuilder {
    component: RigidBodyComponent,
}

impl RigidBodyComponentBuilder {
    pub fn new() -> Self {
        Self {
            component: RigidBodyComponent::new(),
        }
    }

    pub fn body_type(mut self, body_type: RigidBodyType) -> Self {
        self.component.body_type = body_type;
        self
    }

    // pub fn body_type(mut self, body_type: RigidBodyType) -> Self {
    //     self.component = match body_type {
    //         RigidBodyType::Dynamic => RigidBodyBuilder::dynamic(),
    //         RigidBodyType::Fixed => RigidBodyBuilder::fixed(),
    //         RigidBodyType::KinematicPositionBased => RigidBodyBuilder::kinematic_position_based(),
    //         RigidBodyType::KinematicVelocityBased => RigidBodyBuilder::kinematic_velocity_based(),
    //     };
    //     self
    // }

    pub fn locked_axes(mut self, locked_axes: LockedAxes) -> Self {
        self.component.locked_axes = locked_axes;
        self
    }

    pub fn linear_damping(mut self, linear_damping: Real) -> Self {
        self.component.linear_damping = linear_damping;
        self
    }

    pub fn angular_damping(mut self, angular_damping: Real) -> Self {
        self.component.angular_damping = angular_damping;
        self
    }

    pub fn can_sleep(mut self, can_sleep: bool) -> Self {
        self.component.can_sleep = can_sleep;
        self
    }

    pub fn ccd_enabled(mut self, ccd_enabled: bool) -> Self {
        self.component.ccd_enabled = ccd_enabled;
        self
    }

    pub fn build(self) -> RigidBodyComponent {
        self.component
    }
}

impl PillTypeMapKey for RigidBodyComponent {
    type Storage = ComponentStorage<RigidBodyComponent>;
}

// Implement Into<rapier3d::dynamics::RigidBody> for RigidBodyComponent
impl Into<RigidBody> for RigidBodyComponent {
    fn into(self) -> rapier3d::dynamics::RigidBody {
        let builder = rapier3d::dynamics::RigidBodyBuilder::new(self.body_type)
            .pose(self.initial_position)
            .locked_axes(self.locked_axes)
            .linear_damping(self.linear_damping)
            .angular_damping(self.angular_damping)
            .gravity_scale(self.gravity_scale)
            .ccd_enabled(self.ccd_enabled)
            .can_sleep(self.can_sleep)
            .enabled(self.enabled)
            .dominance_group(self.dominance_group);

        // if let Some(mass_props) = self.additional_mass {
        //     builder = builder.additional_mass_properties(mass_props);
        // }

        builder.build()
    }
}

impl Component for RigidBodyComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine
            .get_global_component_mut::<DeferredUpdateComponent>()
            .expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager =
            Some(deferred_update_component.borrow_deferred_update_manager());

        // self.register_in_physics_world(engine)

        //     .context(format!("Registering {} {} in physics world", "Component".gobj_style(), get_type_name::<Self>().sobj_style()))?;

        Ok(())
    }

    fn pass_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {
        self.scene_handle = Some(self_scene_handle);
        self.entity_handle = Some(self_entity_handle);

        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_ADD);
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> {
        match request {
            DEFERRED_REQUEST_VARIANT_ADD => {
                let entity_handle = self
                    .entity_handle
                    .expect("Critical: Cannot register RigidBodyComponent without EntityHandle");
                let scene_handle = self
                    .scene_handle
                    .expect("Critical: Cannot register RigidBodyComponent without SceneHandle");

                let (position, rotation_deg) = {
                    let transform = engine
                        .scene_manager
                        .get_entity_component::<TransformComponent>(entity_handle, scene_handle)?;

                    (transform.position, transform.rotation)
                };
                let rotation = Rotation::from_euler(
                    EulerRot::XYZ,
                    rotation_deg.x.to_radians(),
                    rotation_deg.y.to_radians(),
                    rotation_deg.z.to_radians(),
                );

                let mut rigid_body: RigidBody = self.clone().into();

                // Sync the transform to Rapier, after that dynamic rigidbodies are handled by
                // Rapier only
                rigid_body.set_position(Pose::from_parts(position, rotation), false);

                let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
                self.rigid_body_handle = Some(physics_world.rigid_body_set.insert(rigid_body));
            }
            _ => {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
            }
        }

        Ok(())
    }
}
