//! `RayTracingScene` — the renderer-owned acceleration-structure scene.
//!
//! Owns the BLAS cache, active-scene TLAS, instance-ID table, and dirty
//! graph. Provides the per-frame build-command interface consumed by the
//! main render loop.

use crate::ray_tracing::{
    blas::{BlasBuildState, RayTracingMesh, SubmissionSerial},
    instance_table::{GpuRtInstance, RtInstanceId, RtInstanceTable},
    tlas::RayTracingTlas,
    transform::model_to_tlas_transform,
};
use pill_core::{debug, info, warn, LogContext, PillSlotMapKey};
use pill_engine::internal::{
    HardwareRayQueryCapabilities, RenderInstance, RendererMeshHandle,
};
use std::collections::HashMap;

/// The renderer-owned ray-tracing scene, managing all acceleration-structure
/// state for one active scene.
pub struct RayTracingScene {
    /// Cached BLAS entries keyed by renderer mesh handle.
    pub blas_cache: HashMap<RendererMeshHandle, RayTracingMesh>,
    /// The active-scene TLAS.
    pub tlas: RayTracingTlas,
    /// 24-bit instance-ID allocator and GPU metadata table.
    pub instance_table: RtInstanceTable,
    /// Pending BLAS build entries collected for the next frame.
    pub pending_blas_entries: Vec<PendingBlasEntry>,
    /// Pending TLAS instances for the next build.
    pub pending_tlas_instances: Vec<wgpu::TlasInstance>,
    /// Map from entity index to allocated `RtInstanceId`.
    pub entity_to_instance_id: HashMap<u32, RtInstanceId>,
    /// Device capabilities snapshot.
    pub capabilities: HardwareRayQueryCapabilities,
    /// Frame epoch counter for tracking build ordering.
    pub frame_epoch: u64,
    /// Current submission serial.
    pub submission: SubmissionSerial,
    /// Maximum configured instances.
    pub max_instances: u32,
}

/// A pending BLAS build entry with its associated geometry buffers and
/// size descriptor.
pub struct PendingBlasEntry {
    pub blas: wgpu::Blas,
    pub size_descriptor: wgpu::BlasTriangleGeometrySizeDescriptor,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub mesh_handle: RendererMeshHandle,
    pub vertex_count: u32,
    pub index_count: u32,
}

impl RayTracingScene {
    /// Create a new RT scene with the given capabilities and capacity.
    pub fn new(
        device: &wgpu::Device,
        capabilities: HardwareRayQueryCapabilities,
        max_instances: u32,
    ) -> Self {
        let initial_capacity = 16u32.min(max_instances).min(capabilities.max_tlas_instance_count);
        let tlas = RayTracingTlas::new(device, initial_capacity, "active_scene_tlas");
        let instance_table = RtInstanceTable::new(device, max_instances);

        Self {
            blas_cache: HashMap::new(),
            tlas,
            instance_table,
            pending_blas_entries: Vec::new(),
            pending_tlas_instances: Vec::new(),
            entity_to_instance_id: HashMap::new(),
            capabilities,
            frame_epoch: 0,
            submission: SubmissionSerial::new(0),
            max_instances,
        }
    }

    /// Begin a new frame. Advances the epoch and clears pending build lists.
    pub fn begin_frame(&mut self) {
        self.frame_epoch = self.frame_epoch.wrapping_add(1);
        self.pending_blas_entries.clear();
        self.pending_tlas_instances.clear();
    }

    /// Register a pending BLAS build for this frame.
    pub fn queue_blas_build(&mut self, entry: PendingBlasEntry) {
        self.pending_blas_entries.push(entry);
    }

    /// Queue a TLAS instance for the current frame's build.
    /// Returns `true` when the instance was added successfully.
    pub fn queue_tlas_instance(
        &mut self,
        queue: &wgpu::Queue,
        instance: &RenderInstance,
        mesh: &RayTracingMesh,
    ) -> bool {
        // Resolve the instance ID (allocate or reuse).
        let instance_id = match self.entity_to_instance_id.get(&instance.entity.data().index) {
            Some(&existing) => existing,
            None => {
                match self.instance_table.allocate() {
                    Some(id) => {
                        self.entity_to_instance_id.insert(instance.entity.data().index, id);
                        id
                    }
                    None => {
                        warn!(LogContext::Rendering =>
                            "RT instance table full ({} slots); entity {} excluded from TLAS",
                            self.instance_table.capacity(),
                            instance.entity.data().index,
                        );
                        return false;
                    }
                }
            }
        };

        // Convert the model matrix to TLAS row-major 3x4 format.
        let transform = match model_to_tlas_transform(&instance.model) {
            Some(t) => t,
            None => {
                warn!(LogContext::Rendering =>
                    "Non-finite or singular transform for entity {}; excluded from TLAS",
                    instance.entity.data().index,
                );
                return false;
            }
        };

        // Build the TlasInstance using the wgpu 30 constructor.
        let tlas_instance = wgpu::TlasInstance::new(
            &mesh.blas,
            transform,
            instance_id.index,
            instance.ray_visibility.mask,
        );

        self.pending_tlas_instances.push(tlas_instance);

        // Write the GPU metadata record.
        self.instance_table.write_instance(
            queue,
            instance_id,
            &GpuRtInstance {
                mesh_metadata_index: 0, // Reserved for Phase 5
                material_metadata_index: 0, // Reserved for Phase 5
                entity_debug_id: instance.entity.data().index,
                flags: 0x1, // bit 0 = opaque (V1)
            },
        );

        true
    }

    /// Build all pending BLASes and TLAS in one call.
    /// Must be called within an active command encoder.
    pub fn build_acceleration_structures(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        // Prepare BLAS build entries.
        let blas_entries: Vec<wgpu::BlasBuildEntry> = self
            .pending_blas_entries
            .iter()
            .map(|entry| wgpu::BlasBuildEntry {
                blas: &entry.blas,
                geometry: wgpu::BlasGeometries::TriangleGeometries(
                    vec![wgpu::BlasTriangleGeometry {
                        size: &entry.size_descriptor,
                        vertex_buffer: &entry.vertex_buffer,
                        first_vertex: 0,
                        vertex_stride: std::mem::size_of::<pill_engine::internal::MeshVertex>() as u64,
                        index_buffer: Some(&entry.index_buffer),
                        first_index: Some(0),
                        transform_buffer: None,
                        transform_buffer_offset: None,
                    }],
                ),
            })
            .collect();

        if !blas_entries.is_empty() || !self.pending_tlas_instances.is_empty() {
            // wgpu 30: build_acceleration_structures takes &[BlasBuildEntry]
            // and &[Tlas] (the TLAS itself, not a build entry).
            let tlas_slice = std::slice::from_ref(&self.tlas.tlas);
            encoder.build_acceleration_structures(&blas_entries, tlas_slice);

            // Advance BLAS states.
            for entry in &self.pending_blas_entries {
                if let Some(mesh) = self.blas_cache.get_mut(&entry.mesh_handle) {
                    mesh.build_state = BlasBuildState::Encoded {
                        frame_epoch: self.frame_epoch,
                    };
                }
            }

            self.tlas.instance_count = self.pending_tlas_instances.len() as u32;
            self.tlas.dirty = false;
            self.tlas.revision = self.tlas.revision.wrapping_add(1);

            debug!(LogContext::Frame =>
                "AS build: {} BLAS(es), {} TLAS instance(s), revision {}",
                blas_entries.len(),
                self.pending_tlas_instances.len(),
                self.tlas.revision,
            );
        }
    }

    /// Called after queue submission to advance BLAS states.
    pub fn on_submission(&mut self) {
        self.submission = self.submission.next();

        // Advance all Encoded states to Submitted.
        for mesh in self.blas_cache.values_mut() {
            if let BlasBuildState::Encoded { .. } = &mesh.build_state {
                mesh.build_state = BlasBuildState::Submitted {
                    submission: self.submission,
                };
            }
        }

        // Advance the instance table generation.
        self.instance_table.advance_submission(self.submission);

        debug!(LogContext::Frame =>
            "Submission {}: BLAS states advanced, {} active RT instances",
            self.submission.as_u64(),
            self.instance_table.allocated_count(),
        );
    }

    /// Remove a mesh from the BLAS cache and queue its BLAS for retirement.
    pub fn remove_mesh_blas(&mut self, mesh_handle: RendererMeshHandle) {
        if self.blas_cache.remove(&mesh_handle).is_some() {
            debug!(LogContext::Rendering =>
                "Removing BLAS for mesh {:?}; queued for retirement",
                mesh_handle,
            );
        }
    }

    /// Mark the TLAS dirty when an instance transform or mask changes.
    pub fn invalidate_instance_transforms(&mut self) {
        self.tlas.mark_dirty();
    }

    /// Mark the TLAS dirty when instance membership changes.
    pub fn invalidate_instance_membership(&mut self) {
        self.tlas.mark_dirty();
    }

    /// Check whether the TLAS needs a rebuild this frame.
    pub fn needs_tlas_rebuild(&self) -> bool {
        self.tlas.dirty || !self.pending_blas_entries.is_empty()
    }

    /// Returns the TLAS bind group for shader binding.
    pub fn tlas_bind_group(&self) -> &wgpu::BindGroup {
        &self.tlas.bind_group
    }

    /// Returns the TLAS bind-group layout.
    pub fn tlas_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.tlas.bind_group_layout
    }

    /// Grow TLAS capacity if needed. Returns `true` when growth occurred.
    pub fn ensure_tlas_capacity(
        &mut self,
        device: &wgpu::Device,
        needed: u32,
    ) -> bool {
        if needed <= self.tlas.capacity {
            return false;
        }

        let new_capacity = match RayTracingTlas::next_growth_capacity(
            self.tlas.capacity,
            needed,
            self.max_instances,
            self.capabilities.max_tlas_instance_count,
        ) {
            Some(c) => c,
            None => {
                warn!(LogContext::Rendering =>
                    "Cannot grow TLAS: need {needed}, current {}, max {}",
                    self.tlas.capacity,
                    self.capabilities.max_tlas_instance_count,
                );
                return false;
            }
        };

        info!(LogContext::Rendering =>
            "Growing TLAS from {} to {} instances",
            self.tlas.capacity,
            new_capacity,
        );

        self.tlas.grow(device, new_capacity, "active_scene_tlas");
        true
    }
}
