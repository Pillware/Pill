//! What touched what, as plain data game code can act on.
//!
//! # Responsibilities
//!
//! - Declares the three event kinds a project reacts to: a contact starting, a
//!   contact ending, and the force a contact carried.
//! - Holds one frame's events in [`PhysicsEvents`], which the drain empties at
//!   the start of the next frame.
//!
//! # Design
//!
//! A rapier event names collider handles, which mean nothing outside this crate.
//! The manager translates each handle into the entity that owns the collider, so
//! a project never sees a handle and never needs rapier in its dependencies.

// Current crate
use pill_engine::{resource, Entity};

// =============================================================================
// CollisionStarted
// =============================================================================

/// Two entities began touching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionStarted {
    /// The first of the two entities.
    pub entity_a: Entity,
    /// The second of the two entities.
    pub entity_b: Entity,
}

// =============================================================================
// CollisionStopped
// =============================================================================

/// Two entities stopped touching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionStopped {
    /// The first of the two entities.
    pub entity_a: Entity,
    /// The second of the two entities.
    pub entity_b: Entity,
}

// =============================================================================
// ContactForce
// =============================================================================

/// The force a contact carried, which is what breaks a crate or triggers a
/// sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContactForce {
    /// The first of the two entities.
    pub entity_a: Entity,
    /// The second of the two entities.
    pub entity_b: Entity,
    /// The largest force the contact carried, in newtons.
    pub magnitude: f32,
    /// The world direction the largest force pointed in, as a unit vector.
    pub direction: [f32; 3],
}

// =============================================================================
// PhysicsEvents
// =============================================================================

/// One frame's collisions.
///
/// The manager empties this at the start of each frame and fills it while the
/// simulation steps, so a system that reads it always sees the frame that just
/// ran. Game code that needs an event to outlive the frame copies it out.
#[resource]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhysicsEvents {
    /// Contacts that began this frame.
    pub started: Vec<CollisionStarted>,
    /// Contacts that ended this frame.
    pub stopped: Vec<CollisionStopped>,
    /// The force of every contact that reported one this frame.
    pub contact_forces: Vec<ContactForce>,
}

impl PhysicsEvents {
    /// Whether nothing collided this frame.
    pub fn is_empty(&self) -> bool {
        self.started.is_empty() && self.stopped.is_empty() && self.contact_forces.is_empty()
    }

    /// Drops every event, which the drain does before filling the collection
    /// again.
    pub fn clear(&mut self) {
        self.started.clear();
        self.stopped.clear();
        self.contact_forces.clear();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_collection_is_empty() {
        assert!(PhysicsEvents::default().is_empty());
    }

    #[test]
    fn clearing_drops_every_kind() {
        let mut events = PhysicsEvents::default();
        events.started.push(CollisionStarted {
            entity_a: Entity::NULL,
            entity_b: Entity::NULL,
        });
        events.contact_forces.push(ContactForce {
            entity_a: Entity::NULL,
            entity_b: Entity::NULL,
            magnitude: 3.0,
            direction: [0.0, 1.0, 0.0],
        });
        assert!(!events.is_empty());
        events.clear();
        assert!(events.is_empty());
    }
}
