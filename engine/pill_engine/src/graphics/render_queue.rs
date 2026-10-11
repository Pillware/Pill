use crate::{
    renderer::resources::{RendererMaterial, RendererMesh},
    resources::{Material, MaterialHandle, Mesh, MeshHandle, ResourceManager},
};

use pill_core::PillSlotMapKey;

use pill_core::Result;

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

// Layout is documented in config.rs (RenderQueueKeyType)
pub const RENDER_QUEUE_KEY_MATERIAL_INDEX: RenderQueueFieldDesc =
    field(0xFF00_0000_0000_0000, 56, 255);
pub const RENDER_QUEUE_KEY_MATERIAL_VERSION: RenderQueueFieldDesc =
    field(0x00FF_0000_0000_0000, 48, 255);
pub const RENDER_QUEUE_KEY_MESH_INDEX: RenderQueueFieldDesc = field(0x0000_FF00_0000_0000, 40, 255);
pub const RENDER_QUEUE_KEY_MESH_VERSION: RenderQueueFieldDesc =
    field(0x0000_00FF_0000_0000, 32, 255);

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
        let mesh = resource_manager.get_resource::<Mesh>(mesh_handle)?;

        let renderer_material_handle =
            resource_manager.get_resource_handle::<RendererMaterial>(&material.name)?;
        let renderer_mesh_handle =
            resource_manager.get_resource_handle::<RendererMesh>(&mesh.name)?;

        Ok(((renderer_material_handle.data().index as RenderQueueKey)
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
    pub material_index: u8,
    pub material_version: u8,
    pub mesh_index: u8,
    pub mesh_version: u8,
}

pub fn decompose_render_queue_key(key: RenderQueueKey) -> RenderQueueKeyFields {
    RenderQueueKeyFields {
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
