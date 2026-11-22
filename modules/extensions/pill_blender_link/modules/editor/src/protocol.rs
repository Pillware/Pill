//! The wire format the editor and the Blender script exchange.
//!
//! # Responsibilities
//!
//! - Frame a message: an 8-byte prefix (header length, body length, both
//!   little-endian `u32`), a UTF-8 JSON header, then a raw binary body.
//! - Cut complete frames off the front of a receive buffer, refusing a header
//!   or body larger than the link allows.
//! - Encode a [`Mesh`] as the geometry body and decode a body back into one,
//!   checking every length and index before anything reaches the asset.
//! - Wrap the same body with its two counts as a self-contained geometry
//!   payload, the state an undo step keeps, and fingerprint a mesh's geometry
//!   (independently of its vertex order) so an unsaved change can be told
//!   from the saved state.
//! - Decode a `mesh_positions` body: new positions and normals only, for a
//!   drag that leaves the topology, the texture coordinates and the vertex
//!   order as they were.
//!
//! # Design
//!
//! The geometry travels in the engine's own conventions: Y up, texture V
//! counted from the top edge, one index list of triangles over single-indexed
//! vertices. The Blender script converts to and from Blender's Z-up, bottom-up
//! V space on its side, so this file never needs to know Blender exists.
//!
//! The body is four tightly packed little-endian arrays, in this order:
//! positions (`3 * f32` per vertex), texture coordinates (`2 * f32`), normals
//! (`3 * f32`), then indices (`u32`). The header carries `vertex_count` and
//! `index_count`, so the body's length is known before it is read.
//!
//! # Create requests
//!
//! A mesh that lives only in Blender becomes a Pill asset through two messages
//! that carry no body format of their own:
//!
//! - `create_mesh` (Blender to editor): `session`, `path` (the file the person
//!   picked in Blender's save dialog, which does not exist yet) and the same
//!   `vertex_count` / `index_count` pair, with the geometry body above. The
//!   editor treats the body exactly as it treats a `mesh_update` body, and
//!   writes the file only when the path is under the project's `res`.
//! - `create_result` (editor to Blender): `session`, `asset_name`, `ok` and
//!   `detail`, plus `link_id` when `ok` is true. A refusal carries the reason
//!   in `detail` and no link id, so the add-on can show it.
//!
//! # Connect requests
//!
//! An object can also be pointed at a file the project already holds:
//!
//! - `connect_asset` (Blender to editor): `session`, `object` (the Blender
//!   object's name) and `path` (the file the person picked). No body: the
//!   editor keeps the name rules and turns the path into the asset name below
//!   the project's `res`.
//! - `connect_result` (editor to Blender): `session`, `object`, `asset_name`,
//!   `ok` and `detail`, plus `link_id` when `ok` is true. The object becomes a
//!   link, so its edits stream into the asset like any other.
//!
//! # The greeting
//!
//! The editor opens every connection with `welcome`: `protocol`, `session`,
//! `editor_title` (the editor's own window title, which the add-on shows as
//! the connection's status) and `res_directory`, the project's mounted `res`
//! folder - the folder Blender's two pickers start in, and the only place a
//! create may write. It is empty when the editor has none mounted, and a
//! create is then refused.

// Standard library
use std::hash::{Hash, Hasher};

// External crates
use pill_master_renderer_data::assets::{Mesh, MeshVertex};
use serde_json::Value as JsonValue;

// =============================================================================
// Constants
// =============================================================================

/// The protocol revision both sides announce in their greeting.
pub const PROTOCOL_VERSION: u64 = 1;

/// The length of the prefix in front of every frame, in bytes.
pub const FRAME_PREFIX_BYTES: usize = 8;

/// The largest header the link accepts, in bytes.
pub const MAXIMUM_HEADER_BYTES: usize = 64 * 1024;

/// The largest body the link accepts, in bytes: enough for a few million
/// vertices, small enough that a corrupted length cannot exhaust memory.
pub const MAXIMUM_BODY_BYTES: usize = 512 * 1024 * 1024;

/// Bytes one vertex takes in the geometry body: position, texture coordinates
/// and normal.
const VERTEX_BODY_BYTES: usize = (3 + 2 + 3) * 4;

// =============================================================================
// Types
// =============================================================================

/// One message: its JSON header and its binary body, which may be empty.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkFrame {
    /// The message's header; its `type` field names the message.
    pub header: JsonValue,
    /// The message's binary body.
    pub body: Vec<u8>,
}

impl LinkFrame {
    /// Returns the header's `type` field, or an empty string when it has none.
    pub fn message_type(&self) -> &str {
        self.header
            .get("type")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
    }

    /// Returns a header field as an unsigned number, when it holds one.
    pub fn header_number(&self, field_name: &str) -> Option<u64> {
        self.header.get(field_name).and_then(JsonValue::as_u64)
    }

    /// Returns a header field as text, when it holds text.
    pub fn header_text(&self, field_name: &str) -> Option<&str> {
        self.header.get(field_name).and_then(JsonValue::as_str)
    }
}

/// Why a receive buffer could not be read as frames. Every case ends the
/// connection, because the stream cannot be resynchronised.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameDecodeError {
    /// The prefix announced a header above [`MAXIMUM_HEADER_BYTES`].
    HeaderTooLarge(usize),
    /// The prefix announced a body above [`MAXIMUM_BODY_BYTES`].
    BodyTooLarge(usize),
    /// The header was not a JSON object.
    HeaderNotJson(String),
}

impl std::fmt::Display for FrameDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HeaderTooLarge(byte_count) => {
                write!(formatter, "a {byte_count}-byte header is over the limit")
            }
            Self::BodyTooLarge(byte_count) => {
                write!(formatter, "a {byte_count}-byte body is over the limit")
            }
            Self::HeaderNotJson(detail) => write!(formatter, "the header is not JSON: {detail}"),
        }
    }
}

/// Why a geometry body could not become a mesh.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeometryDecodeError {
    /// The body's length does not match the counts in the header.
    LengthMismatch {
        /// The length the counts call for, in bytes.
        expected_bytes: usize,
        /// The body's length, in bytes.
        actual_bytes: usize,
    },
    /// The index count is zero or not a multiple of three.
    NotTriangles(usize),
    /// An index points past the last vertex.
    IndexOutOfRange {
        /// The offending index value.
        index: u32,
        /// The number of vertices in the body.
        vertex_count: usize,
    },
    /// A position, texture coordinate or normal is NaN or infinite.
    NotFinite,
}

impl std::fmt::Display for GeometryDecodeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch {
                expected_bytes,
                actual_bytes,
            } => write!(
                formatter,
                "the body holds {actual_bytes} bytes, its counts call for {expected_bytes}"
            ),
            Self::NotTriangles(index_count) => {
                write!(
                    formatter,
                    "{index_count} indices do not form whole triangles"
                )
            }
            Self::IndexOutOfRange {
                index,
                vertex_count,
            } => write!(
                formatter,
                "index {index} is past the last of {vertex_count} vertices"
            ),
            Self::NotFinite => write!(formatter, "a vertex value is not a finite number"),
        }
    }
}

// =============================================================================
// Framing
// =============================================================================

/// Encodes one frame: the prefix, the header as JSON, then the body.
pub fn encode_frame(header: &JsonValue, body: &[u8]) -> Vec<u8> {
    let header_bytes = header.to_string().into_bytes();
    let mut frame_bytes = Vec::with_capacity(FRAME_PREFIX_BYTES + header_bytes.len() + body.len());
    frame_bytes.extend_from_slice(&(header_bytes.len() as u32).to_le_bytes());
    frame_bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame_bytes.extend_from_slice(&header_bytes);
    frame_bytes.extend_from_slice(body);
    frame_bytes
}

/// Cuts the first complete frame off the front of `receive_buffer`.
///
/// Returns `Ok(None)` while the buffer holds only part of a frame, and leaves
/// that part in place for the next read.
///
/// # Errors
///
/// Returns [`FrameDecodeError::HeaderTooLarge`] or
/// [`FrameDecodeError::BodyTooLarge`] when the prefix announces more than the
/// link accepts, and [`FrameDecodeError::HeaderNotJson`] when a complete
/// header does not parse as a JSON object.
pub fn take_frame(receive_buffer: &mut Vec<u8>) -> Result<Option<LinkFrame>, FrameDecodeError> {
    // Step 1: read the prefix, refusing lengths no honest peer sends.
    if receive_buffer.len() < FRAME_PREFIX_BYTES {
        return Ok(None);
    }
    let header_byte_count = u32::from_le_bytes([
        receive_buffer[0],
        receive_buffer[1],
        receive_buffer[2],
        receive_buffer[3],
    ]) as usize;
    let body_byte_count = u32::from_le_bytes([
        receive_buffer[4],
        receive_buffer[5],
        receive_buffer[6],
        receive_buffer[7],
    ]) as usize;
    if header_byte_count > MAXIMUM_HEADER_BYTES {
        return Err(FrameDecodeError::HeaderTooLarge(header_byte_count));
    }
    if body_byte_count > MAXIMUM_BODY_BYTES {
        return Err(FrameDecodeError::BodyTooLarge(body_byte_count));
    }
    // Step 2: wait for the whole frame, then cut it off the buffer.
    let frame_byte_count = FRAME_PREFIX_BYTES + header_byte_count + body_byte_count;
    if receive_buffer.len() < frame_byte_count {
        return Ok(None);
    }
    let header_end = FRAME_PREFIX_BYTES + header_byte_count;
    let header: JsonValue = serde_json::from_slice(&receive_buffer[FRAME_PREFIX_BYTES..header_end])
        .map_err(|error| FrameDecodeError::HeaderNotJson(error.to_string()))?;
    if !header.is_object() {
        return Err(FrameDecodeError::HeaderNotJson(
            "the header is not an object".to_owned(),
        ));
    }
    let body = receive_buffer[header_end..frame_byte_count].to_vec();
    receive_buffer.drain(..frame_byte_count);
    Ok(Some(LinkFrame { header, body }))
}

// =============================================================================
// Geometry
// =============================================================================

/// Encodes a mesh's geometry as a body: positions, texture coordinates,
/// normals, then indices. The tangents are left out; the receiver derives
/// them.
pub fn encode_mesh_geometry(mesh: &Mesh) -> Vec<u8> {
    let mut body =
        Vec::with_capacity(mesh.vertices.len() * VERTEX_BODY_BYTES + mesh.indices.len() * 4);
    let push_floats = |floats: &[f32], body: &mut Vec<u8>| {
        for float in floats {
            body.extend_from_slice(&float.to_le_bytes());
        }
    };
    for vertex in &mesh.vertices {
        push_floats(&vertex.position, &mut body);
    }
    for vertex in &mesh.vertices {
        push_floats(&vertex.texture_coordinates, &mut body);
    }
    for vertex in &mesh.vertices {
        push_floats(&vertex.normal, &mut body);
    }
    for index in &mesh.indices {
        body.extend_from_slice(&index.to_le_bytes());
    }
    body
}

/// Decodes a geometry body into vertices and indices ready for a [`Mesh`].
///
/// The tangent and bitangent of each vertex are zero; the caller derives them
/// with [`Mesh::calculate_tangents`] once the geometry is in place.
///
/// # Errors
///
/// Returns [`GeometryDecodeError::LengthMismatch`] when the body's length
/// does not match the counts, [`GeometryDecodeError::NotTriangles`] when the
/// index count is zero or not a multiple of three,
/// [`GeometryDecodeError::IndexOutOfRange`] when an index points past the last
/// vertex, and [`GeometryDecodeError::NotFinite`] when a vertex value is NaN
/// or infinite.
pub fn decode_mesh_geometry(
    body: &[u8],
    vertex_count: usize,
    index_count: usize,
) -> Result<(Vec<MeshVertex>, Vec<u32>), GeometryDecodeError> {
    // Step 1: check the shape before reading a single value.
    let expected_bytes = vertex_count * VERTEX_BODY_BYTES + index_count * 4;
    if body.len() != expected_bytes {
        return Err(GeometryDecodeError::LengthMismatch {
            expected_bytes,
            actual_bytes: body.len(),
        });
    }
    if index_count == 0 || !index_count.is_multiple_of(3) {
        return Err(GeometryDecodeError::NotTriangles(index_count));
    }
    // Step 2: read the three vertex arrays and the index array in order.
    let mut words = body
        .chunks_exact(4)
        .map(|word| [word[0], word[1], word[2], word[3]]);
    let mut next_float = || f32::from_le_bytes(words.next().unwrap_or_default());
    let mut vertices = Vec::with_capacity(vertex_count);
    for _ in 0..vertex_count {
        vertices.push(MeshVertex {
            position: [next_float(), next_float(), next_float()],
            texture_coordinates: [0.0; 2],
            normal: [0.0; 3],
            tangent: [0.0; 3],
            bitangent: [0.0; 3],
        });
    }
    for vertex in &mut vertices {
        vertex.texture_coordinates = [next_float(), next_float()];
    }
    for vertex in &mut vertices {
        vertex.normal = [next_float(), next_float(), next_float()];
    }
    let index_bytes = &body[vertex_count * VERTEX_BODY_BYTES..];
    let indices: Vec<u32> = index_bytes
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    // Step 3: refuse what would break the tangent pass or the renderer.
    let all_finite = vertices.iter().all(|vertex| {
        vertex
            .position
            .iter()
            .chain(&vertex.texture_coordinates)
            .chain(&vertex.normal)
            .all(|component| component.is_finite())
    });
    if !all_finite {
        return Err(GeometryDecodeError::NotFinite);
    }
    if let Some(index) = indices
        .iter()
        .copied()
        .find(|index| *index as usize >= vertex_count)
    {
        return Err(GeometryDecodeError::IndexOutOfRange {
            index,
            vertex_count,
        });
    }
    Ok((vertices, indices))
}

/// Encodes a mesh's geometry as a self-contained payload: the vertex count
/// and the index count (little-endian `u32`), then the geometry body.
pub fn encode_geometry_payload(mesh: &Mesh) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(mesh.vertices.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(mesh.indices.len() as u32).to_le_bytes());
    payload.extend(encode_mesh_geometry(mesh));
    payload
}

/// Decodes a payload [`encode_geometry_payload`] wrote.
///
/// # Errors
///
/// Returns [`GeometryDecodeError::LengthMismatch`] when the payload is too
/// short to hold its counts, and every error [`decode_mesh_geometry`] returns
/// for the body.
pub fn decode_geometry_payload(
    payload: &[u8],
) -> Result<(Vec<MeshVertex>, Vec<u32>), GeometryDecodeError> {
    if payload.len() < 8 {
        return Err(GeometryDecodeError::LengthMismatch {
            expected_bytes: 8,
            actual_bytes: payload.len(),
        });
    }
    let vertex_count = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let index_count = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    decode_mesh_geometry(&payload[8..], vertex_count as usize, index_count as usize)
}

/// Fingerprints a mesh's geometry - what its triangles draw - independently
/// of how the vertices and triangles are ordered.
///
/// Each triangle is hashed from its three corners (position, texture
/// coordinates and normal, as exact bits) in sorted order, and the triangle
/// hashes are summed. The same surface written in another vertex order - the
/// OBJ importer's, or Blender's - therefore has the same fingerprint, so a
/// re-sent or re-imported mesh is not taken for an edit. The tangents are
/// left out because they are derived from the rest.
pub fn geometry_fingerprint(mesh: &Mesh) -> u64 {
    let corner_hash = |index: u32| {
        let vertex = &mesh.vertices[index as usize];
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for component in vertex
            .position
            .iter()
            .chain(&vertex.texture_coordinates)
            .chain(&vertex.normal)
        {
            // `-0.0` and `0.0` draw the same, so they hash the same.
            (component + 0.0).to_bits().hash(&mut hasher);
        }
        hasher.finish()
    };
    let mut fingerprint = mesh.indices.len() as u64;
    for triangle in mesh.indices.chunks_exact(3) {
        let mut corners = [
            corner_hash(triangle[0]),
            corner_hash(triangle[1]),
            corner_hash(triangle[2]),
        ];
        corners.sort_unstable();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        corners.hash(&mut hasher);
        fingerprint = fingerprint.wrapping_add(hasher.finish());
    }
    fingerprint
}

/// Returns whether two meshes hold the same vertices and indices in the same
/// order, which is what a positions-only update needs to be applied to.
pub fn same_vertex_order(first: &Mesh, second: &Mesh) -> bool {
    encode_mesh_geometry(first) == encode_mesh_geometry(second)
}

/// The new positions and normals a `mesh_positions` update carries, one of
/// each per vertex, in the mesh's vertex order.
pub type PositionsAndNormals = (Vec<[f32; 3]>, Vec<[f32; 3]>);

/// Decodes a `mesh_positions` body: the new positions (`3 * f32` per vertex),
/// then the new normals (`3 * f32` per vertex), for a mesh whose vertex order,
/// texture coordinates and indices are unchanged.
///
/// # Errors
///
/// Returns [`GeometryDecodeError::LengthMismatch`] when the body does not
/// hold exactly `vertex_count` positions and normals, and
/// [`GeometryDecodeError::NotFinite`] when a value is NaN or infinite.
pub fn decode_mesh_positions(
    body: &[u8],
    vertex_count: usize,
) -> Result<PositionsAndNormals, GeometryDecodeError> {
    let expected_bytes = vertex_count * 6 * 4;
    if body.len() != expected_bytes {
        return Err(GeometryDecodeError::LengthMismatch {
            expected_bytes,
            actual_bytes: body.len(),
        });
    }
    let floats: Vec<f32> = body
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect();
    if !floats.iter().all(|value| value.is_finite()) {
        return Err(GeometryDecodeError::NotFinite);
    }
    let triples = |values: &[f32]| -> Vec<[f32; 3]> {
        values
            .chunks_exact(3)
            .map(|triple| [triple[0], triple[1], triple[2]])
            .collect()
    };
    let (positions, normals) = floats.split_at(vertex_count * 3);
    Ok((triples(positions), triples(normals)))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame cut from a buffer that also holds part of the next one returns
    /// the first and keeps the rest.
    #[test]
    fn a_frame_round_trips_and_leaves_the_next_one_in_the_buffer() {
        let header = serde_json::json!({ "type": "hello", "protocol": 1 });
        let mut receive_buffer = encode_frame(&header, &[1, 2, 3]);
        receive_buffer.extend_from_slice(&[9, 0, 0]);
        let frame = take_frame(&mut receive_buffer).unwrap().unwrap();
        assert_eq!(frame.message_type(), "hello");
        assert_eq!(frame.header_number("protocol"), Some(1));
        assert_eq!(frame.body, vec![1, 2, 3]);
        assert_eq!(receive_buffer, vec![9, 0, 0]);
        assert_eq!(take_frame(&mut receive_buffer), Ok(None));
    }

    #[test]
    fn an_oversized_header_is_refused() {
        let mut receive_buffer = Vec::new();
        receive_buffer.extend_from_slice(&(MAXIMUM_HEADER_BYTES as u32 + 1).to_le_bytes());
        receive_buffer.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            take_frame(&mut receive_buffer),
            Err(FrameDecodeError::HeaderTooLarge(_))
        ));
    }

    #[test]
    fn geometry_round_trips_through_the_body() {
        let mesh = Mesh::triangle();
        let body = encode_mesh_geometry(&mesh);
        let (vertices, indices) =
            decode_mesh_geometry(&body, mesh.vertices.len(), mesh.indices.len()).unwrap();
        assert_eq!(indices, mesh.indices);
        for (decoded, original) in vertices.iter().zip(&mesh.vertices) {
            assert_eq!(decoded.position, original.position);
            assert_eq!(decoded.texture_coordinates, original.texture_coordinates);
            assert_eq!(decoded.normal, original.normal);
        }
    }

    #[test]
    fn a_geometry_payload_round_trips() {
        let mesh = Mesh::triangle();
        let (vertices, indices) = decode_geometry_payload(&encode_geometry_payload(&mesh)).unwrap();
        assert_eq!(vertices.len(), 3);
        assert_eq!(indices, mesh.indices);
    }

    /// The fingerprint follows the geometry and ignores the derived tangents.
    #[test]
    fn the_fingerprint_follows_the_geometry_only() {
        let mesh = Mesh::triangle();
        let mut retangented = mesh.clone();
        retangented.vertices[0].tangent = [0.0, 0.0, 1.0];
        assert_eq!(
            geometry_fingerprint(&mesh),
            geometry_fingerprint(&retangented)
        );
        let mut moved = mesh.clone();
        moved.vertices[0].position[0] += 1.0;
        assert_ne!(geometry_fingerprint(&mesh), geometry_fingerprint(&moved));
    }

    /// The same surface in another vertex order and another triangle start
    /// has the same fingerprint, but not the same vertex order.
    #[test]
    fn the_fingerprint_ignores_the_vertex_order() {
        let mesh = Mesh::triangle();
        let mut reordered = mesh.clone();
        reordered.vertices.swap(0, 2);
        reordered.indices = vec![1, 0, 2];
        assert_eq!(
            geometry_fingerprint(&mesh),
            geometry_fingerprint(&reordered)
        );
        assert!(!same_vertex_order(&mesh, &reordered));
        assert!(same_vertex_order(&mesh, &mesh.clone()));
    }

    #[test]
    fn a_positions_body_round_trips_and_is_checked() {
        let mut body = Vec::new();
        for value in [1.0f32, 2.0, 3.0, 0.0, 1.0, 0.0] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        let (positions, normals) = decode_mesh_positions(&body, 1).unwrap();
        assert_eq!(positions, vec![[1.0, 2.0, 3.0]]);
        assert_eq!(normals, vec![[0.0, 1.0, 0.0]]);
        assert!(decode_mesh_positions(&body, 2).is_err());
        body[0..4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(
            decode_mesh_positions(&body, 1),
            Err(GeometryDecodeError::NotFinite)
        );
    }

    #[test]
    fn geometry_with_an_index_past_the_vertices_is_refused() {
        let mut mesh = Mesh::triangle();
        mesh.indices = vec![0, 1, 3];
        let body = encode_mesh_geometry(&mesh);
        assert!(matches!(
            decode_mesh_geometry(&body, 3, 3),
            Err(GeometryDecodeError::IndexOutOfRange {
                index: 3,
                vertex_count: 3
            })
        ));
    }

    #[test]
    fn geometry_whose_counts_disagree_with_the_body_is_refused() {
        let body = encode_mesh_geometry(&Mesh::triangle());
        assert!(matches!(
            decode_mesh_geometry(&body, 4, 3),
            Err(GeometryDecodeError::LengthMismatch { .. })
        ));
    }
}
