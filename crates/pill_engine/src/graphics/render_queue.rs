use core::fmt;
use std::{cmp::Ordering, fmt::Display, ops::{Range}, path::{Path, PathBuf}};
use std::{fmt::Binary, ops::{Add, Not, Shl, Sub}};

use core::fmt::Debug;
use anyhow::{Result, Context, Error};
use pill_core::PillSlotMapKey;
use crate::{ecs::ComponentStorage, game::{Engine, TransformComponent}, resources::{Material, MaterialHandle, Mesh, MeshData, MeshHandle, TextureHandle, TextureType}};
use crate::ecs::Scene;
use lazy_static::lazy_static;
use crate::resources::{ RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle, RendererTextureHandle };



// --- Render queue 


// --- Render queue item
pub struct RenderQueueItem {
    pub key: RenderQueueKey,
    pub entity_index: u32,
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

impl Eq for RenderQueueItem { }

impl Display for RenderQueueItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({} {})", self.key, self.entity_index)
    }
}

impl Debug for RenderQueueItem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({} {})", self.key, self.entity_index)
    }
}

// --- Render queue field 
struct RenderQueueField<T>  {
    mask_range: core::ops::Range<T>,
    mask_shift: T,
    mask: T,
}

impl<T> RenderQueueField<T> 
where
    T: Copy + Default + Binary + From<u8> + From<u32>  + Ord + Shl<Output = T> + Sub<Output = T> + Add<Output = T> + Not<Output = T>,
{
    pub fn new(mask_range: core::ops::Range<T>) -> Self { // Compile-time evaluable function
        let mask_size = T::from(std::mem::size_of::<T>() as u8 * 8);
        let mask_shift = mask_size - mask_range.end - T::from(1 as u32);
        let mask: T = pill_core::create_bitmask_from_range::<T>(&mask_range);

        RenderQueueField {
            mask_range,
            mask_shift,
            mask,
        }
    }
}


// Creates pill engine render queue composed from order, material index, material version, mesh index, mesh version
pub fn compose_render_queue_key(engine: &Engine, material_handle: &MaterialHandle, mesh_handle: &MeshHandle) -> Result<RenderQueueKey> { 
    let material = engine.resource_manager.get_resource::<MaterialHandle, Material>(material_handle)?;
    let mesh = engine.resource_manager.get_resource::<MeshHandle, Mesh>(mesh_handle)?;
   
    let render_queue_key: RenderQueueKey = 
        ((material.order as RenderQueueKey) << RENDERQUEUE_ORDER.mask_shift) | 
        ((material.renderer_resource_handle.unwrap().data().index as RenderQueueKey) << RENDERQUEUE_MATERIAL_INDEX.mask_shift) | 
        ((material.renderer_resource_handle.unwrap().data().version.get() as RenderQueueKey) << RENDERQUEUE_MATERIAL_VERSION.mask_shift) | 
        ((mesh.renderer_resource_handle.unwrap().data().index as RenderQueueKey) << RENDERQUEUE_MESH_INDEX.mask_shift ) | 
        ((mesh.renderer_resource_handle.unwrap().data().version.get() as RenderQueueKey) << RENDERQUEUE_MESH_VERSION.mask_shift);

    Ok(render_queue_key)
}

pub struct RenderQueueKeyFields {
    pub order: u8,
    pub material_index: u8,
    pub material_version: u8,
    pub mesh_index: u8,
    pub mesh_version: u8,
}

// Decomposes pill engine render queue key into separate fields
pub fn decompose_render_queue_key(render_queue_key: RenderQueueKey) -> Result<RenderQueueKeyFields> { 

    // [TODO] What if render queue key is not valid
    let order: u8 = ((render_queue_key & RENDERQUEUE_ORDER.mask as RenderQueueKey) >> RENDERQUEUE_ORDER.mask_shift as RenderQueueKey) as u8;
    let material_index: u8 = ((render_queue_key & RENDERQUEUE_MATERIAL_INDEX.mask) >> RENDERQUEUE_MATERIAL_INDEX.mask_shift) as u8;
    let material_version: u8 = ((render_queue_key & RENDERQUEUE_MATERIAL_VERSION.mask) >> RENDERQUEUE_MATERIAL_VERSION.mask_shift) as u8;
    let mesh_index: u8 = ((render_queue_key & RENDERQUEUE_MESH_INDEX.mask) >> RENDERQUEUE_MESH_INDEX.mask_shift) as u8;
    let mesh_version: u8 = ((render_queue_key & RENDERQUEUE_MESH_VERSION.mask) >> RENDERQUEUE_MESH_VERSION.mask_shift) as u8;

    let render_queue_key_fields = RenderQueueKeyFields {
        order,
        material_index,
        material_version,
        mesh_index,
        mesh_version,
    };

    Ok(render_queue_key_fields)
}

// --- Render queue fields config 
pub type RenderQueueKey = u64;

lazy_static! { // This will be initialized in runtime instead of compile-time (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
    static ref RENDERQUEUE_ORDER: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(0..4);
    static ref RENDERQUEUE_MATERIAL_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(5..10);
    static ref RENDERQUEUE_MATERIAL_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(11..18);
    static ref RENDERQUEUE_MESH_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(19..26);
    static ref RENDERQUEUE_MESH_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(27..34);
}




#[test]
fn new_render_queue_field_test() {
    let render_queue_field: RenderQueueField<u64> = RenderQueueField::<u64>::new(5..10);

    let expected_mask: u64 = 0b0000_0111_1110_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000;
    assert_eq!(render_queue_field.mask, expected_mask);

    let expected_mask_range: Range<u64> = 5..10;
    assert_eq!(render_queue_field.mask_range, expected_mask_range);

    let expected_mask_shift: u64 = 53;
    assert_eq!(render_queue_field.mask_shift, expected_mask_shift);
}

// #[test]
// fn compose_render_queue_key_test() {
//     let render_queue_key = compose_render_queue_key(engine: &Engine, material_handle: &MaterialHandle, mesh_handle: &MeshHandle).unwrap();
// }

#[test]
fn decompose_render_queue_key_test() {

    let order: u8 = 18;
    let material_index: u8 = 102;
    let material_version: u8 = 1;
    let mesh_index: u8 = 240;
    let mesh_version: u8 = 52;
    let mask: u64 = 0b10010_01100110_00000001_11110000_00110100_000000000000000000000000000;

    let render_queue_key_fields = decompose_render_queue_key(mask).unwrap();

    assert_eq!(render_queue_key_fields.order, order);
    assert_eq!(render_queue_key_fields.material_index, material_index);
    assert_eq!(render_queue_key_fields.material_version, material_version);
    assert_eq!(render_queue_key_fields.mesh_index, mesh_index);
    assert_eq!(render_queue_key_fields.mesh_version, mesh_version);
}