//! The manager that owns the rapier world and keeps it in step with the
//! entities.
//!
//! # Responsibilities
//!
//! - Owns the rapier [`PhysicsWorld`]: its gravity, its integration settings and
//!   every body and collider the module created.
//! - Translates a [`RigidBodyComponent`] and a [`ColliderComponent`] into a
//!   rapier body, and a stepped rapier body back into a
//!   [`TransformComponent`].
//! - Remembers which entity owns which body, in both directions, so a contact or
//!   a raycast answer can name the entity.
//! - Steps the simulation on a fixed timestep, fed by the frame delta.
//! - Notices an edit made outside the simulation - a component changed in the
//!   Inspector, a dynamic body's transform moved by the gizmo or by game code -
//!   and carries it into the body instead of overwriting it.
//!
//! # Design
//!
//! The manager is the only place rapier is named, apart from the system that
//! drives it. Entities and components carry plain numbers, so a saved scene, the
//! editor and a project never need rapier in their own dependencies.
//!
//! It is a shared resource: a reload whose layout is unchanged keeps the value,
//! so the bodies keep their pose and velocity across it. When the layout did
//! change the engine drops the value, and the system rebuilds every body from
//! the components on its next pass.

// Standard library
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;

// External crates
use rapier3d::math::{Pose, Rotation, Vector};
use rapier3d::prelude::{
    ActiveEvents, ChannelEventCollector, Collider, ColliderBuilder, ColliderHandle, CollisionEvent,
    ContactForceEvent, Group, InteractionGroups, InteractionTestMode, LockedAxes, RigidBody,
    RigidBodyBuilder, RigidBodyHandle,
};

// Current crate
use crate::components::{BodyKind, ColliderComponent, ColliderShape, RigidBodyComponent};
use crate::events::{CollisionStarted, CollisionStopped, ContactForce, PhysicsEvents};
use crate::settings::PhysicsSettings;
use pill_engine::{Entity, PillResource, TransformComponent};

// =============================================================================
// Constants
// =============================================================================

/// Downward acceleration applied to a body with a gravity scale of one, in
/// world units per second squared.
pub const DEFAULT_GRAVITY: [f32; 3] = [0.0, -9.81, 0.0];

/// The simulation's step length in seconds. Every step advances the world by
/// exactly this much, whatever the frame rate did, so the same scene settles the
/// same way on every machine.
pub const DEFAULT_FIXED_TIMESTEP_SECONDS: f32 = 1.0 / 60.0;

/// The most steps one frame may run, so a long frame cannot spiral into a
/// longer one.
pub const MAXIMUM_SUBSTEPS_PER_FRAME: u32 = 8;

/// The most frame time the accumulator may hold, in seconds.
const MAXIMUM_ACCUMULATED_SECONDS: f32 = 0.25;

// =============================================================================
// PhysicsStatistics
// =============================================================================

/// A running count of what the simulation did, for logs and tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicsStatistics {
    /// Steps run since the manager was created.
    pub steps: u64,
    /// Bodies created since the manager was created.
    pub created_bodies: u64,
    /// Bodies dropped since the manager was created.
    pub dropped_bodies: u64,
}

// =============================================================================
// PhysicsManager
// =============================================================================

/// The simulation, and the map from entities to the bodies it built for them.
#[derive(PillResource)]
#[pill(shared, layout_sources = ["Cargo.toml", "src/components.rs", "src/manager.rs"])]
pub struct PhysicsManager {
    /// The rapier world: gravity, settings, bodies, colliders and broad phase.
    simulation: rapier3d::prelude::PhysicsWorld,
    /// The body of each simulated entity.
    body_handles: HashMap<Entity, RigidBodyHandle>,
    /// The collider of each simulated entity.
    collider_handles: HashMap<Entity, ColliderHandle>,
    /// The entity each collider belongs to, for contacts and raycast answers.
    collider_owners: HashMap<ColliderHandle, Entity>,
    /// The component values each body was last built from, so an edit to
    /// either component is noticed and the body rebuilt from it.
    applied_components: HashMap<Entity, (RigidBodyComponent, ColliderComponent)>,
    /// The pose last written into each dynamic body's transform. A transform
    /// that no longer holds it was moved by someone else, and the body follows.
    written_poses: HashMap<Entity, ([f32; 3], [f32; 4])>,
    /// The pose each body held before its last step, which interpolation blends
    /// towards the pose the step produced.
    previous_poses: HashMap<Entity, ([f32; 3], [f32; 4])>,
    /// Frame time not yet spent on steps, in seconds.
    accumulator_seconds: f32,
    /// The document the manager is running with.
    settings: PhysicsSettings,
    /// The channel rapier's collision events arrive on.
    ///
    /// The receiver sits behind a lock because a resource has to be shared
    /// between threads, and a receiver may only be taken from by one at a time.
    collision_events: Mutex<Receiver<CollisionEvent>>,
    /// The channel rapier's contact-force events arrive on.
    contact_force_events: Mutex<Receiver<ContactForceEvent>>,
    /// The collector handed to every step, which sends those two channels.
    event_collector: ChannelEventCollector,
    /// What the simulation did since it was created.
    statistics: PhysicsStatistics,
}

impl PhysicsManager {
    /// Creates a manager with world gravity and the default fixed step.
    pub fn new() -> Self {
        let mut simulation = rapier3d::prelude::PhysicsWorld::default();
        let settings = PhysicsSettings::default();
        simulation.gravity = vector_from_array(settings.gravity);
        simulation.integration_parameters.dt = settings.fixed_timestep;
        let (collision_sender, collision_events) = channel();
        let (contact_force_sender, contact_force_events) = channel();
        let (soft_body_tear_sender, _soft_body_tear_events) = channel();
        Self {
            simulation,
            body_handles: HashMap::new(),
            collider_handles: HashMap::new(),
            collider_owners: HashMap::new(),
            applied_components: HashMap::new(),
            written_poses: HashMap::new(),
            previous_poses: HashMap::new(),
            accumulator_seconds: 0.0,
            settings,
            collision_events: Mutex::new(collision_events),
            contact_force_events: Mutex::new(contact_force_events),
            event_collector: ChannelEventCollector::new(
                collision_sender,
                contact_force_sender,
                soft_body_tear_sender,
            ),
            statistics: PhysicsStatistics::default(),
        }
    }

    /// The rapier world, for the parts of the crate that query it.
    pub fn simulation(&self) -> &rapier3d::prelude::PhysicsWorld {
        &self.simulation
    }

    /// How many bodies the simulation currently holds.
    pub fn body_count(&self) -> usize {
        self.body_handles.len()
    }

    /// Whether an entity already has a body.
    pub fn has_body(&self, entity: Entity) -> bool {
        self.body_handles.contains_key(&entity)
    }

    /// The entity that owns a collider, if the collider is one of ours.
    pub fn entity_of_collider(&self, collider: ColliderHandle) -> Option<Entity> {
        self.collider_owners.get(&collider).copied()
    }

    /// What the simulation did since it was created.
    pub fn statistics(&self) -> PhysicsStatistics {
        self.statistics
    }

    /// The world gravity the simulation uses, in world units per second
    /// squared.
    pub fn gravity(&self) -> [f32; 3] {
        array_from_vector(self.simulation.gravity)
    }

    /// Sets the world gravity, in world units per second squared.
    pub fn set_gravity(&mut self, gravity: [f32; 3]) {
        self.simulation.gravity = vector_from_array(gravity);
    }

    /// The length of one simulation step, in seconds.
    pub fn fixed_timestep(&self) -> f32 {
        self.settings.fixed_timestep
    }

    /// Sets the step length and the most steps one frame may run.
    pub fn set_fixed_timestep(&mut self, fixed_timestep: f32, max_substeps: u32) {
        if !fixed_timestep.is_finite() || fixed_timestep <= 0.0 {
            return;
        }
        self.settings.fixed_timestep = fixed_timestep;
        self.settings.max_substeps = max_substeps.max(1);
        self.simulation.integration_parameters.dt = fixed_timestep;
    }

    /// Whether a drawn pose is blended between the last two steps.
    pub fn interpolation_enabled(&self) -> bool {
        self.settings.interpolate
    }

    /// Turns the blend between the last two steps on or off.
    pub fn set_interpolation(&mut self, interpolate: bool) {
        self.settings.interpolate = interpolate;
    }

    /// Whether the colliders are drawn as outlines.
    pub fn debug_draw_enabled(&self) -> bool {
        self.settings.debug_draw
    }

    /// Turns the collider outlines on or off.
    pub fn set_debug_draw(&mut self, debug_draw: bool) {
        self.settings.debug_draw = debug_draw;
    }

    /// The entity and collider handle of every simulated body.
    pub fn collider_owners(&self) -> impl Iterator<Item = (Entity, ColliderHandle)> + '_ {
        self.collider_handles
            .iter()
            .map(|(entity, handle)| (*entity, *handle))
    }

    /// A simulated collider, by its handle.
    pub fn collider_of(&self, handle: ColliderHandle) -> Option<&Collider> {
        self.simulation.colliders.get(handle)
    }

    /// Empties the two event channels into one frame's worth of events.
    ///
    /// An event naming a collider this module did not build is dropped: the
    /// owner map is the only route from a rapier handle back to an entity.
    pub fn drain_events(&mut self) -> PhysicsEvents {
        // Step 1: Take both channels empty first. The guards cannot stay alive
        // while the handles are translated, because that reads this manager.
        let mut collisions = Vec::new();
        if let Ok(receiver) = self.collision_events.get_mut() {
            while let Ok(event) = receiver.try_recv() {
                collisions.push(event);
            }
        }
        let mut contact_forces = Vec::new();
        if let Ok(receiver) = self.contact_force_events.get_mut() {
            while let Ok(event) = receiver.try_recv() {
                contact_forces.push(event);
            }
        }

        // Step 2: Name the two entities behind each pair of colliders.
        let mut events = PhysicsEvents::default();
        for event in collisions {
            match event {
                CollisionEvent::Started(first, second, _) => {
                    if let Some((entity_a, entity_b)) = self.pair_of(first, second) {
                        events.started.push(CollisionStarted { entity_a, entity_b });
                    }
                }
                CollisionEvent::Stopped(first, second, _) => {
                    if let Some((entity_a, entity_b)) = self.pair_of(first, second) {
                        events.stopped.push(CollisionStopped { entity_a, entity_b });
                    }
                }
            }
        }
        for event in contact_forces {
            if let Some((entity_a, entity_b)) = self.pair_of(event.collider1, event.collider2) {
                let direction = event.max_force_direction;
                events.contact_forces.push(ContactForce {
                    entity_a,
                    entity_b,
                    magnitude: event.max_force_magnitude,
                    direction: [direction.x, direction.y, direction.z],
                });
            }
        }
        events
    }

    /// The two entities behind a pair of collider handles, when both are ours.
    pub fn pair_of(
        &self,
        first: ColliderHandle,
        second: ColliderHandle,
    ) -> Option<(Entity, Entity)> {
        let entity_a = self.entity_of_collider(first)?;
        let entity_b = self.entity_of_collider(second)?;
        Some((entity_a, entity_b))
    }

    /// Builds a rapier body and collider for an entity from its components.
    ///
    /// The pose comes from the entity's transform, so a body appears where game
    /// code put it. Calling this for an entity that already has a body replaces
    /// the old one.
    pub fn spawn_body(
        &mut self,
        entity: Entity,
        transform: &TransformComponent,
        body: &RigidBodyComponent,
        collider: &ColliderComponent,
    ) {
        self.despawn_body(entity);
        let rigid_body = build_rigid_body(transform, body);
        let rapier_collider = build_collider(collider);
        let (body_handle, collider_handle) = self.simulation.insert(rigid_body, rapier_collider);
        self.body_handles.insert(entity, body_handle);
        self.collider_handles.insert(entity, collider_handle);
        self.collider_owners.insert(collider_handle, entity);
        self.applied_components.insert(entity, (*body, *collider));
        self.statistics.created_bodies += 1;
    }

    /// Rebuilds an entity's body when either component no longer holds the
    /// values the body was built from, and reports whether it did.
    ///
    /// The rebuilt body starts at the transform's pose. It keeps the speed the
    /// old one had reached unless the edit was to that velocity itself: the
    /// component's velocities are starting values, so changing the gravity
    /// scale must not throw a falling body back to its first speed.
    pub fn apply_component_edits(
        &mut self,
        entity: Entity,
        transform: &TransformComponent,
        body: &RigidBodyComponent,
        collider: &ColliderComponent,
    ) -> bool {
        let Some((applied_body, applied_collider)) = self.applied_components.get(&entity).copied()
        else {
            return false;
        };
        if applied_body == *body && applied_collider == *collider {
            return false;
        }
        // Step 1: Read the speed the old body reached before it is dropped.
        let live_velocities = self
            .body_handles
            .get(&entity)
            .and_then(|handle| self.simulation.bodies.get(*handle))
            .map(|rigid_body| (rigid_body.linvel(), rigid_body.angvel()));
        // Step 2: Build the new body from the edited components.
        self.spawn_body(entity, transform, body, collider);
        self.previous_poses.remove(&entity);
        // Step 3: Carry over each velocity the edit did not touch.
        let Some((linear_velocity, angular_velocity)) = live_velocities else {
            return true;
        };
        let Some(rigid_body) = self
            .body_handles
            .get(&entity)
            .and_then(|handle| self.simulation.bodies.get_mut(*handle))
        else {
            return true;
        };
        if applied_body.linear_velocity == body.linear_velocity {
            rigid_body.set_linvel(linear_velocity, true);
        }
        if applied_body.angular_velocity == body.angular_velocity {
            rigid_body.set_angvel(angular_velocity, true);
        }
        true
    }

    /// Moves a dynamic body to its transform when something other than the
    /// simulation moved that transform since the last write-back: the editor's
    /// gizmo, an Inspector edit, or game code placing the entity.
    ///
    /// The body keeps its velocity and is woken, so it carries on from the new
    /// place. A transform still holding the written pose is left alone.
    pub fn follow_moved_transform(&mut self, entity: Entity, transform: &TransformComponent) {
        let Some(written_pose) = self.written_poses.get(&entity).copied() else {
            return;
        };
        if written_pose == (transform.translation, transform.rotation) {
            return;
        }
        let Some(rigid_body) = self
            .body_handles
            .get(&entity)
            .and_then(|handle| self.simulation.bodies.get_mut(*handle))
        else {
            return;
        };
        rigid_body.set_position(
            Pose::from_parts(
                vector_from_array(transform.translation),
                rotation_from_array(transform.rotation),
            ),
            true,
        );
        // The blend would otherwise start from the pose before the move.
        self.previous_poses.remove(&entity);
        self.written_poses.remove(&entity);
    }

    /// Drops an entity's body and collider. An entity without a body is left
    /// alone.
    pub fn despawn_body(&mut self, entity: Entity) {
        let Some(body_handle) = self.body_handles.remove(&entity) else {
            return;
        };
        if let Some(collider_handle) = self.collider_handles.remove(&entity) {
            self.collider_owners.remove(&collider_handle);
        }
        self.applied_components.remove(&entity);
        self.written_poses.remove(&entity);
        let _ = self
            .simulation
            .remove_body_with_colliders(body_handle, true);
        self.statistics.dropped_bodies += 1;
    }

    /// Drops the bodies of every entity that is not in the given set, which is
    /// how a despawned or stripped entity leaves the simulation.
    pub fn despawn_missing(&mut self, live_entities: &HashSet<Entity>) {
        let stale: Vec<Entity> = self
            .body_handles
            .keys()
            .copied()
            .filter(|entity| !live_entities.contains(entity))
            .collect();
        for entity in stale {
            self.despawn_body(entity);
        }
    }

    /// Hands a kinematic body the pose its transform was moved to, which the
    /// next step carries it to.
    pub fn push_kinematic(&mut self, entity: Entity, transform: &TransformComponent) {
        let Some(handle) = self.body_handles.get(&entity).copied() else {
            return;
        };
        let Some(body) = self.simulation.bodies.get_mut(handle) else {
            return;
        };
        body.set_next_kinematic_translation(vector_from_array(transform.translation));
        body.set_next_kinematic_rotation(rotation_from_array(transform.rotation));
    }

    /// Copies a stepped body's pose into an entity's transform.
    ///
    /// A body the simulation does not move keeps whatever the transform already
    /// holds, so a fixed or kinematic body is left alone.
    pub fn write_back(&mut self, entity: Entity, transform: &mut TransformComponent) {
        let Some(handle) = self.body_handles.get(&entity).copied() else {
            return;
        };
        let Some(body) = self.simulation.bodies.get(handle) else {
            return;
        };
        if body.is_fixed() || body.is_kinematic() {
            return;
        }
        let pose = body.position();
        transform.translation = array_from_vector(pose.translation);
        transform.rotation = array_from_quaternion(pose.rotation);
        self.blend_towards_last_step(entity, transform);
        self.written_poses
            .insert(entity, (transform.translation, transform.rotation));
    }

    /// Blends a freshly written pose back towards the pose before the last
    /// step, when interpolation is on.
    pub fn blend_towards_last_step(&self, entity: Entity, transform: &mut TransformComponent) {
        if !self.settings.interpolate {
            return;
        }
        // Step 1: Blend towards the pose the last step produced, so the drawn
        // frame sits between two steps instead of snapping back to the last
        // one. The blend weight is the frame time not yet spent on a step.
        let Some((previous_translation, previous_rotation)) =
            self.previous_poses.get(&entity).copied()
        else {
            return;
        };
        let blend = (self.accumulator_seconds / self.settings.fixed_timestep).clamp(0.0, 1.0);
        if blend <= 0.0 {
            return;
        }
        let from = rotation_from_array(previous_rotation);
        let to = rotation_from_array(transform.rotation);
        let blended = from.slerp(to, blend);
        transform.translation = [
            previous_translation[0] + (transform.translation[0] - previous_translation[0]) * blend,
            previous_translation[1] + (transform.translation[1] - previous_translation[1]) * blend,
            previous_translation[2] + (transform.translation[2] - previous_translation[2]) * blend,
        ];
        transform.rotation = array_from_quaternion(blended);
    }

    /// Advances the simulation by one frame's time and returns how many fixed
    /// steps ran.
    ///
    /// The frame time is added to an accumulator and spent one fixed step at a
    /// time, so the simulation advances at the same rate whatever the frame rate
    /// is. A hitch spends at most
    /// [`MAXIMUM_SUBSTEPS_PER_FRAME`](self) steps and drops the rest instead of
    /// running faster than real time to catch up.
    pub fn step(&mut self, delta_seconds: f32) -> u32 {
        self.accumulator_seconds =
            (self.accumulator_seconds + delta_seconds.max(0.0)).min(MAXIMUM_ACCUMULATED_SECONDS);
        if self.accumulator_seconds >= self.settings.fixed_timestep {
            self.remember_poses();
        }
        let mut steps = 0;
        while self.accumulator_seconds >= self.settings.fixed_timestep
            && steps < self.settings.max_substeps
        {
            self.simulation.step_with_events(&(), &self.event_collector);
            self.accumulator_seconds -= self.settings.fixed_timestep;
            steps += 1;
            self.statistics.steps += 1;
        }
        if steps == self.settings.max_substeps {
            self.accumulator_seconds = 0.0;
        }
        steps
    }

    /// Records where every body stands, which the next write-back blends from
    /// when interpolation is on.
    pub fn remember_poses(&mut self) {
        self.previous_poses.clear();
        for (entity, handle) in self.body_handles.iter() {
            let Some(body) = self.simulation.bodies.get(*handle) else {
                continue;
            };
            let pose = body.position();
            self.previous_poses.insert(
                *entity,
                (
                    array_from_vector(pose.translation),
                    array_from_quaternion(pose.rotation),
                ),
            );
        }
    }
}

impl std::default::Default for PhysicsManager {
    /// A manager with world gravity and the default fixed step.
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// rapier bodies from components
// =============================================================================

/// Builds the rapier body a component pair describes.
pub fn build_rigid_body(transform: &TransformComponent, body: &RigidBodyComponent) -> RigidBody {
    let builder = match body.body_kind {
        BodyKind::Dynamic => RigidBodyBuilder::dynamic(),
        BodyKind::Fixed => RigidBodyBuilder::fixed(),
        BodyKind::KinematicPosition => RigidBodyBuilder::kinematic_position_based(),
        BodyKind::KinematicVelocity => RigidBodyBuilder::kinematic_velocity_based(),
    };
    let [translation_x, translation_y, translation_z] = transform.translation;
    builder
        .pose(Pose::from_parts(
            Vector::new(translation_x, translation_y, translation_z),
            rotation_from_array(transform.rotation),
        ))
        .linvel(vector_from_array(body.linear_velocity))
        .angvel(vector_from_array(body.angular_velocity))
        .gravity_scale(body.gravity_scale)
        .linear_damping(body.linear_damping)
        .angular_damping(body.angular_damping)
        .locked_axes(LockedAxes::from_bits_truncate(body.locked_axes))
        .ccd_enabled(body.continuous_collision_detection)
        .build()
}

/// Builds the rapier collider a component describes.
pub fn build_collider(collider: &ColliderComponent) -> rapier3d::prelude::Collider {
    let [first, second, third] = collider.dimensions;
    let shape = match collider.shape {
        ColliderShape::Ball => ColliderBuilder::ball(first),
        ColliderShape::Cuboid => ColliderBuilder::cuboid(first, second, third),
        ColliderShape::Capsule => ColliderBuilder::capsule_y(first, second),
        ColliderShape::Cylinder => ColliderBuilder::cylinder(first, second),
    };
    shape
        .friction(collider.friction)
        .restitution(collider.restitution)
        .density(collider.density)
        .sensor(collider.sensor)
        .active_events(ActiveEvents::COLLISION_EVENTS | ActiveEvents::CONTACT_FORCE_EVENTS)
        .collision_groups(InteractionGroups::new(
            Group::from_bits_truncate(collider.collision_group),
            Group::from_bits_truncate(collider.collision_mask),
            InteractionTestMode::And,
        ))
        .build()
}

// =============================================================================
// Conversions
// =============================================================================

/// Reads a three-element array as a rapier vector.
pub fn vector_from_array(values: [f32; 3]) -> Vector {
    Vector::new(values[0], values[1], values[2])
}

/// Reads a rapier vector as a three-element array.
pub fn array_from_vector(vector: Vector) -> [f32; 3] {
    [vector.x, vector.y, vector.z]
}

/// Reads an `[x, y, z, w]` quaternion as a rotation, treating a value that is
/// zero or not finite as the identity rather than using it.
pub fn rotation_from_array(rotation: [f32; 4]) -> Rotation {
    let [x, y, z, w] = rotation;
    let length_squared = x * x + y * y + z * z + w * w;
    if !length_squared.is_finite() || length_squared < f32::EPSILON {
        return Rotation::IDENTITY;
    }
    Rotation::from_xyzw(x, y, z, w).normalize()
}

/// Reads a rotation as an `[x, y, z, w]` array.
pub fn array_from_quaternion(rotation: Rotation) -> [f32; 4] {
    [rotation.x, rotation.y, rotation.z, rotation.w]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::World;

    /// The radius of the ball every test drops.
    const BALL_RADIUS: f32 = 0.5;
    /// How tall the test floor's top face sits above the origin.
    const FLOOR_TOP: f32 = 0.5;
    /// Where the ball rests once it lands: its radius above the floor's top.
    const RESTING_HEIGHT: f32 = FLOOR_TOP + BALL_RADIUS;
    /// How far a resting ball may sit from the height the geometry predicts.
    const LANDING_TOLERANCE: f32 = 0.05;

    /// Mints an entity from a world, which is the only source of entity ids.
    fn entity(world: &mut World) -> Entity {
        world
            .create_entity()
            .build()
            .expect("a new world allocates an entity")
    }

    /// A transform at the given height with no rotation.
    fn transform_at(height: f32) -> TransformComponent {
        TransformComponent {
            translation: [0.0, height, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    /// A box collider spanning the test floor, whose top face is
    /// [`FLOOR_TOP`] above the origin.
    fn floor_collider() -> ColliderComponent {
        ColliderComponent {
            shape: ColliderShape::Cuboid,
            dimensions: [5.0, FLOOR_TOP, 5.0],
            ..ColliderComponent::default()
        }
    }

    /// A ball collider of [`BALL_RADIUS`].
    fn ball_collider() -> ColliderComponent {
        ColliderComponent {
            shape: ColliderShape::Ball,
            dimensions: [BALL_RADIUS, 0.0, 0.0],
            ..ColliderComponent::default()
        }
    }

    /// Adds the floor and a ball above it, returning both entities.
    fn drop_a_ball(manager: &mut PhysicsManager, world: &mut World) -> (Entity, Entity) {
        let floor = entity(world);
        manager.spawn_body(
            floor,
            &transform_at(0.0),
            &RigidBodyComponent {
                body_kind: BodyKind::Fixed,
                ..RigidBodyComponent::default()
            },
            &floor_collider(),
        );
        let ball = entity(world);
        manager.spawn_body(
            ball,
            &transform_at(3.0),
            &RigidBodyComponent::default(),
            &ball_collider(),
        );
        (floor, ball)
    }

    /// Steps the simulation for the given number of frames.
    fn run_frames(manager: &mut PhysicsManager, frames: u32) {
        for _ in 0..frames {
            manager.step(DEFAULT_FIXED_TIMESTEP_SECONDS);
        }
    }

    #[test]
    fn a_dropped_ball_lands_on_the_floor() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let (_floor, ball) = drop_a_ball(&mut manager, &mut world);

        run_frames(&mut manager, 300);

        let mut transform = transform_at(3.0);
        manager.write_back(ball, &mut transform);
        assert!(
            (transform.translation[1] - RESTING_HEIGHT).abs() < LANDING_TOLERANCE,
            "the ball should rest at {RESTING_HEIGHT}, but it is at {}",
            transform.translation[1]
        );
        assert_eq!(manager.body_count(), 2);
        assert!(manager.statistics().steps >= 300);
    }

    #[test]
    fn a_fixed_body_keeps_the_pose_game_code_gave_it() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let floor = entity(&mut world);
        manager.spawn_body(
            floor,
            &transform_at(0.0),
            &RigidBodyComponent {
                body_kind: BodyKind::Fixed,
                ..RigidBodyComponent::default()
            },
            &floor_collider(),
        );

        run_frames(&mut manager, 60);
        let mut transform = transform_at(7.0);
        manager.write_back(floor, &mut transform);

        assert_eq!(transform.translation, [0.0, 7.0, 0.0]);
    }

    #[test]
    fn rebuilding_a_manager_from_the_components_lands_the_ball_the_same_way() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let (_floor, ball) = drop_a_ball(&mut manager, &mut world);
        run_frames(&mut manager, 300);

        let mut settled = transform_at(3.0);
        manager.write_back(ball, &mut settled);

        // What a layout change does: the engine drops the value and the system
        // rebuilds every body from the components it can still read.
        let mut rebuilt = PhysicsManager::new();
        let floor = entity(&mut world);
        rebuilt.spawn_body(
            floor,
            &transform_at(0.0),
            &RigidBodyComponent {
                body_kind: BodyKind::Fixed,
                ..RigidBodyComponent::default()
            },
            &floor_collider(),
        );
        rebuilt.spawn_body(
            ball,
            &settled,
            &RigidBodyComponent::default(),
            &ball_collider(),
        );
        run_frames(&mut rebuilt, 30);

        let mut after_rebuild = settled;
        rebuilt.write_back(ball, &mut after_rebuild);
        assert!(
            (after_rebuild.translation[1] - settled.translation[1]).abs() < LANDING_TOLERANCE,
            "the rebuilt world should hold the ball where the old one left it"
        );
    }

    #[test]
    fn a_despawned_entity_leaves_no_body_behind() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let (_floor, ball) = drop_a_ball(&mut manager, &mut world);

        let live: HashSet<Entity> = HashSet::new();
        manager.despawn_missing(&live);

        assert_eq!(manager.body_count(), 0);
        assert!(!manager.has_body(ball));
        assert_eq!(manager.statistics().dropped_bodies, 2);
    }

    #[test]
    fn a_landing_reports_the_contact_and_its_force() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let (floor, ball) = drop_a_ball(&mut manager, &mut world);

        run_frames(&mut manager, 120);
        let events = manager.drain_events();

        let names_both = |entity_a: Entity, entity_b: Entity| {
            (entity_a == floor && entity_b == ball) || (entity_a == ball && entity_b == floor)
        };
        assert!(
            events
                .started
                .iter()
                .any(|event| names_both(event.entity_a, event.entity_b)),
            "the landing should report one started contact"
        );
        assert!(
            events
                .contact_forces
                .iter()
                .any(|event| names_both(event.entity_a, event.entity_b) && event.magnitude > 0.0),
            "the landing should report the force it carried"
        );
        assert!(
            manager.drain_events().is_empty(),
            "a drain empties both channels"
        );
    }

    #[test]
    fn a_quaternion_that_is_not_usable_reads_as_the_identity() {
        assert_eq!(
            array_from_quaternion(rotation_from_array([0.0; 4])),
            [0.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            array_from_quaternion(rotation_from_array([0.0, 0.0, 0.0, 2.0])),
            [0.0, 0.0, 0.0, 1.0]
        );
    }

    /// An edit to a component after the body exists reaches the simulation:
    /// a ball whose gravity scale is set to zero mid-fall stops speeding up
    /// but keeps the speed it had, instead of falling as it was built.
    #[test]
    fn an_edited_component_rebuilds_the_body_and_keeps_its_speed() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let ball = entity(&mut world);
        let mut body = RigidBodyComponent::default();
        manager.spawn_body(ball, &transform_at(10.0), &body, &ball_collider());
        run_frames(&mut manager, 30);
        let mut transform = transform_at(10.0);
        manager.write_back(ball, &mut transform);

        // Step 1: The same values are not an edit.
        assert!(!manager.apply_component_edits(ball, &transform, &body, &ball_collider()));

        // Step 2: Turning gravity off rebuilds the body where it is now.
        body.gravity_scale = 0.0;
        assert!(manager.apply_component_edits(ball, &transform, &body, &ball_collider()));
        let height_after_edit = transform.translation[1];
        run_frames(&mut manager, 30);
        manager.write_back(ball, &mut transform);
        let fallen = height_after_edit - transform.translation[1];
        // Half a second at the speed reached after half a second of falling
        // (about 4.9 units per second) is about 2.45 units: no faster, since
        // gravity is off, and not zero, since the speed was kept.
        assert!(
            (2.2..2.7).contains(&fallen),
            "the ball should coast about 2.45 units, but it fell {fallen}"
        );
    }

    /// A dynamic body's transform moved by someone other than the simulation
    /// (the editor's gizmo, game code) moves the body there.
    #[test]
    fn a_moved_transform_moves_its_dynamic_body() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let (_floor, ball) = drop_a_ball(&mut manager, &mut world);
        run_frames(&mut manager, 300);
        let mut transform = transform_at(3.0);
        manager.write_back(ball, &mut transform);

        // Step 1: An untouched transform leaves the resting body alone.
        manager.follow_moved_transform(ball, &transform);
        run_frames(&mut manager, 1);
        manager.write_back(ball, &mut transform);
        assert!((transform.translation[1] - RESTING_HEIGHT).abs() < LANDING_TOLERANCE);

        // Step 2: Lift it, and it falls back from the new height.
        transform.translation[1] = 8.0;
        manager.follow_moved_transform(ball, &transform);
        run_frames(&mut manager, 1);
        manager.write_back(ball, &mut transform);
        assert!(
            transform.translation[1] > 7.5,
            "the ball should start falling from 8, but it is at {}",
            transform.translation[1]
        );
    }
}
