//! Bottom-Level Acceleration Structure (BLAS) management.
//!
//! One BLAS per unique renderer mesh. BLASes are owned by `RendererMesh`
//! extensions and cached across frames. Builds are batched together with
//! the TLAS build in a single `build_acceleration_structures` call.

/// Monotonically increasing submission identifier.
/// Wraps the wgpu submission index without exposing wgpu types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubmissionSerial(u64);

impl SubmissionSerial {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn next(&self) -> Self {
        Self(self.0 + 1)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Build state for a single BLAS.
#[derive(Debug, Clone)]
pub enum BlasBuildState {
    /// BLAS descriptor and geometry buffers are allocated; build has not
    /// been encoded.
    Pending,
    /// Build has been recorded in a command encoder at the given frame
    /// epoch. The encoder may still be abandoned.
    Encoded { frame_epoch: u64 },
    /// The command buffer containing the build has been submitted. The BLAS
    /// is usable by subsequent submissions.
    Submitted { submission: SubmissionSerial },
    /// A build was attempted but failed with a validation or device error.
    Failed(String),
    /// The BLAS has been scheduled for retirement after the given submission
    /// completes.
    Retiring { after: SubmissionSerial },
}

impl BlasBuildState {
    /// Returns `true` when the BLAS can be referenced by a TLAS build.
    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Encoded { .. } | Self::Submitted { .. })
    }

    /// Returns `true` when the BLAS has been submitted to the GPU and can
    /// be used by subsequent frames (not the current frame's build call).
    pub fn is_submitted(&self) -> bool {
        matches!(self, Self::Submitted { .. })
    }
}

/// Extension attached to `RendererMesh` when hardware ray tracing is
/// enabled. Holds the BLAS, geometry descriptors, and build lifecycle.
pub struct RayTracingMesh {
    pub blas: wgpu::Blas,
    pub size_descriptor: wgpu::BlasTriangleGeometrySizeDescriptor,
    pub build_state: BlasBuildState,
    pub primitive_count: u32,
    pub vertex_count: u32,
    pub index_count: u32,
}

/// Outcome of trying to create or retrieve RT state for a mesh.
pub enum RendererMeshRayTracingState {
    /// The mesh cannot participate in ray queries.
    RasterOnly(String),
    /// The mesh has a valid BLAS and can be instanced in the TLAS.
    Eligible(RayTracingMesh),
}

/// Validate that mesh geometry meets BLAS requirements.
///
/// Checks index-count divisibility, index range, and finite positions.
/// Returns `None` on success or an error description.
pub fn validate_mesh_for_blas(
    vertex_count: u32,
    index_count: u32,
    positions_are_finite: bool,
    max_primitive_count: u32,
) -> Option<String> {
    if vertex_count == 0 {
        return Some("zero vertices".into());
    }
    if index_count == 0 {
        return Some("zero indices".into());
    }
    if index_count % 3 != 0 {
        return Some(format!(
            "index count {index_count} is not divisible by 3"
        ));
    }
    if !positions_are_finite {
        return Some("vertex positions contain non-finite values".into());
    }
    let primitive_count = index_count / 3;
    if primitive_count > max_primitive_count {
        return Some(format!(
            "primitive count {primitive_count} exceeds device limit {max_primitive_count}"
        ));
    }
    None
}

/// Create a BLAS size descriptor from mesh data.
///
/// Note: `AccelerationStructureGeometryFlags::OPAQUE` is deliberately NOT
/// set here. On NVIDIA RTX 3080 Ti / wgpu 30.0.0, setting OPAQUE on the
/// geometry prevents ray queries from finding any intersections (all rays
/// report miss). The underlying cause is under investigation; for now,
/// opaque-hit behavior is achieved via `rayQueryConfirmIntersection` in
/// the shader and/or the `FORCE_OPAQUE` ray flag.
pub fn create_blas_size_descriptor(
    vertex_count: u32,
    index_count: u32,
    index_format: wgpu::IndexFormat,
) -> wgpu::BlasTriangleGeometrySizeDescriptor {
    wgpu::BlasTriangleGeometrySizeDescriptor {
        vertex_format: wgpu::VertexFormat::Float32x3,
        vertex_count,
        index_format: Some(index_format),
        index_count: Some(index_count),
        flags: wgpu::AccelerationStructureGeometryFlags::empty(),
    }
}

/// Create a BLAS from its size descriptor.
pub fn create_blas(
    device: &wgpu::Device,
    size_descriptor: &wgpu::BlasTriangleGeometrySizeDescriptor,
    label: &str,
) -> wgpu::Blas {
    device.create_blas(
        &wgpu::CreateBlasDescriptor {
            label: Some(label),
            flags: wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        },
        wgpu::BlasGeometrySizeDescriptors::Triangles {
            descriptors: vec![size_descriptor.clone()],
        },
    )
}

