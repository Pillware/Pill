//! The shadow pass: one ray tracing dispatch per surface that fills its mask.
//!
//! # Responsibilities
//!
//! - Create the ray tracing pipeline, its descriptor set layout and the shader
//!   binding table once, from the cooked SPIR-V stages ([`ShadowPass::new`]).
//! - Bind the frame's top-level structure and the surface's mask into a
//!   descriptor set, then record the dispatch into wgpu's command stream
//!   ([`ShadowPass::record`]).
//! - Destroy every raw object it owns, before the device it was made on.
//!
//! # Design
//!
//! wgpu exposes no way to create a ray tracing pipeline or to dispatch one, so
//! this is raw Vulkan: modules, groups, the table, the set and
//! `vkCmdTraceRaysKHR`. It is recorded into an encoder the renderer already
//! owns, so the dispatch sits in the same command stream as the raster work
//! and the queue orders both.
//!
//! One descriptor set is kept per (frame slot, surface): the frame slot keeps a
//! set from being rewritten while a submitted frame still executes it, and the
//! surface keeps two surfaces of one frame from fighting over the mask they
//! bind.

// Standard library
use std::collections::HashMap;
use std::ffi::CStr;
use std::mem;

// External crates
use ash::vk;

// Current crate
use crate::ray_tracing::{
    device::RawDevice,
    memory::{MemoryUse, RawBuffer},
};

// =============================================================================
// Constants
// =============================================================================

/// The cooked ray generation stage.
const RAYGEN_STAGE: &[u8] = include_bytes!("../../res/shaders/raytracing/shadow_raygen.spv");
/// The cooked miss stage.
const MISS_STAGE: &[u8] = include_bytes!("../../res/shaders/raytracing/shadow_miss.spv");
/// The cooked closest-hit stage.
const CLOSEST_HIT_STAGE: &[u8] =
    include_bytes!("../../res/shaders/raytracing/shadow_closesthit.spv");

/// The entry point every stage's module carries.
///
/// slangc names each stage's SPIR-V entry point main, whatever the source
/// called it, and a ray tracing pipeline names the entry it wants, so all
/// three stages are addressed by that one name.
const STAGE_ENTRY: &CStr = c"main";

/// How many shader groups the pipeline has: one ray generation, one miss and
/// one hit group.
const SHADER_GROUP_COUNT: usize = 3;

/// How deep a ray may recurse. One bounce is all a mask needs.
const MAXIMUM_RECURSION_DEPTH: u32 = 1;

// =============================================================================
// Push constants
// =============================================================================

/// The values the ray generation stage reads for one dispatch, laid out as its
/// `ShadowPush` declares them: the surface camera's inverse view-projection,
/// then the mask's size.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ShadowPushConstants {
    /// Clip space back to world space, column major.
    pub(crate) inverse_view_projection: [[f32; 4]; 4],
    /// How many pixels wide the mask is.
    pub(crate) mask_width: u32,
    /// How many pixels tall the mask is.
    pub(crate) mask_height: u32,
    /// Padding to the block's std140 size, which the shader declares too.
    pub(crate) reserved: [u32; 2],
}

// =============================================================================
// Shadow pass
// =============================================================================

/// The ray tracing pipeline, its binding table and the sets the dispatch binds.
pub(crate) struct ShadowPass {
    /// The pipeline itself.
    pipeline: vk::Pipeline,
    /// The layout the dispatch binds: the set and the push constants.
    layout: vk::PipelineLayout,
    /// The one set: the top-level structure and the mask.
    descriptor_layout: vk::DescriptorSetLayout,
    /// Where the sets above are allocated from.
    descriptor_pool: vk::DescriptorPool,
    /// One set per (frame slot, surface).
    sets: HashMap<(usize, u32), vk::DescriptorSet>,
    /// The modules the pipeline was built from, kept until it is destroyed.
    shader_modules: Vec<vk::ShaderModule>,
    /// The shader binding table: one record per group.
    binding_table: RawBuffer,
    /// The ray generation region of the table.
    raygen_region: vk::StridedDeviceAddressRegionKHR,
    /// The miss region of the table.
    miss_region: vk::StridedDeviceAddressRegionKHR,
    /// The hit region of the table.
    hit_region: vk::StridedDeviceAddressRegionKHR,
}

impl ShadowPass {
    /// Builds the pipeline, its layout and its binding table.
    ///
    /// # Errors
    ///
    /// Returns a message when a module, the layout, the pool, the pipeline or
    /// the table cannot be created. The ray tracing path is then reported as
    /// unusable rather than retried every frame.
    pub(crate) fn new(raw: &RawDevice) -> Result<Self, String> {
        // Step 1: The device's shader-group handle size and alignments, which
        // the binding table's layout follows.
        let properties = ray_tracing_properties(raw);
        let handle_size = properties.shader_group_handle_size as vk::DeviceSize;
        let handle_alignment = properties.shader_group_handle_alignment.max(1) as vk::DeviceSize;
        let base_alignment = properties.shader_group_base_alignment.max(1) as vk::DeviceSize;
        let record_size = align_up(handle_size, handle_alignment);

        // Step 2: The three modules.
        let raygen = ShaderStage::new(
            raw,
            RAYGEN_STAGE,
            vk::ShaderStageFlags::RAYGEN_KHR,
            STAGE_ENTRY,
        )?;
        let miss = ShaderStage::new(raw, MISS_STAGE, vk::ShaderStageFlags::MISS_KHR, STAGE_ENTRY)?;
        let closest_hit = ShaderStage::new(
            raw,
            CLOSEST_HIT_STAGE,
            vk::ShaderStageFlags::CLOSEST_HIT_KHR,
            STAGE_ENTRY,
        )?;
        let shader_modules: Vec<vk::ShaderModule> = [&raygen, &miss, &closest_hit]
            .iter()
            .map(|stage| stage.module)
            .collect();

        // Step 3: The layout of the set the dispatch binds, and the layout the
        // push constants are declared on.
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::RAYGEN_KHR),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::RAYGEN_KHR),
        ];
        // SAFETY: the bindings are valid, and the device outlives the layout.
        let descriptor_layout = unsafe {
            raw.device().create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                None,
            )
        }
        .map_err(|error| format!("shadow descriptor layout: {error}"))?;
        let push_constant_size = mem::size_of::<ShadowPushConstants>() as u32;
        let push_constant_range = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::RAYGEN_KHR)
            .offset(0)
            .size(push_constant_size)];
        let set_layouts = [descriptor_layout];
        // SAFETY: the set layout was just created and the range is a constant.
        let layout = unsafe {
            raw.device().create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&push_constant_range),
                None,
            )
        }
        .map_err(|error| format!("shadow pipeline layout: {error}"))?;

        // Step 4: The pipeline: one general group for the ray generation stage,
        // one for the miss stage, and a triangle hit group for the closest-hit
        // stage.
        let stages = [
            raygen.pipeline_stage(),
            miss.pipeline_stage(),
            closest_hit.pipeline_stage(),
        ];
        let unused = vk::SHADER_UNUSED_KHR;
        let groups = [
            vk::RayTracingShaderGroupCreateInfoKHR::default()
                .ty(vk::RayTracingShaderGroupTypeKHR::GENERAL)
                .general_shader(0)
                .closest_hit_shader(unused)
                .any_hit_shader(unused)
                .intersection_shader(unused),
            vk::RayTracingShaderGroupCreateInfoKHR::default()
                .ty(vk::RayTracingShaderGroupTypeKHR::GENERAL)
                .general_shader(1)
                .closest_hit_shader(unused)
                .any_hit_shader(unused)
                .intersection_shader(unused),
            vk::RayTracingShaderGroupCreateInfoKHR::default()
                .ty(vk::RayTracingShaderGroupTypeKHR::TRIANGLES_HIT_GROUP)
                .general_shader(unused)
                .closest_hit_shader(2)
                .any_hit_shader(unused)
                .intersection_shader(unused),
        ];
        let pipeline_info = [vk::RayTracingPipelineCreateInfoKHR::default()
            .stages(&stages)
            .groups(&groups)
            .max_pipeline_ray_recursion_depth(MAXIMUM_RECURSION_DEPTH)
            .layout(layout)];
        // SAFETY: every stage, group and layout was just created on this
        // device, and a pipeline created without a deferred operation needs
        // none.
        let pipelines = unsafe {
            raw.ray_tracing_pipelines().create_ray_tracing_pipelines(
                vk::DeferredOperationKHR::null(),
                vk::PipelineCache::null(),
                &pipeline_info,
                None,
            )
        }
        .map_err(|(_, error)| format!("shadow pipeline: {error}"))?;
        let Some(pipeline) = pipelines.first().copied() else {
            return Err("shadow pipeline: the device created none".to_string());
        };

        // Step 5: The binding table: the three group handles, each in its own
        // region, aligned the way the device asks for.
        // SAFETY: the pipeline was just created with three groups, and the
        // buffer is sized for all of their handles.
        let handles = unsafe {
            raw.ray_tracing_pipelines()
                .get_ray_tracing_shader_group_handles(
                    pipeline,
                    0,
                    SHADER_GROUP_COUNT as u32,
                    SHADER_GROUP_COUNT * handle_size as usize,
                )
        }
        .map_err(|error| format!("shadow shader group handles: {error}"))?;
        let raygen_offset = 0;
        let miss_offset = align_up(record_size, base_alignment);
        let hit_offset = miss_offset + align_up(record_size, base_alignment);
        let table_size = hit_offset + record_size;
        let binding_table = RawBuffer::new(
            raw,
            table_size,
            vk::BufferUsageFlags::SHADER_BINDING_TABLE_KHR
                | vk::BufferUsageFlags::SHADER_DEVICE_ADDRESS
                | vk::BufferUsageFlags::STORAGE_BUFFER,
            "shadow shader binding table",
            MemoryUse::Upload,
        )?;
        let mut table = vec![0u8; table_size as usize];
        for (group, offset) in [(0usize, raygen_offset), (1, miss_offset), (2, hit_offset)] {
            let start = offset as usize;
            let handle = &handles[group * handle_size as usize..(group + 1) * handle_size as usize];
            table[start..start + handle.len()].copy_from_slice(handle);
        }
        binding_table.write(raw, &table)?;
        // SAFETY: the table was just written and stays alive with the pass.
        let table_address = unsafe { binding_table.address(raw) };
        let region = |offset: vk::DeviceSize| vk::StridedDeviceAddressRegionKHR {
            device_address: table_address + offset,
            stride: record_size,
            size: record_size,
        };
        let raygen_region = region(raygen_offset);
        let miss_region = region(miss_offset);
        let hit_region = region(hit_offset);

        // Step 6: The pool the per-frame sets come from.
        let pool_sizes = [
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
                .descriptor_count(8),
            vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(8),
        ];
        let descriptor_pool = unsafe {
            raw.device().create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default()
                    .max_sets(8)
                    .pool_sizes(&pool_sizes),
                None,
            )
        }
        .map_err(|error| format!("shadow descriptor pool: {error}"))?;

        Ok(Self {
            pipeline,
            layout,
            descriptor_layout,
            descriptor_pool,
            sets: HashMap::new(),
            shader_modules,
            binding_table,
            raygen_region,
            miss_region,
            hit_region,
        })
    }

    /// Records one surface's dispatch: bind the frame's top-level structure and
    /// the surface's mask, then trace one ray per pixel.
    ///
    /// The dispatch is recorded into `command_buffer`, which the caller
    /// recorded raster work into as well, so it is ordered with it.
    ///
    /// # Errors
    ///
    /// Returns a message when the set cannot be allocated or the table is
    /// unusable.
    pub(crate) fn record(
        &mut self,
        raw: &RawDevice,
        command_buffer: vk::CommandBuffer,
        slot: usize,
        surface: u32,
        top_level: vk::AccelerationStructureKHR,
        mask: vk::Buffer,
        size: (u32, u32),
        inverse_view_projection: [[f32; 4]; 4],
    ) -> Result<(), String> {
        let set = self.set_for(raw, slot, surface, top_level, mask)?;
        let push = ShadowPushConstants {
            inverse_view_projection,
            mask_width: size.0.max(1),
            mask_height: size.1.max(1),
            reserved: [0, 0],
        };
        let push_bytes: &[u8] = bytemuck::bytes_of(&push);
        let device = raw.device();
        // SAFETY: every handle bound here was created by this pass on this
        // device, and the command buffer is recording.
        unsafe {
            device.cmd_bind_pipeline(
                command_buffer,
                vk::PipelineBindPoint::RAY_TRACING_KHR,
                self.pipeline,
            );
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::RAY_TRACING_KHR,
                self.layout,
                0,
                &[set],
                &[],
            );
            device.cmd_push_constants(
                command_buffer,
                self.layout,
                vk::ShaderStageFlags::RAYGEN_KHR,
                0,
                push_bytes,
            );
            let empty = vk::StridedDeviceAddressRegionKHR::default();
            raw.ray_tracing_pipelines().cmd_trace_rays(
                command_buffer,
                &self.raygen_region,
                &self.miss_region,
                &self.hit_region,
                &empty,
                push.mask_width,
                push.mask_height,
                1,
            );
        }
        Ok(())
    }

    /// Returns the set for `slot` and `surface`, pointing it at `top_level` and
    /// `mask`.
    ///
    /// A set is rewritten only for the frame slot that is being recorded, which
    /// no submitted frame is still reading.
    fn set_for(
        &mut self,
        raw: &RawDevice,
        slot: usize,
        surface: u32,
        top_level: vk::AccelerationStructureKHR,
        mask: vk::Buffer,
    ) -> Result<vk::DescriptorSet, String> {
        let key = (slot, surface);
        let set = match self.sets.get(&key) {
            Some(set) => *set,
            None => {
                let layouts = [self.descriptor_layout];
                // SAFETY: the pool and the layout were created by this pass.
                let allocated = unsafe {
                    raw.device().allocate_descriptor_sets(
                        &vk::DescriptorSetAllocateInfo::default()
                            .descriptor_pool(self.descriptor_pool)
                            .set_layouts(&layouts),
                    )
                }
                .map_err(|error| format!("shadow descriptor set: {error}"))?;
                let Some(set) = allocated.first().copied() else {
                    return Err("shadow descriptor set: the pool returned none".to_string());
                };
                self.sets.insert(key, set);
                set
            }
        };
        let mut acceleration_structure = vk::WriteDescriptorSetAccelerationStructureKHR::default()
            .acceleration_structures(std::slice::from_ref(&top_level));
        let structure_write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::ACCELERATION_STRUCTURE_KHR)
            .descriptor_count(1)
            .push_next(&mut acceleration_structure);
        let mask_info = vk::DescriptorBufferInfo::default()
            .buffer(mask)
            .offset(0)
            .range(vk::WHOLE_SIZE);
        let mask_write = vk::WriteDescriptorSet::default()
            .dst_set(set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .buffer_info(std::slice::from_ref(&mask_info));
        // SAFETY: the set belongs to this pool, and both writes name live
        // objects of this device.
        unsafe {
            raw.device()
                .update_descriptor_sets(&[structure_write, mask_write], &[]);
        }
        Ok(set)
    }

    /// Destroys every raw object the pass owns.
    ///
    /// # Safety
    ///
    /// No command still uses the pipeline, the table or the sets.
    pub(crate) unsafe fn destroy(mut self, raw: &RawDevice) {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            raw.device().destroy_pipeline(self.pipeline, None);
            raw.device().destroy_pipeline_layout(self.layout, None);
            raw.device()
                .destroy_descriptor_pool(self.descriptor_pool, None);
            raw.device()
                .destroy_descriptor_set_layout(self.descriptor_layout, None);
            for module in self.shader_modules.drain(..) {
                raw.device().destroy_shader_module(module, None);
            }
            self.binding_table.destroy(raw);
        }
    }
}

// =============================================================================
// Shader stage
// =============================================================================

/// One cooked stage: its module and the entry point the pipeline names.
struct ShaderStage {
    /// The module the device compiled.
    module: vk::ShaderModule,
    /// The stage the module's entry point runs at.
    stage: vk::ShaderStageFlags,
    /// The entry point's name.
    entry: &'static CStr,
}

impl ShaderStage {
    /// Loads one cooked `.spv` file into a module.
    fn new(
        raw: &RawDevice,
        bytes: &[u8],
        stage: vk::ShaderStageFlags,
        entry: &'static CStr,
    ) -> Result<Self, String> {
        let mut reader = std::io::Cursor::new(bytes);
        let words = ash::util::read_spv(&mut reader)
            .map_err(|error| format!("shadow stage is not SPIR-V: {error}"))?;
        // SAFETY: the words are a whole SPIR-V module, read from the cook's own
        // output.
        let module = unsafe {
            raw.device()
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&words), None)
        }
        .map_err(|error| format!("shadow shader module: {error}"))?;
        Ok(Self {
            module,
            stage,
            entry,
        })
    }

    /// The pipeline stage this module runs in.
    fn pipeline_stage(&self) -> vk::PipelineShaderStageCreateInfo<'_> {
        vk::PipelineShaderStageCreateInfo::default()
            .stage(self.stage)
            .module(self.module)
            .name(self.entry)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Rounds `value` up to the next multiple of `alignment`.
fn align_up(value: vk::DeviceSize, alignment: vk::DeviceSize) -> vk::DeviceSize {
    let alignment = alignment.max(1);
    value.div_ceil(alignment) * alignment
}

/// The device's ray tracing pipeline properties: the handle size and the
/// alignments the binding table follows.
fn ray_tracing_properties(
    raw: &RawDevice,
) -> vk::PhysicalDeviceRayTracingPipelinePropertiesKHR<'_> {
    let mut properties = vk::PhysicalDeviceRayTracingPipelinePropertiesKHR::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut properties);
    // SAFETY: the chain is built from live locals.
    unsafe {
        raw.instance()
            .get_physical_device_properties2(raw.physical_device(), &mut properties2);
    }
    properties
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_push_constant_block_holds_the_matrix_and_the_size() {
        assert_eq!(mem::size_of::<ShadowPushConstants>(), 80);
    }

    #[test]
    fn the_binding_table_regions_follow_the_device_alignment() {
        // The record is the handle size rounded up to the handle alignment, and
        // each region starts at the group base alignment.
        assert_eq!(align_up(32, 32), 32);
        assert_eq!(align_up(33, 32), 64);
        assert_eq!(align_up(0, 64), 0);
        let record = align_up(32, 32);
        let miss = align_up(record, 64);
        let hit = miss + align_up(record, 64);
        assert_eq!((miss, hit), (64, 128));
    }
}
