//! Hardware ray tracing: whether this build and this machine can trace rays.
//!
//! # Responsibilities
//!
//! - Decide whether the path can run - the build must carry the feature, the
//!   backend must be Vulkan and the adapter must offer the wgpu features the
//!   path needs - and name the reason when it cannot.
//! - Name the extra device features the renderer asks for when it runs.
//!
//! # Design
//!
//! wgpu-hal 30 already knows how to open a Vulkan device for ray tracing:
//! requesting `Features::EXPERIMENTAL_RAY_TRACING_PIPELINES` (with wgpu's
//! experimental token) makes it enable `VK_KHR_deferred_host_operations`,
//! `VK_KHR_acceleration_structure`, `VK_KHR_buffer_device_address` and
//! `VK_KHR_ray_tracing_pipeline` with its feature struct, and it advertises
//! that feature only when the driver supports the extension. So the renderer
//! asks for wgpu features instead of building a `VkDeviceCreateInfo` itself,
//! and there is no hand-written extension list to keep right.
//!
//! The decision is one value with its reason, so the startup line and the
//! tests read the same answer and a session can say why the path did not run.
//! The build option is the only ask: a build compiled with the `ray_tracing`
//! feature takes the path whenever its adapter supports it, and a game turns
//! the shadows it produces on and off with
//! [`pill_renderer_api::RayTracedShadows`].

// External crates
use wgpu::Features;

// =============================================================================
// Modules
// =============================================================================

/// The acceleration structures: one bottom-level structure per mesh, one
/// top-level structure for the frame.
#[cfg(all(feature = "ray_tracing", not(target_arch = "wasm32")))]
pub(crate) mod acceleration;
/// The raw Vulkan handles behind wgpu's device, and the functions loaded from
/// them.
#[cfg(all(feature = "ray_tracing", not(target_arch = "wasm32")))]
pub(crate) mod device;
/// Buffers with an allocation of their own, for what wgpu has no usage flag
/// for.
#[cfg(all(feature = "ray_tracing", not(target_arch = "wasm32")))]
pub(crate) mod memory;
/// The raw ray tracing pipeline whose dispatch fills each surface's shadow
/// mask.
#[cfg(all(feature = "ray_tracing", not(target_arch = "wasm32")))]
pub(crate) mod shadow_pass;

/// The acceleration structures the renderer builds alongside its meshes.
#[cfg(all(feature = "ray_tracing", not(target_arch = "wasm32")))]
pub(crate) use acceleration::RayTracingScene;

// =============================================================================
// Decision
// =============================================================================

/// Whether this run traces rays, and why not when it does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// The path runs: the device is opened with the ray tracing features.
    On,
    /// The path does not run, and this is the reason the startup line prints.
    Off(&'static str),
}

impl Decision {
    /// Whether the ray tracing path runs.
    pub(crate) fn is_on(self) -> bool {
        matches!(self, Self::On)
    }

    /// The reason the path is off, or `None` when it runs.
    pub(crate) fn reason(self) -> Option<&'static str> {
        match self {
            Self::On => None,
            Self::Off(reason) => Some(reason),
        }
    }
}

// =============================================================================
// The decision
// =============================================================================

/// Decides whether the ray tracing path runs, from what the build and the
/// adapter offer.
///
/// The reasons are the ones the startup line prints, so they say what a person
/// can change: compile the feature, use Vulkan, or run on a driver that
/// supports the extension.
pub(crate) fn decide(
    compiled: bool,
    backend: wgpu::Backend,
    adapter_features: Features,
) -> Decision {
    if !compiled {
        return Decision::Off("feature not compiled");
    }
    if backend != wgpu::Backend::Vulkan {
        return Decision::Off("not the Vulkan backend");
    }
    if !adapter_features.contains(device_features()) {
        return Decision::Off("adapter without the ray tracing extensions");
    }
    Decision::On
}

/// The device features the renderer adds when the path runs.
///
/// `EXPERIMENTAL_RAY_QUERY` gives wgpu's acceleration structures and the buffer
/// device addresses wgpu-hal allocates for them; `EXPERIMENTAL_RAY_TRACING_PIPELINES`
/// is what makes wgpu-hal enable the ray tracing pipeline extension the raw
/// passes are recorded with.
pub(crate) fn device_features() -> Features {
    Features::EXPERIMENTAL_RAY_QUERY | Features::EXPERIMENTAL_RAY_TRACING_PIPELINES
}

/// Whether this build carries the feature.
pub(crate) const COMPILED: bool = cfg!(feature = "ray_tracing");

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_the_feature_says_so_before_anything_else() {
        assert_eq!(
            decide(false, wgpu::Backend::Vulkan, Features::all()),
            Decision::Off("feature not compiled")
        );
    }

    #[test]
    fn another_backend_is_refused_and_a_vulkan_one_with_the_extensions_runs() {
        assert_eq!(
            decide(true, wgpu::Backend::Dx12, Features::all()),
            Decision::Off("not the Vulkan backend")
        );
        assert_eq!(
            decide(true, wgpu::Backend::Vulkan, Features::all()),
            Decision::On
        );
    }

    #[test]
    fn a_vulkan_adapter_without_the_extensions_says_which_part_is_missing() {
        assert_eq!(
            decide(
                true,
                wgpu::Backend::Vulkan,
                Features::EXPERIMENTAL_RAY_QUERY
            ),
            Decision::Off("adapter without the ray tracing extensions")
        );
    }

    #[test]
    fn the_decision_carries_its_reason_for_the_startup_line() {
        assert_eq!(Decision::On.reason(), None);
        assert!(Decision::On.is_on());
        assert_eq!(
            Decision::Off("not requested").reason(),
            Some("not requested")
        );
        assert!(!Decision::Off("not requested").is_on());
    }
}
