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

// Calculate curl of a 3D noise field
fn curl_noise(p: Vector3f, time: f32) -> Vector3f {
    let epsilon = 0.1;
    let scale = 0.05;

    // Animate the noise field
    let animated_p = Vector3f::new(
        p.x * scale + time * 1.5,
        p.y * scale + time * 1.5,
        p.z * scale + time * 1.5,
    );

    // Sample noise at offset positions to compute derivatives
    let dx = Vector3f::new(epsilon, 0.0, 0.0);
    let dy = Vector3f::new(0.0, epsilon, 0.0);
    let dz = Vector3f::new(0.0, 0.0, epsilon);

    // Potential field samples
    let px_pos = noise3d(animated_p + dx);
    let px_neg = noise3d(animated_p - dx);
    let py_pos = noise3d(animated_p + dy);
    let py_neg = noise3d(animated_p - dy);
    let pz_pos = noise3d(animated_p + dz);
    let pz_neg = noise3d(animated_p - dz);

    // Second potential field (offset in w-dimension via different scale)
    let offset = Vector3f::new(100.0, 100.0, 100.0);
    let qx_pos = noise3d(animated_p + dx + offset);
    let qx_neg = noise3d(animated_p - dx + offset);
    let qy_pos = noise3d(animated_p + dy + offset);
    let qy_neg = noise3d(animated_p - dy + offset);
    let qz_pos = noise3d(animated_p + dz + offset);
    let qz_neg = noise3d(animated_p - dz + offset);

    // Compute gradients
    let dpdy = (py_pos - py_neg) / (2.0 * epsilon);
    let dpdz = (pz_pos - pz_neg) / (2.0 * epsilon);
    let dqdx = (qx_pos - qx_neg) / (2.0 * epsilon);
    let dqdz = (qz_pos - qz_neg) / (2.0 * epsilon);

    // Additional gradient for z component
    let dpdx = (px_pos - px_neg) / (2.0 * epsilon);
    let dqdy = (qy_pos - qy_neg) / (2.0 * epsilon);

    // Curl = ∇ × (P, Q, 0)
    Vector3f::new(dpdy - dqdz, dqdz - dpdx, dqdx - dqdy)
}

pub fn curl_noise_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let time = engine.get_global_component::<TimeComponent>()?.time;

    // Curl noise box bounds
    let box_min = Vector3f::new(-20.0, 0.0, -20.0);
    let box_max = Vector3f::new(20.0, 40.0, 20.0);

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
            let force = curl * curl_component.curl_strength;

            // Apply force to velocity (integrate acceleration)
            curl_component.velocity = curl_component.velocity + force * delta_time;

            // Apply stronger damping for stability
            curl_component.velocity = curl_component.velocity * 0.98;

            // Clamp velocity to prevent extreme speeds
            let max_speed = 100.0;
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
