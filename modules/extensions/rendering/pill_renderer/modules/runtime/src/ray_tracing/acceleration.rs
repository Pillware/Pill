//! Acceleration structures: one bottom-level structure per mesh and one
//! top-level structure for the frame.
//!
//! # Responsibilities
//!
//! - Build a bottom-level structure for every mesh the frame draws, from the
//!   vertex and index buffers wgpu already uploaded ([`RayTracingScene`]).
//! - Rebuild the structure of a mesh whose buffers moved, and forget the one of
//!   a mesh that is gone.
//! - Build the frame's top-level structure over the frame's instances, from the
//!   instance matrices the renderer already built for its draws.
//! - Destroy every raw object it owns, before the device it was made on.
//!
//! # Design
//!
//! The structures are raw because wgpu keeps the Vulkan handle of a structure it
//! built private, so a structure wgpu built cannot be bound into a pipeline of
//! our own. wgpu still owns the geometry: a build reads the same `vk::Buffer`s,
//! through the device addresses wgpu enabled when ray tracing was asked for.
//!
//! A frame's builds are recorded into one command buffer taken from one of
//! wgpu's own encoders, so the raw work travels in wgpu's submission stream and
//! the queue orders it against the uploads that came before it. A mesh structure
//! is built when its mesh appears or changes, which is rare; the top-level
//! structure is built once per frame.
//!
//! The top-level structures live in one of [FRAMES_IN_FLIGHT] slots and the
//! ring advances every frame, so a slot is rewritten only after the frames that
//! read it have been submitted. A structure a mesh build replaced waits the
//! same number of frames before it is destroyed: wgpu does not track raw reads,
//! so it would otherwise free memory the GPU is still reading.

// Standard library
use std::collections::{HashMap, HashSet};

// External crates
use ash::vk;

// Current crate
use crate::{
    instance::Instance,
    ray_tracing::{
        device::RawDevice,
        memory::{MemoryUse, RawBuffer},
    },
    render_queue::RenderQueueItem,
    resources::{RendererMesh, RendererMeshHandle, RendererResourceStorage},
};

// =============================================================================
// Constants
// =============================================================================

/// How many frames' top-level structures are kept alive at once.
///
/// A slot is rewritten only after this many frames, which is what keeps the GPU
/// from reading instance records the CPU has already overwritten.
pub(crate) const FRAMES_IN_FLIGHT: usize = 2;

/// The build flags every structure is built with.
///
/// `PREFER_FAST_TRACE` trades build time for traversal speed, which suits a
/// structure built when a mesh appears and then read every frame.
const BUILD_FLAGS: vk::BuildAccelerationStructureFlagsKHR =
    vk::BuildAccelerationStructureFlagsKHR::PREFER_FAST_TRACE;

/// The mask an instance is built with: every ray hits it.
const INSTANCE_MASK: u32 = 0xFF;

/// The smallest buffer Vulkan accepts, in bytes.
const MINIMUM_BUFFER_SIZE: vk::DeviceSize = 1;

// =============================================================================
// Instance record
// =============================================================================

/// One instance of the frame's top-level structure, laid out as
/// `VkAccelerationStructureInstanceKHR` expects.
///
/// The record is plain bytes rather than the `ash` struct, because its layout is
/// what the renderer already has: the three row-major model rows its vertex
/// shader reads, two packed 32-bit fields, and the address of the mesh's
/// structure.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceRecord {
    /// The instance's model matrix, row-major as Vulkan wants it.
    transform: [f32; 12],
    /// The instance's index in the low 24 bits, its mask in the high 8.
    custom_index_and_mask: u32,
    /// The shader binding table record in the low 24 bits, flags in the high 8.
    /// Every instance here uses record 0 with no flags.
    offset_and_flags: u32,
    /// The device address of the mesh's bottom-level structure.
    acceleration_structure: u64,
}

impl InstanceRecord {
    /// Lays one instance of `mesh_address` out as instance number `index`.
    fn new(index: usize, model_rows: [[f32; 4]; 3], mesh_address: vk::DeviceAddress) -> Self {
        let mut transform = [0.0_f32; 12];
        for (row, values) in model_rows.iter().enumerate() {
            transform[row * 4..row * 4 + 4].copy_from_slice(values);
        }
        Self {
            transform,
            custom_index_and_mask: (index as u32 & 0x00FF_FFFF) | (INSTANCE_MASK << 24),
            offset_and_flags: 0,
            acceleration_structure: mesh_address,
        }
    }
}

// =============================================================================
// Report
// =============================================================================

/// What one frame's build did, for the debug line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BuildReport {
    /// How many mesh structures were built, new or rebuilt.
    pub(crate) meshes: u32,
    /// How many instances the top-level structure holds.
    pub(crate) instances: u32,
}

// =============================================================================
// Mesh structures
// =============================================================================

/// A mesh's bottom-level structure, with what its build read.
struct MeshStructure {
    /// The structure itself.
    structure: vk::AccelerationStructureKHR,
    /// The memory the structure lives in.
    storage: RawBuffer,
    /// The address the top-level structure refers to it by.
    address: vk::DeviceAddress,
    /// The vertex buffer the build read, kept alive while the structure refers
    /// to it: wgpu does not see the raw build, so it would free a replaced
    /// buffer the GPU still reads.
    #[expect(
        dead_code,
        reason = "held so wgpu cannot free a buffer a raw build still reads"
    )]
    vertex_buffer: wgpu::Buffer,
    /// The index buffer the build read, kept alive for the same reason.
    #[expect(
        dead_code,
        reason = "held so wgpu cannot free a buffer a raw build still reads"
    )]
    index_buffer: wgpu::Buffer,
}

impl MeshStructure {
    /// Destroys the structure and its storage.
    ///
    /// # Safety
    ///
    /// No command still reads the structure.
    unsafe fn destroy(self, raw: &RawDevice) {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            raw.structures()
                .destroy_acceleration_structure(self.structure, None);
            self.storage.destroy(raw);
        }
    }
}

/// A mesh structure to build, with everything the recording step needs.
struct PendingMesh {
    /// The mesh the structure belongs to.
    handle: RendererMeshHandle,
    /// The structure, created but not built yet.
    built: MeshStructure,
    /// The geometry the build reads.
    geometry: vk::AccelerationStructureGeometryKHR<'static>,
    /// How many triangles the geometry holds.
    triangle_count: u32,
    /// How much scratch memory the build needs.
    scratch_size: vk::DeviceSize,
    /// Where in the shared scratch memory this build works.
    scratch_offset: vk::DeviceSize,
}

// =============================================================================
// The frame's structure
// =============================================================================

/// The frame's top-level structure and the buffers it is built from.
struct FrameStructure {
    /// The structure itself.
    structure: vk::AccelerationStructureKHR,
    /// The memory the structure lives in.
    storage: RawBuffer,
    /// The memory the build works in.
    scratch: RawBuffer,
    /// The instance records the build reads, written by the CPU every frame.
    instances: RawBuffer,
    /// How many instance records the slot is sized for.
    capacity: u32,
}

impl FrameStructure {
    /// Builds an empty slot sized for `capacity` instances.
    fn new(raw: &RawDevice, capacity: u32) -> Result<Self, String> {
        let capacity = capacity.max(1);
        let sizes = top_level_sizes(raw, capacity);
        let storage = RawBuffer::new(
            raw,
            sizes.acceleration_structure_size,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "frame acceleration structure",
            MemoryUse::DeviceOnly,
        )?;
        let structure = create_structure(
            raw,
            storage.handle(),
            sizes.acceleration_structure_size,
            vk::AccelerationStructureTypeKHR::TOP_LEVEL,
        )?;
        let instances = RawBuffer::new(
            raw,
            instance_records_size(capacity),
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "frame instances",
            MemoryUse::Upload,
        )?;
        let scratch = RawBuffer::new(
            raw,
            sizes.build_scratch_size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "frame build scratch",
            MemoryUse::DeviceOnly,
        )?;
        Ok(Self {
            structure,
            storage,
            scratch,
            instances,
            capacity,
        })
    }

    /// Makes the slot hold `instances` records, replacing it when the frame
    /// outgrew it.
    fn ensure_capacity(&mut self, raw: &RawDevice, instances: u32) -> Result<(), String> {
        if self.capacity >= instances.max(1) {
            return Ok(());
        }
        let replacement = Self::new(raw, instances)?;
        let old = std::mem::replace(self, replacement);
        // SAFETY: the frame that read the old structure was submitted before
        // this one, and nothing reads the structure between builds.
        unsafe { old.destroy(raw) };
        Ok(())
    }

    /// Destroys the structure and its buffers.
    ///
    /// # Safety
    ///
    /// No command still reads the slot.
    unsafe fn destroy(self, raw: &RawDevice) {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            raw.structures()
                .destroy_acceleration_structure(self.structure, None);
            self.storage.destroy(raw);
            self.scratch.destroy(raw);
            self.instances.destroy(raw);
        }
    }
}

// =============================================================================
// Scene
// =============================================================================

/// Every acceleration structure the renderer holds: one per mesh, plus the
/// frame's top-level structure.
pub(crate) struct RayTracingScene {
    /// The raw device every structure is made on.
    raw: RawDevice,
    /// The offset alignment a build's scratch region needs.
    scratch_alignment: vk::DeviceSize,
    /// One bottom-level structure per mesh, keyed the way the mesh storage keys
    /// its meshes.
    meshes: HashMap<RendererMeshHandle, MeshStructure>,
    /// The meshes whose buffers moved since their structure was built.
    changed: HashSet<RendererMeshHandle>,
    /// The ring of top-level structures, one per frame in flight, so a
    /// submitted frame's instance records are not rewritten under it.
    frames: Vec<FrameStructure>,
    /// Which slot of the ring the next frame uses.
    next_frame: usize,
    /// The frame's instance records, kept between frames.
    records: Vec<InstanceRecord>,
    /// Build scratch memory for the mesh structures, sized to the largest build
    /// seen.
    mesh_scratch: RawBuffer,
    /// How many bytes `mesh_scratch` holds.
    mesh_scratch_size: vk::DeviceSize,
    /// The mesh structures that were replaced or forgotten, each with the
    /// number of frames it still has to outlive before it is destroyed.
    retired: Vec<(MeshStructure, usize)>,
}

impl RayTracingScene {
    /// Creates the scene with no structures built yet.
    ///
    /// # Errors
    ///
    /// Returns a message when the device is not Vulkan - where the ray tracing
    /// path never runs - or when the scratch memory cannot be allocated.
    pub(crate) fn new(device: &wgpu::Device, instance: &wgpu::Instance) -> Result<Self, String> {
        // SAFETY: the scene lives in the renderer's own state, whose field order
        // destroys it before the device it was made on.
        let raw = unsafe { RawDevice::new(device, instance) }
            .ok_or_else(|| "the ray tracing path needs the Vulkan backend".to_string())?;
        let scratch_alignment = scratch_alignment(&raw);
        let mesh_scratch = RawBuffer::new(
            &raw,
            4096,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "acceleration structure build scratch",
            MemoryUse::DeviceOnly,
        )?;
        let mesh_scratch_size = mesh_scratch.size();
        Ok(Self {
            raw,
            scratch_alignment,
            meshes: HashMap::new(),
            changed: HashSet::new(),
            frames: Vec::with_capacity(FRAMES_IN_FLIGHT),
            next_frame: 0,
            records: Vec::new(),
            mesh_scratch,
            mesh_scratch_size,
            retired: Vec::new(),
        })
    }

    /// Marks a mesh's structure stale, because its buffers were rewritten or
    /// replaced.
    pub(crate) fn mesh_changed(&mut self, mesh: RendererMeshHandle) {
        self.changed.insert(mesh);
    }

    /// Forgets a mesh's structure, because the mesh is gone.
    pub(crate) fn mesh_removed(&mut self, mesh: RendererMeshHandle) {
        self.changed.remove(&mesh);
        if let Some(structure) = self.meshes.remove(&mesh) {
            self.retired.push((structure, FRAMES_IN_FLIGHT));
        }
    }

    /// Builds whatever the frame needs and reports what it built.
    ///
    /// `queue_items` and `instances` are the frame's draws in queue order: one
    /// instance per item, so the two line up. Every mesh the frame draws whose
    /// structure is missing or stale is built first; then the frame's top-level
    /// structure is built over every instance whose mesh has one.
    ///
    /// # Errors
    ///
    /// Returns a message when a structure cannot be created, sized or recorded.
    /// The caller logs it and keeps drawing: a frame without ray tracing is
    /// better than a frame that does not arrive.
    pub(crate) fn build_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        storage: &RendererResourceStorage,
        queue_items: &[RenderQueueItem],
        instances: &[Instance],
    ) -> Result<BuildReport, String> {
        // Step 1: Create the structures of the meshes that need one, in queue
        // order and once each.
        let mut pending: Vec<PendingMesh> = Vec::new();
        let mut seen: Vec<RendererMeshHandle> = Vec::new();
        for item in queue_items {
            let stale = self.changed.contains(&item.mesh) || !self.meshes.contains_key(&item.mesh);
            if !stale || seen.contains(&item.mesh) {
                continue;
            }
            seen.push(item.mesh);
            if let Some(mesh) = storage.meshes.get(item.mesh) {
                pending.push(self.plan_mesh(item.mesh, mesh)?);
            }
        }

        // Step 2: Give every build its own region of the shared scratch memory.
        let mut scratch_end = 0;
        for build in &mut pending {
            build.scratch_offset = align_up(scratch_end, self.scratch_alignment);
            scratch_end = build.scratch_offset + build.scratch_size;
        }
        self.grow_mesh_scratch(scratch_end)?;

        // Step 3: Fill the frame's instance records, from the structures that
        // exist and the ones about to.
        self.records.clear();
        for (index, (item, instance)) in queue_items.iter().zip(instances).enumerate() {
            let address = match self.meshes.get(&item.mesh) {
                Some(structure) => Some(structure.address),
                None => pending
                    .iter()
                    .find(|build| build.handle == item.mesh)
                    .map(|build| build.built.address),
            };
            if let Some(address) = address {
                self.records
                    .push(InstanceRecord::new(index, instance.model_rows, address));
            }
        }
        let report = BuildReport {
            meshes: pending.len() as u32,
            instances: self.records.len() as u32,
        };
        let instance_count = self.records.len().max(1) as u32;
        while self.frames.len() < FRAMES_IN_FLIGHT {
            self.frames
                .push(FrameStructure::new(&self.raw, instance_count)?);
        }
        let slot = self.next_frame % FRAMES_IN_FLIGHT;
        self.next_frame = self.next_frame.wrapping_add(1);
        let frame = &mut self.frames[slot];
        frame.ensure_capacity(&self.raw, instance_count)?;
        let record_bytes: Vec<u8> = bytemuck::cast_slice(&self.records).to_vec();
        frame.instances.write(&self.raw, &record_bytes)?;

        // Step 4: Record every build into one wgpu encoder and submit it.
        self.record_builds(device, queue, slot, &pending, report.instances)?;

        // Step 5: Install the new mesh structures. What they replace is retired
        // rather than destroyed: the frame submitted before this one may still
        // read it through its own top-level structure.
        for build in pending {
            if let Some(old) = self.meshes.insert(build.handle, build.built) {
                self.retired.push((old, FRAMES_IN_FLIGHT));
            }
            self.changed.remove(&build.handle);
        }
        // A structure is destroyed only once this many frames have been built
        // since it was replaced: wgpu cannot wait for the raw reads it never
        // saw, so the frame that referred to it has to be finished first.
        let mut still_waiting = Vec::with_capacity(self.retired.len());
        for (structure, frames_left) in self.retired.drain(..) {
            if frames_left <= 1 {
                // SAFETY: the frames that read the structure were submitted
                // FRAMES_IN_FLIGHT frames ago.
                unsafe { structure.destroy(&self.raw) };
            } else {
                still_waiting.push((structure, frames_left - 1));
            }
        }
        self.retired = still_waiting;

        Ok(report)
    }

    /// Creates the structure of one mesh, sized and ready to build.
    fn plan_mesh(
        &self,
        handle: RendererMeshHandle,
        mesh: &RendererMesh,
    ) -> Result<PendingMesh, String> {
        // Step 1: Copy the raw handles out and drop the guards: they hold
        // wgpu's resource lock while they live.
        // SAFETY: both buffers outlive the build, and this path only runs on the
        // Vulkan backend.
        let vertex_handle = unsafe {
            mesh.vertex_buffer
                .as_hal::<wgpu::hal::api::Vulkan>()
                .map(|guard| guard.raw_handle())
        }
        .ok_or_else(|| format!("mesh `{}` has no Vulkan vertex buffer", mesh.name))?;
        // SAFETY: as above.
        let index_handle = unsafe {
            mesh.index_buffer
                .as_hal::<wgpu::hal::api::Vulkan>()
                .map(|guard| guard.raw_handle())
        }
        .ok_or_else(|| format!("mesh `{}` has no Vulkan index buffer", mesh.name))?;
        // SAFETY: both handles were just read from live buffers.
        let vertex_address = unsafe { self.raw.buffer_address(vertex_handle) };
        // SAFETY: as above.
        let index_address = unsafe { self.raw.buffer_address(index_handle) };

        // Step 2: Describe the one triangle geometry the build reads. The
        // position is the first field of the interleaved vertex, and the indices
        // are `u32`, which is what the mesh layout already is.
        let triangles = vk::AccelerationStructureGeometryTrianglesDataKHR::default()
            .vertex_format(vk::Format::R32G32B32_SFLOAT)
            .vertex_data(vk::DeviceOrHostAddressConstKHR {
                device_address: vertex_address,
            })
            .vertex_stride(std::mem::size_of::<crate::assets::MeshVertex>() as u64)
            .max_vertex(mesh.vertex_count.saturating_sub(1))
            .index_type(vk::IndexType::UINT32)
            .index_data(vk::DeviceOrHostAddressConstKHR {
                device_address: index_address,
            });
        let geometry = vk::AccelerationStructureGeometryKHR::default()
            .geometry_type(vk::GeometryTypeKHR::TRIANGLES)
            .geometry(vk::AccelerationStructureGeometryDataKHR { triangles })
            .flags(vk::GeometryFlagsKHR::OPAQUE);
        let triangle_count = mesh.index_count / 3;

        // Step 3: Ask how much memory the structure and its build need.
        let mut build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
            .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
            .flags(BUILD_FLAGS)
            .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
            .geometries(std::slice::from_ref(&geometry));
        build_info.geometry_count = 1;
        let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
        // SAFETY: the build info and the geometry outlive the call.
        unsafe {
            self.raw
                .structures()
                .get_acceleration_structure_build_sizes(
                    vk::AccelerationStructureBuildTypeKHR::DEVICE,
                    &build_info,
                    &[triangle_count],
                    &mut sizes,
                );
        }

        // Step 4: Create the structure over fresh storage, and keep what the
        // build reads alive with it.
        let storage = RawBuffer::new(
            &self.raw,
            sizes.acceleration_structure_size,
            vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "mesh acceleration structure",
            MemoryUse::DeviceOnly,
        )?;
        let structure = create_structure(
            &self.raw,
            storage.handle(),
            sizes.acceleration_structure_size,
            vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL,
        )?;
        // SAFETY: the structure was just created on this device.
        let address = unsafe {
            self.raw
                .structures()
                .get_acceleration_structure_device_address(
                    &vk::AccelerationStructureDeviceAddressInfoKHR::default()
                        .acceleration_structure(structure),
                )
        };

        Ok(PendingMesh {
            handle,
            built: MeshStructure {
                structure,
                storage,
                address,
                vertex_buffer: mesh.vertex_buffer.clone(),
                index_buffer: mesh.index_buffer.clone(),
            },
            geometry,
            triangle_count,
            scratch_size: sizes.build_scratch_size,
            scratch_offset: 0,
        })
    }

    /// Records the frame's builds into one command buffer and submits it.
    fn record_builds(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        slot: usize,
        pending: &[PendingMesh],
        instance_count: u32,
    ) -> Result<(), String> {
        // Nothing was built and nothing is drawn, so there is nothing to
        // record.
        if pending.is_empty() && instance_count == 0 {
            return Ok(());
        }
        let Some(frame) = self.frames.get(slot) else {
            return Ok(());
        };
        // SAFETY: the addresses are read from live buffers, and the instance
        // buffer was written just before this.
        let scratch_address = unsafe { self.mesh_scratch.address(&self.raw) };
        // SAFETY: as above.
        let instance_address = unsafe { frame.instances.address(&self.raw) };
        // SAFETY: as above.
        let frame_scratch_address = unsafe { frame.scratch.address(&self.raw) };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("acceleration structures"),
        });
        {
            let raw = &self.raw;
            let frame_structure = frame.structure;
            // SAFETY: wgpu hands the raw encoder of a command encoder that is
            // recording, and every buffer named in the closure is alive for the
            // submission that follows.
            unsafe {
                encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, ()>(|hal_encoder| {
                    let Some(hal_encoder) = hal_encoder else {
                        return;
                    };
                    // SAFETY: the hal encoder is recording, so it holds a
                    // command buffer.
                    let command_buffer = hal_encoder.raw_handle();
                    // SAFETY: the barriers recorded below cover the writes that
                    // came before them.
                    record_commands(
                        raw,
                        command_buffer,
                        pending,
                        scratch_address,
                        instance_address,
                        frame_scratch_address,
                        frame_structure,
                        instance_count,
                    );
                });
            }
        }
        queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }

    /// Grows the shared mesh scratch memory to `size` bytes when it is too
    /// small.
    fn grow_mesh_scratch(&mut self, size: vk::DeviceSize) -> Result<(), String> {
        if size <= self.mesh_scratch_size {
            return Ok(());
        }
        let replacement = RawBuffer::new(
            &self.raw,
            size,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS,
            "acceleration structure build scratch",
            MemoryUse::DeviceOnly,
        )?;
        self.mesh_scratch_size = replacement.size();
        let old = std::mem::replace(&mut self.mesh_scratch, replacement);
        // SAFETY: every build that used the old memory was submitted before this
        // frame.
        unsafe { old.destroy(&self.raw) };
        Ok(())
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Records the barriers and the builds themselves.
///
/// The first barrier waits for everything recorded before it - the frame's
/// buffer uploads, the CPU's instance records and any earlier build - before a
/// build reads it. The second orders the mesh builds before the top-level build
/// that refers to them, and the third makes the finished structures visible to
/// the ray tracing shaders that will read them.
///
/// # Safety
///
/// `command_buffer` must be recording on `raw`'s device, and every structure and
/// address passed in must still be alive when the command buffer is submitted.
unsafe fn record_commands(
    raw: &RawDevice,
    command_buffer: vk::CommandBuffer,
    pending: &[PendingMesh],
    scratch_address: vk::DeviceAddress,
    instance_address: vk::DeviceAddress,
    frame_scratch_address: vk::DeviceAddress,
    frame_structure: vk::AccelerationStructureKHR,
    instance_count: u32,
) {
    let device = raw.device();
    let build_stage = vk::PipelineStageFlags::ACCELERATION_STRUCTURE_BUILD_KHR;
    let memory_write = vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE | vk::AccessFlags::SHADER_WRITE)
        .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);
    let structure_ready = vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
        .dst_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR);
    let structure_readable = vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::ACCELERATION_STRUCTURE_WRITE_KHR)
        .dst_access_mask(
            vk::AccessFlags::ACCELERATION_STRUCTURE_READ_KHR | vk::AccessFlags::SHADER_READ,
        );
    // SAFETY: the command buffer is recording, and one barrier with
    // `ALL_COMMANDS` on the source side covers every writer that came before:
    // wgpu's uploads, the CPU's writes and earlier builds.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            vk::PipelineStageFlags::ALL_COMMANDS,
            build_stage,
            vk::DependencyFlags::empty(),
            &[memory_write],
            &[],
            &[],
        );
    }

    // The mesh structures, one build per mesh.
    if !pending.is_empty() {
        let mut infos = Vec::with_capacity(pending.len());
        let mut ranges = Vec::with_capacity(pending.len());
        for build in pending {
            let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
                .ty(vk::AccelerationStructureTypeKHR::BOTTOM_LEVEL)
                .flags(BUILD_FLAGS)
                .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
                .dst_acceleration_structure(build.built.structure)
                .geometries(std::slice::from_ref(&build.geometry))
                .scratch_data(vk::DeviceOrHostAddressKHR {
                    device_address: scratch_address + build.scratch_offset,
                });
            info.geometry_count = 1;
            infos.push(info);
            ranges.push(
                vk::AccelerationStructureBuildRangeInfoKHR::default()
                    .primitive_count(build.triangle_count),
            );
        }
        let range_slices: Vec<&[vk::AccelerationStructureBuildRangeInfoKHR]> =
            ranges.iter().map(std::slice::from_ref).collect();
        // SAFETY: every build names a live structure over readable geometry.
        unsafe {
            raw.structures().cmd_build_acceleration_structures(
                command_buffer,
                &infos,
                &range_slices,
            );
        }
        // The top-level build reads these, so it waits for them.
        // SAFETY: the command buffer is recording.
        unsafe {
            device.cmd_pipeline_barrier(
                command_buffer,
                build_stage,
                build_stage,
                vk::DependencyFlags::empty(),
                &[structure_ready],
                &[],
                &[],
            );
        }
    }

    // The top-level structure over the frame's instances.
    let instances = vk::AccelerationStructureGeometryInstancesDataKHR::default()
        .array_of_pointers(false)
        .data(vk::DeviceOrHostAddressConstKHR {
            device_address: instance_address,
        });
    let geometry = vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { instances })
        .flags(vk::GeometryFlagsKHR::OPAQUE);
    let mut info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
        .flags(BUILD_FLAGS)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .dst_acceleration_structure(frame_structure)
        .geometries(std::slice::from_ref(&geometry))
        .scratch_data(vk::DeviceOrHostAddressKHR {
            device_address: frame_scratch_address,
        });
    info.geometry_count = 1;
    let range =
        vk::AccelerationStructureBuildRangeInfoKHR::default().primitive_count(instance_count);
    let range_slices: Vec<&[vk::AccelerationStructureBuildRangeInfoKHR]> =
        vec![std::slice::from_ref(&range)];
    // SAFETY: the structure and its instance buffer are alive, and the geometry
    // outlives the call.
    unsafe {
        raw.structures().cmd_build_acceleration_structures(
            command_buffer,
            std::slice::from_ref(&info),
            &range_slices,
        );
    }

    // The structures are ready for the shaders that read them next.
    // SAFETY: the command buffer is recording.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            build_stage,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[structure_readable],
            &[],
            &[],
        );
    }
}

/// Rounds `value` up to the next multiple of `alignment`.
fn align_up(value: vk::DeviceSize, alignment: vk::DeviceSize) -> vk::DeviceSize {
    let alignment = alignment.max(1);
    value.div_ceil(alignment) * alignment
}

/// How many bytes `capacity` instance records take.
fn instance_records_size(capacity: u32) -> vk::DeviceSize {
    let records =
        capacity as vk::DeviceSize * std::mem::size_of::<InstanceRecord>() as vk::DeviceSize;
    records.max(MINIMUM_BUFFER_SIZE)
}

/// Creates an acceleration structure over `size` bytes of `storage`.
fn create_structure(
    raw: &RawDevice,
    storage: vk::Buffer,
    size: vk::DeviceSize,
    kind: vk::AccelerationStructureTypeKHR,
) -> Result<vk::AccelerationStructureKHR, String> {
    let create_info = vk::AccelerationStructureCreateInfoKHR::default()
        .buffer(storage)
        .offset(0)
        .size(size.max(MINIMUM_BUFFER_SIZE))
        .ty(kind);
    // SAFETY: the storage is live and at least `size` bytes long.
    unsafe {
        raw.structures()
            .create_acceleration_structure(&create_info, None)
    }
    .map_err(|error| format!("acceleration structure: {error}"))
}

/// The sizes of a top-level structure holding `capacity` instances.
fn top_level_sizes(
    raw: &RawDevice,
    capacity: u32,
) -> vk::AccelerationStructureBuildSizesInfoKHR<'static> {
    let instances = vk::AccelerationStructureGeometryInstancesDataKHR::default();
    let geometry = vk::AccelerationStructureGeometryKHR::default()
        .geometry_type(vk::GeometryTypeKHR::INSTANCES)
        .geometry(vk::AccelerationStructureGeometryDataKHR { instances })
        .flags(vk::GeometryFlagsKHR::OPAQUE);
    let mut build_info = vk::AccelerationStructureBuildGeometryInfoKHR::default()
        .ty(vk::AccelerationStructureTypeKHR::TOP_LEVEL)
        .flags(BUILD_FLAGS)
        .mode(vk::BuildAccelerationStructureModeKHR::BUILD)
        .geometries(std::slice::from_ref(&geometry));
    build_info.geometry_count = 1;
    let mut sizes = vk::AccelerationStructureBuildSizesInfoKHR::default();
    // SAFETY: the build info and the geometry outlive the call.
    unsafe {
        raw.structures().get_acceleration_structure_build_sizes(
            vk::AccelerationStructureBuildTypeKHR::DEVICE,
            &build_info,
            &[capacity],
            &mut sizes,
        );
    }
    sizes
}

/// The offset alignment a build's scratch region needs, or one byte when the
/// driver asks for nothing stricter.
fn scratch_alignment(raw: &RawDevice) -> vk::DeviceSize {
    let mut properties = vk::PhysicalDeviceAccelerationStructurePropertiesKHR::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut properties);
    // SAFETY: the chain is built from live locals.
    unsafe {
        raw.instance()
            .get_physical_device_properties2(raw.physical_device(), &mut properties2);
    }
    properties
        .min_acceleration_structure_scratch_offset_alignment
        .max(1) as vk::DeviceSize
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_record_is_the_size_vulkan_expects() {
        // `VkAccelerationStructureInstanceKHR` is 64 bytes: a 3x4 transform, two
        // packed fields and the structure address.
        assert_eq!(std::mem::size_of::<InstanceRecord>(), 64);
    }

    #[test]
    fn an_instance_record_carries_the_model_rows_and_the_structure_address() {
        let rows = [
            [2.0, 0.0, 0.0, 1.0],
            [0.0, 2.0, 0.0, 2.0],
            [0.0, 0.0, 2.0, 3.0],
        ];
        let record = InstanceRecord::new(7, rows, 0x1234_5678);

        assert_eq!(&record.transform[0..4], &rows[0]);
        assert_eq!(&record.transform[4..8], &rows[1]);
        assert_eq!(&record.transform[8..12], &rows[2]);
        assert_eq!(record.custom_index_and_mask & 0x00FF_FFFF, 7);
        assert_eq!(record.custom_index_and_mask >> 24, INSTANCE_MASK);
        assert_eq!(record.offset_and_flags, 0);
        assert_eq!(record.acceleration_structure, 0x1234_5678);
    }

    #[test]
    fn scratch_offsets_are_rounded_up_to_the_alignment() {
        assert_eq!(align_up(0, 256), 0);
        assert_eq!(align_up(1, 256), 256);
        assert_eq!(align_up(256, 256), 256);
        assert_eq!(align_up(257, 256), 512);
        assert_eq!(align_up(0, 0), 0);
    }

    #[test]
    fn an_empty_frame_still_asks_for_one_record() {
        assert_eq!(instance_records_size(0), MINIMUM_BUFFER_SIZE);
        assert_eq!(
            instance_records_size(3),
            3 * std::mem::size_of::<InstanceRecord>() as vk::DeviceSize
        );
    }
}
