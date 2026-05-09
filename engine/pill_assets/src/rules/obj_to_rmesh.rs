use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use bytemuck::{Pod, Zeroable};

use crate::Rule;

/// Must stay bit-for-bit identical to `pill_engine::resources::MeshVertex`.
/// Both are `#[repr(C)]` with the same 14 floats in the same order.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    texture_coordinates: [f32; 2],
    normal: [f32; 3],
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

/// OBJ → RMESH pre-processed mesh.
///
/// Format: `b"RMSH" | u32 version=1 | u32 vertex_count | u32 index_count`
///         `| vertex_count * sizeof(Vertex) bytes | index_count * 4 bytes`
/// All integers are little-endian. Vertices and indices are raw LE memory images.
///
/// UV Y-coordinate is pre-flipped (1.0 − v) to match the engine default.
pub struct ObjToRmesh;

impl Rule for ObjToRmesh {
    fn name(&self) -> &'static str {
        "obj_to_rmesh"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.obj"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("rmesh")
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let opts = tobj::LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        };
        let (models, _) =
            tobj::load_obj(input, &opts).with_context(|| format!("load obj {input:?}"))?;

        if models.is_empty() {
            bail!("{input:?}: OBJ file contains no meshes");
        }
        if models.len() > 1 {
            bail!(
                "{input:?}: OBJ file contains {} meshes; only single-mesh files are supported",
                models.len()
            );
        }

        let mesh = &models[0].mesh;
        let mut vertices: Vec<Vertex> = Vec::with_capacity(mesh.positions.len() / 3);

        for i in 0..mesh.positions.len() / 3 {
            let uv_v = *mesh.texcoords.get(i * 2 + 1).unwrap_or(&0.0);
            vertices.push(Vertex {
                position: [
                    mesh.positions[i * 3],
                    mesh.positions[i * 3 + 1],
                    mesh.positions[i * 3 + 2],
                ],
                texture_coordinates: [
                    *mesh.texcoords.get(i * 2).unwrap_or(&0.0),
                    1.0 - uv_v,
                ],
                normal: [
                    *mesh.normals.get(i * 3).unwrap_or(&0.0),
                    *mesh.normals.get(i * 3 + 1).unwrap_or(&0.0),
                    *mesh.normals.get(i * 3 + 2).unwrap_or(&0.0),
                ],
                tangent: [0.0; 3],
                bitangent: [0.0; 3],
            });
        }

        let indices = &mesh.indices;
        let mut tri_count = vec![0usize; vertices.len()];

        for tri in indices.chunks(3) {
            let i0 = tri[0] as usize;
            let i1 = tri[1] as usize;
            let i2 = tri[2] as usize;

            let pos0 = vertices[i0].position;
            let pos1 = vertices[i1].position;
            let pos2 = vertices[i2].position;
            let uv0 = vertices[i0].texture_coordinates;
            let uv1 = vertices[i1].texture_coordinates;
            let uv2 = vertices[i2].texture_coordinates;

            let dp1 = sub3(pos1, pos0);
            let dp2 = sub3(pos2, pos0);
            let du1 = sub2(uv1, uv0);
            let du2 = sub2(uv2, uv0);

            let det = du1[0] * du2[1] - du1[1] * du2[0];
            if det.abs() < 1e-8 {
                continue;
            }
            let r = 1.0 / det;

            let tangent = scale3(sub3(scale3(dp1, du2[1]), scale3(dp2, du1[1])), r);
            let bitangent = scale3(sub3(scale3(dp2, du1[0]), scale3(dp1, du2[0])), r);

            for &i in &[i0, i1, i2] {
                vertices[i].tangent = add3(vertices[i].tangent, tangent);
                vertices[i].bitangent = add3(vertices[i].bitangent, bitangent);
                tri_count[i] += 1;
            }
        }

        for (i, &n) in tri_count.iter().enumerate() {
            if n > 0 {
                let denom = 1.0 / n as f32;
                vertices[i].tangent = normalize3(scale3(vertices[i].tangent, denom));
                vertices[i].bitangent = normalize3(scale3(vertices[i].bitangent, denom));
            }
        }

        let vertex_bytes: &[u8] = bytemuck::cast_slice(&vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(indices);

        let mut out = Vec::with_capacity(16 + vertex_bytes.len() + index_bytes.len());
        out.extend_from_slice(b"RMSH");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(vertices.len() as u32).to_le_bytes());
        out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        out.extend_from_slice(vertex_bytes);
        out.extend_from_slice(index_bytes);

        std::fs::write(output, &out).with_context(|| format!("write {output:?}"))?;
        Ok(())
    }
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn normalize3(a: [f32; 3]) -> [f32; 3] {
    let len = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if len < 1e-10 {
        a
    } else {
        [a[0] / len, a[1] / len, a[2] / len]
    }
}
