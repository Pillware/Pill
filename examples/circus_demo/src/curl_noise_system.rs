use pill_engine::game::*;

use crate::game::CurlNoiseComponent;

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
fn curl_noise(p: Vector3f, time: f32) -> Vector3f {
    let epsilon = 0.002; // Small step for numerical derivatives
    let scale = 0.03;

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
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let time = engine.get_global_component::<TimeComponent>()?.time;

    // Curl noise box bounds
    let box_min = Vector3f::new(-120.0, 0.0, -120.0);
    let box_max = Vector3f::new(120.0, 140.0, 120.0);

    for (_, transform, curl_component) in
        engine.iterate_two_components_mut::<TransformComponent, CurlNoiseComponent>()?
    {
        let position = transform.position;

        // Check if position is within the curl noise box
        if position.x >= box_min.x
            && position.x <= box_max.x
            && position.y >= box_min.y
            && position.y <= box_max.y
            && position.z >= box_min.z
            && position.z <= box_max.z
        {
            // Calculate curl noise force
            let curl = curl_noise(position, time);
            let mut force = curl * curl_component.curl_strength;

            // Add attraction to center of the box
            let box_center = Vector3f::new(
                (box_min.x + box_max.x) / 12.0,
                (box_min.y + box_max.y) / 12.0,
                (box_min.z + box_max.z) / 12.0,
            );
            let to_center = box_center - position;
            let distance_to_center =
                (to_center.x * to_center.x + to_center.y * to_center.y + to_center.z * to_center.z)
                    .sqrt();

            // Apply center attraction force (stronger when farther from center)
            if distance_to_center > 0.1 {
                let attraction_strength = 50.0; // Adjust for stronger/weaker attraction
                let center_force = to_center * (attraction_strength / distance_to_center);
                force = force + center_force;
            }

            // Apply force to velocity (integrate acceleration)
            curl_component.velocity = curl_component.velocity + force * delta_time;

            // Apply lighter damping for more flowy motion
            curl_component.velocity = curl_component.velocity * 0.99;

            // Clamp velocity to prevent extreme speeds
            let max_speed = 200.0;
            let speed = (curl_component.velocity.x * curl_component.velocity.x
                + curl_component.velocity.y * curl_component.velocity.y
                + curl_component.velocity.z * curl_component.velocity.z)
                .sqrt();

            if speed > max_speed {
                let scale = max_speed / speed;
                curl_component.velocity = curl_component.velocity * scale;
            }

            // Update position
            let new_position = position + curl_component.velocity * delta_time;

            // Soft boundary constraints - push objects back toward center if they go too far
            let mut bounded_position = new_position;
            let boundary_softness = 5.0;

            if bounded_position.x < box_min.x + boundary_softness {
                let push = (box_min.x + boundary_softness - bounded_position.x) / boundary_softness;
                curl_component.velocity.x += push * 10.0 * delta_time;
            } else if bounded_position.x > box_max.x - boundary_softness {
                let push =
                    (bounded_position.x - (box_max.x - boundary_softness)) / boundary_softness;
                curl_component.velocity.x -= push * 10.0 * delta_time;
            }

            if bounded_position.y < box_min.y + boundary_softness {
                let push = (box_min.y + boundary_softness - bounded_position.y) / boundary_softness;
                curl_component.velocity.y += push * 10.0 * delta_time;
            } else if bounded_position.y > box_max.y - boundary_softness {
                let push =
                    (bounded_position.y - (box_max.y - boundary_softness)) / boundary_softness;
                curl_component.velocity.y -= push * 10.0 * delta_time;
            }

            if bounded_position.z < box_min.z + boundary_softness {
                let push = (box_min.z + boundary_softness - bounded_position.z) / boundary_softness;
                curl_component.velocity.z += push * 10.0 * delta_time;
            } else if bounded_position.z > box_max.z - boundary_softness {
                let push =
                    (bounded_position.z - (box_max.z - boundary_softness)) / boundary_softness;
                curl_component.velocity.z -= push * 10.0 * delta_time;
            }

            // Hard clamp to ensure objects never leave the box
            bounded_position.x = bounded_position.x.clamp(box_min.x, box_max.x);
            bounded_position.y = bounded_position.y.clamp(box_min.y, box_max.y);
            bounded_position.z = bounded_position.z.clamp(box_min.z, box_max.z);

            transform.set_position(bounded_position);
        }
    }

    Ok(())
}
