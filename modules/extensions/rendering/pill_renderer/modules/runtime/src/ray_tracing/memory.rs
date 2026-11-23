//! Raw Vulkan buffers with an allocation of their own.
//!
//! # Responsibilities
//!
//! - Create a buffer, allocate dedicated memory for it and bind the two,
//!   choosing a memory type from the flags the caller needs
//!   ([`RawBuffer::new`]).
//! - Hand out the buffer handle, its device address and its mapped bytes, so an
//!   acceleration-structure build and the instance list can use it.
//! - Destroy both together, in the one order Vulkan allows
//!   ([`RawBuffer::destroy`]).
//!
//! # Design
//!
//! Acceleration-structure storage and build scratch memory have no wgpu usage
//! flag, so wgpu cannot own them. Each one gets its own `vkAllocateMemory`: the
//! path holds a handful of objects, and a dedicated allocation keeps destruction
//! to one pair of calls per object with no allocator crate in the dependency
//! graph. Every buffer here is readable through a device address, so each
//! allocation asks for the device-address flag that makes that address valid.

// Standard library
use std::ptr;

// External crates
use ash::vk;

// Current crate
use crate::ray_tracing::device::RawDevice;

// =============================================================================
// Memory use
// =============================================================================

/// Where an allocation's memory has to be usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemoryUse {
    /// Memory the GPU reads and writes, never the CPU.
    DeviceOnly,
    /// Memory the CPU writes into and the GPU reads.
    Upload,
}

// =============================================================================
// Raw buffer
// =============================================================================

/// One raw buffer and the memory bound to it.
pub(crate) struct RawBuffer {
    /// The buffer handle.
    buffer: vk::Buffer,
    /// The memory the buffer is bound to.
    memory: vk::DeviceMemory,
    /// How many bytes the buffer holds.
    size: vk::DeviceSize,
    /// What the buffer is for, named in every error message about it.
    name: &'static str,
}

impl RawBuffer {
    /// Creates `size` bytes of buffer memory with `usage`.
    ///
    /// `memory_use` picks the memory type: a build reads from upload memory the
    /// CPU wrote, and a structure's storage lives in device memory.
    ///
    /// # Errors
    ///
    /// Returns a message naming the buffer when it cannot be created, no memory
    /// type fits it, or the allocation or the bind fails. Each of those is a
    /// real refusal - a device out of memory, or a usage flag the GPU does not
    /// support - so it is reported rather than ignored.
    pub(crate) fn new(
        raw: &RawDevice,
        size: vk::DeviceSize,
        usage: vk::BufferUsageFlags,
        name: &'static str,
        memory_use: MemoryUse,
    ) -> Result<Self, String> {
        // Step 1: Create the buffer. Vulkan refuses a zero-sized one, and an
        // empty instance list is an ordinary frame, so the size is at least one
        // byte.
        let size = size.max(1);
        let create_info = vk::BufferCreateInfo::default()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: the create info is valid and the device outlives the buffer.
        let buffer = unsafe { raw.device().create_buffer(&create_info, None) }
            .map_err(|error| format!("`{name}` buffer: {error}"))?;
        // SAFETY: `buffer` was just created on this device.
        let requirements = unsafe { raw.device().get_buffer_memory_requirements(buffer) };

        // Step 2: Allocate memory of its own, with the device-address flag the
        // buffers read through an address need.
        let Some(memory_type_index) =
            find_memory_type(raw.memory_properties(), requirements, memory_use)
        else {
            // SAFETY: `buffer` was created here and is not bound.
            unsafe { raw.device().destroy_buffer(buffer, None) };
            return Err(format!("`{name}`: no memory type fits its requirements"));
        };
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().buffer(buffer);
        let mut flags =
            vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index)
            .push_next(&mut flags)
            .push_next(&mut dedicated);
        // SAFETY: the allocate info outlives the call and names a live buffer.
        let memory = match unsafe { raw.device().allocate_memory(&allocate_info, None) } {
            Ok(memory) => memory,
            Err(error) => {
                // SAFETY: `buffer` was created here and is not bound.
                unsafe { raw.device().destroy_buffer(buffer, None) };
                return Err(format!("`{name}` memory: {error}"));
            }
        };

        // Step 3: Bind it, or undo both.
        // SAFETY: both objects were created by this device, and the memory is at
        // least as large as the requirements read above.
        if let Err(error) = unsafe { raw.device().bind_buffer_memory(buffer, memory, 0) } {
            // SAFETY: both objects were created here and nothing uses them.
            unsafe {
                raw.device().destroy_buffer(buffer, None);
                raw.device().free_memory(memory, None);
            }
            return Err(format!("`{name}` bind: {error}"));
        }

        Ok(Self {
            buffer,
            memory,
            size,
            name,
        })
    }

    /// The buffer handle.
    pub(crate) fn handle(&self) -> vk::Buffer {
        self.buffer
    }

    /// How many bytes the buffer holds.
    pub(crate) fn size(&self) -> vk::DeviceSize {
        self.size
    }

    /// Returns the address the GPU reads the buffer at.
    ///
    /// # Safety
    ///
    /// The buffer must still be alive.
    pub(crate) unsafe fn address(&self, raw: &RawDevice) -> vk::DeviceAddress {
        // SAFETY: forwarded from this function's contract.
        unsafe { raw.buffer_address(self.buffer) }
    }

    /// Copies `bytes` into the start of the buffer.
    ///
    /// # Errors
    ///
    /// Returns a message when `bytes` does not fit or the memory cannot be
    /// mapped. Mapping per write keeps no pointer in the renderer's state, so
    /// nothing raw has to travel between threads.
    pub(crate) fn write(&self, raw: &RawDevice, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as vk::DeviceSize > self.size {
            return Err(format!(
                "`{}` holds {} bytes, not {}",
                self.name,
                self.size,
                bytes.len()
            ));
        }
        // SAFETY: the memory was allocated here and is not mapped yet.
        let pointer = unsafe {
            raw.device()
                .map_memory(self.memory, 0, self.size, vk::MemoryMapFlags::empty())
        }
        .map_err(|error| format!("`{}` map: {error}", self.name))?;
        // SAFETY: the pointer was just mapped, and the copy stays inside the
        // buffer's size, checked above.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast::<u8>(), bytes.len());
            raw.device().unmap_memory(self.memory);
        }
        Ok(())
    }

    /// Destroys the buffer and frees its memory.
    ///
    /// # Safety
    ///
    /// No command still reads the buffer: the GPU has finished every submission
    /// that named it.
    pub(crate) unsafe fn destroy(self, raw: &RawDevice) {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            raw.device().destroy_buffer(self.buffer, None);
            raw.device().free_memory(self.memory, None);
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the index of the first memory type the requirements accept.
///
/// `Upload` asks for host-visible coherent memory the CPU writes and
/// `DeviceOnly` for device-local memory. Both also require the type bits the
/// buffer set: a type the driver did not offer for this buffer is not a
/// candidate, whatever its flags say.
fn find_memory_type(
    properties: vk::PhysicalDeviceMemoryProperties,
    requirements: vk::MemoryRequirements,
    memory_use: MemoryUse,
) -> Option<u32> {
    let required = match memory_use {
        MemoryUse::DeviceOnly => vk::MemoryPropertyFlags::DEVICE_LOCAL,
        MemoryUse::Upload => {
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT
        }
    };
    (0..properties.memory_type_count).find(|index| {
        let supported = requirements.memory_type_bits & (1 << index) != 0;
        let flags = properties.memory_types[*index as usize].property_flags;
        supported && flags.contains(required)
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The memory types the test device offers: type 0 is device-local only and
    /// type 1 is host-visible coherent.
    fn two_types() -> vk::PhysicalDeviceMemoryProperties {
        let mut properties = vk::PhysicalDeviceMemoryProperties {
            memory_type_count: 2,
            ..Default::default()
        };
        properties.memory_types[0].property_flags = vk::MemoryPropertyFlags::DEVICE_LOCAL;
        properties.memory_types[1].property_flags =
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        properties
    }

    /// Requirements only a buffer in the first memory type accepts.
    fn first_type_only() -> vk::MemoryRequirements {
        vk::MemoryRequirements {
            size: 256,
            alignment: 64,
            memory_type_bits: 0b01,
        }
    }

    #[test]
    fn device_memory_is_taken_from_a_device_local_type() {
        assert_eq!(
            find_memory_type(two_types(), first_type_only(), MemoryUse::DeviceOnly),
            Some(0)
        );
    }

    #[test]
    fn upload_memory_is_taken_from_whatever_host_visible_type_is_allowed() {
        let requirements = vk::MemoryRequirements {
            memory_type_bits: 0b11,
            ..first_type_only()
        };
        assert_eq!(
            find_memory_type(two_types(), requirements, MemoryUse::Upload),
            Some(1)
        );
    }

    #[test]
    fn a_buffer_no_offered_type_fits_reports_nothing() {
        // Host-visible memory exists, but this buffer may not be placed in it.
        assert_eq!(
            find_memory_type(two_types(), first_type_only(), MemoryUse::Upload),
            None
        );
    }
}
