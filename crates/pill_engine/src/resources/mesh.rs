use crate::{
    engine::Engine,
    graphics::{ RendererMeshHandle }, 
    resources::{ ResourceStorage, Resource },
    ecs::{ DeferredUpdateManagerPointer, MeshRenderingComponent },
    config::*,
};

use pill_core::{ EngineError, PillSlotMapKey, PillTypeMap, PillTypeMapKey, Vector3f, PillStyle, get_type_name };

use std::path::{ Path, PathBuf };
use boolinator::Boolinator;
use cgmath::InnerSpace;
use tobj::LoadOptions;
use anyhow::{Result, Context, Error};


pill_core::define_new_pill_slotmap_key! { 
    pub struct MeshHandle;
}

#[readonly::make]
pub struct Mesh {
    #[readonly]
    pub name: String,
    #[readonly]
    path: PathBuf,
    pub(crate) renderer_resource_handle: Option<RendererMeshHandle>,
    mesh_data: Option<MeshData>,
}

impl Mesh {
    pub fn new(name: &str, path: PathBuf) -> Self {  
        Self { 
            name: name.to_string(),
            path,
            renderer_resource_handle: None,
            mesh_data: None,
        }
    }
}

impl PillTypeMapKey for Mesh {
    type Storage = ResourceStorage<Mesh>; 
}

impl Resource for Mesh {
    type Handle = MeshHandle;

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { // [TODO] What if renderer fails to create mesh?
        let error_message = format!("Initializing {} {} failed", "Resource".gobj_style(), get_type_name::<Self>().sobj_style());
        
        // Check if path to asset is correct
        pill_core::validate_asset_path(&self.path, "obj").context(error_message.clone())?;

        // Create mesh data
        let mesh_data = MeshData::new(&self.path).context(error_message.clone())?;
        self.mesh_data = Some(mesh_data);
  
        // Create new renderer mesh resource
        let renderer_resource_handle = engine.renderer.create_mesh(&self.name, &self.mesh_data.as_ref().unwrap()).context(error_message.clone())?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {

        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_mesh(v).unwrap();
        }

        // Find mesh rendering components that use this mesh and update them
        for scene in engine.scene_manager.scenes.iter() {
            for mesh_rendering_component_slot in engine.scene_manager.fetch_one_component_storage::<MeshRenderingComponent>(scene.0)? {
                if let Some(mesh_rendering_component) = mesh_rendering_component_slot.borrow_mut().as_mut() {
                    if let Some(mesh_handle) = mesh_rendering_component.mesh_handle {
                        // If mesh rendering component has handle to this mesh 
                        if mesh_handle.data() == self_handle.data() {
                            mesh_rendering_component.mesh_handle = None;
                            mesh_rendering_component.update_render_queue_key(&engine.resource_manager).unwrap();
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
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
        let (models, _materials) = tobj::load_obj(path.as_path(), &load_options)?;

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
            v.tangent = (Vector3f::from(v.tangent) * denom).normalize().into();
            v.bitangent = (Vector3f::from(v.bitangent) * denom).normalize().into();
        }

        let mesh_data = MeshData {
            vertices: vertices,
            indices: mesh.indices.clone(),
        };

        Ok(mesh_data)
    }    
}





