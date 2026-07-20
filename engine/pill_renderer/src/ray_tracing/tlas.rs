//! Top-Level Acceleration Structure (TLAS) management.
//!
//! V1 owns one TLAS for the active scene. It is rebuilt whenever instance
//! membership, transforms, masks, or referenced BLASes change. Camera-only
//! changes do not trigger a rebuild.

/// The single active-scene TLAS and its associated state.
pub struct RayTracingTlas {
    pub tlas: wgpu::Tlas,
    pub bind_group: wgpu::BindGroup,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub instance_count: u32,
    pub capacity: u32,
    pub dirty: bool,
    /// Incremented on every successful build; used by the dirty graph.
    pub revision: u64,
}

impl RayTracingTlas {
    /// Create a new TLAS with the given maximum instance capacity.
    /// The TLAS is initially empty and must be bootstrapped before binding.
    pub fn new(
        device: &wgpu::Device,
        capacity: u32,
        label: &str,
    ) -> Self {
        let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: Some(label),
            max_instances: capacity,
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });

        // Create a bind-group layout for the TLAS binding at group 0,
        // binding slot reserved for acceleration structures.
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(&format!("{label}_tlas_bind_group_layout")),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 1, // (set = 0, binding = 1) — TLAS slot
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::AccelerationStructure {
                        vertex_return: false,
                    },
                    count: None,
                }],
            });

        // Bootstrap an empty bind group. The TLAS must be built before
        // shader use, but the binding must exist for pipeline creation.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}_tlas_bind_group")),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::AccelerationStructure(&tlas),
            }],
        });

        Self {
            tlas,
            bind_group,
            bind_group_layout,
            instance_count: 0,
            capacity,
            dirty: true,
            revision: 0,
        }
    }

    /// Grow the TLAS to a new capacity. Creates a replacement TLAS and
    /// bind group. The old TLAS must be retired after the submission
    /// that uses the new one completes.
    pub fn grow(
        &mut self,
        device: &wgpu::Device,
        new_capacity: u32,
        label: &str,
    ) -> (wgpu::Tlas, wgpu::BindGroup) {
        let old_tlas = std::mem::replace(
            &mut self.tlas,
            device.create_tlas(&wgpu::CreateTlasDescriptor {
                label: Some(label),
                max_instances: new_capacity,
                flags: wgpu::AccelerationStructureFlags::PREFER_FAST_BUILD,
                update_mode: wgpu::AccelerationStructureUpdateMode::Build,
            }),
        );
        let old_bind_group = std::mem::replace(
            &mut self.bind_group,
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("{label}_tlas_bind_group")),
                layout: &self.bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::AccelerationStructure(&self.tlas),
                }],
            }),
        );

        self.capacity = new_capacity;
        self.dirty = true;

        (old_tlas, old_bind_group)
    }

    /// Rebuild the TLAS bind group to reference the current TLAS.
    /// Needed after TLAS growth or replacement.
    pub fn rebind(&mut self, device: &wgpu::Device, label: &str) {
        self.bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{label}_tlas_bind_group")),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::AccelerationStructure(&self.tlas),
            }],
        });
    }

    /// Mark the TLAS dirty so it will be rebuilt next frame.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Compute the next power-of-two capacity for growth.
    /// Capped by both the configured max instances and the device limit.
    pub fn next_growth_capacity(
        current: u32,
        needed: u32,
        max_configured: u32,
        max_device: u32,
    ) -> Option<u32> {
        let target = needed.max(current + 1).next_power_of_two();
        let capped = target.min(max_configured).min(max_device);
        if capped <= current {
            None // Cannot grow further
        } else {
            Some(capped)
        }
    }
}
