use crate::{
    ecs::MeshRenderingComponent,
    engine::Engine,
    graphics::RendererMeshHandle,
    resources::{Resource, ResourceStorage},
};

use pill_core::{
    get_type_name, EngineError, PillSlotMapKey, PillStyle, PillTypeMapKey, Vector2f, Vector3f,
};

use pill_core::{ErrorContext, Result};
use std::path::{Path, PathBuf};

#[cfg(feature = "obj_loading")]
fn obj_load_options() -> tobj::LoadOptions {
    tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ..Default::default()
    }
}

pill_core::define_new_pill_slotmap_key! {
    pub struct MeshHandle;
}

#[readonly::make]
pub struct Mesh {
    #[readonly]
    pub name: String,
    #[readonly]
    pub path: PathBuf,
    pub(crate) renderer_resource_handle: Option<RendererMeshHandle>,
    mesh_data: Option<MeshData>,

    // When exporting from Blender, V coordinate is flipped, so we need to flip it back
    // Should be set to false when importing a mesh exported as obj from Blender
    flip_uv_y: bool,
}

// TODO: Add posibility to load from bytes using ResourceLoader
impl Mesh {
    pub fn new(name: &str, path: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            path,
            renderer_resource_handle: None,
            mesh_data: None,
            flip_uv_y: false,
        }
    }

    pub fn with_uv_flip(mut self, flip: bool) -> Self {
        self.flip_uv_y = flip;
        self
    }

    pub fn from_data(name: &str, mesh_data: MeshData) -> Self {
        Self {
            name: name.to_string(),
            path: PathBuf::new(),
            renderer_resource_handle: None,
            mesh_data: Some(mesh_data),
            flip_uv_y: false,
        }
    }

    pub fn cube(name: &str, size: f32) -> Self {
        Self::from_data(name, MeshData::cube(size))
    }

    /// Build a mesh from pre-converted `.runtime_mesh` bytes (e.g. `include_bytes!(...)`).
    /// Works on all targets including wasm. Produces smaller binaries than
    /// `from_obj_bytes` — run `pill_launcher -a assets` to generate `.runtime_mesh` files.
    pub fn from_runtime_mesh_bytes(name: &str, bytes: &[u8]) -> Result<Self> {
        Ok(Self::from_data(name, load_runtime_mesh(bytes)?))
    }

    /// Parse a mesh from raw OBJ bytes; use `include_bytes!` to bundle assets into the binary (required on WASM).
    #[cfg(feature = "obj_loading")]
    pub fn from_obj_bytes(name: &str, bytes: &[u8]) -> Result<Self> {
        Ok(Self::from_data(
            name,
            MeshData::from_obj_bytes(bytes, false)?,
        ))
    }
}

impl PillTypeMapKey for Mesh {
    type Storage = ResourceStorage<Mesh>;
}

fn load_runtime_mesh(bytes: &[u8]) -> Result<MeshData> {
    if bytes.len() < 16 || &bytes[0..4] != b"RMSH" {
        return Err("not a valid .runtime_mesh file (bad magic or truncated header)".into());
    }
    let vertex_count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let index_count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let vertex_size = std::mem::size_of::<MeshVertex>();
    let expected = 16 + vertex_count * vertex_size + index_count * 4;
    if bytes.len() < expected {
        return Err(format!(
            ".runtime_mesh truncated: expected {} bytes, got {}",
            expected,
            bytes.len()
        )
        .into());
    }
    let vertex_bytes = &bytes[16..16 + vertex_count * vertex_size];
    let index_bytes = &bytes[16 + vertex_count * vertex_size..expected];
    // cast_slice requires the source pointer to be aligned to the target type.
    // include_bytes! data is only 1-byte aligned, so collect through Vec<u32>
    // (which allocates at 4-byte alignment) before casting to MeshVertex / u32.
    let vertex_u32s: Vec<u32> = vertex_bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    let vertices: Vec<MeshVertex> = bytemuck::cast_slice(&vertex_u32s).to_vec();
    let indices: Vec<u32> = index_bytes
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    Ok(MeshData { vertices, indices })
}

impl Resource for Mesh {
    type Handle = MeshHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!(
            "Initializing {} {} failed",
            "Resource".general_object_style(),
            get_type_name::<Self>().specific_object_style()
        );

        // Load from file only if mesh_data not already set (procedural mesh)
        if self.mesh_data.is_none() {
            let base = engine.game_resources_directory_path.join(&self.path);
            let runtime_mesh_path = base.with_extension("runtime_mesh");

            if runtime_mesh_path.exists() {
                let bytes =
                    std::fs::read(&runtime_mesh_path).map_err(|e| -> pill_core::PillError {
                        format!("Failed to read mesh {runtime_mesh_path:?}: {e}").into()
                    })?;
                self.mesh_data = Some(load_runtime_mesh(&bytes).context(error_message.clone())?);
            } else {
                #[cfg(feature = "obj_loading")]
                {
                    pill_core::validate_asset_path(&base, &["obj"])
                        .context(error_message.clone())?;
                    let mesh_data = MeshData::new(&base, self.flip_uv_y)
                        .context(error_message.clone())
                        .context(format!(
                            "Failed to create mesh data from {} file",
                            base.file_name().unwrap().to_string_lossy()
                        ))?;
                    self.mesh_data = Some(mesh_data);
                }
                #[cfg(not(feature = "obj_loading"))]
                return Err(pill_core::PillError::from(format!(
                    "No preprocessed .runtime_mesh found for {:?}; run `pill_launcher -a assets`",
                    base
                )));
            }
        }

        // Create new renderer mesh resource
        let renderer_resource_handle = engine
            .renderer
            .create_mesh(&self.name, self.mesh_data.as_ref().unwrap())
            .context(error_message.clone())?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_mesh(v).unwrap();
        }

        // Find mesh rendering components that use this mesh and update them
        for (_scene_handle, scene) in engine.scene_manager.scenes.iter_mut() {
            for (_entity_handle, mesh_rendering_component) in
                scene.get_one_component_iterator_mut::<MeshRenderingComponent>()?
            {
                if let Some(mesh_handle) = mesh_rendering_component.mesh_handle {
                    // If mesh rendering component has handle to this mesh
                    if mesh_handle.data() == self_handle.data() {
                        mesh_rendering_component.set_mesh_handle(Option::<MeshHandle>::None);
                        mesh_rendering_component
                            .update_render_queue_key(&engine.resource_manager)
                            .unwrap();
                    }
                }
            }
        }

        Ok(())
    }
}

#[repr(C)]
// bytemuck::Pod indicates that Vertex is "Plain Old Data", and thus can be interpretted as a &[u8]
// bytemuck::Zeroable indicates that Vertex can be used with std::mem::zeroed()
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
    #[cfg(feature = "obj_loading")]
    pub fn new(path: &Path, flip_uv_y: bool) -> Result<Self> {
        let (models, _materials) = tobj::load_obj(path, &obj_load_options())?;
        Self::from_tobj_models(models, &path.display().to_string(), flip_uv_y)
    }

    #[cfg(feature = "obj_loading")]
    pub fn from_obj_bytes(bytes: &[u8], flip_uv_y: bool) -> Result<Self> {
        let mut reader = std::io::Cursor::new(bytes);
        let (models, _materials) = tobj::load_obj_buf(&mut reader, &obj_load_options(), |_| {
            Err(tobj::LoadError::OpenFileFailed)
        })?;
        Self::from_tobj_models(models, "<in-memory>", flip_uv_y)
    }

    #[cfg(feature = "obj_loading")]
    fn from_tobj_models(models: Vec<tobj::Model>, source: &str, flip_uv_y: bool) -> Result<Self> {
        // Check data validity
        if models.len() > 1 {
            return Err(EngineError::InvalidModelFileMultipleMeshes(source.to_string()).into());
        }

        if models.is_empty() {
            return Err(EngineError::InvalidModelFile(source.to_string()).into());
        }

        // Load vertex data from model
        let mesh = &models[0].mesh;

        // Read vertices
        let mut vertices = Vec::new();
        for i in 0..mesh.positions.len() / 3 {
            let uv_y = *mesh.texcoords.get(i * 2 + 1).unwrap_or(&0.0);
            let final_uv_y = if flip_uv_y { uv_y } else { 1.0 - uv_y };

            vertices.push(MeshVertex {
                position: [
                    mesh.positions[i * 3],
                    mesh.positions[i * 3 + 1],
                    mesh.positions[i * 3 + 2],
                ],
                texture_coordinates: [
                    // Blender uses V coordinate flipped
                    *mesh.texcoords.get(i * 2).unwrap_or(&0.0),
                    final_uv_y,
                ],
                normal: [
                    mesh.normals[i * 3],
                    mesh.normals[i * 3 + 1],
                    mesh.normals[i * 3 + 2],
                ],
                tangent: [0.0; 3],
                bitangent: [0.0; 3],
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

            let pos0: Vector3f = v0.position.into();
            let pos1: Vector3f = v1.position.into();
            let pos2: Vector3f = v2.position.into();

            let uv0: Vector2f = v0.texture_coordinates.into();
            let uv1: Vector2f = v1.texture_coordinates.into();
            let uv2: Vector2f = v2.texture_coordinates.into();

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
            vertices[c[0] as usize].tangent =
                (tangent + Vector3f::from(vertices[c[0] as usize].tangent)).into();
            vertices[c[1] as usize].tangent =
                (tangent + Vector3f::from(vertices[c[1] as usize].tangent)).into();
            vertices[c[2] as usize].tangent =
                (tangent + Vector3f::from(vertices[c[2] as usize].tangent)).into();
            vertices[c[0] as usize].bitangent =
                (bitangent + Vector3f::from(vertices[c[0] as usize].bitangent)).into();
            vertices[c[1] as usize].bitangent =
                (bitangent + Vector3f::from(vertices[c[1] as usize].bitangent)).into();
            vertices[c[2] as usize].bitangent =
                (bitangent + Vector3f::from(vertices[c[2] as usize].bitangent)).into();

            // Prepare data for averaging tangents and bitangents
            triangles_included[c[0] as usize] += 1;
            triangles_included[c[1] as usize] += 1;
            triangles_included[c[2] as usize] += 1;
        }

        // Average the tangents and bitangents
        for (i, n) in triangles_included.into_iter().enumerate() {
            let denom = 1.0 / n as f32;
            let vertex = &mut vertices[i];
            vertex.tangent = (Vector3f::from(vertex.tangent) * denom).normalize().into();
            vertex.bitangent = (Vector3f::from(vertex.bitangent) * denom)
                .normalize()
                .into();
        }

        let mesh_data = MeshData {
            vertices,
            indices: mesh.indices.clone(),
        };

        Ok(mesh_data)
    }

    pub fn cube(size: f32) -> Self {
        let s = size / 2.0;

        // 6 faces, 4 vertices each = 24 vertices (each face has unique normals)
        let vertices = vec![
            // Front face (Z+)
            MeshVertex {
                position: [-s, -s, s],
                texture_coordinates: [0.0, 1.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, -s, s],
                texture_coordinates: [1.0, 1.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, s, s],
                texture_coordinates: [1.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, s, s],
                texture_coordinates: [0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            // Back face (Z-)
            MeshVertex {
                position: [s, -s, -s],
                texture_coordinates: [0.0, 1.0],
                normal: [0.0, 0.0, -1.0],
                tangent: [-1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, -s, -s],
                texture_coordinates: [1.0, 1.0],
                normal: [0.0, 0.0, -1.0],
                tangent: [-1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, s, -s],
                texture_coordinates: [1.0, 0.0],
                normal: [0.0, 0.0, -1.0],
                tangent: [-1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, s, -s],
                texture_coordinates: [0.0, 0.0],
                normal: [0.0, 0.0, -1.0],
                tangent: [-1.0, 0.0, 0.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            // Top face (Y+)
            MeshVertex {
                position: [-s, s, s],
                texture_coordinates: [0.0, 1.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, -1.0],
            },
            MeshVertex {
                position: [s, s, s],
                texture_coordinates: [1.0, 1.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, -1.0],
            },
            MeshVertex {
                position: [s, s, -s],
                texture_coordinates: [1.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, -1.0],
            },
            MeshVertex {
                position: [-s, s, -s],
                texture_coordinates: [0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, -1.0],
            },
            // Bottom face (Y-)
            MeshVertex {
                position: [-s, -s, -s],
                texture_coordinates: [0.0, 1.0],
                normal: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, 1.0],
            },
            MeshVertex {
                position: [s, -s, -s],
                texture_coordinates: [1.0, 1.0],
                normal: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, 1.0],
            },
            MeshVertex {
                position: [s, -s, s],
                texture_coordinates: [1.0, 0.0],
                normal: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, 1.0],
            },
            MeshVertex {
                position: [-s, -s, s],
                texture_coordinates: [0.0, 0.0],
                normal: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0],
                bitangent: [0.0, 0.0, 1.0],
            },
            // Right face (X+)
            MeshVertex {
                position: [s, -s, s],
                texture_coordinates: [0.0, 1.0],
                normal: [1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, -1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, -s, -s],
                texture_coordinates: [1.0, 1.0],
                normal: [1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, -1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, s, -s],
                texture_coordinates: [1.0, 0.0],
                normal: [1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, -1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [s, s, s],
                texture_coordinates: [0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, -1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            // Left face (X-)
            MeshVertex {
                position: [-s, -s, -s],
                texture_coordinates: [0.0, 1.0],
                normal: [-1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, 1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, -s, s],
                texture_coordinates: [1.0, 1.0],
                normal: [-1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, 1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, s, s],
                texture_coordinates: [1.0, 0.0],
                normal: [-1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, 1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
            MeshVertex {
                position: [-s, s, -s],
                texture_coordinates: [0.0, 0.0],
                normal: [-1.0, 0.0, 0.0],
                tangent: [0.0, 0.0, 1.0],
                bitangent: [0.0, 1.0, 0.0],
            },
        ];

        // Triangle-list index buffer: each face is two triangles (a,b,c) + (a,c,d)
        // over its 4 vertices, sharing the a-c diagonal — 6 indices per face, 36 total.
        // See: https://learnopengl.com/Getting-started/Hello-Triangle
        let indices = vec![
            0, 1, 2, 0, 2, 3, // front
            4, 5, 6, 4, 6, 7, // back
            8, 9, 10, 8, 10, 11, // top
            12, 13, 14, 12, 14, 15, // bottom
            16, 17, 18, 16, 18, 19, // right
            20, 21, 22, 20, 22, 23, // left
        ];

        MeshData { vertices, indices }
    }
}
