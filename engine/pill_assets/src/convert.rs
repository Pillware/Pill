//! Shared mesh/texture byte-format conversion — the single source of the cooked formats.
//!
//! Pure (no filesystem, no glTF/image deps): the vertex assembly, tangent generation, matrix math
//! and `RMSH`/`RTEX` encoders used by both the build-time cooker (`GlbToCookedMesh`) and the
//! engine's runtime glTF loader.

use anyhow::{bail, Result};
use bytemuck::{Pod, Zeroable};

use crate::util::vec_math::{add3, cross3, normalize3, scale3, sub2, sub3};

/// Cooked vertex layout. Must stay bit-for-bit identical to `pill_engine::resources::MeshVertex`
/// (the engine casts `RMSH` bytes straight to that type).
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct CookedVertex {
    pub position: [f32; 3],
    pub texture_coordinates: [f32; 2],
    pub normal: [f32; 3],
    pub tangent: [f32; 3],
    pub bitangent: [f32; 3],
}

/// Row-major 4x4.
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// One triangle-list primitive's attributes as read from glTF. Optional attributes fall back to
/// sequential indices, zero UVs and smooth normals from faces; empty `tangents` are derived from
/// positions + UVs.
pub struct PrimitiveAttributes {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub texture_coordinates: Option<Vec<[f32; 2]>>,
    pub tangents: Vec<[f32; 4]>,
    pub indices: Option<Vec<u32>>,
}

/// Builds cooked vertices + indices for one primitive, baking `world_transform` into positions,
/// normals and tangents.
pub fn build_primitive(
    attributes: PrimitiveAttributes,
    world_transform: Mat4,
) -> Result<(Vec<CookedVertex>, Vec<u32>)> {
    let PrimitiveAttributes {
        positions,
        normals,
        texture_coordinates,
        tangents,
        indices,
    } = attributes;

    let vertex_count = positions.len();
    let counts = [
        ("NORMAL", normals.as_ref().map_or(vertex_count, Vec::len)),
        (
            "TEXCOORD_0",
            texture_coordinates.as_ref().map_or(vertex_count, Vec::len),
        ),
        (
            "TANGENT",
            if tangents.is_empty() {
                vertex_count
            } else {
                tangents.len()
            },
        ),
    ];
    for (name, count) in counts {
        if count != vertex_count {
            bail!("{name} has {count} entries, POSITION has {vertex_count}");
        }
    }
    let mut indices = indices.unwrap_or_else(|| (0..vertex_count as u32).collect());
    if let Some(&index) = indices
        .iter()
        .find(|&&index| index as usize >= vertex_count)
    {
        bail!("index {index} out of range for {vertex_count} vertices");
    }
    indices.truncate(indices.len() / 3 * 3);
    // Face normals from the original winding, before the mirror fix-up below
    let normals = normals.unwrap_or_else(|| face_normals(&positions, &indices));
    // A mirrored node (negative scale) flips winding and cross(N, T): swap the winding back for
    // back-face culling and correct the bitangent sign with `handedness`
    let mirrored = determinant3(world_transform) < 0.0;
    if mirrored {
        for triangle in indices.chunks_mut(3) {
            triangle.swap(1, 2);
        }
    }
    let handedness = if mirrored { -1.0 } else { 1.0 };
    let texture_coordinates =
        texture_coordinates.unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

    let mut vertices: Vec<CookedVertex> = (0..positions.len())
        .map(|vertex_index| {
            let normal = transform_normal(world_transform, normals[vertex_index]);
            let (tangent_x, tangent_y, tangent_z, sign) = tangents
                .get(vertex_index)
                .map(|tangent| (tangent[0], tangent[1], tangent[2], tangent[3]))
                .unwrap_or((1.0, 0.0, 0.0, 1.0));
            let sign = sign * handedness;
            let tangent_world =
                transform_normal(world_transform, [tangent_x, tangent_y, tangent_z]);
            let bitangent_x = (normal[1] * tangent_world[2] - normal[2] * tangent_world[1]) * sign;
            let bitangent_y = (normal[2] * tangent_world[0] - normal[0] * tangent_world[2]) * sign;
            let bitangent_z = (normal[0] * tangent_world[1] - normal[1] * tangent_world[0]) * sign;
            CookedVertex {
                position: transform_point(world_transform, positions[vertex_index]),
                texture_coordinates: texture_coordinates[vertex_index],
                normal,
                tangent: tangent_world,
                bitangent: [bitangent_x, bitangent_y, bitangent_z],
            }
        })
        .collect();

    if tangents.is_empty() {
        compute_tangents(&mut vertices, &indices);
    }
    Ok((vertices, indices))
}

/// RTEX v1 from owned RGBA8 texels; the header is prepended in the same allocation.
pub fn rtex_from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Vec<u8> {
    let mut header = [0u8; 16];
    header[0..4].copy_from_slice(b"RTEX");
    header[4..8].copy_from_slice(&1u32.to_le_bytes());
    header[8..12].copy_from_slice(&width.to_le_bytes());
    header[12..16].copy_from_slice(&height.to_le_bytes());
    // Exact capacity, so a later `into_boxed_slice` doesn't reallocate
    rgba.reserve_exact(16);
    rgba.splice(0..0, header);
    rgba
}

/// RMSH v1: `b"RMSH"` + u32 version=1 + u32 vertex_count + u32 index_count + vertices + indices.
pub fn rmsh_bytes(vertices: &[CookedVertex], indices: &[u32]) -> Vec<u8> {
    let vertex_bytes: &[u8] = bytemuck::cast_slice(vertices);
    let index_bytes: &[u8] = bytemuck::cast_slice(indices);
    let mut output = Vec::with_capacity(16 + vertex_bytes.len() + index_bytes.len());
    output.extend_from_slice(b"RMSH");
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&(vertices.len() as u32).to_le_bytes());
    output.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    output.extend_from_slice(vertex_bytes);
    output.extend_from_slice(index_bytes);
    output
}

/// RTEX v1: RGBA8 texels, sRGB-encoded color (linear data such as LUTs uses v2).
pub fn rtex_bytes(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(16 + rgba.len());
    output.extend_from_slice(b"RTEX");
    output.extend_from_slice(&1u32.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(rgba);
    output
}

/// RTEX v2: Rgba32Float (f32 LE per channel), linear HDR.
pub fn rtex_hdr_bytes(width: u32, height: u32, rgba_f32: &[f32]) -> Vec<u8> {
    let mut output = Vec::with_capacity(16 + rgba_f32.len() * 4);
    output.extend_from_slice(b"RTEX");
    output.extend_from_slice(&2u32.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    for &value in rgba_f32 {
        output.extend_from_slice(&value.to_le_bytes());
    }
    output
}

/// RTEX v4: Rgba32Float mip chain (HDR specular prefilter).
/// Header: RTEX | 4 | width | height | mip_count, then each mip's f32 texels.
pub fn rtex_hdr_mip_bytes(width: u32, height: u32, mips: &[Vec<f32>]) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(b"RTEX");
    output.extend_from_slice(&4u32.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(&(mips.len() as u32).to_le_bytes());
    for mip in mips {
        for &value in mip {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    output
}

/// Smooth per-vertex normals from area-weighted face normals, for primitives without NORMAL.
fn face_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0f32; 3]; positions.len()];
    for triangle in indices.chunks(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|index| index as usize);
        let face = cross3(
            sub3(positions[b], positions[a]),
            sub3(positions[c], positions[a]),
        );
        for index in [a, b, c] {
            normals[index] = add3(normals[index], face);
        }
    }
    normals.into_iter().map(normalize3).collect()
}

fn compute_tangents(vertices: &mut [CookedVertex], indices: &[u32]) {
    // Accumulate from zero; vertices not in any valid triangle keep their default tangent
    let mut tangent_sums = vec![[0.0f32; 3]; vertices.len()];
    let mut bitangent_sums = vec![[0.0f32; 3]; vertices.len()];
    let mut triangle_counts = vec![0usize; vertices.len()];

    for triangle in indices.chunks(3) {
        let (index_0, index_1, index_2) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        let position_0 = vertices[index_0].position;
        let position_1 = vertices[index_1].position;
        let position_2 = vertices[index_2].position;
        let texture_coordinates_0 = vertices[index_0].texture_coordinates;
        let texture_coordinates_1 = vertices[index_1].texture_coordinates;
        let texture_coordinates_2 = vertices[index_2].texture_coordinates;

        let delta_position_1 = sub3(position_1, position_0);
        let delta_position_2 = sub3(position_2, position_0);
        let delta_texture_coordinates_1 = sub2(texture_coordinates_1, texture_coordinates_0);
        let delta_texture_coordinates_2 = sub2(texture_coordinates_2, texture_coordinates_0);

        let determinant = delta_texture_coordinates_1[0] * delta_texture_coordinates_2[1]
            - delta_texture_coordinates_1[1] * delta_texture_coordinates_2[0];
        if determinant.abs() < 1e-8 {
            continue;
        }
        let inverse_determinant = 1.0 / determinant;
        let tangent = scale3(
            sub3(
                scale3(delta_position_1, delta_texture_coordinates_2[1]),
                scale3(delta_position_2, delta_texture_coordinates_1[1]),
            ),
            inverse_determinant,
        );
        let bitangent = scale3(
            sub3(
                scale3(delta_position_2, delta_texture_coordinates_1[0]),
                scale3(delta_position_1, delta_texture_coordinates_2[0]),
            ),
            inverse_determinant,
        );

        for &index in &[index_0, index_1, index_2] {
            tangent_sums[index] = add3(tangent_sums[index], tangent);
            bitangent_sums[index] = add3(bitangent_sums[index], bitangent);
            triangle_counts[index] += 1;
        }
    }

    for (index, &count) in triangle_counts.iter().enumerate() {
        if count > 0 {
            let inverse_count = 1.0 / count as f32;
            vertices[index].tangent = normalize3(scale3(tangent_sums[index], inverse_count));
            // glTF's V axis points down the image; negate so the bitangent matches cross(N, T)·w
            vertices[index].bitangent = normalize3(scale3(bitangent_sums[index], -inverse_count));
        }
    }
}

// --- Matrix math ---

pub fn mat4_mul(left: Mat4, right: Mat4) -> Mat4 {
    let mut result = [[0.0f32; 4]; 4];
    for row in 0..4 {
        for column in 0..4 {
            result[row][column] = (0..4)
                .map(|inner| left[row][inner] * right[inner][column])
                .sum();
        }
    }
    result
}

/// Builds a row-major Mat4 from glTF's column-major 4×4.
pub fn mat4_from_cols(matrix: [[f32; 4]; 4]) -> Mat4 {
    [
        [matrix[0][0], matrix[1][0], matrix[2][0], matrix[3][0]],
        [matrix[0][1], matrix[1][1], matrix[2][1], matrix[3][1]],
        [matrix[0][2], matrix[1][2], matrix[2][2], matrix[3][2]],
        [matrix[0][3], matrix[1][3], matrix[2][3], matrix[3][3]],
    ]
}

/// Determinant of the upper-left 3x3 (sign tells whether the transform mirrors).
fn determinant3(m: Mat4) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn transform_point(matrix: Mat4, point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * point[0] + matrix[0][1] * point[1] + matrix[0][2] * point[2] + matrix[0][3],
        matrix[1][0] * point[0] + matrix[1][1] * point[1] + matrix[1][2] * point[2] + matrix[1][3],
        matrix[2][0] * point[0] + matrix[2][1] * point[1] + matrix[2][2] * point[2] + matrix[2][3],
    ]
}

fn transform_normal(matrix: Mat4, normal: [f32; 3]) -> [f32; 3] {
    // Normals transform by the inverse-transpose of the upper-left 3x3.
    // For uniform or orthogonal scaling this equals the 3x3 itself (re-normalized).
    let transformed = [
        matrix[0][0] * normal[0] + matrix[0][1] * normal[1] + matrix[0][2] * normal[2],
        matrix[1][0] * normal[0] + matrix[1][1] * normal[1] + matrix[1][2] * normal[2],
        matrix[2][0] * normal[0] + matrix[2][1] * normal[1] + matrix[2][2] * normal[2],
    ];
    normalize3(transformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIRROR_X: Mat4 = [
        [-1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    // CCW triangle in the XY plane, facing +Z
    fn triangle() -> PrimitiveAttributes {
        PrimitiveAttributes {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: None,
            texture_coordinates: None,
            tangents: Vec::new(),
            indices: None,
        }
    }

    fn approx(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5)
    }

    #[test]
    fn mirrored_without_normal_keeps_outward_normal() {
        let (vertices, _) = build_primitive(triangle(), MIRROR_X).unwrap();
        assert!(
            approx(vertices[0].normal, [0.0, 0.0, 1.0]),
            "{:?}",
            vertices[0].normal
        );
    }

    #[test]
    fn mirrored_with_tangent_keeps_bitangent() {
        let attributes = PrimitiveAttributes {
            normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
            tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
            ..triangle()
        };
        let (vertices, _) = build_primitive(attributes, MIRROR_X).unwrap();
        assert!(
            approx(vertices[0].bitangent, [0.0, 1.0, 0.0]),
            "{:?}",
            vertices[0].bitangent
        );
    }

    #[test]
    fn normal_count_mismatch_is_rejected() {
        let attributes = PrimitiveAttributes {
            normals: Some(vec![[0.0, 0.0, 1.0]; 2]),
            ..triangle()
        };
        assert!(build_primitive(attributes, IDENTITY).is_err());
    }

    #[test]
    fn out_of_range_index_is_rejected() {
        let attributes = PrimitiveAttributes {
            indices: Some(vec![0, 1, 3]),
            ..triangle()
        };
        assert!(build_primitive(attributes, IDENTITY).is_err());
    }

    #[test]
    fn rtex_from_rgba_prepends_header() {
        let rtex = rtex_from_rgba(1, 1, vec![1, 2, 3, 4]);
        assert_eq!(rtex, rtex_bytes(1, 1, &[1, 2, 3, 4]));
    }
}
