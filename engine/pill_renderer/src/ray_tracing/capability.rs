//! Adapter enumeration, feature/limit validation, and device-policy
//! resolution for hardware ray tracing.
//!
//! All experimental-API opt-in (`unsafe { ExperimentalFeatures::enabled() }`)
//! is isolated in one reviewed helper (`request_rt_device`).

use pill_core::{info, warn, LogContext};
use pill_engine::internal::{EngineConfig, HardwareRayQueryCapabilities, RayTracingMode, RendererBackend, RendererCapabilities};

/// Structured reason why ray tracing could not be enabled.
#[derive(Debug, Clone)]
pub enum RayTracingDisabledReason {
    /// The `hardware_ray_tracing` Cargo feature was not compiled in.
    CompileTimeFeatureAbsent,
    /// Running on an unsupported target (e.g. WASM).
    TargetUnsupported,
    /// The policy is `Off`.
    PolicyOff,
    /// No surface-compatible adapter was found.
    NoSurfaceCompatibleAdapter,
    /// The adapter does not advertise `EXPERIMENTAL_RAY_QUERY`.
    FeatureBitAbsent,
    /// The adapter's advertized backend is not in the certified support matrix.
    BackendNotSupported { backend: String },
    /// One or more required acceleration-structure limits is too small.
    RequiredLimitTooSmall { limit_name: String, required: u32, actual: u32 },
    /// The device request was rejected by the backend.
    DeviceRequestRejected { reason: String },
    /// The user requested a backend set that cannot provide ray queries.
    ExplicitBackendSetIncompatible,
    /// Fallback after failed `Prefer` attempt.
    PreferFallback { reason: String },
}

impl RayTracingDisabledReason {
    pub fn as_str(&self) -> String {
        match self {
            Self::CompileTimeFeatureAbsent => "compile-time feature absent".into(),
            Self::TargetUnsupported => "target unsupported".into(),
            Self::PolicyOff => "policy off".into(),
            Self::NoSurfaceCompatibleAdapter => "no surface-compatible adapter".into(),
            Self::FeatureBitAbsent => "feature bit absent".into(),
            Self::BackendNotSupported { backend } => {
                format!("backend '{backend}' not in certified support matrix")
            }
            Self::RequiredLimitTooSmall { limit_name, required, actual } => {
                format!("limit '{limit_name}' too small: need {required}, got {actual}")
            }
            Self::DeviceRequestRejected { reason } => {
                format!("device request rejected: {reason}")
            }
            Self::ExplicitBackendSetIncompatible => {
                "requested backend set cannot provide ray queries".into()
            }
            Self::PreferFallback { reason } => {
                format!("prefer fallback: {reason}")
            }
        }
    }
}

/// The resolved ray-tracing policy and capability state after device creation.
#[derive(Debug, Clone)]
pub enum RayTracingPolicyResult {
    /// RT is enabled; the device has `EXPERIMENTAL_RAY_QUERY` and valid AS
    /// limits.
    Enabled {
        capabilities: HardwareRayQueryCapabilities,
    },
    /// RT is disabled for a specific reason.
    Disabled {
        reason: RayTracingDisabledReason,
    },
}

impl RayTracingPolicyResult {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled { .. })
    }

    pub fn capabilities(&self) -> Option<&HardwareRayQueryCapabilities> {
        match self {
            Self::Enabled { capabilities } => Some(capabilities),
            Self::Disabled { .. } => None,
        }
    }
}

/// Resolve the user-configured ray-tracing mode from `EngineConfig`.
pub fn resolve_ray_tracing_mode(config: &EngineConfig) -> RayTracingMode {
    match config.get_str("RAY_TRACING_MODE") {
        Ok(value) => RayTracingMode::from_config_string(&value),
        Err(_) => RayTracingMode::Off,
    }
}

/// Check compile-time and target preconditions for hardware ray tracing.
///
/// Returns `None` if preconditions are satisfied, or a reason why they are
/// not. This is cheap to call before any GPU work.
pub fn check_compile_time_preconditions() -> Option<RayTracingDisabledReason> {
    #[cfg(not(feature = "hardware_ray_tracing"))]
    {
        return Some(RayTracingDisabledReason::CompileTimeFeatureAbsent);
    }

    #[cfg(target_arch = "wasm32")]
    {
        return Some(RayTracingDisabledReason::TargetUnsupported);
    }

    None
}

/// Map a `wgpu::Backend` to a `RendererBackend`.
pub fn wgpu_backend_to_renderer_backend(backend: wgpu::Backend) -> RendererBackend {
    match backend {
        wgpu::Backend::Vulkan => RendererBackend::Vulkan,
        wgpu::Backend::Dx12 => RendererBackend::Dx12,
        wgpu::Backend::Metal => RendererBackend::Metal,
        wgpu::Backend::Gl => RendererBackend::Gl,
        wgpu::Backend::BrowserWebGpu => RendererBackend::BrowserWebGpu,
        _ => RendererBackend::Unknown,
    }
}

/// The certified backend support matrix for V1 ray queries.
/// Returns `true` when the backend is in the tested allowlist.
pub fn is_certified_rt_backend(backend: wgpu::Backend) -> bool {
    matches!(backend, wgpu::Backend::Vulkan)
}

/// Validate that the adapter's acceleration-structure limits meet the minimum
/// needed for the configured scene capacity.
///
/// Returns `None` on success, or the first failing limit as a reason.
pub fn validate_as_limits(
    limits: &wgpu::Limits,
    max_instances: u32,
) -> Option<RayTracingDisabledReason> {
    // Minimum values we need for a functional scene.
    let min_primitive_count: u32 = 1;
    let min_geometry_count: u32 = 1;
    let min_instance_count: u32 = max_instances.max(1);
    let min_as_per_stage: u32 = 1;
    let min_buffers_and_as_per_stage: u32 = 8;

    if limits.max_blas_primitive_count < min_primitive_count {
        return Some(RayTracingDisabledReason::RequiredLimitTooSmall {
            limit_name: "max_blas_primitive_count".into(),
            required: min_primitive_count,
            actual: limits.max_blas_primitive_count,
        });
    }
    if limits.max_blas_geometry_count < min_geometry_count {
        return Some(RayTracingDisabledReason::RequiredLimitTooSmall {
            limit_name: "max_blas_geometry_count".into(),
            required: min_geometry_count,
            actual: limits.max_blas_geometry_count,
        });
    }
    if limits.max_tlas_instance_count < min_instance_count {
        return Some(RayTracingDisabledReason::RequiredLimitTooSmall {
            limit_name: "max_tlas_instance_count".into(),
            required: min_instance_count,
            actual: limits.max_tlas_instance_count,
        });
    }
    if limits.max_acceleration_structures_per_shader_stage < min_as_per_stage {
        return Some(RayTracingDisabledReason::RequiredLimitTooSmall {
            limit_name: "max_acceleration_structures_per_shader_stage".into(),
            required: min_as_per_stage,
            actual: limits.max_acceleration_structures_per_shader_stage,
        });
    }
    if limits.max_buffers_and_acceleration_structures_per_shader_stage < min_buffers_and_as_per_stage {
        return Some(RayTracingDisabledReason::RequiredLimitTooSmall {
            limit_name: "max_buffers_and_acceleration_structures_per_shader_stage".into(),
            required: min_buffers_and_as_per_stage,
            actual: limits.max_buffers_and_acceleration_structures_per_shader_stage,
        });
    }

    None
}

/// Build `RendererCapabilities` from the created device state.
pub fn build_capabilities(
    adapter: &wgpu::Adapter,
    _device: &wgpu::Device,
    rt_policy: &RayTracingPolicyResult,
) -> RendererCapabilities {
    let info = adapter.get_info();
    let backend = wgpu_backend_to_renderer_backend(info.backend);

    let hardware_ray_query = rt_policy.capabilities().copied();

    RendererCapabilities {
        backend,
        adapter_name: info.name,
        hardware_ray_query,
    }
}

/// Log a structured startup diagnostic record for ray tracing.
pub fn log_startup_diagnostic(
    policy: RayTracingMode,
    adapter: &wgpu::Adapter,
    rt_result: &RayTracingPolicyResult,
) {
    let info = adapter.get_info();
    let backend = wgpu_backend_to_renderer_backend(info.backend);
    let features = adapter.features();

    match rt_result {
        RayTracingPolicyResult::Enabled { capabilities } => {
            info!(LogContext::Rendering =>
                "RT startup: policy={} backend={} adapter='{}' \
                 feature=EXPERIMENTAL_RAY_QUERY \
                 max_blas_prim={} max_blas_geom={} max_tlas_inst={} \
                 max_as_per_stage={} max_bufs_and_as_per_stage={}",
                policy.as_str(),
                backend.as_str(),
                info.name,
                capabilities.max_blas_primitive_count,
                capabilities.max_blas_geometry_count,
                capabilities.max_tlas_instance_count,
                capabilities.max_acceleration_structures_per_shader_stage,
                capabilities.max_buffers_and_acceleration_structures_per_shader_stage,
            );
        }
        RayTracingPolicyResult::Disabled { reason } => {
            warn!(LogContext::Rendering =>
                "RT startup: policy={} backend={} adapter='{}' \
                 experimental_query_supported={} mode=disabled reason='{}'",
                policy.as_str(),
                backend.as_str(),
                info.name,
                features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY),
                reason.as_str(),
            );
        }
    }
}
