use crate::{
    ecs::components::{ Component, GlobalComponent, GlobalComponentStorage } 
};

use pill_core::PillTypeMapKey;
use rapier3d::prelude::*;
use anyhow::{Result, Error, Context};

/// A global component that manages the Rapier physics world.
/// 
/// This component contains all the physics simulation data including rigid bodies,
/// colliders, joints, and the physics pipeline. It should be registered as a global
/// component in your engine.
/// 
/// # Usage
/// 
/// The physics world is automatically stepped by the physics system. You can access
/// it to modify global physics parameters:
/// 
/// ```rust
/// // Set custom gravity
/// let physics_world = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
/// physics_world.set_gravity(vector![0.0, -9.81, 0.0]);
/// ```
pub struct PhysicsWorldComponent {
    pub rigid_body_set: RigidBodySet,
    pub collider_set: ColliderSet,
    pub gravity: Vector<Real>,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: BroadPhase,
    pub narrow_phase: NarrowPhase,
    pub impulse_joint_set: ImpulseJointSet,
    pub multibody_joint_set: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
    pub query_pipeline: QueryPipeline,
    pub physics_hooks: (),
    pub event_handler: (),
}

impl PhysicsWorldComponent {
    pub fn new() -> Self {
        Self {
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            gravity: vector![0.0, -9.81, 0.0],
            integration_parameters: IntegrationParameters::default(),
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: BroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            physics_hooks: (),
            event_handler: (),
        }
    }

    pub fn set_gravity(&mut self, gravity: Vector<Real>) {
        self.gravity = gravity;
    }

    pub fn step(&mut self) {
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &self.physics_hooks,
            &self.event_handler,
        );
    }
}

impl Component for PhysicsWorldComponent {}

impl PillTypeMapKey for PhysicsWorldComponent {
    type Storage = GlobalComponentStorage<PhysicsWorldComponent>;
}

impl GlobalComponent for PhysicsWorldComponent {}
