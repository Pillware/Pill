//! The per-instance model matrix in the layout the vertex shader reads, and
//! the vertex buffer layout that binds it.
//!
//! # Responsibilities
//!
//! - Carry one queued instance's model matrix, packed into the three `float4`
//!   rows the shader reads ([`Instance`]).
//! - Build that matrix from a [`TransformComponent`] (`Instance::new`).
//! - Build every queued instance's matrix for a frame, in queue order, on
//!   the shared thread pool ([`build_queued_instances`]).
//! - Describe the vertex buffer layout for the instance step mode: three
//!   attributes at consecutive offsets, read once per instance.
//!
//! # Design
//!
//! The matrix is built once per instance on the CPU, and the shader only puts
//! the rows back into a matrix. Building it per vertex instead would repeat
//! the same work for every vertex of a mesh that is drawn many times, and
//! sending the rotation as Euler angles would cost three inverse trigonometric
//! calls per instance.
//!
//! Only the top three rows of the matrix are sent. The fourth row is always
//! `0, 0, 0, 1`, so the shader supplies it itself. `#[repr(C)]` over three
//! `[f32; 4]` rows keeps the layout free of padding, so the descriptor's
//! offsets match the shader's attributes.

// External crates
use pill_core::rayon::prelude::*;

// Current crate
use crate::components::TransformComponent;
use crate::frame::RenderInstance;
use crate::render_queue::RenderQueueItem;
use crate::resources::Vertex;

// =============================================================================
// Constants
// =============================================================================

/// How many instances one chunk of the parallel build holds.
///
/// The frame's instances are split into chunks of this size. The size is small
/// enough that a large frame still spreads across the pool, and large enough
/// that the cost of scheduling a chunk stays far below the work in it.
const INSTANCES_PER_CHUNK: usize = 2048;

// =============================================================================
// Instance
// =============================================================================

/// One instance's model matrix, packed the way the vertex shader reads it.
///
/// The renderer builds one for every queued instance and uploads the whole set
/// as the buffer the draws bind. The shader reads each as three `float4`
/// attributes.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    /// The top three rows of the model matrix. Row `i` holds the `i`th value
    /// of each of the three scaled rotation axes, then the `i`th value of the
    /// translation.
    pub(crate) model_rows: [[f32; 4]; 3],
}

impl Instance {
    /// Builds one instance's model matrix from a transform component: the
    /// scale is applied first, then the rotation, then the translation.
    ///
    /// The matrix itself is built by `pill_rendering_core`, which every
    /// renderer shares; this is the type that carries it to a vertex shader.
    pub fn new(transform_component: &TransformComponent) -> Instance {
        Instance {
            model_rows: pill_rendering_core::instance::model_rows(transform_component),
        }
    }
}

impl Vertex for Instance {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        const ROW_SIZE: wgpu::BufferAddress = mem::size_of::<[f32; 4]>() as wgpu::BufferAddress;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Instance>() as wgpu::BufferAddress,
            // The shader reads the next instance's rows once per instance, not
            // once per vertex.
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    // Model matrix row 0; slangc maps TEXCOORD1 to @location(1).
                    offset: 0,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    // Model matrix row 1; slangc maps TEXCOORD2 to @location(2).
                    offset: ROW_SIZE,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    // Model matrix row 2; slangc maps TEXCOORD3 to @location(3).
                    offset: 2 * ROW_SIZE,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Writes the model matrix of every queued instance into `instance_matrices`,
/// in queue order.
///
/// Queue order is draw order. The result is uploaded unchanged, so a draw
/// finds its instances by their queue positions. The work runs on Rayon's
/// global pool, the one inside `pill_core.dll` that every DLL shares: those
/// threads are already running, and none of them is left running this DLL's
/// code when the host reloads it, because the call returns only after every
/// task has finished.
///
/// # Panics
///
/// Panics when a queue item names a frame instance index that
/// `frame_instances` does not have.
pub(crate) fn build_queued_instances(
    render_queue: &[RenderQueueItem],
    frame_instances: &[RenderInstance],
    instance_matrices: &mut Vec<Instance>,
) {
    // Step 1: Size the output to the queue, so every queue position has a slot.
    instance_matrices.clear();
    instance_matrices.resize(render_queue.len(), bytemuck::Zeroable::zeroed());
    // Step 2: Fill the chunks in parallel. Each chunk writes its own slice of
    // the output, so the queue order is kept.
    instance_matrices
        .par_chunks_mut(INSTANCES_PER_CHUNK)
        .zip(render_queue.par_chunks(INSTANCES_PER_CHUNK))
        .for_each(|(instance_part, queue_part)| {
            for (instance, queue_item) in instance_part.iter_mut().zip(queue_part) {
                *instance = Instance::new(
                    &frame_instances[queue_item.frame_instance_index as usize].transform,
                );
            }
        });
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    use pill_renderer_api::AssetKey;

    /// A queue item for `frame_instance_index`; the handles are irrelevant to
    /// the instance it builds.
    fn queue_item(frame_instance_index: u32) -> RenderQueueItem {
        let version = std::num::NonZeroU32::new(1).unwrap();
        RenderQueueItem {
            key: 0,
            frame_instance_index,
            shader: crate::resources::RendererShaderHandle::new(0, version),
            material: crate::resources::RendererMaterialHandle::new(0, version),
            mesh: crate::resources::RendererMeshHandle::new(0, version),
        }
    }

    #[test]
    fn the_instance_layout_matches_the_shader_attributes() {
        // The shader declares three `float4` attributes at consecutive
        // offsets, and `Instance` must stay exactly twelve floats with no
        // padding. The layout must keep reading them at those raw offsets. A
        // field added here or a format changed there would otherwise show up
        // as garbled transforms, not as a compile error.
        assert_eq!(std::mem::size_of::<Instance>(), 48);

        let layout = <Instance as Vertex>::data_layout_descriptor();

        assert_eq!(layout.array_stride, 48);
        assert_eq!(layout.step_mode, wgpu::VertexStepMode::Instance);
        let attributes: Vec<(u64, u32, wgpu::VertexFormat)> = layout
            .attributes
            .iter()
            .map(|attribute| {
                (
                    attribute.offset,
                    attribute.shader_location,
                    attribute.format,
                )
            })
            .collect();
        assert_eq!(
            attributes,
            [
                (0, 1, wgpu::VertexFormat::Float32x4),
                (16, 2, wgpu::VertexFormat::Float32x4),
                (32, 3, wgpu::VertexFormat::Float32x4),
            ]
        );
    }

    /// Any live entity handle: the instance tests read only transforms.
    fn test_entity() -> pill_engine::Entity {
        pill_engine::World::new().reserve_entity()
    }

    #[test]
    fn instances_split_across_tasks_keep_queue_order() {
        let instance_count = INSTANCES_PER_CHUNK * 3 + 7;
        let frame_instances: Vec<RenderInstance> = (0..instance_count)
            .map(|index| RenderInstance {
                entity: test_entity(),
                transform: TransformComponent {
                    translation: [index as f32, 0.0, 0.0],
                    rotation: Quat::IDENTITY.to_array(),
                    scale: [1.0; 3],
                },
                mesh: AssetKey(0),
                material: AssetKey(0),
                rendering_order: 0,
            })
            .collect();
        // The queue in reverse entity order.
        let queue: Vec<RenderQueueItem> =
            (0..instance_count as u32).rev().map(queue_item).collect();
        let mut built_instances = Vec::new();

        build_queued_instances(&queue, &frame_instances, &mut built_instances);

        assert_eq!(built_instances.len(), instance_count);
        for (position, instance) in built_instances.iter().enumerate() {
            assert_eq!(
                instance.model_rows[0][3],
                (instance_count - 1 - position) as f32
            );
        }
    }

    #[test]
    fn built_instances_follow_queue_order() {
        let frame_instances: Vec<RenderInstance> = (0..3)
            .map(|index| RenderInstance {
                entity: test_entity(),
                transform: TransformComponent {
                    translation: [index as f32, 0.0, 0.0],
                    rotation: Quat::IDENTITY.to_array(),
                    scale: [1.0; 3],
                },
                mesh: AssetKey(0),
                material: AssetKey(0),
                rendering_order: 0,
            })
            .collect();
        let queue = [2, 0, 1].map(queue_item);
        let mut built_instances = Vec::new();

        build_queued_instances(&queue, &frame_instances, &mut built_instances);

        let translations: Vec<f32> = built_instances
            .iter()
            .map(|instance| instance.model_rows[0][3])
            .collect();
        assert_eq!(translations, [2.0, 0.0, 1.0]);
    }
}
