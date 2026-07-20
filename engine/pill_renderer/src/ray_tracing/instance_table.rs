//! 24-bit ray-instance-ID allocation and GPU metadata table.
//!
//! `TlasInstance::custom_data` holds a 24-bit index into a parallel
//! `GpuRtInstance` table. The table maps the index to mesh/material metadata
//! indices and debug information for hit reconstruction.
//!
//! Slots are generation-tracked to prevent use-after-free. Freed slots are
//! not reused until all in-flight submissions that reference them have
//! completed.

use crate::ray_tracing::blas::SubmissionSerial;

/// Maximum number of concurrent ray-tracing instances (24-bit address space).
pub const MAX_RT_INSTANCE_ID: u32 = (1 << 24) - 1;

/// A unique, generation-tracked ray-tracing instance identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RtInstanceId {
    /// Index into the metadata table (0..MAX_RT_INSTANCE_ID).
    pub index: u32,
    /// Generation counter to detect stale references.
    pub generation: u32,
}

/// GPU-side metadata record for a single TLAS instance.
/// Must match the WGSL struct layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuRtInstance {
    /// Index into the global RT mesh metadata table (reserved for Phase 5).
    pub mesh_metadata_index: u32,
    /// Index into the global RT material metadata table (reserved for Phase 5).
    pub material_metadata_index: u32,
    /// Entity debug ID for diagnostic readback.
    pub entity_debug_id: u32,
    /// Reserved flags: bit 0 = is_opaque, others reserved.
    pub flags: u32,
}

/// A single slot in the instance table.
#[derive(Debug, Clone)]
struct InstanceSlot {
    generation: u32,
    /// The last submission that referenced this slot. The slot cannot be
    /// reused until that submission completes.
    last_referenced_submission: SubmissionSerial,
    /// `true` when this slot is currently allocated.
    allocated: bool,
}

/// Bounded, generation-tracked allocator for 24-bit ray-instance IDs.
pub struct RtInstanceTable {
    slots: Vec<InstanceSlot>,
    /// Parallel GPU buffer of `GpuRtInstance` records.
    gpu_buffer: wgpu::Buffer,
    free_list: Vec<u32>,
    next_submission: SubmissionSerial,
}

impl RtInstanceTable {
    /// Create a new instance table with the given capacity.
    pub fn new(device: &wgpu::Device, capacity: u32) -> Self {
        let capacity = capacity.min(MAX_RT_INSTANCE_ID + 1);
        let slots = (0..capacity)
            .map(|_| InstanceSlot {
                generation: 0,
                last_referenced_submission: SubmissionSerial::new(0),
                allocated: false,
            })
            .collect::<Vec<_>>();

        // GPU buffer: one `GpuRtInstance` per slot, zero-initialized.
        let gpu_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rt_instance_table"),
            size: (capacity as u64) * std::mem::size_of::<GpuRtInstance>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            slots,
            gpu_buffer,
            free_list: Vec::new(),
            next_submission: SubmissionSerial::new(1),
        }
    }

    /// Allocate a new instance ID. Returns `None` when the table is full.
    pub fn allocate(&mut self) -> Option<RtInstanceId> {
        // Prefer the free list.
        if let Some(index) = self.free_list.pop() {
            let slot = &mut self.slots[index as usize];
            if !slot.allocated {
                slot.allocated = true;
                slot.last_referenced_submission = self.next_submission;
                return Some(RtInstanceId {
                    index,
                    generation: slot.generation,
                });
            }
        }

        // Linear scan for an unallocated slot.
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if !slot.allocated {
                slot.allocated = true;
                slot.last_referenced_submission = self.next_submission;
                return Some(RtInstanceId {
                    index: index as u32,
                    generation: slot.generation,
                });
            }
        }

        None
    }

    /// Free an instance ID. The slot is not immediately reusable; it will
    /// be returned to the free list once all referencing submissions have
    /// completed.
    pub fn free(&mut self, id: RtInstanceId) {
        if (id.index as usize) < self.slots.len() {
            let slot = &mut self.slots[id.index as usize];
            if slot.allocated && slot.generation == id.generation {
                slot.allocated = false;
                slot.generation = slot.generation.wrapping_add(1);
                slot.last_referenced_submission = self.next_submission;
                // Defer reuse — slot will be collected by
                // `collect_retired_slots` after submission completion.
            }
        }
    }

    /// Advance the submission counter and collect slots whose last
    /// referencing submission has completed.
    pub fn advance_submission(&mut self, completed_through: SubmissionSerial) {
        self.next_submission = self.next_submission.next();

        // Collect retired slots for reuse.
        let mut i = 0;
        while i < self.slots.len() {
            let slot = &self.slots[i];
            if !slot.allocated && slot.last_referenced_submission <= completed_through {
                self.free_list.push(i as u32);
            }
            i += 1;
        }
    }

    /// Write a `GpuRtInstance` record to the GPU buffer for the given ID.
    pub fn write_instance(
        &self,
        queue: &wgpu::Queue,
        id: RtInstanceId,
        record: &GpuRtInstance,
    ) {
        let offset = id.index as u64 * std::mem::size_of::<GpuRtInstance>() as u64;
        queue.write_buffer(
            &self.gpu_buffer,
            offset,
            bytemuck::bytes_of(record),
        );
    }

    /// Returns a reference to the GPU storage buffer.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.gpu_buffer
    }

    /// Current allocated slot count.
    pub fn allocated_count(&self) -> usize {
        self.slots.iter().filter(|s| s.allocated).count()
    }

    /// Total capacity.
    pub fn capacity(&self) -> u32 {
        self.slots.len() as u32
    }
}
