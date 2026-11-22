//! Moves every pill through an animated curl noise field.
//!
//! # Responsibilities
//!
//! - Sample the curl of a noise vector potential at each pill, pull the pill
//!   towards the middle of the field, damp and clamp its velocity, and keep it
//!   inside the field's box ([`curl_noise_system`]).
//!
//! # Design
//!
//! The field is Bridson, Hourihan and Nordenstam's curl noise ("Curl-Noise for
//! Procedural Fluid Flow", SIGGRAPH 2007). A vector potential Ψ is built from
//! three decorrelated noise fields, and the pills move along its curl ∇ × Ψ.
//! A curl has no divergence, so the flow never forms sinks and the pills swirl
//! instead of bunching up. Each pill depends only on its own state, so the
//! query runs in parallel.

// External crates
use glam::Vec3;
use pill_engine::{pill_hot, system, Query, Res, SystemError, Time};
use pill_rendering::TransformComponent;

// Current crate
use pills_rust::{CurlNoiseComponent, DemoState};

/// The lowest corner of the box the pill positions are clamped into.
const BOX_MINIMUM: Vec3 = Vec3::new(-120.0, -50.0, -120.0);
/// The highest corner of that box.
const BOX_MAXIMUM: Vec3 = Vec3::new(120.0, 140.0, 120.0);

/// The point the pills are pulled towards: the sum of the box's two corners
/// divided by twelve, which is (0, 7.5, 0). The demo this project recreates
/// uses that point, and the look is tuned to it.
const ATTRACTION_CENTER: Vec3 = Vec3::new(
    (BOX_MINIMUM.x + BOX_MAXIMUM.x) / 12.0,
    (BOX_MINIMUM.y + BOX_MAXIMUM.y) / 12.0,
    (BOX_MINIMUM.z + BOX_MAXIMUM.z) / 12.0,
);

/// Fastest a pill may move, in world units per second.
const MAXIMUM_SPEED: f32 = 20.0;

/// How fast the noise field scrolls, per second, along every axis.
const FIELD_SCROLL_SPEED: f32 = 0.4;

/// Offsets that decorrelate the three noise fields of the vector potential Ψ.
const FIELD_OFFSETS: [Vec3; 3] = [
    Vec3::ZERO,
    Vec3::new(31.416, 0.0, 0.0),
    Vec3::new(0.0, 67.254, 0.0),
];

/// A pseudo random value in `(-1, 1)` for a lattice point.
fn hash(point: Vec3) -> f32 {
    let sine = (point.x * 127.1 + point.y * 311.7 + point.z * 74.7).sin();
    (sine * 43_758.547).fract()
}

/// The eight corner hashes of one cell of the noise lattice.
///
/// Hashing is the expensive part of the noise, because every hash costs a
/// `sin`. The curl takes six samples only a small step apart (`curl_epsilon`),
/// so they almost always fall in the same cells. A cell keeps its hashes, so
/// every sample inside it reuses them instead of hashing the same eight
/// corners again.
#[derive(Clone, Copy)]
struct LatticeCell {
    /// The cell's lowest corner: the sample point rounded down.
    origin: Vec3,
    /// Corner hashes indexed by `x + 2y + 4z`, each axis 0 or 1.
    corners: [f32; 8],
}

impl LatticeCell {
    /// Hashes the eight corners of the cell whose lowest corner is `origin`.
    fn at(origin: Vec3) -> Self {
        let corner = |x: f32, y: f32, z: f32| hash(origin + Vec3::new(x, y, z));
        Self {
            origin,
            corners: [
                corner(0.0, 0.0, 0.0),
                corner(1.0, 0.0, 0.0),
                corner(0.0, 1.0, 0.0),
                corner(1.0, 1.0, 0.0),
                corner(0.0, 0.0, 1.0),
                corner(1.0, 0.0, 1.0),
                corner(0.0, 1.0, 1.0),
                corner(1.0, 1.0, 1.0),
            ],
        }
    }

    /// Smooth noise at `point`, which lies in this cell. The eight corner
    /// hashes are blended along each axis with a smoothstep (a curve that
    /// starts and ends flat, so the result has no sharp edges).
    fn noise(&self, point: Vec3) -> f32 {
        let offset = point - self.origin;
        let blend = offset * offset * (Vec3::splat(3.0) - 2.0 * offset);
        let lerp = |from: f32, to: f32, amount: f32| from + (to - from) * amount;
        let corners = &self.corners;

        let bottom_front = lerp(corners[0], corners[1], blend.x);
        let top_front = lerp(corners[2], corners[3], blend.x);
        let bottom_back = lerp(corners[4], corners[5], blend.x);
        let top_back = lerp(corners[6], corners[7], blend.x);

        let front = lerp(bottom_front, top_front, blend.y);
        let back = lerp(bottom_back, top_back, blend.y);
        lerp(front, back, blend.z)
    }
}

/// The vector potential Ψ at `point`.
///
/// Ψ is three noise fields, each sampled at the point plus its own offset so
/// that the three do not repeat each other. `cells` holds one cached cell per
/// field, taken at a nearby point: a field whose point falls inside its cached
/// cell reuses its hashes, and any other field hashes its own corners.
fn potential(point: Vec3, cells: &[LatticeCell; 3]) -> Vec3 {
    let field = |index: usize| {
        let field_point = point + FIELD_OFFSETS[index];
        let origin = field_point.floor();
        if origin == cells[index].origin {
            cells[index].noise(field_point)
        } else {
            LatticeCell::at(origin).noise(field_point)
        }
    };
    Vec3::new(field(0), field(1), field(2))
}

/// The curl of Ψ at `point`, with the field scrolled by `elapsed_seconds`.
///
/// Each derivative is a central difference: it samples the potential a small
/// `curl_epsilon` before and after the point and divides the change by twice
/// that step. For Ψ = (ψ1, ψ2, ψ3),
/// ∇ × Ψ = (∂ψ3/∂y - ∂ψ2/∂z, ∂ψ1/∂z - ∂ψ3/∂x, ∂ψ2/∂x - ∂ψ1/∂y).
fn curl_noise(point: Vec3, elapsed_seconds: f32, curl_epsilon: f32, curl_scale: f32) -> Vec3 {
    let animated = point * curl_scale + Vec3::splat(elapsed_seconds * FIELD_SCROLL_SPEED);
    // Cache the cell of each field around the centre point. The six samples
    // sit within `curl_epsilon` of the centre, so they almost always fall in
    // these cells, and the whole curl hashes 24 corners (three fields of
    // eight) instead of 144.
    let cells = FIELD_OFFSETS.map(|offset| LatticeCell::at((animated + offset).floor()));

    // The derivative of every component of Ψ along one axis.
    let derivative = |axis: Vec3| {
        (potential(animated + axis * curl_epsilon, &cells)
            - potential(animated - axis * curl_epsilon, &cells))
            / (2.0 * curl_epsilon)
    };
    let along_x = derivative(Vec3::X);
    let along_y = derivative(Vec3::Y);
    let along_z = derivative(Vec3::Z);

    Vec3::new(
        along_y.z - along_z.y,
        along_z.x - along_x.z,
        along_x.y - along_y.x,
    )
}

/// Advances one pill by one frame and returns its new position and velocity.
///
/// The curl and the pull towards the middle set the force; damping, the speed
/// cap and the box keep the result in bounds.
fn step_pill(
    position: Vec3,
    velocity: Vec3,
    curl_strength: f32,
    demo_state: &DemoState,
    elapsed_seconds: f32,
    delta_time: f32,
) -> (Vec3, Vec3) {
    let curl = curl_noise(
        position,
        elapsed_seconds,
        demo_state.curl_epsilon,
        demo_state.curl_scale,
    );
    let mut force = curl * curl_strength;

    // Pull towards the middle of the field with a strength that does not
    // depend on the distance. It is skipped within 0.1 world units of the
    // centre, where the pull has no clear direction.
    let to_center = ATTRACTION_CENTER - position;
    let distance_to_center = to_center.length();
    if distance_to_center > 0.1 {
        force += to_center * (demo_state.curl_attraction / distance_to_center);
    }

    // Keep a third of the previous velocity, then apply damping. The demo this
    // project recreates uses that fraction, and the look is tuned to it.
    let velocity = (velocity / 3.0 + force * delta_time) * demo_state.curl_damping;
    let velocity = velocity.clamp_length_max(MAXIMUM_SPEED);

    let position = (position + velocity * delta_time).clamp(BOX_MINIMUM, BOX_MAXIMUM);
    (position, velocity)
}

/// Moves every pill one frame through the field, in parallel.
#[system(name = "curl_noise", order = 520)]
pub(crate) fn curl_noise_system(
    time: Res<Time>,
    demo_state: Res<DemoState>,
    mut pills: Query<(&mut TransformComponent, &mut CurlNoiseComponent)>,
) -> Result<(), SystemError> {
    let (Some(time), Some(demo_state)) = (time.get(), demo_state.get()) else {
        return Ok(());
    };
    let delta_time = time.delta_seconds();
    let elapsed_seconds = time.elapsed_seconds();
    let demo_state: &DemoState = demo_state;

    pills
        .par_iter_mut()
        .label("pills_rust::curl_noise")
        .for_each(|(mut transform, mut pill)| {
            let (position, velocity) = step_pill(
                Vec3::from_array(transform.translation),
                Vec3::from_array(pill.velocity),
                pill.curl_strength,
                demo_state,
                elapsed_seconds,
                delta_time,
            );
            transform.translation = position.to_array();
            pill.velocity = velocity.to_array();
        });
    Ok(())
}
