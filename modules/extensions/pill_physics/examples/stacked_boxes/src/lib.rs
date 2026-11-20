//! The physics example: a stack of boxes falling onto a static ground plane.
//!
//! # Responsibilities
//!
//! - Spawns one static ground plane - a collider with no rigid body - and a
//!   stack of dynamic boxes above it.
//! - Reports the moment the simulation is running, so a headless run can be
//!   checked without a renderer.
//! - Reports where the stack stands every few simulated seconds, which is what
//!   shows the contacts were resolved and what makes a module reload visible.
//!
//! # Design
//!
//! `pill_physics` owns the simulation; a project only says what exists. An
//! entity with a [`RigidBodyComponent`] and a [`ColliderComponent`] is a body
//! the simulation moves; a collider alone is static geometry. Either way its
//! pose lives in the [`TransformComponent`] every other system already reads.
//!
//! Every line this example writes starts with `[stacked_boxes]`, its folder
//! name. That is the convention the extension-example suite reads: a run passes
//! when the log carries `[<folder name>]`, so a new example states its own name
//! once and never needs an entry in a test.

// Standard library
use std::sync::atomic::{AtomicU64, Ordering};

// External crates
use pill_engine::*;
use pill_physics::{
    BodyKind, ColliderComponent, ColliderShape, PhysicsManager, RigidBodyComponent,
};

// =============================================================================
// Constants
// =============================================================================

/// How many boxes the stack holds.
const BOX_COUNT: u32 = 5;

/// The half extent of one box, so a box is a metre across.
const BOX_HALF_EXTENT: f32 = 0.5;

/// The gap between two boxes when the stack is built, so they start apart and
/// have to fall onto each other.
const BOX_GAP: f32 = 0.05;

/// How often the stack is reported, in microseconds of simulated time. The
/// first report lands after the stack has had time to settle, and the later
/// ones are what make a module reload visible: a line after a reload still
/// carrying the same heights means the simulation kept its bodies.
const REPORT_EVERY_MICROS: u64 = 5_000_000;

/// Counts the simulated time the project's report system has seen, in
/// microseconds: a frame at four thousand a second is a quarter of a
/// millisecond, which whole milliseconds would round away to nothing.
static REPORTED_MICROS: AtomicU64 = AtomicU64::new(0);

/// The simulated time the next report is due at, in microseconds.
static NEXT_REPORT_MICROS: AtomicU64 = AtomicU64::new(REPORT_EVERY_MICROS);

// =============================================================================
// Entry points
// =============================================================================

/// The half a reload repeats, before [`init`] on the first load and instead of
/// it on every later one.
///
/// The example declares no components, resources or systems of its own - the
/// module owns the simulation and the components carry the state - so there is
/// nothing to re-declare. It exists so a reload has somewhere to go other than
/// `init`: without it the host falls back to running `init` again and the scene
/// is seeded twice, on top of itself.
///
/// Returns 0.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

/// Builds the scene once, on the project's first load.
///
/// Returns `0` when the scene is ready, or `1` when an entity could not be
/// created - the host treats a non-zero status as a failed load.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();

    // Step 1: The ground. A collider with no rigid body is static geometry:
    // the boxes land on it, and it never moves on its own.
    let ground = ColliderComponent {
        shape: ColliderShape::Cuboid,
        dimensions: [10.0, BOX_HALF_EXTENT, 10.0],
        ..ColliderComponent::default()
    };
    if let Err(error) = world
        .create_entity()
        .with(TransformComponent {
            translation: [0.0, -BOX_HALF_EXTENT, 0.0],
            ..TransformComponent::default()
        })
        .with(ground)
        .build()
    {
        tracing::error!("[stacked_boxes] ground not spawned: {error}");
        return 1;
    }

    // Step 2: The stack. Each box starts a little above the one below it, so the
    // first frames are a fall and the rest is the contacts resolving.
    let box_collider = ColliderComponent {
        shape: ColliderShape::Cuboid,
        dimensions: [BOX_HALF_EXTENT, BOX_HALF_EXTENT, BOX_HALF_EXTENT],
        friction: 0.8,
        ..ColliderComponent::default()
    };
    for index in 0..BOX_COUNT {
        let height = BOX_HALF_EXTENT + (index as f32) * (BOX_HALF_EXTENT * 2.0 + BOX_GAP);
        if let Err(error) = world
            .create_entity()
            .with(TransformComponent {
                translation: [0.0, height, 0.0],
                ..TransformComponent::default()
            })
            .with(RigidBodyComponent::default())
            .with(box_collider)
            .build()
        {
            tracing::error!("[stacked_boxes] box {index} not spawned: {error}");
            return 1;
        }
    }

    tracing::info!(
        "[stacked_boxes] ready: {} boxes above a static ground plane",
        BOX_COUNT
    );
    0
}

// =============================================================================
// Reporting system
// =============================================================================

/// Reports where the stack stands every few simulated seconds.
///
/// The report is what a headless run reads to know the contacts were resolved:
/// the lowest box resting at its half extent above the origin means the ground
/// held, and a box that fell through reports a height no stack could hold. The
/// line also carries the applied step length and gravity, so a run shows the
/// settings document was read. Repeated lines are what show a module reload
/// kept the simulation: the heights after it match the ones before it.
#[system(name = "stacked_boxes_report", order = 1000)]
pub fn stack_report(
    time: Res<Time>,
    manager: Res<PhysicsManager>,
    mut bodies: Query<(&RigidBodyComponent, &TransformComponent)>,
) {
    let Some(time) = time.get() else {
        return;
    };
    let previous_micros = REPORTED_MICROS.fetch_add(
        (time.delta_seconds() * 1_000_000.0) as u64,
        Ordering::Relaxed,
    );
    if previous_micros < NEXT_REPORT_MICROS.load(Ordering::Relaxed) {
        return;
    }
    NEXT_REPORT_MICROS.store(previous_micros + REPORT_EVERY_MICROS, Ordering::Relaxed);

    // Step 1: The highest and lowest dynamic box say whether the stack stands
    // where the ground and the contacts put it.
    let mut highest: Option<f32> = None;
    let mut lowest: Option<f32> = None;
    for (body, transform) in bodies.iter_mut() {
        if body.body_kind != BodyKind::Dynamic {
            continue;
        }
        let height = transform.translation[1];
        highest = Some(highest.map_or(height, |current| current.max(height)));
        lowest = Some(lowest.map_or(height, |current| current.min(height)));
    }
    let (Some(highest), Some(lowest)) = (highest, lowest) else {
        tracing::warn!("[stacked_boxes] no box is left in the simulation");
        return;
    };

    // Step 2: The tuning the run actually used, read back from the manager.
    let (gravity_y, fixed_timestep) = match manager.get() {
        Some(manager) => (manager.gravity()[1], manager.fixed_timestep()),
        None => (f32::NAN, f32::NAN),
    };
    tracing::info!(
        "[stacked_boxes] stack: boxes y={lowest:.3}..{highest:.3} gravity_y={gravity_y:.3} step={fixed_timestep:.4}"
    );
}
