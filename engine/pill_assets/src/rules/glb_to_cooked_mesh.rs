use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::convert::{self, CookedVertex, Mat4, PrimitiveAttributes};
use crate::util::{rtex::write_rtex, write_atomic};
use crate::Rule;

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

    fn extra_outputs(&self, input: &Path) -> Vec<PathBuf> {
        // Which sidecars a GLB yields depends on its materials: report the ones on disk.
        let stem = input
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let directory = input.parent().unwrap_or_else(|| Path::new(""));
        ["albedo", "normal", "metallic_roughness", "emissive"]
            .iter()
            .map(|texture_kind| directory.join(format!("{stem}_{texture_kind}.cooked_tex")))
            .filter(|path| path.exists())
            .collect()
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let bytes = std::fs::read(input).with_context(|| format!("read {input:?}"))?;
        let (document, buffers, images) =
            gltf::import_slice(&bytes).with_context(|| format!("parse GLB {input:?}"))?;

        // --- Mesh ---
        // Node world transforms are baked into the vertices.

        let mut all_vertices: Vec<CookedVertex> = Vec::new();
        let mut all_indices: Vec<u32> = Vec::new();

        // Collect (mesh_index, world_transform_4x4) pairs by traversing the scene graph.
        let mut mesh_instances: Vec<(usize, Mat4)> = Vec::new();
        let default_scene = document
            .default_scene()
            .or_else(|| document.scenes().next());
        if let Some(scene) = default_scene {
            let mut stack: Vec<(gltf::Node, Mat4)> = scene
                .nodes()
                .map(|node| (node, convert::IDENTITY))
                .collect();
            while let Some((node, parent_transform)) = stack.pop() {
                let local_transform = convert::mat4_from_cols(node.transform().matrix());
                let world_transform = convert::mat4_mul(parent_transform, local_transform);
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
                mesh_instances.push((mesh_index, convert::IDENTITY));
            }
        }

        if mesh_instances.is_empty() {
            anyhow::bail!("{input:?}: no meshes in GLB");
        }

        let meshes: Vec<gltf::Mesh> = document.meshes().collect();
        for (mesh_index, world_transform) in mesh_instances {
            let mesh = &meshes[mesh_index];
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    eprintln!(
                        "warning: {input:?}: skipping {:?} primitive (only triangle lists are supported)",
                        primitive.mode()
                    );
                    continue;
                }
                let reader = primitive.reader(|buffer| Some(&*buffers[buffer.index()]));
                let vertex_offset = all_vertices.len() as u32;

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .with_context(|| format!("{input:?}: missing positions"))?
                    .collect();
                let (vertices, primitive_indices) = convert::build_primitive(
                    PrimitiveAttributes {
                        positions,
                        normals: reader.read_normals().map(|normals| normals.collect()),
                        texture_coordinates: reader
                            .read_tex_coords(0)
                            .map(|coordinates| coordinates.into_f32().collect()),
                        tangents: reader
                            .read_tangents()
                            .map(|tangents| tangents.collect())
                            .unwrap_or_default(),
                        indices: reader
                            .read_indices()
                            .map(|indices| indices.into_u32().collect()),
                    },
                    world_transform,
                )
                .with_context(|| format!("{input:?}: mesh {mesh_index}"))?;

                all_indices.extend(primitive_indices.iter().map(|&index| index + vertex_offset));
                all_vertices.extend(vertices);
            }
        }

        if all_indices.is_empty() {
            anyhow::bail!("{input:?}: no triangle primitives in GLB");
        }

        // --- Textures (side outputs) ---
        // Before the mesh (mtime anchor), so a failed texture makes the rule rerun.

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

        // --- Mesh (anchor output, last) ---
        write_atomic(output, &convert::rmsh_bytes(&all_vertices, &all_indices))
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
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|&[red, green, blue]| [red, green, blue, 255u8])
                .collect(),
        ),
        format => bail!("{input:?}: {label}: unsupported GLB image format: {format:?}"),
    };
    // glTF UV origin (0,0) is top-left, matching wgpu/Vulkan — no row flip needed.
    write_rtex(&path, image.width, image.height, &rgba)
        .with_context(|| format!("{input:?}: {label}"))
}
