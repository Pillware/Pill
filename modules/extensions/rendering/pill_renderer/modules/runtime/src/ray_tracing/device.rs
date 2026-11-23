//! The raw Vulkan handles behind the device wgpu opened.
//!
//! # Responsibilities
//!
//! - Copy the Vulkan instance, physical device and device out of wgpu's own
//!   objects, so raw commands can be recorded against the device the rest of
//!   the renderer draws with ([`RawDevice::new`]).
//! - Load the acceleration-structure functions the structures are built with.
//! - Read a device address from a buffer wgpu owns, which is how a raw build
//!   reads bytes wgpu uploaded.
//!
//! # Design
//!
//! wgpu hands its objects out through `as_hal` guards that hold a lock while
//! they live, so every handle is copied out and the guard dropped at once: a
//! guard kept across frames would block wgpu from destroying any resource and
//! can deadlock. `ash::Instance` and `ash::Device` are dispatch tables holding
//! the handle, so cloning one is cheap and keeps no lock.

// External crates
use ash::vk;

// =============================================================================
// Raw device
// =============================================================================

/// The raw Vulkan objects every structure in this module is made with.
pub(crate) struct RawDevice {
    /// The instance wgpu created, holding the entry points loaded here.
    instance: ash::Instance,
    /// The GPU the device runs on.
    physical_device: vk::PhysicalDevice,
    /// The device itself: allocation, buffer creation and destruction.
    device: ash::Device,
    /// The acceleration-structure functions, which no wgpu API exposes.
    structures: ash::khr::acceleration_structure::Device,
    /// The ray tracing pipeline functions: creating pipelines, reading the
    /// shader group handles and tracing rays.
    pipelines: ash::khr::ray_tracing_pipeline::Device,
    /// The memory types and heaps, read once for every allocation.
    memory_properties: vk::PhysicalDeviceMemoryProperties,
}

impl RawDevice {
    /// Returns the raw handles, or `None` when the renderer does not run on the
    /// Vulkan backend.
    ///
    /// # Safety
    ///
    /// The handles are only valid while `device` and `instance` are alive, and
    /// every raw object made from them has to be destroyed before the wgpu
    /// device is. The renderer keeps this value in its own state, whose field
    /// order destroys it first.
    pub(crate) unsafe fn new(device: &wgpu::Device, instance: &wgpu::Instance) -> Option<Self> {
        let (raw_device, physical_device) = {
            // SAFETY: forwarded from this function's contract.
            let guard = unsafe { device.as_hal::<wgpu::hal::api::Vulkan>() }?;
            (guard.raw_device().clone(), guard.raw_physical_device())
        };
        let raw_instance = {
            // SAFETY: forwarded from this function's contract.
            let guard = unsafe { instance.as_hal::<wgpu::hal::api::Vulkan>() }?;
            guard.shared_instance().raw_instance().clone()
        };
        // SAFETY: the device belongs to the instance, and both outlive it.
        let structures = ash::khr::acceleration_structure::Device::new(&raw_instance, &raw_device);
        // SAFETY: as above.
        let pipelines = ash::khr::ray_tracing_pipeline::Device::new(&raw_instance, &raw_device);
        // SAFETY: the physical device belongs to the instance.
        let memory_properties =
            unsafe { raw_instance.get_physical_device_memory_properties(physical_device) };
        Some(Self {
            instance: raw_instance,
            physical_device,
            device: raw_device,
            structures,
            pipelines,
            memory_properties,
        })
    }

    /// The device raw objects are created on and destroyed with.
    pub(crate) fn device(&self) -> &ash::Device {
        &self.device
    }

    /// The instance the device was created from.
    pub(crate) fn instance(&self) -> &ash::Instance {
        &self.instance
    }

    /// The GPU the device runs on.
    pub(crate) fn physical_device(&self) -> vk::PhysicalDevice {
        self.physical_device
    }

    /// The acceleration-structure functions.
    pub(crate) fn structures(&self) -> &ash::khr::acceleration_structure::Device {
        &self.structures
    }

    /// The ray tracing pipeline functions.
    pub(crate) fn ray_tracing_pipelines(&self) -> &ash::khr::ray_tracing_pipeline::Device {
        &self.pipelines
    }

    /// The memory types and heaps every allocation is placed in.
    pub(crate) fn memory_properties(&self) -> vk::PhysicalDeviceMemoryProperties {
        self.memory_properties
    }

    /// Returns the address the GPU reads `buffer` at.
    ///
    /// The ray tracing device asks for `VK_KHR_buffer_device_address`, which is
    /// what lets a build read a buffer wgpu created without copying the bytes
    /// anywhere.
    ///
    /// # Safety
    ///
    /// `buffer` must be a live buffer of this device, created with
    /// `SHADER_DEVICE_ADDRESS`.
    pub(crate) unsafe fn buffer_address(&self, buffer: vk::Buffer) -> vk::DeviceAddress {
        let info = vk::BufferDeviceAddressInfo::default().buffer(buffer);
        // SAFETY: forwarded from this function's contract.
        unsafe { self.device.get_buffer_device_address(&info) }
    }
}
