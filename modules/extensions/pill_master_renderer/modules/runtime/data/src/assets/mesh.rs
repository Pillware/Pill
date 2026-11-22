//! Mesh assets: the vertices and indices the renderer draws, and the decoders
//! that build them.
//!
//! # Responsibilities
//!
//! - Carry the geometry the renderer uploads: vertices interleaved in the
//!   order the pipeline's vertex step reads them, plus a 32-bit index list.
//! - Build a mesh three ways: from buffers the game already holds, from the
//!   built-in triangle used as a stand-in, and by decoding a Wavefront OBJ
//!   buffer. The decode flips V by default; the top-down decode keeps it as
//!   authored.
//! - Derive tangent space after an OBJ decode, when the import settings ask
//!   for it: the tangents and bitangents are accumulated from the mesh's
//!   triangles and their UVs, then normalized.
//! - Load through a metadata file ([`ImportedAsset`]): the
//!   [`MeshImportSettings`] in `<model>.obj.meta` hold the decode choices.
//!
//! # Design
//!
//! [`MeshVertex`] is `repr(C)` and `Pod`, so a mesh's vertices upload as a
//! plain byte slice, and that same field order is the attribute layout the
//! renderer declares to wgpu. A mesh is an asset ([`pill_engine::Asset`]), so
//! the renderer rebuilds its buffers when the asset's version moves, and a
//! game never touches renderer state mid frame.

// External crates
use pill_engine::{
    asset, pill_mirror_impl, pill_mirror_method, pill_mirror_object, AssetLoadError,
    AssetLoadResult, ImportedAsset, PillMirror,
};
use serde::{Deserialize, Serialize};

// =============================================================================
// MeshVertex
// =============================================================================

/// One vertex of a mesh, in the layout of the vertex buffer.
///
/// The type is `repr(C)` and `Pod`, so a run of vertices uploads to the GPU as
/// raw bytes. The field order is also the attribute layout the renderer
/// declares to wgpu, so the two have to agree.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable, PillMirror)]
pub struct MeshVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Texture coordinates for the shader's UV input. The OBJ decoder flips
    /// V, because a file counts it upward from the bottom edge while the
    /// renderer samples from the top. `Mesh::from_obj_bytes_top_down` decodes
    /// without the flip, for an asset authored top-down.
    pub texture_coordinates: [f32; 2],
    /// Object-space normal. A vertex whose file entry has no normal falls
    /// back to +Y.
    pub normal: [f32; 3],
    /// Tangent along the U direction, one axis of the TBN frame (tangent,
    /// bitangent, normal) the shaders use for normal mapping.
    pub tangent: [f32; 3],
    /// Bitangent along the V direction, the TBN frame's third axis. The OBJ
    /// decode starts the tangent and the bitangent at zero and fills them
    /// from the triangles.
    pub bitangent: [f32; 3],
}

// =============================================================================
// Mesh
// =============================================================================

/// One drawable piece of geometry, holding interleaved vertices and a 32-bit
/// index list.
///
/// The renderer builds a GPU copy once and rebuilds it when the asset's
/// version moves. A game edits the mesh it holds and lets the renderer notice,
/// instead of touching buffers itself.
#[asset(imported)]
#[pill_mirror_object(asset, import, shared = "pill_master_renderer::Mesh")]
#[derive(Clone, Debug)]
pub struct Mesh {
    /// Name used in logs and error messages, and as the label of the GPU
    /// buffers built from the mesh.
    pub name: String,
    /// Vertices, interleaved as [`MeshVertex`] lays them out.
    pub vertices: Vec<MeshVertex>,
    /// Indices into [`Self::vertices`], one triangle per three entries.
    pub indices: Vec<u32>,
}

#[pill_mirror_impl]
impl Mesh {
    /// Builds a mesh from finished vertex and index buffers.
    ///
    /// The buffers are stored as given. Nothing checks the indices against
    /// the vertex list, so the caller supplies a pair that already agrees.
    #[pill_mirror_method]
    pub fn from_data(
        name: impl Into<String>,
        vertices: Vec<MeshVertex>,
        indices: Vec<u32>,
    ) -> Self {
        Self {
            name: name.into(),
            vertices,
            indices,
        }
    }

    /// The built-in stand-in mesh is one triangle in the XY plane, one unit
    /// wide and one unit tall, facing +Z, with its UVs and tangent space
    /// filled in.
    ///
    /// A project with no geometry of its own can register this and draw it
    /// through the same material and pipeline as any loaded mesh. That keeps
    /// a fresh scene renderable before its assets exist.
    #[pill_mirror_method]
    pub fn triangle() -> Self {
        let make_vertex = |position, texture_coordinates| MeshVertex {
            position,
            texture_coordinates,
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
        };
        Self::from_data(
            "triangle",
            vec![
                make_vertex([-0.5, -0.5, 0.0], [0.0, 1.0]),
                make_vertex([0.5, -0.5, 0.0], [1.0, 1.0]),
                make_vertex([0.0, 0.5, 0.0], [0.5, 0.0]),
            ],
            vec![0, 1, 2],
        )
    }

    /// Decodes a Wavefront OBJ buffer into an indexed, tangent-space mesh.
    ///
    /// The C# bridge hands the host a model file's raw bytes and gets a mesh
    /// back through this function, and the renderer's asset preview decodes a
    /// file through it too.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when the buffer is not a readable
    /// OBJ or holds no triangles. Both cases carry the mesh's name, so the
    /// failure points at the asset rather than at the bytes.
    #[pill_mirror_method]
    pub fn from_obj_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, AssetLoadError> {
        Self::decode_obj(name.into(), bytes, &MeshImportSettings::default())
    }

    /// Decodes a Wavefront OBJ buffer whose V coordinates are kept as
    /// authored, top-down, with `0.0` at the image's top edge. The renderer
    /// samples the same way.
    ///
    /// [`Self::from_obj_bytes`] decodes the other way, flipping V because an
    /// OBJ file counts V up from the bottom edge. Use this function for a
    /// file written for a top-left sampler, such as a port of an older demo.
    /// Its texture coordinates then still point at the same parts of the
    /// image. The metadata import makes the same choice through
    /// [`MeshImportSettings::flip_v`].
    ///
    /// # Errors
    ///
    /// Returns the same [`AssetLoadError::Decode`] cases as
    /// [`Self::from_obj_bytes`].
    #[pill_mirror_method]
    pub fn from_obj_bytes_top_down(
        name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<Self, AssetLoadError> {
        let settings = MeshImportSettings {
            flip_v: false,
            ..MeshImportSettings::default()
        };
        Self::decode_obj(name.into(), bytes, &settings)
    }

    /// Rebuilds every vertex's tangent and bitangent from the triangles and
    /// their texture coordinates, the same derivation an OBJ import runs.
    ///
    /// For geometry built outside the decoders - a live link from a modelling
    /// tool, a procedural mesh - that still wants normal mapping to work.
    pub fn calculate_tangents(&mut self) {
        for vertex in &mut self.vertices {
            vertex.tangent = [0.0; 3];
            vertex.bitangent = [0.0; 3];
        }
        calculate_tangent_space(&mut self.vertices, &self.indices);
    }

    /// The OBJ decode behind [`Self::from_obj_bytes`] and the metadata import.
    ///
    /// `settings` says whether the decode flips V and whether it derives
    /// tangent space.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when `tobj` cannot read the buffer
    /// as an OBJ, and when the result holds no vertices or no indices.
    fn decode_obj(
        name: String,
        bytes: &[u8],
        settings: &MeshImportSettings,
    ) -> Result<Self, AssetLoadError> {
        // Step 1: parse the buffer, ignoring any material libraries it names.
        let mut byte_cursor = std::io::Cursor::new(bytes);
        let load_options = tobj::LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        };
        let (models, _) = tobj::load_obj_buf(&mut byte_cursor, &load_options, |_| {
            Ok((Vec::new(), Default::default()))
        })
        .map_err(|error| AssetLoadError::Decode {
            label: name.clone(),
            detail: error.to_string(),
        })?;

        // Step 2: copy every model's vertices and indices into the combined
        // buffers.
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for model in models {
            let model_mesh = model.mesh;
            let first_vertex_index = vertices.len() as u32;
            for index in 0..model_mesh.positions.len() / 3 {
                let position = [
                    model_mesh.positions[index * 3],
                    model_mesh.positions[index * 3 + 1],
                    model_mesh.positions[index * 3 + 2],
                ];
                let texture_coordinates = if model_mesh.texcoords.len() >= index * 2 + 2 {
                    // A file counts V upward from the bottom edge, but the
                    // renderer samples from the top.
                    let v_coordinate = model_mesh.texcoords[index * 2 + 1];
                    [
                        model_mesh.texcoords[index * 2],
                        if settings.flip_v {
                            1.0 - v_coordinate
                        } else {
                            v_coordinate
                        },
                    ]
                } else {
                    [0.0; 2]
                };
                let normal = if model_mesh.normals.len() >= index * 3 + 3 {
                    [
                        model_mesh.normals[index * 3],
                        model_mesh.normals[index * 3 + 1],
                        model_mesh.normals[index * 3 + 2],
                    ]
                } else {
                    [0.0, 1.0, 0.0]
                };
                vertices.push(MeshVertex {
                    position,
                    texture_coordinates,
                    normal,
                    tangent: [0.0; 3],
                    bitangent: [0.0; 3],
                });
            }
            indices.extend(
                model_mesh
                    .indices
                    .into_iter()
                    .map(|index| first_vertex_index + index),
            );
        }

        // Step 3: derive tangent space when the settings ask for it.
        if settings.calculate_tangents {
            calculate_tangent_space(&mut vertices, &indices);
        }
        // Step 4: refuse a decode that produced nothing to draw.
        if vertices.is_empty() || indices.is_empty() {
            return Err(AssetLoadError::Decode {
                label: name,
                detail: "the OBJ buffer contained no triangles".to_owned(),
            });
        }
        Ok(Self::from_data(name, vertices, indices))
    }
}

// The metadata type name defaults to the shared name in the attribute above,
// so every binary's copy of `Mesh` reads the same `.meta` files.
impl ImportedAsset for Mesh {
    type ImportSettings = MeshImportSettings;
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["obj"];

    fn import(
        name: &str,
        source_bytes: &[u8],
        settings: &MeshImportSettings,
    ) -> AssetLoadResult<Self> {
        Self::decode_obj(name.to_owned(), source_bytes, settings)
    }
}

// =============================================================================
// MeshImportSettings
// =============================================================================

/// How to decode a model file into a mesh; a metadata file holds these
/// choices.
///
/// The defaults flip V and derive tangents. An import with no metadata file
/// uses them, and so does [`Mesh::from_obj_bytes`]. Every field is part of the
/// metadata file format, and `#[serde(default)]` lets a file written before a
/// field existed still load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshImportSettings {
    /// Whether the decode flips the V texture coordinate (`v` becomes
    /// `1 - v`). A file counts V upward from the bottom edge; the renderer
    /// samples from the top.
    pub flip_v: bool,
    /// Whether the decode derives tangents and bitangents from the triangles
    /// and their UVs. Normal mapping needs them; with this off they stay zero.
    pub calculate_tangents: bool,
}

impl Default for MeshImportSettings {
    fn default() -> Self {
        Self {
            flip_v: true,
            calculate_tangents: true,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds every vertex's tangent and bitangent from the mesh's triangles and
/// their UV coordinates, then normalizes the two.
fn calculate_tangent_space(vertices: &mut [MeshVertex], indices: &[u32]) {
    // Step 1: add each triangle's tangent and bitangent to its three corners.
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let p0 = glam::Vec3::from(vertices[a].position);
        let p1 = glam::Vec3::from(vertices[b].position);
        let p2 = glam::Vec3::from(vertices[c].position);
        let uv0 = glam::Vec2::from(vertices[a].texture_coordinates);
        let uv1 = glam::Vec2::from(vertices[b].texture_coordinates);
        let uv2 = glam::Vec2::from(vertices[c].texture_coordinates);
        let edge1 = p1 - p0;
        let edge2 = p2 - p0;
        let delta1 = uv1 - uv0;
        let delta2 = uv2 - uv0;
        let determinant = delta1.x * delta2.y - delta1.y * delta2.x;
        if determinant.abs() < 1.0e-8 {
            continue;
        }
        let determinant_reciprocal = determinant.recip();
        let tangent = (edge1 * delta2.y - edge2 * delta1.y) * determinant_reciprocal;
        let bitangent = (edge2 * delta1.x - edge1 * delta2.x) * determinant_reciprocal;
        for index in [a, b, c] {
            vertices[index].tangent = (glam::Vec3::from(vertices[index].tangent) + tangent).into();
            vertices[index].bitangent =
                (glam::Vec3::from(vertices[index].bitangent) + bitangent).into();
        }
    }
    // Step 2: normalize each vertex's pair, using an axis when a vertex got no
    // usable triangle.
    for vertex in vertices {
        vertex.tangent =
            normalize_or_fallback(glam::Vec3::from(vertex.tangent), glam::Vec3::X).into();
        vertex.bitangent =
            normalize_or_fallback(glam::Vec3::from(vertex.bitangent), glam::Vec3::Y).into();
    }
}

/// Returns the vector scaled to length 1, or `fallback_axis` when the vector
/// is not finite or too short to normalize.
fn normalize_or_fallback(vector: glam::Vec3, fallback_axis: glam::Vec3) -> glam::Vec3 {
    if vector.is_finite() && vector.length_squared() > 1.0e-8 {
        vector.normalize()
    } else {
        fallback_axis
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TRIANGLE_OBJ: &str = "\
v 0.0 0.0 0.0\n\
v 1.0 0.0 0.0\n\
v 0.0 1.0 0.0\n\
vt 0.0 0.0\n\
vt 1.0 0.0\n\
vt 0.0 1.0\n\
f 1/1 2/2 3/3\n";

    #[test]
    fn decodes_a_minimal_obj_into_an_indexed_triangle() {
        let mesh = Mesh::from_obj_bytes("triangle", TRIANGLE_OBJ.as_bytes()).expect("valid OBJ");
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
    }

    #[test]
    fn rejects_a_buffer_with_no_triangles() {
        assert!(Mesh::from_obj_bytes("empty", b"").is_err());
    }

    /// The top-down decode keeps every V as authored, so it mirrors the
    /// default decode's flip and changes nothing else.
    #[test]
    fn the_top_down_decode_mirrors_the_default_flip() {
        let flipped = Mesh::from_obj_bytes("t", TRIANGLE_OBJ.as_bytes()).unwrap();
        let top_down = Mesh::from_obj_bytes_top_down("t", TRIANGLE_OBJ.as_bytes()).unwrap();
        assert_eq!(flipped.indices, top_down.indices);
        assert_eq!(flipped.vertices.len(), top_down.vertices.len());
        for (flipped, top_down) in flipped.vertices.iter().zip(&top_down.vertices) {
            assert_eq!(flipped.position, top_down.position);
            assert_eq!(
                flipped.texture_coordinates[0],
                top_down.texture_coordinates[0]
            );
            let mirrored = flipped.texture_coordinates[1] + top_down.texture_coordinates[1];
            assert!((mirrored - 1.0).abs() < 1.0e-6, "{mirrored}");
        }
    }

    /// An import with no metadata file decodes with the default settings,
    /// which is what `from_obj_bytes` uses.
    #[test]
    fn importing_with_default_settings_matches_from_obj_bytes() {
        let decoded = Mesh::from_obj_bytes("t", TRIANGLE_OBJ.as_bytes()).unwrap();
        let imported =
            Mesh::import("t", TRIANGLE_OBJ.as_bytes(), &MeshImportSettings::default()).unwrap();
        let positions_uvs_and_tangents = |mesh: &Mesh| {
            mesh.vertices
                .iter()
                .map(|vertex| (vertex.position, vertex.texture_coordinates, vertex.tangent))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            positions_uvs_and_tangents(&imported),
            positions_uvs_and_tangents(&decoded)
        );
        // V was flipped, so the second texture coordinate of `vt 0.0 1.0` is 0.
        assert_eq!(imported.vertices[2].texture_coordinates, [0.0, 0.0]);
    }

    #[test]
    fn settings_control_the_v_flip_and_the_tangents() {
        let settings = MeshImportSettings {
            flip_v: false,
            calculate_tangents: false,
        };
        let mesh = Mesh::import("t", TRIANGLE_OBJ.as_bytes(), &settings).unwrap();
        assert_eq!(mesh.vertices[2].texture_coordinates, [0.0, 1.0]);
        assert!(mesh
            .vertices
            .iter()
            .all(|vertex| vertex.tangent == [0.0; 3]));

        let with_tangents =
            Mesh::import("t", TRIANGLE_OBJ.as_bytes(), &MeshImportSettings::default()).unwrap();
        assert!(with_tangents
            .vertices
            .iter()
            .any(|vertex| vertex.tangent != [0.0; 3]));
    }

    /// The on-disk form: field names as written, and defaults for missing ones.
    #[test]
    fn settings_serialize_with_their_field_names() {
        assert_eq!(
            serde_json::to_string(&MeshImportSettings::default()).unwrap(),
            r#"{"flip_v":true,"calculate_tangents":true}"#
        );
        let partial: MeshImportSettings = serde_json::from_str(r#"{"flip_v": false}"#).unwrap();
        assert_eq!(
            partial,
            MeshImportSettings {
                flip_v: false,
                calculate_tangents: true
            }
        );
    }
}
