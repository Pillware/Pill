//! The demo's components, its tuning resource, the scene it builds and the
//! keyboard controls that tune the field.
//!
//! # Responsibilities
//!
//! - Define [`CurlNoiseComponent`], [`CameraMovementComponent`] and the
//!   [`DemoState`] resource, and register them ([`register`]).
//! - Build the scene once ([`create_scene`]): the camera and [`PILL_COUNT`]
//!   pills scattered through the field.
//! - Tune the field from the keyboard ([`demo_control_system`]).
//!
//! # Design
//!
//! The scene is built only into a world that does not hold it yet. The `init`
//! that a reload runs then finds the live scene instead of making a second
//! one.
//!
//! The scatter is a pure function of a pill's index ([`scatter_between`]),
//! not a generator with state. A thread-local generator owned by the project
//! DLL would register its destructor there. A hot reload unmaps the DLL while
//! the thread lives on, so the destructor would point at code that is no
//! longer loaded.

// Standard library
use std::f32::consts::PI;

// External crates
use pill_engine::{
    component, pill_hot, system, tracing, Handle, Input, KeyCode, Query, Res, ResMut, Resource,
    SystemError, World,
};
use pill_master_renderer_data::{
    CameraComponent, Material, Mesh, MeshRendererComponent, TransformComponent,
};
use pill_project_ui::GameUi;
use serde::{Deserialize, Serialize};

/// How many pills drift through the field.
pub const PILL_COUNT: usize = 200_000;

/// Uniform scale of every pill.
const PILL_SCALE: f32 = 0.35;

/// Where the camera starts: eye height, ten units out along -Z.
const CAMERA_START_TRANSLATION: [f32; 3] = [0.0, 1.6, -10.0];
/// Yaw that turns the camera around to face the origin. A camera looks down
/// its local -Z axis.
const CAMERA_START_YAW_DEGREES: f32 = 180.0;

/// How far each tuning key moves its value per frame.
const CURL_SCALE_STEP: f32 = 0.0005;
const CURL_ATTRACTION_STEP: f32 = 5.0;
const CURL_DAMPING_STEP: f32 = 0.01;
const CURL_EPSILON_STEP: f32 = 0.001;

/// One pill's motion through the curl noise field.
#[component(shared = "pills_rust::CurlNoiseComponent", persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CurlNoiseComponent {
    /// Current velocity, in world units per second.
    pub velocity: [f32; 3],
    /// How strongly the field pushes this pill.
    pub curl_strength: f32,
    /// Per-pill noise scale, set at spawn; the field uses the shared scale
    /// instead.
    pub noise_scale: f32,
}

/// Settings and smoothed state of the free flying camera.
#[component(shared = "pills_rust::CameraMovementComponent", persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct CameraMovementComponent {
    /// Flying speed, in world units per second.
    pub move_speed: f32,
    /// Speed factor while Shift is held.
    pub sprint_multiplier: f32,
    /// How quickly the velocity follows the keys.
    pub lerp_speed: f32,
    /// Degrees turned per pixel of mouse movement.
    pub mouse_sensitivity: f32,
    /// How quickly the view follows the mouse.
    pub rotation_lerp_speed: f32,
    /// Velocity this frame, in world units per second.
    pub current_velocity: [f32; 3],
    /// Velocity the keys ask for.
    pub target_velocity: [f32; 3],
    /// Current view as pitch and yaw in degrees. The third value is unused.
    pub current_rotation: [f32; 3],
    /// The view the mouse asks for, as pitch and yaw in degrees.
    pub target_rotation: [f32; 3],
}

/// The curl noise field's tuning, adjustable while the demo runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DemoState {
    /// Spatial frequency of the noise.
    pub curl_scale: f32,
    /// Finite difference step used to take the curl.
    pub curl_epsilon: f32,
    /// Strength of the pull towards the field's attraction point.
    pub curl_attraction: f32,
    /// Velocity multiplier applied every frame.
    pub curl_damping: f32,
}

impl Default for DemoState {
    /// The tuning the demo starts with.
    fn default() -> Self {
        Self {
            curl_scale: 0.1005,
            curl_epsilon: 0.0038,
            curl_attraction: 155.0,
            curl_damping: 2.91,
        }
    }
}

impl Resource for DemoState {
    fn shared_name() -> Option<&'static str> {
        Some("pills_rust::DemoState")
    }
}

/// Registers the demo's components and its tuning resource, and inserts the
/// resource unless a previous generation left one behind.
pub(crate) fn register(world: &mut World) {
    __pill_register_CurlNoiseComponent(world);
    __pill_register_CameraMovementComponent(world);
    world.register_persistable_resource::<DemoState>();
    if !world.has_resource::<DemoState>() {
        world.insert_resource(DemoState::default());
    }
}

/// Builds the scene into a world that does not hold it yet.
///
/// # Errors
///
/// Returns the engine's entity creation error as text.
pub(crate) fn create_scene(
    world: &mut World,
    mesh: Handle<Mesh>,
    material: Handle<Material>,
) -> Result<(), String> {
    if Query::<&CameraMovementComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        spawn_camera(world)?;
    }
    if Query::<&CurlNoiseComponent>::new(world)
        .iter_mut()
        .next()
        .is_none()
    {
        spawn_pills(world, mesh, material, PILL_COUNT)?;
    }
    Ok(())
}

/// Spawns the free flying camera at the scene's starting spot.
///
/// # Errors
///
/// Returns the engine's entity creation error as text.
fn spawn_camera(world: &mut World) -> Result<(), String> {
    let start_rotation_degrees = [0.0, CAMERA_START_YAW_DEGREES, 0.0];
    let start_rotation_quaternion = glam::Quat::from_euler(
        glam::EulerRot::YXZ,
        CAMERA_START_YAW_DEGREES.to_radians(),
        0.0,
        0.0,
    );
    world
        .create_entity()
        .with(TransformComponent {
            translation: CAMERA_START_TRANSLATION,
            rotation: start_rotation_quaternion.to_array(),
            ..Default::default()
        })
        .with(CameraComponent {
            vertical_fov: 75.0,
            ..Default::default()
        })
        .with(CameraMovementComponent {
            move_speed: 25.0,
            sprint_multiplier: 2.0,
            lerp_speed: 8.0,
            mouse_sensitivity: 0.1,
            rotation_lerp_speed: 25.0,
            current_rotation: start_rotation_degrees,
            target_rotation: start_rotation_degrees,
            ..Default::default()
        })
        .build()
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Scatters `pill_count` pills through the field, each turned and pushed at
/// random.
///
/// # Errors
///
/// Returns the engine's entity creation error as text.
fn spawn_pills(
    world: &mut World,
    mesh: Handle<Mesh>,
    material: Handle<Material>,
    pill_count: usize,
) -> Result<(), String> {
    let mesh_renderer = MeshRendererComponent::builder()
        .mesh(&mesh)
        .material(&material)
        .build();
    for index in 0..pill_count {
        // Each pill takes its eight random values from eight seeds of its own,
        // so the scatter depends only on the index. Building the scene again
        // gives the same layout.
        let seed = index as u64 * 8;
        let rotation = glam::Quat::from_euler(
            glam::EulerRot::XYZ,
            scatter_between(seed, 0.0, 2.0 * PI),
            scatter_between(seed + 1, 0.0, 2.0 * PI),
            scatter_between(seed + 2, 0.0, 2.0 * PI),
        );
        world
            .create_entity()
            .with(CurlNoiseComponent {
                velocity: [0.0; 3],
                curl_strength: scatter_between(seed + 3, 70.0, 120.0),
                noise_scale: scatter_between(seed + 4, 0.1, 0.2),
            })
            .with(TransformComponent {
                translation: [
                    scatter_between(seed + 5, -20.0, 20.0),
                    scatter_between(seed + 6, 0.0, 40.0),
                    scatter_between(seed + 7, -20.0, 20.0),
                ],
                rotation: rotation.to_array(),
                scale: [PILL_SCALE; 3],
            })
            .with(mesh_renderer)
            .build()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Returns a pseudo random value in `[0, 1)` for a seed.
///
/// This is the mixing step of SplitMix64. It is a bijection on 64 bits, so
/// two seeds that differ by one give unrelated values.
fn scatter_unit_value(seed: u64) -> f32 {
    let mut mixed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    mixed ^= mixed >> 31;
    // The top 24 bits fill an f32 mantissa exactly.
    (mixed >> 40) as f32 / (1u64 << 24) as f32
}

/// Returns a pseudo random value in `[minimum, maximum)` for a seed.
fn scatter_between(seed: u64, minimum: f32, maximum: f32) -> f32 {
    minimum + scatter_unit_value(seed) * (maximum - minimum)
}

/// Tunes the curl noise field while its keys are held, and logs the new values
/// on every frame that changed them.
///
/// The overlay gets the keyboard first. While it wants it - a text field has
/// focus or a widget listens for keys - the tuning keys belong to the UI.
/// Typing in the overlay then does not also retune the field.
#[system(name = "demo_control", order = 530)]
pub(crate) fn demo_control_system(
    input: Res<Input>,
    game_ui: Res<GameUi>,
    mut demo_state: ResMut<DemoState>,
) -> Result<(), SystemError> {
    if game_ui.get().is_some_and(GameUi::wants_keyboard) {
        return Ok(());
    }
    let Some(input) = input.get() else {
        return Ok(());
    };
    let Some(mut demo_state) = demo_state.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("DemoState"),
        });
    };

    // Each value has two keys, one to raise it and one to lower it (O/P,
    // L/K, M/N, V/B).
    let mut tuned = false;
    let mut tune = |key: KeyCode, step: f32, tuning_value: &mut f32| {
        if input.key_held(key) {
            *tuning_value += step;
            tuned = true;
        }
    };
    tune(KeyCode::KeyO, CURL_SCALE_STEP, &mut demo_state.curl_scale);
    tune(KeyCode::KeyP, -CURL_SCALE_STEP, &mut demo_state.curl_scale);
    tune(
        KeyCode::KeyL,
        CURL_ATTRACTION_STEP,
        &mut demo_state.curl_attraction,
    );
    tune(
        KeyCode::KeyK,
        -CURL_ATTRACTION_STEP,
        &mut demo_state.curl_attraction,
    );
    tune(
        KeyCode::KeyM,
        CURL_DAMPING_STEP,
        &mut demo_state.curl_damping,
    );
    tune(
        KeyCode::KeyN,
        -CURL_DAMPING_STEP,
        &mut demo_state.curl_damping,
    );
    tune(
        KeyCode::KeyV,
        CURL_EPSILON_STEP,
        &mut demo_state.curl_epsilon,
    );
    tune(
        KeyCode::KeyB,
        -CURL_EPSILON_STEP,
        &mut demo_state.curl_epsilon,
    );

    if tuned {
        tracing::info!(
            "[pills_rust] curl scale: {:.4}, attraction: {:.2}, damping: {:.4}, epsilon: {:.4}",
            demo_state.curl_scale,
            demo_state.curl_attraction,
            demo_state.curl_damping,
            demo_state.curl_epsilon
        );
    }
    Ok(())
}
