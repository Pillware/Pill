use pill_engine::game::*;
use rayon::prelude::*;

use crate::game::{CurlNoiseComponent, DemoStateComponent};

// 3D Perlin-like noise function (simplified gradient noise)
fn hash(x: f32, y: f32, z: f32) -> f32 {
    let p = Vector3f::new(x, y, z);
    let mut h = (p.x * 127.1 + p.y * 311.7 + p.z * 74.7).sin();
    h = (h * 43758.5453123).fract();
    h
}

fn noise3d(p: Vector3f) -> f32 {
    let i = Vector3f::new(p.x.floor(), p.y.floor(), p.z.floor());
    let f = Vector3f::new(p.x.fract(), p.y.fract(), p.z.fract());

    // Smooth interpolation (smoothstep)
    let u = Vector3f::new(
        f.x * f.x * (3.0 - 2.0 * f.x),
        f.y * f.y * (3.0 - 2.0 * f.y),
        f.z * f.z * (3.0 - 2.0 * f.z),
    );

    // Sample corners of the cube
    let n000 = hash(i.x, i.y, i.z);
    let n100 = hash(i.x + 1.0, i.y, i.z);
    let n010 = hash(i.x, i.y + 1.0, i.z);
    let n110 = hash(i.x + 1.0, i.y + 1.0, i.z);
    let n001 = hash(i.x, i.y, i.z + 1.0);
    let n101 = hash(i.x + 1.0, i.y, i.z + 1.0);
    let n011 = hash(i.x, i.y + 1.0, i.z + 1.0);
    let n111 = hash(i.x + 1.0, i.y + 1.0, i.z + 1.0);

    // Trilinear interpolation
    let nx00 = n000 * (1.0 - u.x) + n100 * u.x;
    let nx10 = n010 * (1.0 - u.x) + n110 * u.x;
    let nx01 = n001 * (1.0 - u.x) + n101 * u.x;
    let nx11 = n011 * (1.0 - u.x) + n111 * u.x;

    let nxy0 = nx00 * (1.0 - u.y) + nx10 * u.y;
    let nxy1 = nx01 * (1.0 - u.y) + nx11 * u.y;

    nxy0 * (1.0 - u.z) + nxy1 * u.z
}

// Bridson & Kim Curl Noise Implementation (SIGGRAPH 2007)
// "Curl-Noise for Procedural Fluid Flow"
//
// The algorithm works by:
// 1. Create a 3D vector potential field Ψ(x,y,z) = (ψ1, ψ2, ψ3)
// 2. Compute the curl: v = ∇ × Ψ
// 3. Result is automatically divergence-free (∇ · v = 0)

// Create vector potential field Ψ from Perlin noise
// Each component uses different offsets to decorrelate them
fn potential_psi(p: Vector3f) -> Vector3f {
    Vector3f::new(
        noise3d(p),
        noise3d(p + Vector3f::new(31.416, 0.0, 0.0)),
        noise3d(p + Vector3f::new(0.0, 67.254, 0.0)),
    )
}

// Compute curl of potential field using finite differences
// ∇ × Ψ = (∂ψ3/∂y - ∂ψ2/∂z, ∂ψ1/∂z - ∂ψ3/∂x, ∂ψ2/∂x - ∂ψ1/∂y)
fn curl_noise(p: Vector3f, time: f32, epsilon: f32, scale: f32) -> Vector3f {
    // Animate the noise field
    let animated_p = Vector3f::new(
        p.x * scale + time * 0.4,
        p.y * scale + time * 0.4,
        p.z * scale + time * 0.4,
    );

    // Compute derivatives using central differences
    // For each axis, sample Ψ at ±epsilon
    let eps_x = Vector3f::new(epsilon, 0.0, 0.0);
    let eps_y = Vector3f::new(0.0, epsilon, 0.0);
    let eps_z = Vector3f::new(0.0, 0.0, epsilon);

    // Sample potential field at all required positions
    let psi_px = potential_psi(animated_p + eps_x);
    let psi_nx = potential_psi(animated_p - eps_x);
    let psi_py = potential_psi(animated_p + eps_y);
    let psi_ny = potential_psi(animated_p - eps_y);
    let psi_pz = potential_psi(animated_p + eps_z);
    let psi_nz = potential_psi(animated_p - eps_z);

    // Compute partial derivatives: ∂ψi/∂xj
    let d_psi1_dy = (psi_py.x - psi_ny.x) / (2.0 * epsilon);
    let d_psi1_dz = (psi_pz.x - psi_nz.x) / (2.0 * epsilon);

    let d_psi2_dx = (psi_px.y - psi_nx.y) / (2.0 * epsilon);
    let d_psi2_dz = (psi_pz.y - psi_nz.y) / (2.0 * epsilon);

    let d_psi3_dx = (psi_px.z - psi_nx.z) / (2.0 * epsilon);
    let d_psi3_dy = (psi_py.z - psi_ny.z) / (2.0 * epsilon);

    // Compute curl: v = ∇ × Ψ
    Vector3f::new(
        d_psi3_dy - d_psi2_dz, // v_x = ∂ψ3/∂y - ∂ψ2/∂z
        d_psi1_dz - d_psi3_dx, // v_y = ∂ψ1/∂z - ∂ψ3/∂x
        d_psi2_dx - d_psi1_dy, // v_z = ∂ψ2/∂x - ∂ψ1/∂y
    )
}

pub fn curl_noise_system(engine: &mut Engine) -> Result<()> {
    let demo_state_component = engine.get_global_component::<DemoStateComponent>()?;
    let curl_epsilon = demo_state_component.curl_epsilon;
    let curl_scale = demo_state_component.curl_scale;
    let curl_attraction = demo_state_component.curl_attraction;
    let curl_damping = demo_state_component.curl_damping;

    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let time = engine.get_global_component::<TimeComponent>()?.time;

    // Curl noise box bounds
    let box_min = Vector3f::new(-120.0, -50.0, -120.0);
    let box_max = Vector3f::new(120.0, 140.0, 120.0);
    let box_center = Vector3f::new(
        (box_min.x + box_max.x) / 12.0,
        (box_min.y + box_max.y) / 12.0,
        (box_min.z + box_max.z) / 12.0,
    );

    // Step 1: Collect all entity data that needs processing
    let mut entities_data: Vec<(Vector3f, f32, Vector3f)> = Vec::new();

    for (_, transform, curl_component) in
        engine.iterate_two_components_mut::<TransformComponent, CurlNoiseComponent>()?
    {
        let position = transform.position;

        // Only process entities within the box
        entities_data.push((
            position,
            curl_component.curl_strength,
            curl_component.velocity,
        ));
    }

    // Step 2: Process all entities in parallel using Rayon
    let results: Vec<(Vector3f, Vector3f)> = entities_data
        .par_iter()
        .map(|(position, curl_strength, velocity)| {
            // Calculate curl noise force
            let curl = curl_noise(*position, time, curl_epsilon, curl_scale);
            let mut force = curl * (*curl_strength);

            // Add attraction to center
            let to_center = box_center - *position;
            let distance_to_center =
                (to_center.x * to_center.x + to_center.y * to_center.y + to_center.z * to_center.z)
                    .sqrt();

            if distance_to_center > 0.1 {
                let attraction_strength = curl_attraction;
                let center_force = to_center * (attraction_strength / distance_to_center);
                force = force + center_force;
            }

            // Apply force to velocity
            let mut new_velocity = *velocity + force * delta_time;

            // Apply damping
            new_velocity = new_velocity * curl_damping;

            // Clamp velocity
            let max_speed = 40.0;
            let speed = (new_velocity.x * new_velocity.x
                + new_velocity.y * new_velocity.y
                + new_velocity.z * new_velocity.z)
                .sqrt();

            if speed > max_speed {
                let scale = max_speed / speed;
                new_velocity = new_velocity * scale;
            }

            // Calculate new position
            let mut new_position = *position + new_velocity * delta_time;

            // Hard clamp
            new_position.x = new_position.x.clamp(box_min.x, box_max.x);
            new_position.y = new_position.y.clamp(box_min.y, box_max.y);
            new_position.z = new_position.z.clamp(box_min.z, box_max.z);

            (new_position, new_velocity)
        })
        .collect();

    // Step 3: Apply results back to entities
    let mut result_idx = 0;
    for (_, transform, curl_component) in
        engine.iterate_two_components_mut::<TransformComponent, CurlNoiseComponent>()?
    {
        let position = transform.position;

        // Only apply to entities that were processed
        if result_idx < results.len() {
            let (new_position, new_velocity) = results[result_idx];
            transform.set_position(new_position);
            curl_component.velocity = new_velocity;
            result_idx += 1;
        }
    }

    Ok(())
}
