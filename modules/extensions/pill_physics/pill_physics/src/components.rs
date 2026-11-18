//! The two components that describe a physics body and its collider.
//!
//! # Responsibilities
//!
//! - Declares [`RigidBodyComponent`], the simulation inputs of one entity.
//! - Declares [`ColliderComponent`], the shape and material of one entity.
//! - Registers both with the world, field layout included, so the Inspector
//!   draws real rows and a saved scene restores every value. The two enums
//!   are declared with `PillMirror`, which is what makes their fields
//!   variant pickers instead of opaque bytes.
//!
//! # Design
//!
//! Both components hold plain numbers only: no rapier type appears here, so a
//! project, a saved scene and the editor never need rapier in their
//! dependencies. The manager converts these values when it builds a body.

// External crates
use serde::{Deserialize, Serialize};

// Current crate
use pill_engine::{component, PillMirror};

// =============================================================================
// BodyKind
// =============================================================================

/// How the simulation is allowed to move a body.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, PillMirror)]
pub enum BodyKind {
    /// The simulation moves it: gravity, forces and contacts decide the pose.
    #[default]
    Dynamic = 0,
    /// It never moves. This is static level geometry.
    Fixed = 1,
    /// Game code sets its pose, and it pushes whatever it touches on the way
    /// there.
    KinematicPosition = 2,
    /// Game code sets its velocity, and the simulation moves it from there.
    KinematicVelocity = 3,
}

// =============================================================================
// ColliderShape
// =============================================================================

/// The shape of a collider, which decides what its dimensions mean.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, PillMirror)]
pub enum ColliderShape {
    /// A sphere. `dimensions[0]` is the radius.
    #[default]
    Ball = 0,
    /// A box. `dimensions` are the half extents along x, y and z.
    Cuboid = 1,
    /// A capsule standing on its y axis. `dimensions[0]` is the half height of
    /// the cylindrical part and `dimensions[1]` is the radius.
    Capsule = 2,
    /// A cylinder standing on its y axis, read the same way as a capsule.
    Cylinder = 3,
}

// =============================================================================
// RigidBodyComponent
// =============================================================================

/// What the simulation needs to know about one body.
///
/// The pose is not here: it stays in
/// [`TransformComponent`](pill_engine::TransformComponent), which the body is
/// built from and which the simulation writes back to.
#[component(persistable, shared)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RigidBodyComponent {
    /// How the simulation may move this body.
    pub body_kind: BodyKind,
    /// Velocity in world units per second, in world space.
    pub linear_velocity: [f32; 3],
    /// Angular velocity in radians per second, in world space.
    pub angular_velocity: [f32; 3],
    /// Multiplier on world gravity. `0.0` floats, `-1.0` falls upwards.
    pub gravity_scale: f32,
    /// Fraction of linear velocity lost per second.
    pub linear_damping: f32,
    /// Fraction of angular velocity lost per second.
    pub angular_damping: f32,
    /// Bit 0 locks x, bit 1 y and bit 2 z translation; bit 3 locks x, bit 4 y
    /// and bit 5 z rotation. A set bit means that motion never happens, which
    /// is how a character walks upright without tipping over.
    pub locked_axes: u8,
    /// Whether the body sweeps its path instead of sampling it, so a fast body
    /// cannot pass through a thin wall. Costs time, so it is off by default.
    pub continuous_collision_detection: bool,
}

impl RigidBodyComponent {
    /// Whether the body may move on the given axis at all.
    ///
    /// Bit 0 is x translation, bit 1 y and bit 2 z; bits 3 to 5 lock the same
    /// axes for rotation. The manager hands the whole mask to rapier, whose
    /// `LockedAxes` uses those six bits in the same order.
    pub fn locks_translation(&self, axis: usize) -> bool {
        axis < 3 && self.locked_axes & (1 << axis) != 0
    }

    /// Whether the body may turn about the given axis at all.
    pub fn locks_rotation(&self, axis: usize) -> bool {
        axis < 3 && self.locked_axes & (1 << (3 + axis)) != 0
    }
}

impl std::default::Default for RigidBodyComponent {
    /// A dynamic body under full gravity that may move freely.
    fn default() -> Self {
        Self {
            body_kind: BodyKind::Dynamic,
            linear_velocity: [0.0; 3],
            angular_velocity: [0.0; 3],
            gravity_scale: 1.0,
            linear_damping: 0.0,
            angular_damping: 0.0,
            locked_axes: 0,
            continuous_collision_detection: false,
        }
    }
}

// =============================================================================
// ColliderComponent
// =============================================================================

/// The shape an entity occupies in the simulation and how it responds to
/// touching something.
///
/// A body has no collision without a collider, so the manager simulates the
/// entities that carry both components. The component also declares the gizmo
/// that outlines it: the engine draws it wherever a gizmo pass is installed,
/// and the settings document's `debug_draw` switch decides whether it draws.
#[component(persistable, shared, gizmo = crate::debug_draw::draw_collider_outline)]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderComponent {
    /// The shape the dimensions are read as.
    pub shape: ColliderShape,
    /// The shape's measurements, in world units; see [`ColliderShape`] for
    /// which slots each shape uses. Unused slots are ignored.
    pub dimensions: [f32; 3],
    /// Where the shape's centre sits relative to the entity's position, in the
    /// entity's own axes: it turns with the entity's rotation.
    pub offset: [f32; 3],
    /// How much a contact resists sliding. `0.0` is ice, `1.0` grips.
    pub friction: f32,
    /// How much speed a contact gives back. `0.0` lands dead, `1.0` bounces
    /// forever.
    pub restitution: f32,
    /// Mass per cubic meter. Two bodies of the same size push each other
    /// according to this, so a stone ball shoves a wooden crate aside.
    pub density: f32,
    /// Whether the collider only detects contacts without pushing anything.
    pub sensor: bool,
    /// The collision groups this collider belongs to, as a bit mask.
    pub collision_group: u32,
    /// The collision groups this collider may touch, as a bit mask.
    pub collision_mask: u32,
}

impl std::default::Default for ColliderComponent {
    /// A ball of wood with unit dimensions (a radius of one), centred on the
    /// entity, that collides with everything.
    fn default() -> Self {
        Self {
            shape: ColliderShape::Ball,
            dimensions: [1.0, 1.0, 1.0],
            offset: [0.0, 0.0, 0.0],
            friction: 0.7,
            restitution: 0.0,
            density: 1.0,
            sensor: false,
            collision_group: 1,
            collision_mask: u32::MAX,
        }
    }
}

// =============================================================================
// Registration
// =============================================================================

/// Registers both components with the world, field layout included.
///
/// The derive writes `__pill_register_<Type>` as a private item of this module,
/// so [`crate::register`] reaches the two through here rather than by path - and
/// so does a crate that links them without running the module entry point, such
/// as the editor tool or a test world.
pub fn register(world: &mut pill_engine::World) {
    __pill_register_RigidBodyComponent(world);
    __pill_register_ColliderComponent(world);
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_body_is_dynamic_under_full_gravity() {
        let body = RigidBodyComponent::default();
        assert_eq!(body.body_kind, BodyKind::Dynamic);
        assert_eq!(body.gravity_scale, 1.0);
        assert!(!body.locks_translation(0) && !body.locks_translation(1));
        assert!(!body.locks_rotation(0) && !body.locks_rotation(1));
    }

    #[test]
    fn locking_the_upright_axes_leaves_only_x_and_yaw() {
        // Bits 1 and 4 lock translation on y and rotation about y: an upright
        // character that cannot leave the ground plane.
        let body = RigidBodyComponent {
            locked_axes: (1 << 1) | (1 << 4),
            ..RigidBodyComponent::default()
        };
        assert!(!body.locks_translation(0));
        assert!(body.locks_translation(1));
        assert!(!body.locks_translation(2));
        assert!(!body.locks_rotation(0));
        assert!(body.locks_rotation(1));
        assert!(!body.locks_rotation(2));
    }

    #[test]
    fn a_new_collider_is_a_solid_ball() {
        let collider = ColliderComponent::default();
        assert_eq!(collider.shape, ColliderShape::Ball);
        assert_eq!(collider.dimensions, [1.0, 1.0, 1.0]);
        assert_eq!(collider.offset, [0.0, 0.0, 0.0]);
        assert!(!collider.sensor);
        assert_eq!(collider.collision_mask, u32::MAX);
    }
}
