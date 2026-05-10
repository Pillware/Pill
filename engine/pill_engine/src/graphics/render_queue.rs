use crate::{
    config::*,
    renderer::resources::{RendererMaterial, RendererMesh, RendererShader},
    resources::{
        Material, MaterialHandle, Mesh, MeshHandle, PBRMaterial, PBRMaterialHandle,
        ResourceManager, Shader,
    },
};

use pill_core::PillSlotMapKey;

use core::fmt;
use pill_core::Result;
use std::cmp::Ordering;

// --- Render queue item ---

#[derive(Clone, Copy)]
pub struct RenderQueueItem {
    pub key: RenderQueueKey,
    pub entity_index: u32,
    /// Raw transform copied (no trig) during the cache-warm sequential ECS scan;
    /// the vertex shader builds the model matrix on the GPU. xyz used, w padding. [Aaltonen GDC 2023]
    pub position: [f32; 4],
    pub rotation: [f32; 4],
    pub scale: [f32; 4],
}

impl Ord for RenderQueueItem {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for RenderQueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for RenderQueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RenderQueueItem {}

impl fmt::Display for RenderQueueItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {})", self.key, self.entity_index)
    }
}

impl fmt::Debug for RenderQueueItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {})", self.key, self.entity_index)
    }
}

// --- Render queue key type ---

pub type RenderQueueKey = crate::config::RenderQueueKeyType;

// --- Bit-field descriptors ---

pub struct RenderQueueFieldDesc {
    pub mask: RenderQueueKey,
    pub mask_shift: u32,
    pub max: RenderQueueKey,
}

const fn field(mask: u64, shift: u32, max: u64) -> RenderQueueFieldDesc {
    RenderQueueFieldDesc {
        mask,
        mask_shift: shift,
        max,
    }
}

pub const RENDER_QUEUE_KEY_ORDER: RenderQueueFieldDesc = field(0xF800_0000_0000_0000, 59, 31);
pub const RENDER_QUEUE_KEY_SHADER_INDEX: RenderQueueFieldDesc =
    field(0x07F8_0000_0000_0000, 51, 255);
pub const RENDER_QUEUE_KEY_SHADER_VERSION: RenderQueueFieldDesc =
    field(0x0007_F800_0000_0000, 43, 255);
pub const RENDER_QUEUE_KEY_MATERIAL_INDEX: RenderQueueFieldDesc =
    field(0x0000_07F8_0000_0000, 35, 255);
pub const RENDER_QUEUE_KEY_MATERIAL_VERSION: RenderQueueFieldDesc =
    field(0x0000_0007_F800_0000, 27, 255);
pub const RENDER_QUEUE_KEY_MESH_INDEX: RenderQueueFieldDesc = field(0x0000_0000_07F8_0000, 19, 255);
pub const RENDER_QUEUE_KEY_MESH_VERSION: RenderQueueFieldDesc =
    field(0x0000_0000_0007_F800, 11, 255);

// --- Compose ---

#[cfg_attr(feature = "headless", allow(unused_variables))]
pub fn compose_render_queue_key(
    resource_manager: &ResourceManager,
    material_handle: &MaterialHandle,
    mesh_handle: &MeshHandle,
) -> Result<RenderQueueKey> {
    #[cfg(feature = "headless")]
    return Ok(0);

    #[cfg(not(feature = "headless"))]
    {
        let material = resource_manager.get_resource::<Material>(material_handle)?;
        let shader = resource_manager.get_resource::<Shader>(&material.shader_handle)?;
        let mesh = resource_manager.get_resource::<Mesh>(mesh_handle)?;

        let renderer_shader_handle =
            resource_manager.get_resource_handle::<RendererShader>(&shader.name)?;
        let renderer_material_handle =
            resource_manager.get_resource_handle::<RendererMaterial>(&material.name)?;
        let renderer_mesh_handle =
            resource_manager.get_resource_handle::<RendererMesh>(&mesh.name)?;

        Ok(
            ((RENDER_QUEUE_KEY_ORDER.max - material.rendering_order as RenderQueueKey)
                << RENDER_QUEUE_KEY_ORDER.mask_shift)
                | ((renderer_shader_handle.data().index as RenderQueueKey)
                    << RENDER_QUEUE_KEY_SHADER_INDEX.mask_shift)
                | ((renderer_shader_handle.data().version.get() as RenderQueueKey)
                    << RENDER_QUEUE_KEY_SHADER_VERSION.mask_shift)
                | ((renderer_material_handle.data().index as RenderQueueKey)
                    << RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift)
                | ((renderer_material_handle.data().version.get() as RenderQueueKey)
                    << RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift)
                | ((renderer_mesh_handle.data().index as RenderQueueKey)
                    << RENDER_QUEUE_KEY_MESH_INDEX.mask_shift)
                | ((renderer_mesh_handle.data().version.get() as RenderQueueKey)
                    << RENDER_QUEUE_KEY_MESH_VERSION.mask_shift),
        )
    }
}

/// PBR path: PBRMaterial always uses default lit shader
#[cfg_attr(feature = "headless", allow(unused_variables))]
pub fn compose_pbr_render_queue_key(
    resource_manager: &ResourceManager,
    material_handle: PBRMaterialHandle,
    mesh_handle: &MeshHandle,
) -> Result<RenderQueueKey> {
    #[cfg(feature = "headless")]
    return Ok(0);

    #[cfg(not(feature = "headless"))]
    {
        let material = resource_manager.get_resource::<PBRMaterial>(&material_handle)?;
        let mesh = resource_manager.get_resource::<Mesh>(mesh_handle)?;

        let renderer_shader_handle =
            resource_manager.get_resource_handle::<RendererShader>(DEFAULT_LIT_SHADER_NAME)?;
        let renderer_material_handle =
            resource_manager.get_resource_handle::<RendererMaterial>(&material.name)?;
        let renderer_mesh_handle =
            resource_manager.get_resource_handle::<RendererMesh>(&mesh.name)?;

        Ok(((renderer_shader_handle.data().index as RenderQueueKey)
            << RENDER_QUEUE_KEY_SHADER_INDEX.mask_shift)
            | ((renderer_shader_handle.data().version.get() as RenderQueueKey)
                << RENDER_QUEUE_KEY_SHADER_VERSION.mask_shift)
            | ((renderer_material_handle.data().index as RenderQueueKey)
                << RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift)
            | ((renderer_material_handle.data().version.get() as RenderQueueKey)
                << RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift)
            | ((renderer_mesh_handle.data().index as RenderQueueKey)
                << RENDER_QUEUE_KEY_MESH_INDEX.mask_shift)
            | ((renderer_mesh_handle.data().version.get() as RenderQueueKey)
                << RENDER_QUEUE_KEY_MESH_VERSION.mask_shift))
    }
}

// --- Decompose ---

pub struct RenderQueueKeyFields {
    pub order: u8,
    pub shader_index: u8,
    pub shader_version: u8,
    pub material_index: u8,
    pub material_version: u8,
    pub mesh_index: u8,
    pub mesh_version: u8,
}

pub fn decompose_render_queue_key(key: RenderQueueKey) -> RenderQueueKeyFields {
    RenderQueueKeyFields {
        order: ((key & RENDER_QUEUE_KEY_ORDER.mask) >> RENDER_QUEUE_KEY_ORDER.mask_shift) as u8,
        shader_index: ((key & RENDER_QUEUE_KEY_SHADER_INDEX.mask)
            >> RENDER_QUEUE_KEY_SHADER_INDEX.mask_shift) as u8,
        shader_version: ((key & RENDER_QUEUE_KEY_SHADER_VERSION.mask)
            >> RENDER_QUEUE_KEY_SHADER_VERSION.mask_shift) as u8,
        material_index: ((key & RENDER_QUEUE_KEY_MATERIAL_INDEX.mask)
            >> RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift) as u8,
        material_version: ((key & RENDER_QUEUE_KEY_MATERIAL_VERSION.mask)
            >> RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift) as u8,
        mesh_index: ((key & RENDER_QUEUE_KEY_MESH_INDEX.mask)
            >> RENDER_QUEUE_KEY_MESH_INDEX.mask_shift) as u8,
        mesh_version: ((key & RENDER_QUEUE_KEY_MESH_VERSION.mask)
            >> RENDER_QUEUE_KEY_MESH_VERSION.mask_shift) as u8,
    }
}
