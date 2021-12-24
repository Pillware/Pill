use crate::{ 
    resources::{
        ResourceManager,
        MaterialHandle, 
        TextureHandle, 
        Material,
        MeshHandle,
        Mesh,
    }, 
    config::*,
};

use pill_core::PillSlotMapKey;

use std::{
    cmp::Ordering,
    fmt::{Binary, Display},
    ops::{Add, Not, Shl, Sub, Range}, 
    convert::TryFrom,
    path::{Path, PathBuf}
};
use core::fmt::{Debug, self};
use anyhow::{Result, Context, Error};
use lazy_static::lazy_static;

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
pub struct RenderQueueField<T>  {
    mask_range: core::ops::Range<T>,
    mask_shift: T,
    mask: T,
    max: T, 
}



pub trait Pow {
    fn pow(self, exp: u32) -> Self;
}

impl<T> RenderQueueField<T> 
where
    T: Copy + Default + Pow + Binary + From<u8> + From<u32> + Ord + Shl<Output = T> + Sub<Output = T> + Add<Output = T> + Not<Output = T>,
{
    pub fn new(mask_range: core::ops::Range<T>) -> Self { // Compile-time evaluable function
        let one: T = T::from(1 as u8);
        let two: T = T::from(2 as u8);
        let mask_size: T = T::from(std::mem::size_of::<T>() as u8 * 8);
        let mask_shift: T = mask_size - mask_range.end - one;
        let mask: T = pill_core::create_bitmask_from_range::<T>(&mask_range);
        let max: T = two.pow(u32::try_from(3).unwrap()) - one;

        RenderQueueField {
            mask_range,
            mask_shift,
            mask,
            max,
        }
    }
}

// Creates pill engine render queue composed from order, material index, material version, mesh index, mesh version
pub fn compose_render_queue_key(resource_manager: &ResourceManager, material_handle: &MaterialHandle, mesh_handle: &MeshHandle) -> Result<RenderQueueKey> { 
    let material = resource_manager.get_resource::<Material>(material_handle)?;
    let mesh = resource_manager.get_resource::<Mesh>(mesh_handle)?;
   
    let render_queue_key: RenderQueueKey = 
        ((RENDER_QUEUE_KEY_ORDER.max - material.order as RenderQueueKey) << RENDER_QUEUE_KEY_ORDER.mask_shift) | // Order has to be inverted for proper sorting
        ((material.renderer_resource_handle.unwrap().data().index as RenderQueueKey) << RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift) | 
        ((material.renderer_resource_handle.unwrap().data().version.get() as RenderQueueKey) << RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift) | 
        ((mesh.renderer_resource_handle.unwrap().data().index as RenderQueueKey) << RENDER_QUEUE_KEY_MESH_INDEX.mask_shift ) | 
        ((mesh.renderer_resource_handle.unwrap().data().version.get() as RenderQueueKey) << RENDER_QUEUE_KEY_MESH_VERSION.mask_shift);

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
    let order: u8 = ((render_queue_key & RENDER_QUEUE_KEY_ORDER.mask as RenderQueueKey) >> RENDER_QUEUE_KEY_ORDER.mask_shift as RenderQueueKey) as u8;
    let material_index: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MATERIAL_INDEX.mask) >> RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift) as u8;
    let material_version: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MATERIAL_VERSION.mask) >> RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift) as u8;
    let mesh_index: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MESH_INDEX.mask) >> RENDER_QUEUE_KEY_MESH_INDEX.mask_shift) as u8;
    let mesh_version: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MESH_VERSION.mask) >> RENDER_QUEUE_KEY_MESH_VERSION.mask_shift) as u8;

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
pub type RenderQueueKey = crate::config::RenderQueueKeyType;

impl Pow for RenderQueueKey {
    fn pow(self, exp: u32) -> Self { RenderQueueKey::pow(self, exp) }
}

fn get_render_queue_key_item_range(render_queue_item_index: u8) -> Range<RenderQueueKey> {
    let mut start: RenderQueueKey = 0;
    let mut end: RenderQueueKey = 0;
    for i in 0..render_queue_item_index + 1
    {
        start += i.ne(&0).then(|| RENDER_QUEUE_KEY_ITEMS_LENGTH[i as usize] - 1).unwrap_or(0);   
        end += RENDER_QUEUE_KEY_ITEMS_LENGTH[i as usize];
    }
    start..(end - 1)
}

lazy_static! { // This will be initialized in runtime instead of compile-time (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
    pub static ref RENDER_QUEUE_KEY_ORDER: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_ORDER_IDX));
    pub static ref RENDER_QUEUE_KEY_MATERIAL_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MATERIAL_INDEX_IDX));
    pub static ref RENDER_QUEUE_KEY_MATERIAL_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MATERIAL_VERSION_IDX));
    pub static ref RENDER_QUEUE_KEY_MESH_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MESH_INDEX_IDX));
    pub static ref RENDER_QUEUE_KEY_MESH_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<u64>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MESH_VERSION_IDX));
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