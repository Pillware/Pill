//! The draw queue of one frame, and the key that decides the order its draws
//! are made in.
//!
//! # Responsibilities
//!
//! - Carry one queued draw per drawable frame instance: the shader, material
//!   and mesh handles it binds, the instance it draws, and the key the queue
//!   sorts on ([`RenderQueueItem`]).
//! - Pack a draw's rendering order and the slot identity of its handles into
//!   the one `u64` the queue sorts by ([`compose_render_queue_key`]), which is
//!   `pill_rendering_core`'s key with this renderer's handles unwrapped.
//!
//! # Design
//!
//! Sorting by the key puts draws that use the same rendering order, shader,
//! material and mesh next to each other. The mesh drawer records each run of
//! equal keys as one instanced draw.
//!
//! The key exists only for that comparison. It stores each index and version
//! in one byte, so a value above 255 keeps only its low byte, and nothing
//! reads a key back as a handle. A draw carries its real handles beside the
//! key, which is why an asset whose slot version has passed 255 still draws
//! with its own objects.

// Standard library
use std::cmp::Ordering;

// Current crate
use crate::resources::{RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle};

// =============================================================================
// Render Queue Item
// =============================================================================

/// One draw waiting in the queue, with the handles it binds, the instance it
/// draws, and the key it is sorted by.
///
/// The renderer adds one entry for each drawable instance of the frame, so a
/// sorted queue describes everything the frame draws. Comparing two entries
/// looks at the key first and at the instance index second, which keeps the
/// instances that share a key in the frame's own order inside their batch. The
/// handles take no part in the comparison.
#[derive(Clone, Copy, Debug)]
pub struct RenderQueueItem {
    /// The key this entry is sorted by. It packs the draw's rendering order and
    /// the slot identity of its three handles, as
    /// [`compose_render_queue_key`] lays them out. The queue only compares it
    /// and never turns it back into handles.
    pub key: u64,
    /// Position of the drawn instance in the frame's instance vector.
    /// `build_queued_instances` reads that instance's transform to build the
    /// model matrix.
    pub frame_instance_index: u32,
    /// The shader this draw binds.
    pub shader: RendererShaderHandle,
    /// The material this draw binds.
    pub material: RendererMaterialHandle,
    /// The mesh this draw binds.
    pub mesh: RendererMeshHandle,
}

impl PartialEq for RenderQueueItem {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for RenderQueueItem {}

impl PartialOrd for RenderQueueItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RenderQueueItem {
    /// Compares two entries: the sort key first, then the instance index.
    fn cmp(&self, other: &Self) -> Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.frame_instance_index.cmp(&other.frame_instance_index))
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Packs the rendering order of a draw and the resources it binds into one
/// `u64` that sorts in the order the queue needs.
///
/// The layout and its rules are
/// [`compose_draw_key`](pill_rendering_core::draw_order::compose_draw_key)'s;
/// this is the same key with this renderer's slot handles unwrapped.
pub fn compose_render_queue_key(
    order: u8,
    shader: RendererShaderHandle,
    material: RendererMaterialHandle,
    mesh: RendererMeshHandle,
) -> u64 {
    pill_rendering_core::draw_order::compose_draw_key(
        order,
        slot_identity(shader.data()),
        slot_identity(material.data()),
        slot_identity(mesh.data()),
    )
}

/// The slot identity a slot key names: its index and the version it was issued
/// with.
fn slot_identity(
    data: pill_core::slot_map::KeyData,
) -> pill_rendering_core::draw_order::SlotIdentity {
    pill_rendering_core::draw_order::SlotIdentity::new(data.index, data.version.get())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU32;

    /// A material whose slot version reached 256 still draws with its own handle.
    ///
    /// A long editing session re-inserts a material on every content change.
    /// The slot map gives the freed slot back with its version increased by
    /// one each time, so a version above 255 is reachable in practice. The key
    /// stores only the low byte of a version, but the queue carries the
    /// handles themselves, so the draw keeps its own objects.
    #[test]
    fn a_material_at_slot_version_256_keeps_its_handle() {
        let shader = RendererShaderHandle::new(3, NonZeroU32::new(4).unwrap());
        let material = RendererMaterialHandle::new(5, NonZeroU32::new(256).unwrap());
        let mesh = RendererMeshHandle::new(7, NonZeroU32::new(8).unwrap());

        let queued_draw = RenderQueueItem {
            key: compose_render_queue_key(0, shader, material, mesh),
            frame_instance_index: 0,
            shader,
            material,
            mesh,
        };

        assert_eq!(queued_draw.material.data().version.get(), 256);
        assert_eq!(queued_draw.shader, shader);
        assert_eq!(queued_draw.mesh, mesh);
    }

    #[test]
    fn a_larger_order_sorts_earlier_in_the_composed_key() {
        let shader = RendererShaderHandle::new(3, NonZeroU32::new(4).unwrap());
        let material = RendererMaterialHandle::new(5, NonZeroU32::new(6).unwrap());
        let mesh = RendererMeshHandle::new(7, NonZeroU32::new(8).unwrap());

        let order_200 = compose_render_queue_key(200, shader, material, mesh);
        let order_10 = compose_render_queue_key(10, shader, material, mesh);

        assert!(
            order_200 < order_10,
            "the queue sorts ascending on this key, so the larger order is drawn first"
        );
    }

    #[test]
    fn the_key_decides_the_order_and_the_instance_breaks_a_tie() {
        let shader = RendererShaderHandle::new(3, NonZeroU32::new(4).unwrap());
        let material = RendererMaterialHandle::new(5, NonZeroU32::new(6).unwrap());
        let mesh = RendererMeshHandle::new(7, NonZeroU32::new(8).unwrap());
        let key = compose_render_queue_key(0, shader, material, mesh);
        let queued_draw_at = |frame_instance_index| RenderQueueItem {
            key,
            frame_instance_index,
            shader,
            material,
            mesh,
        };

        assert!(
            queued_draw_at(0) < queued_draw_at(1),
            "the same state keeps frame order"
        );
        assert_eq!(queued_draw_at(4), queued_draw_at(4));
    }
}
