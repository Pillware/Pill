use crate::{
    config::*,
    resources::{Mesh, MeshHandle, PBRMaterial, PBRMaterialHandle, ResourceManager, TextureHandle},
};

use pill_core::Handle;

use anyhow::{Context, Error, Result};
use core::fmt::{self, Debug};
use lazy_static::lazy_static;
use std::{
    cmp::Ordering,
    convert::{TryFrom, TryInto},
    fmt::{Binary, Display},
    ops::{Add, Not, Range, Shl, Sub},
    path::{Path, PathBuf},
};

// --- Render queue

pub struct RenderQueue {
    pub items: Vec<RenderQueueItem>,
    pub context_change_indices: Vec<u32>, // Indices of items after which context changes (e.g. material or mesh change)
}

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

impl Eq for RenderQueueItem {}

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

// --- Render queue field ---

pub struct RenderQueueField<T> {
    pub mask_range: core::ops::Range<T>,
    pub mask_shift: T,
    pub mask: T,
    pub max: T,
}

pub trait Pow {
    fn pow(self, exp: Self) -> Self;
}

impl<T> RenderQueueField<T>
where
    T: Copy
        + Default
        + Pow
        + Binary
        + Debug
        + From<u8>
        + From<u32>
        + Ord
        + Shl<Output = T>
        + Sub<Output = T>
        + Add<Output = T>
        + Not<Output = T>,
{
    pub fn new(mask_range: core::ops::Range<T>) -> Self {
        // Compile-time evaluable function
        let one: T = T::from(1 as u8);
        let two: T = T::from(2 as u8);
        let mask_range_length = mask_range.end - mask_range.start + one; //if mask_range.start == zero { mask_range.end + one } else { mask_range.end - mask_range.start };
        let mask_size: T = T::from(std::mem::size_of::<T>() as u8 * 8);
        let mask_shift: T = mask_size - mask_range.end - one;
        let mask: T = pill_core::create_bitmask_from_range::<T>(&mask_range);
        let max: T = two.pow(mask_range_length) - one;

        RenderQueueField {
            mask_range,
            mask_shift,
            mask,
            max,
        }
    }
}

// Creates pill engine render queue composed from order, material index, material version, mesh index, mesh version
pub fn compose_render_queue_key(
    resource_manager: &ResourceManager,
    mesh_handle: &MeshHandle,
    material_handle: &Option<PBRMaterialHandle>,
) -> Result<RenderQueueKey> {
    let mesh = resource_manager.get_resource::<Mesh>(mesh_handle)?;

    let h_mesh = mesh.renderer_resource_handle.unwrap();
    // Material (optional)
    let (mat_index, mat_gen) = if let Some(mh) = material_handle {
        if let Ok(mat) = resource_manager.get_resource::<PBRMaterial>(mh) {
            if let Some(rh) = mat.renderer_resource_handle {
                (
                    rh.index() as RenderQueueKey,
                    rh.generation() as RenderQueueKey,
                )
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    let render_queue_key: RenderQueueKey = ((h_mesh.index() as RenderQueueKey)
        << RENDER_QUEUE_KEY_MESH_INDEX.mask_shift)
        | ((h_mesh.generation() as RenderQueueKey) << RENDER_QUEUE_KEY_MESH_VERSION.mask_shift)
        | (mat_index << RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift)
        | (mat_gen << RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift);

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
pub fn decompose_render_queue_key(render_queue_key: RenderQueueKey) -> RenderQueueKeyFields {
    // [TODO] What if render queue key is not valid
    let order: u8 = ((render_queue_key & RENDER_QUEUE_KEY_ORDER.mask as RenderQueueKey)
        >> RENDER_QUEUE_KEY_ORDER.mask_shift as RenderQueueKey) as u8;
    let material_index: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MATERIAL_INDEX.mask)
        >> RENDER_QUEUE_KEY_MATERIAL_INDEX.mask_shift) as u8;
    let material_version: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MATERIAL_VERSION.mask)
        >> RENDER_QUEUE_KEY_MATERIAL_VERSION.mask_shift) as u8;
    let mesh_index: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MESH_INDEX.mask)
        >> RENDER_QUEUE_KEY_MESH_INDEX.mask_shift) as u8;
    let mesh_version: u8 = ((render_queue_key & RENDER_QUEUE_KEY_MESH_VERSION.mask)
        >> RENDER_QUEUE_KEY_MESH_VERSION.mask_shift) as u8;

    RenderQueueKeyFields {
        order,
        material_index,
        material_version,
        mesh_index,
        mesh_version,
    }
}

// --- Render queue fields config ---

pub type RenderQueueKey = crate::config::RenderQueueKeyType;

impl Pow for RenderQueueKey {
    fn pow(self, exp: Self) -> Self {
        RenderQueueKey::pow(self, exp.try_into().unwrap())
    }
}

fn get_render_queue_key_item_range(render_queue_item_index: u8) -> Range<RenderQueueKey> {
    let mut start: RenderQueueKey = 0;
    let mut end: RenderQueueKey = 0;
    for i in 0..render_queue_item_index + 1 {
        start += i
            .ne(&0)
            .then(|| RENDER_QUEUE_KEY_ITEMS_LENGTH[i as usize - 1])
            .unwrap_or(0);
        end += RENDER_QUEUE_KEY_ITEMS_LENGTH[i as usize];
    }
    start..(end - 1)
}

lazy_static! { // This will be initialized in runtime instead of compile-time (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
    pub static ref RENDER_QUEUE_KEY_ORDER: RenderQueueField<RenderQueueKey> = RenderQueueField::<RenderQueueKey>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_ORDER_IDX));
    pub static ref RENDER_QUEUE_KEY_MATERIAL_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<RenderQueueKey>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MATERIAL_INDEX_IDX));
    pub static ref RENDER_QUEUE_KEY_MATERIAL_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<RenderQueueKey>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MATERIAL_VERSION_IDX));
    pub static ref RENDER_QUEUE_KEY_MESH_INDEX: RenderQueueField<RenderQueueKey> = RenderQueueField::<RenderQueueKey>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MESH_INDEX_IDX));
    pub static ref RENDER_QUEUE_KEY_MESH_VERSION: RenderQueueField<RenderQueueKey> = RenderQueueField::<RenderQueueKey>::new(get_render_queue_key_item_range(RENDER_QUEUE_KEY_MESH_VERSION_IDX));
}

// --- Borrow-only render query bundle and alias ---

use crate::ecs::{CameraComponent, ComponentStorage, EntityHandle, TransformComponent};

/// Borrow-only bundle of references used by the renderer hot path.
/// Zero-cost: constructed by the caller and passed by value; no allocations/copies.
pub struct RenderQueueRefs<'a> {
    pub active_camera: EntityHandle,
    pub render_queue: &'a Vec<RenderQueueItem>,
    pub camera_components: &'a ComponentStorage<CameraComponent>,
    pub transform_components: &'a ComponentStorage<TransformComponent>,
}

/// Back-compat alias to match existing plan/task wording.
pub type RenderQuery<'a> = RenderQueueRefs<'a>;

/// Zero-cost factory trait producing borrowed render query refs.
pub trait RenderQueueFactory {
    fn get<'a>(&'a self) -> RenderQuery<'a>;
}
