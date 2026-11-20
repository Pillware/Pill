//! The renderer's own resources.
//!
//! # Responsibilities
//!
//! - One module per resource the renderer keeps - shaders, materials, meshes,
//!   textures, cameras and the engine parameters. The folder also holds the GPU
//!   side of a pass, the storage that owns every handle's slot map, and the
//!   bind-group builder a pass and a material share.
//! - Define each resource's handle beside the resource itself, so a handle and
//!   the type it stores are changed together.
//! - Re-export the resources flat, so the renderer and the `drawers` module
//!   reach a resource by name rather than by path. The data crate's
//!   `RenderingManager` - the pipeline a game picks - is re-exported here too.

mod engine_parameters;
mod renderer_camera;
mod renderer_material;
mod renderer_mesh;
mod renderer_pass;
mod renderer_resource_storage;
mod renderer_shader;
mod renderer_texture;
mod shader_bindings;

// --- Use ---

pub use engine_parameters::EngineParameters;
pub use renderer_camera::RendererCamera;
pub use renderer_material::RendererMaterial;
pub use renderer_mesh::{RendererMesh, Vertex};
pub use renderer_pass::RendererPass;
pub use renderer_resource_storage::RendererResourceStorage;
pub use renderer_shader::{RendererShader, UniformGroups};
pub use renderer_texture::RendererTexture;

// The handles are keys into the renderer's own slot maps, and `check_slot_shape`
// checks that a texture's shape matches the slot it binds to. Both stay
// internal: reachable anywhere in the crate, exported nowhere.
pub(crate) use renderer_camera::RendererCameraHandle;
pub(crate) use renderer_material::RendererMaterialHandle;
pub(crate) use renderer_mesh::RendererMeshHandle;
pub(crate) use renderer_shader::check_slot_shape;
pub(crate) use renderer_shader::RendererShaderHandle;
pub(crate) use renderer_texture::RendererTextureHandle;

pub use pill_master_renderer_data::RenderingManager;
