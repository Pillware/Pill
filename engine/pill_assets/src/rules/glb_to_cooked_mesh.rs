use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use bytemuck::{Pod, Zeroable};

use crate::Rule;

/// Must stay bit-for-bit identical to `pill_engine::resources::MeshVertex`.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    texture_coordinates: [f32; 2],
    normal: [f32; 3],
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

/// GLB → cooked_mesh + sidecar textures.
///
/// Primary output: `{stem}.cooked_mesh` (RMSH format, same as ObjToCookedMesh).
/// Side outputs written in the same directory when textures are present:
///   `{stem}_albedo.cooked_tex`             — base color (RTEX RGBA8)
///   `{stem}_normal.cooked_tex`             — normal map (RTEX RGBA8)
///   `{stem}_metallic_roughness.cooked_tex` — G=roughness, B=metallic (RTEX RGBA8)
///   `{stem}_emissive.cooked_tex`           — emissive color (RTEX RGBA8)
pub struct GlbToCookedMesh;

impl Rule for GlbToCookedMesh {
    fn name(&self) -> &'static str {
        "glb_to_cooked_mesh"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.glb"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("cooked_mesh")
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let bytes = std::fs::read(input).with_context(|| format!("read {input:?}"))?;
        let (document, buffers, images) =
            gltf::import_slice(&bytes).with_context(|| format!("parse GLB {input:?}"))?;

        // --- Mesh ---
        // Walk the scene node hierarchy. For each node that references a mesh, apply the
        // node's world transform to vertex positions and normals before cooked output.
        // This bakes the root-node transform (scale, rotation, etc.) into the geometry so
        // game code needs no compensating transform on the mesh entity.

        let mut all_vertices: Vec<Vertex> = Vec::new();
        let mut all_indices: Vec<u32> = Vec::new();

        // Collect (mesh_index, world_transform_4x4) pairs by traversing the scene graph.
        let mut mesh_instances: Vec<(usize, [[f32; 4]; 4])> = Vec::new();
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0f32],
        ];
        let default_scene = document
            .default_scene()
            .or_else(|| document.scenes().next());
        if let Some(scene) = default_scene {
            let mut stack: Vec<(gltf::Node, [[f32; 4]; 4])> =
                scene.nodes().map(|node| (node, identity)).collect();
            while let Some((node, parent_transform)) = stack.pop() {
                let local_transform = node_transform(node.transform());
                let world_transform = mat4_mul(parent_transform, local_transform);
                if let Some(mesh) = node.mesh() {
                    mesh_instances.push((mesh.index(), world_transform));
                }
                for child in node.children() {
                    stack.push((child, world_transform));
                }
            }
        } else {
            // No scene: fall back to processing every mesh with identity transform.
            for (mesh_index, _) in document.meshes().enumerate() {
                mesh_instances.push((mesh_index, identity));
            }
        }

        if mesh_instances.is_empty() {
            anyhow::bail!("{input:?}: no meshes in GLB");
        }

        let meshes: Vec<gltf::Mesh> = document.meshes().collect();
        for (mesh_index, world_transform) in mesh_instances {
            let mesh = &meshes[mesh_index];
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buffer| Some(&*buffers[buffer.index()]));
                let vertex_offset = all_vertices.len() as u32;

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .with_context(|| format!("{input:?}: missing positions"))?
                    .collect();
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .with_context(|| format!("{input:?}: missing normals"))?
                    .collect();
                let texture_coordinates: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .with_context(|| format!("{input:?}: missing UV set 0"))?
                    .into_f32()
                    .collect();
                let glb_tangents: Vec<[f32; 4]> = reader
                    .read_tangents()
                    .map(|tangents_iter| tangents_iter.collect())
                    .unwrap_or_default();
                let primitive_indices: Vec<u32> = reader
                    .read_indices()
                    .with_context(|| format!("{input:?}: missing indices"))?
                    .into_u32()
                    .collect();

                let mut vertices: Vec<Vertex> = (0..positions.len())
                    .map(|vertex_index| {
                        let normal = transform_normal(world_transform, normals[vertex_index]);
                        let (tangent_x, tangent_y, tangent_z, sign) = glb_tangents
                            .get(vertex_index)
                            .map(|tangent| (tangent[0], tangent[1], tangent[2], tangent[3]))
                            .unwrap_or((1.0, 0.0, 0.0, 1.0));
                        let tangent_world =
                            transform_normal(world_transform, [tangent_x, tangent_y, tangent_z]);
                        let bitangent_x =
                            (normal[1] * tangent_world[2] - normal[2] * tangent_world[1]) * sign;
                        let bitangent_y =
                            (normal[2] * tangent_world[0] - normal[0] * tangent_world[2]) * sign;
                        let bitangent_z =
                            (normal[0] * tangent_world[1] - normal[1] * tangent_world[0]) * sign;
                        Vertex {
                            position: transform_point(world_transform, positions[vertex_index]),
                            texture_coordinates: texture_coordinates[vertex_index],
                            normal,
                            tangent: tangent_world,
                            bitangent: [bitangent_x, bitangent_y, bitangent_z],
                        }
                    })
                    .collect();

                if glb_tangents.is_empty() {
                    compute_tangents(&mut vertices, &primitive_indices);
                }

                all_indices.extend(primitive_indices.iter().map(|&index| index + vertex_offset));
                all_vertices.extend(vertices);
            }
        }

        let vertex_bytes: &[u8] = bytemuck::cast_slice(&all_vertices);
        let index_bytes: &[u8] = bytemuck::cast_slice(&all_indices);
        let mut output_bytes = Vec::with_capacity(16 + vertex_bytes.len() + index_bytes.len());
        output_bytes.extend_from_slice(b"RMSH");
        output_bytes.extend_from_slice(&1u32.to_le_bytes());
        output_bytes.extend_from_slice(&(all_vertices.len() as u32).to_le_bytes());
        output_bytes.extend_from_slice(&(all_indices.len() as u32).to_le_bytes());
        output_bytes.extend_from_slice(vertex_bytes);
        output_bytes.extend_from_slice(index_bytes);
        std::fs::write(output, &output_bytes).with_context(|| format!("write {output:?}"))?;

        // --- Textures (side outputs) ---

        let stem = input
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let directory = output
            .parent()
            .with_context(|| "output has no parent directory")?;

        for material in document.materials() {
            let material_index = material.index();
            let suffix = |base: &str| -> String {
                match material_index {
                    None | Some(0) => format!("{stem}_{base}.cooked_tex"),
                    Some(index) => format!("{stem}_mat{index}_{base}.cooked_tex"),
                }
            };
            let pbr_metallic_roughness = material.pbr_metallic_roughness();

            if let Some(info) = pbr_metallic_roughness.base_color_texture() {
                write_cooked_tex(
                    &images,
                    info.texture().source().index(),
                    directory.join(suffix("albedo")),
                    "base color texture",
                    input,
                )?;
            }
            if let Some(info) = material.normal_texture() {
                write_cooked_tex(
                    &images,
                    info.texture().source().index(),
                    directory.join(suffix("normal")),
                    "normal texture",
                    input,
                )?;
            }
            if let Some(info) = pbr_metallic_roughness.metallic_roughness_texture() {
                write_cooked_tex(
                    &images,
                    info.texture().source().index(),
                    directory.join(suffix("metallic_roughness")),
                    "metallic_roughness texture",
                    input,
                )?;
            }
            if let Some(info) = material.emissive_texture() {
                write_cooked_tex(
                    &images,
                    info.texture().source().index(),
                    directory.join(suffix("emissive")),
                    "emissive texture",
                    input,
                )?;
            }
        }

        Ok(())
    }
}

fn write_cooked_tex(
    images: &[gltf::image::Data],
    image_index: usize,
    path: std::path::PathBuf,
    label: &str,
    input: &Path,
) -> Result<()> {
    use gltf::image::Format;
    use std::borrow::Cow;
    let Some(image) = images.get(image_index) else {
        return Ok(());
    };
    let rgba: Cow<[u8]> = match image.format {
        Format::R8G8B8A8 => Cow::Borrowed(&image.pixels),
        Format::R8G8B8 => Cow::Owned(
            image
                .pixels
                .chunks_exact(3)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255u8])
                .collect(),
        ),
        format => bail!("{input:?}: {label}: unsupported GLB image format: {format:?}"),
    };
    // glTF UV origin (0,0) is top-left, matching wgpu/Vulkan — no row flip needed.
    super::procedural_equirect::write_rtex(&path, image.width, image.height, &rgba)
        .with_context(|| format!("{input:?}: {label}"))
}

fn compute_tangents(vertices: &mut [Vertex], indices: &[u32]) {
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
            vertices[index].tangent = add3(vertices[index].tangent, tangent);
            vertices[index].bitangent = add3(vertices[index].bitangent, bitangent);
            triangle_counts[index] += 1;
        }
    }

    for (index, &count) in triangle_counts.iter().enumerate() {
        if count > 0 {
            let inverse_count = 1.0 / count as f32;
            vertices[index].tangent = normalize3(scale3(vertices[index].tangent, inverse_count));
            vertices[index].bitangent =
                normalize3(scale3(vertices[index].bitangent, inverse_count));
        }
    }
}

fn sub3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}
fn sub2(left: [f32; 2], right: [f32; 2]) -> [f32; 2] {
    [left[0] - right[0], left[1] - right[1]]
}
fn scale3(vector: [f32; 3], scalar: f32) -> [f32; 3] {
    [vector[0] * scalar, vector[1] * scalar, vector[2] * scalar]
}
fn add3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}
fn normalize3(vector: [f32; 3]) -> [f32; 3] {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length < 1e-10 {
        vector
    } else {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    }
}

fn mat4_mul(left: [[f32; 4]; 4], right: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
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

fn node_transform(transform: gltf::scene::Transform) -> [[f32; 4]; 4] {
    let matrix = transform.matrix();
    // gltf gives column-major; convert to row-major [[f32;4];4]
    [
        [matrix[0][0], matrix[1][0], matrix[2][0], matrix[3][0]],
        [matrix[0][1], matrix[1][1], matrix[2][1], matrix[3][1]],
        [matrix[0][2], matrix[1][2], matrix[2][2], matrix[3][2]],
        [matrix[0][3], matrix[1][3], matrix[2][3], matrix[3][3]],
    ]
}

fn transform_point(matrix: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * point[0] + matrix[0][1] * point[1] + matrix[0][2] * point[2] + matrix[0][3],
        matrix[1][0] * point[0] + matrix[1][1] * point[1] + matrix[1][2] * point[2] + matrix[1][3],
        matrix[2][0] * point[0] + matrix[2][1] * point[1] + matrix[2][2] * point[2] + matrix[2][3],
    ]
}

fn transform_normal(matrix: [[f32; 4]; 4], normal: [f32; 3]) -> [f32; 3] {
    // Normals transform by the inverse-transpose of the upper-left 3x3.
    // For uniform or orthogonal scaling this equals the 3x3 itself (re-normalized).
    let transformed = [
        matrix[0][0] * normal[0] + matrix[0][1] * normal[1] + matrix[0][2] * normal[2],
        matrix[1][0] * normal[0] + matrix[1][1] * normal[1] + matrix[1][2] * normal[2],
        matrix[2][0] * normal[0] + matrix[2][1] * normal[1] + matrix[2][2] * normal[2],
    ];
    normalize3(transformed)
}
