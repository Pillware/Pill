//! The Uber spline example: one path, edited by the spline tool.
//!
//! # Responsibilities
//!
//! - Spawns the entity that carries the example's [`Spline`].
//! - Reports the path's shape once on the load that created it, so a headless
//!   run can be checked without a window.
//!
//! # Design
//!
//! This is the project the spline proving case is driven against: the editor's
//! spline tool reports its points and tangents as elements, and the editor-only
//! roll extension attaches to the same entity once it carries the extension's
//! extras component. None of that is here, because a game build links neither
//! editor crate - this project owns the data, and the editor owns the way the
//! data is edited.
//!
//! The spawn happens in `register`, not in `init`, and only when the world
//! holds no spline yet: a reload of this project repeats `register`, and the
//! path the reader built up in the editor must not be replaced by the demo one.
//!
//! Every line starts with `[uber_spline]`, this project's folder name, which is
//! the convention the extension-example suite reads.

// External crates
use pill_core::math::Vector3f;
use pill_engine::*;
use pill_spline::Spline;

// =============================================================================
// Constants
// =============================================================================

/// The control points the example's path starts with, in the plane the demo
/// scene lives in.
const CONTROL_POINTS: [[f32; 3]; 4] = [
    [0.0, 0.0, 0.0],
    [120.0, 180.0, 0.0],
    [320.0, -60.0, 0.0],
    [520.0, 120.0, 0.0],
];

// =============================================================================
// Entry points
// =============================================================================

/// Builds the path once, on the project's first load.
///
/// # Errors
///
/// Returns 1 when the entity cannot be created, which the host treats as a
/// failed load.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Fills the world with the example's path if it has none.
///
/// This is the half a reload repeats, so it must be idempotent: an existing
/// path is the reader's work and is never replaced.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: is there a path already? A reload keeps the entities the last
    // generation created, and a dropped column takes them with it, so the
    // question is asked against the live world every time.
    let existing = {
        let mut query = Query::<&Spline>::new(engine.world_mut());
        query.iter_mut().count()
    };
    if existing > 0 {
        tracing::info!("[uber_spline] ready points={existing}");
        return 0;
    }

    // Step 2: the path itself, at the origin.
    let points: Vec<Vector3f> = CONTROL_POINTS
        .iter()
        .map(|point| Vector3f::new(point[0], point[1], point[2]))
        .collect();
    let mut spline = Spline::from_points(&points);
    spline.refit_tangents();
    if let Err(error) = engine
        .world_mut()
        .create_entity()
        .with(spline)
        .with(TransformComponent::default())
        .build()
    {
        tracing::error!("[uber_spline] path not spawned: {error}");
        return 1;
    }

    tracing::info!("[uber_spline] ready points={}", points.len());
    0
}
