//! The GPU side of a mesh: the vertex and index buffers one uploaded mesh is
//! drawn from.
//!
//! # Responsibilities
//!
//! - State the vertex buffer layout contract each vertex type implements, so a
//!   pipeline can describe its buffers without touching the mesh data
//!   ([`Vertex`]).
//! - Upload a decoded [`Mesh`](crate::assets::Mesh) into the vertex and index
//!   buffers one draw needs, refusing a mesh that carries no geometry
//!   ([`RendererMesh`]).
//!
//! # Design
//!
//! The layout descriptor is built from the fields of the interleaved
//! [`MeshVertex`](crate::assets::MeshVertex), not configured per mesh, so the
//! descriptor and the uploaded bytes cannot drift apart. A mesh pipeline pairs
//! it with the per-instance layout ([`Instance`](crate::Instance)). The two
//! layouts share one set of shader locations: the mesh attributes take 0 and 4
//! through 7, the instance attributes take 1 through 3.

// Standard library
use std::mem;

// External crates
use wgpu::util::DeviceExt;

// Current crate
use crate::{
    assets::{Mesh, MeshVertex},
    error::{RendererError, Result},
};

// =============================================================================
// Handle
// =============================================================================

pill_core::define_slot_key!(RendererMeshHandle);

// =============================================================================
// Vertex
// =============================================================================

/// A type that can be uploaded to a vertex buffer, together with the layout
/// the render pipeline uses to read it.
///
/// [`RendererMesh`] describes the buffer of a mesh's vertices and
/// [`Instance`](crate::Instance) the buffer of its instances. A geometry pass
/// takes both layouts, and each shader location belongs to exactly one of
/// them.
pub trait Vertex {
    /// Returns the buffer layout the render pipeline maps this type through.
    ///
    /// The descriptor gives the stride between vertices and where each
    /// attribute sits inside it. It has to match the bytes uploaded into the
    /// buffer and the shader locations the vertex shader declares.
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a>;
}

// =============================================================================
// Mesh
// =============================================================================

/// One mesh's GPU buffers: the vertex data and the index list that draws it.
///
/// The renderer builds one while it syncs mesh assets, when a mesh asset first
/// arrives or its version changes, and keeps it in the resource storage beside
/// every other GPU resource. A draw looks the mesh up by handle every frame,
/// so a rebuilt mesh is used without invalidating any pipeline.
pub struct RendererMesh {
    /// The mesh's name from its asset, used in logs and error messages.
    pub name: String,
    /// The vertex data, bound at slot 0 by every pipeline that draws this mesh.
    pub vertex_buffer: wgpu::Buffer,
    /// The indices into the vertex data, stored as `Uint32`.
    pub index_buffer: wgpu::Buffer,
    /// How many indices a draw of this mesh passes, taken from the asset.
    pub index_count: u32,
}

impl RendererMesh {
    /// Uploads a mesh's vertices and indices, ready to draw.
    ///
    /// `name` is the mesh's name. It labels the GPU buffers and the error
    /// message.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Other`] naming the mesh when it carries no
    /// vertices or no indices. wgpu will not create a zero-sized buffer, so the
    /// empty case has to be refused here. A mesh with nothing in it is a
    /// mistake in the asset, and naming it helps more than an upload failure
    /// buried in the log.
    pub fn new(device: &wgpu::Device, name: &str, mesh: &Mesh) -> Result<Self> {
        // Step 1: Refuse a mesh that has nothing to upload.
        if mesh.vertices.is_empty() || mesh.indices.is_empty() {
            return Err(RendererError::Other {
                detail: format!("mesh `{name}` has no vertices or no indices"),
            });
        }

        // Step 2: Upload the vertices and the indices into their own buffers.
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_vertex_buffer", name)),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_index_buffer", name)),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Step 3: Keep the buffers, the name and the index count for the draw.
        let renderer_mesh = Self {
            name: name.to_string(),
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
        };

        Ok(renderer_mesh)
    }
}

impl Vertex for RendererMesh {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    // Vertex position
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex texture coordinates
                    // slangc maps TEXCOORD0 → @location(4), not 1
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    // Vertex normal
                    // slangc maps NORMAL → @location(5)
                    offset: mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex tangent
                    // slangc maps TANGENT → @location(6)
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex bitangent
                    // slangc maps BINORMAL → @location(7)
                    offset: mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}
