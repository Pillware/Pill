//! Procedural mesh builders (from the demo) — pure geometry, no engine imports.
//! Output format: RMSH binary (b"RMSH" + 4 padding + vtx_count + idx_count + vertex data + index data).
//! MeshVertex layout: pos[f32;3] + uv[f32;2] + normal[f32;3] + tangent[f32;3] + bitangent[f32;3] = 56 bytes.

fn write_rmsh(verts: &[[f32; 14]], indices: &[u32]) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"RMSH");
    data.extend_from_slice(&[0u8; 4]);
    data.extend_from_slice(&(verts.len() as u32).to_le_bytes());
    data.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for v in verts {
        for &f in v {
            data.extend_from_slice(&f.to_le_bytes());
        }
    }
    for &idx in indices {
        data.extend_from_slice(&idx.to_le_bytes());
    }
    data
}

pub(crate) fn make_sphere_mesh_bytes(stacks: u32, slices: u32) -> Vec<u8> {
    use std::f32::consts::PI;
    let mut verts: Vec<[f32; 14]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for i in 0..=stacks {
        let phi = PI * i as f32 / stacks as f32;
        let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
        for j in 0..=slices {
            let theta = 2.0 * PI * j as f32 / slices as f32;
            let (sin_theta, cos_theta) = (theta.sin(), theta.cos());
            let (nx, ny, nz) = (sin_phi * cos_theta, cos_phi, sin_phi * sin_theta);
            // Tangent along U (dtheta); degenerate at poles — fall back to X.
            let (tx, ty, tz) = if sin_phi > 1e-4 {
                (-sin_theta, 0.0, cos_theta)
            } else {
                (1.0, 0.0, 0.0)
            };
            // Bitangent along V (dphi).
            let (bx, by, bz) = (cos_phi * cos_theta, -sin_phi, cos_phi * sin_theta);
            let u = j as f32 / slices as f32;
            let v = 1.0 - i as f32 / stacks as f32; // engine flips V: store pre-flipped
            verts.push([nx, ny, nz, u, v, nx, ny, nz, tx, ty, tz, bx, by, bz]);
        }
    }

    let stride = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let (a, b, c, d) = (
                i * stride + j,
                i * stride + j + 1,
                (i + 1) * stride + j + 1,
                (i + 1) * stride + j,
            );
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }

    write_rmsh(&verts, &indices)
}

/// Pill mesh (RMSH). Long axis = Y; radius=0.5.
/// `height` = total length (cylinder section = height − 2*radius).
/// `extrude` = radial pushout of the blue half. Two seam rings at y=0 (pill center):
///   ring A at r=radius (red side), ring B at r=radius+extrude (blue side, extrusion starts here).
///   Thin disc face between A and B = the shell seam lip. Whole blue half at radius+extrude.
pub(crate) fn make_pill_mesh_bytes(
    cap_stacks: u32,
    slices: u32,
    height: f32,
    extrude: f32,
) -> Vec<u8> {
    use std::f32::consts::PI;
    let radius: f32 = 0.5;
    let cyl = (height - 2.0 * radius).max(0.0);
    let half_cyl = cyl * 0.5;
    let arc_total = radius * PI + cyl;
    let r_blue = radius + extrude;

    let mut verts: Vec<[f32; 14]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let mut push_ring = |r: f32, phi: f32, y_off: f32, arc: f32| {
        let (sp, cp) = (phi.sin(), phi.cos());
        let v = 1.0 - arc / arc_total;
        for j in 0..=slices {
            let theta = 2.0 * PI * j as f32 / slices as f32;
            let (st, ct) = (theta.sin(), theta.cos());
            let (px, py, pz) = (r * sp * ct, radius * cp + y_off, r * sp * st);
            let (nx, ny, nz) = (sp * ct, cp, sp * st);
            let (tx, ty, tz) = if sp > 1e-4 {
                (-st, 0.0, ct)
            } else {
                (1.0, 0.0, 0.0)
            };
            let (bx, by, bz) = (cp * ct, -sp, cp * st);
            verts.push([
                px,
                py,
                pz,
                j as f32 / slices as f32,
                v,
                nx,
                ny,
                nz,
                tx,
                ty,
                tz,
                bx,
                by,
                bz,
            ]);
        }
    };

    let seam_arc = radius * PI * 0.5 + cyl * 0.5; // arc at y=0 → V=0.5
    let blue_eq_arc = radius * PI * 0.5 + cyl;

    // Red cap: phi π→π/2, y=-half_cyl, r=radius
    for i in 0..=cap_stacks {
        let phi = PI - (i as f32 / cap_stacks as f32) * (PI * 0.5);
        push_ring(radius, phi, -half_cyl, radius * (PI - phi));
    }
    // Seam ring A at y=0, r=radius   — red side, no extrusion yet
    push_ring(radius, PI * 0.5, 0.0, seam_arc);
    // Seam ring B at y=0, r=r_blue   — blue side, extrusion starts here; disc A→B = thin shell
    push_ring(r_blue, PI * 0.5, 0.0, seam_arc);
    // Blue cap: phi π/2→0, y=+half_cyl, r=r_blue — whole blue half is wider
    for i in 0..=cap_stacks {
        let phi = PI * 0.5 * (1.0 - i as f32 / cap_stacks as f32);
        push_ring(
            r_blue,
            phi,
            half_cyl,
            blue_eq_arc + radius * (PI * 0.5 - phi),
        );
    }

    let stride = slices + 1;
    let total_rings = (cap_stacks + 1) * 2 + 2; // red cap + 2 seam rings + blue cap
    for ring in 0..total_rings - 1 {
        for j in 0..slices {
            let (a, b) = (ring * stride + j, ring * stride + j + 1);
            let (c, d) = ((ring + 1) * stride + j + 1, (ring + 1) * stride + j);
            indices.extend_from_slice(&[a, c, b, a, d, c]);
        }
    }

    write_rmsh(&verts, &indices)
}
