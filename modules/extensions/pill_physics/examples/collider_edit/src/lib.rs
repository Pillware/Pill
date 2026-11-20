//! The collider-editing example: a box and a ball, ready to be edited.
//!
//! # Responsibilities
//!
//! - Spawns the two entities whose colliders the editor's collider tool edits:
//!   a cuboid and a ball.
//! - Reports once when the scene is ready, so a headless run can be checked
//!   without a window.
//!
//! # Design
//!
//! The colliders are the whole point of the example. The cuboid has three
//! different half extents, so each of its six face handles sits somewhere a
//! reader can tell apart, and it is spawned first, which is the order the
//! entity list reads in. The ball follows, because its shape offers one handle
//! where the cuboid offers six and a suite can compare the two without editing
//! anything.
//!
//! The spawn lives in `register`, not in `init`, and only when the world holds
//! no collider yet: a reload repeats `register`, and the shapes the reader
//! dragged must not be replaced by the example's own.
//!
//! Every line starts with `[collider_edit]`, this project's folder name, which
//! is the convention the extension-example suite reads.

// External crates
use pill_engine::*;
use pill_physics::{ColliderComponent, ColliderShape};

// =============================================================================
// Constants
// =============================================================================

/// The half extents the example's cuboid starts with, one per axis so the
/// six faces are told apart at a glance.
const CUBOID_HALF_EXTENTS: [f32; 3] = [1.0, 1.5, 2.0];

/// The radius the example's ball starts with.
const BALL_RADIUS: f32 = 1.0;

/// Where the example's cuboid sits.
const CUBOID_TRANSLATION: [f32; 3] = [0.0, 2.0, 0.0];

/// Where the example's ball sits, clear of the cuboid's handles.
const BALL_TRANSLATION: [f32; 3] = [5.0, 2.0, 0.0];

// =============================================================================
// Entry points
// =============================================================================

/// Builds the scene once, on the project's first load.
///
/// # Errors
///
/// Returns 1 when the entity cannot be created, which the host treats as a
/// failed load.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Fills the world with the example's collider if it has none.
///
/// This is the half a reload repeats, so it asks the live world rather than
/// remembering anything between generations.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: a collider already there is the reader's work, not the example's
    // to replace.
    let existing = {
        let mut query = Query::<&ColliderComponent>::new(engine.world_mut());
        query.iter_mut().count()
    };
    if existing > 0 {
        tracing::info!("[collider_edit] ready colliders={existing}");
        return 0;
    }

    // Step 2: the two entities this example is about. The cuboid first, so the
    // entity list reads it above the ball.
    let shapes = [
        (
            ColliderComponent {
                shape: ColliderShape::Cuboid,
                dimensions: CUBOID_HALF_EXTENTS,
                ..ColliderComponent::default()
            },
            CUBOID_TRANSLATION,
        ),
        (
            ColliderComponent {
                shape: ColliderShape::Ball,
                dimensions: [BALL_RADIUS, 0.0, 0.0],
                ..ColliderComponent::default()
            },
            BALL_TRANSLATION,
        ),
    ];
    for (collider, translation) in shapes {
        if let Err(error) = engine
            .world_mut()
            .create_entity()
            .with(collider)
            .with(TransformComponent {
                translation,
                ..TransformComponent::default()
            })
            .build()
        {
            tracing::error!("[collider_edit] collider not spawned: {error}");
            return 1;
        }
    }

    tracing::info!(
        "[collider_edit] ready colliders=2 cuboid={:?} ball_radius={}",
        CUBOID_HALF_EXTENTS,
        BALL_RADIUS
    );
    0
}
