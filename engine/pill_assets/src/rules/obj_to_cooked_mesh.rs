use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::convert::{rmsh_bytes, CookedVertex};
use crate::util::vec_math::{add3, normalize3, scale3, sub2, sub3, tangent_frame};
use crate::util::write_atomic;
use crate::Rule;

/// OBJ → cooked_mesh pre-processed mesh.
///
/// Format: `b"RMSH" | u32 version=1 | u32 vertex_count | u32 index_count`
///         `| vertex_count * sizeof(Vertex) bytes | index_count * 4 bytes`
/// All integers are little-endian. Vertices and indices are raw LE memory images.
///
/// UV Y-coordinate is pre-flipped (1.0 − v) to match the engine default.
pub struct ObjToCookedMesh;

impl Rule for ObjToCookedMesh {
    fn name(&self) -> &'static str {
        "obj_to_cooked_mesh"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.obj"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("cooked_mesh")
    }

    /// Loads the OBJ, computes per-vertex tangent space via Gram-Schmidt, and writes a flat binary blob (magic + header + interleaved vertex data + indices).
    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let load_options = tobj::LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        };
        let (models, _) =
            tobj::load_obj(input, &load_options).with_context(|| format!("load obj {input:?}"))?;

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
        let mut vertices: Vec<CookedVertex> = Vec::with_capacity(mesh.positions.len() / 3);

        for i in 0..mesh.positions.len() / 3 {
            let uv_v = *mesh.texcoords.get(i * 2 + 1).unwrap_or(&0.0);
            vertices.push(CookedVertex {
                position: [
                    mesh.positions[i * 3],
                    mesh.positions[i * 3 + 1],
                    mesh.positions[i * 3 + 2],
                ],
                texture_coordinates: [*mesh.texcoords.get(i * 2).unwrap_or(&0.0), 1.0 - uv_v],
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
        let mut triangle_counts = vec![0usize; vertices.len()];

        // Accumulate per-triangle tangent/bitangent contributions into each vertex.
        for triangle in indices.chunks(3) {
            let index_0 = triangle[0] as usize;
            let index_1 = triangle[1] as usize;
            let index_2 = triangle[2] as usize;

            let position_0 = vertices[index_0].position;
            let position_1 = vertices[index_1].position;
            let position_2 = vertices[index_2].position;
            let uv_0 = vertices[index_0].texture_coordinates;
            let uv_1 = vertices[index_1].texture_coordinates;
            let uv_2 = vertices[index_2].texture_coordinates;

            let delta_position_1 = sub3(position_1, position_0);
            let delta_position_2 = sub3(position_2, position_0);
            let delta_uv_1 = sub2(uv_1, uv_0);
            let delta_uv_2 = sub2(uv_2, uv_0);

            let determinant = delta_uv_1[0] * delta_uv_2[1] - delta_uv_1[1] * delta_uv_2[0];
            if determinant.abs() < 1e-8 {
                continue;
            }
            let inverse_determinant = 1.0 / determinant;

            let tangent = scale3(
                sub3(
                    scale3(delta_position_1, delta_uv_2[1]),
                    scale3(delta_position_2, delta_uv_1[1]),
                ),
                inverse_determinant,
            );
            let bitangent = scale3(
                sub3(
                    scale3(delta_position_2, delta_uv_1[0]),
                    scale3(delta_position_1, delta_uv_2[0]),
                ),
                inverse_determinant,
            );

            for &index in &[index_0, index_1, index_2] {
                vertices[index].tangent = add3(vertices[index].tangent, tangent);
                vertices[index].bitangent = add3(vertices[index].bitangent, bitangent);
                triangle_counts[index] += 1;
            }
        }

        // Average accumulated tangents and normalize — approximation of Gram-Schmidt re-orthogonalization.
        for (i, &triangle_count) in triangle_counts.iter().enumerate() {
            if triangle_count > 0 {
                let inverse_count = 1.0 / triangle_count as f32;
                vertices[i].tangent = normalize3(scale3(vertices[i].tangent, inverse_count));
                vertices[i].bitangent = normalize3(scale3(vertices[i].bitangent, inverse_count));
            } else {
                // Only in degenerate-UV triangles: any frame around the normal keeps shading valid
                (vertices[i].tangent, vertices[i].bitangent) = tangent_frame(vertices[i].normal);
            }
        }

        write_atomic(output, &rmsh_bytes(&vertices, indices))
    }
}
