use std::path::Path;
use std::path::PathBuf;



use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use cgmath::InnerSpace;
use pill_core::EngineError;
use tobj::LoadOptions;

use crate::resources::resource_map::Resource;

use crate::resources::resource_manager::ResourceHandle;
use anyhow::{Result, Context, Error};

#[derive(Clone, Copy)]
pub struct MeshHandle {
    pub index: u32,
}

impl ResourceHandle for MeshHandle
{
    fn get_index(&self) -> u32 {
        self.index
    }
}


pub struct Mesh {
    name: String,
    path: PathBuf,
    renderer_resource_index: u32,
    mesh_data: MeshData,
}

impl Mesh {
    // [TODO] What if renderer fails to create mesh?
    pub fn new(renderer: &mut Renderer, name: &str, path: PathBuf) -> Result<Self> {  
        let mesh_data = MeshData::new(&path)?; // read_obj_file(path);
        let renderer_resource_index = renderer.create_mesh(&mesh_data).unwrap();

        let mesh = Self { 
            name: name.to_string(),
            path,
            renderer_resource_index,
            mesh_data,
        };

        Ok(mesh)
    }
}

impl Resource for Mesh {
    type Storage = ResourceStorage<Mesh>; 
}


#[repr(C)]
// bytemuck::Pod indicates that Vertex is "Plain Old Data", and thus can be interpretted as a &[u8]
// bytemuck::Zeroable indicates that we can use std::mem::zeroed()
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    position: [f32; 3],
    texture_coordinates: [f32; 2],
    normal: [f32; 3],
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

pub struct MeshData {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
    pub elements_count: u32,
}

impl MeshData {
    pub fn new(path: &PathBuf) -> Result<Self> {  
        
        // Load model from path using tinyobjloader crate
        let load_options = LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        };

        // Load data
        let (models, materials) = tobj::load_obj(path.as_path(), &load_options)?;

        // [TODO] Check if file has any models
        // Check data validity
        if models.len() > 1 {
            return Err(Error::new(EngineError::InvalidModelFile(path.clone().into_os_string().into_string().unwrap())));
        }

        // Load vertex data from model
        let mesh = &models[0].mesh;

        // Read vertices
        let mut vertices = Vec::new();
        for i in 0..mesh.positions.len() / 3 {
            vertices.push(MeshVertex {
                position: [
                    mesh.positions[i * 3],
                    mesh.positions[i * 3 + 1],
                    mesh.positions[i * 3 + 2],
                ],
                texture_coordinates: [
                    mesh.texcoords[i * 2], 
                    mesh.texcoords[i * 2 + 1]
                ],
                normal: [
                    mesh.normals[i * 3],
                    mesh.normals[i * 3 + 1],
                    mesh.normals[i * 3 + 2],
                ],
                tangent: [0.0; 3].into(),
                bitangent: [0.0; 3].into(),
            });
        }

        // Read indices
        let indices = &mesh.indices;
        let mut triangles_included = (0..vertices.len()).collect::<Vec<_>>();

        // Calculate tangents and bitangets
        for c in indices.chunks(3) {
            let v0 = vertices[c[0] as usize];
            let v1 = vertices[c[1] as usize];
            let v2 = vertices[c[2] as usize];

            let pos0: cgmath::Vector3<_> = v0.position.into();
            let pos1: cgmath::Vector3<_> = v1.position.into();
            let pos2: cgmath::Vector3<_> = v2.position.into();

            let uv0: cgmath::Vector2<_> = v0.texture_coordinates.into();
            let uv1: cgmath::Vector2<_> = v1.texture_coordinates.into();
            let uv2: cgmath::Vector2<_> = v2.texture_coordinates.into();

            // Calculate the edges of the triangle
            let delta_pos1 = pos1 - pos0;
            let delta_pos2 = pos2 - pos0;

            // Calculate the direction needed to calculate the tangent and bitangent
            let delta_uv1 = uv1 - uv0;
            let delta_uv2 = uv2 - uv0;

            // Calculate tangent and bitangent       
            let r = 1.0 / (delta_uv1.x * delta_uv2.y - delta_uv1.y * delta_uv2.x);
            let tangent = (delta_pos1 * delta_uv2.y - delta_pos2 * delta_uv1.y) * r;
            let bitangent = (delta_pos2 * delta_uv1.x - delta_pos1 * delta_uv2.x) * r;

            // Assign same tangent/bitangent to each vertex in the triangle
            vertices[c[0] as usize].tangent = (tangent + cgmath::Vector3::from(vertices[c[0] as usize].tangent)).into();
            vertices[c[1] as usize].tangent = (tangent + cgmath::Vector3::from(vertices[c[1] as usize].tangent)).into();
            vertices[c[2] as usize].tangent = (tangent + cgmath::Vector3::from(vertices[c[2] as usize].tangent)).into();
            vertices[c[0] as usize].bitangent = (bitangent + cgmath::Vector3::from(vertices[c[0] as usize].bitangent)).into();
            vertices[c[1] as usize].bitangent = (bitangent + cgmath::Vector3::from(vertices[c[1] as usize].bitangent)).into();
            vertices[c[2] as usize].bitangent = (bitangent + cgmath::Vector3::from(vertices[c[2] as usize].bitangent)).into();

            // Prepare data for averaging tangents and bitangents
            triangles_included[c[0] as usize] += 1;
            triangles_included[c[1] as usize] += 1;
            triangles_included[c[2] as usize] += 1;
        }

        // Average the tangents and bitangents
        for (i, n) in triangles_included.into_iter().enumerate() {
            let denom = 1.0 / n as f32;
            let mut v = &mut vertices[i];
            v.tangent = (cgmath::Vector3::from(v.tangent) * denom).normalize().into();
            v.bitangent = (cgmath::Vector3::from(v.bitangent) * denom).normalize().into();
        }

        let mesh_data = MeshData {
            vertices: vertices,
            indices: mesh.indices.clone(),
            elements_count: mesh.indices.len() as u32,
        };

        Ok(mesh_data)
    }    
}






// pub struct Mesh {
//     vertices:
//     indices:
//     normals:
//     tangents:
//     bitangents:
//     texture_coordinates:
// }\


// pub struct MeshResource {
//     //submeshes: Vec<SubMesh>,
// }

// impl MeshResource {
//     // pub fn new() -> Self {
//     //     // Read data from file using importer
//     //     // For each submesh in file create submesh push it to vector of submeshes


//     // }

    
// }