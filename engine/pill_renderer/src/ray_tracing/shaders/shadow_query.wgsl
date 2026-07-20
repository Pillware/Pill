// shadow_query.wgsl
//
// Inline ray-query helper for single-bounce hard shadow evaluation.
// Called from a fragment shader that has reconstructed world position
// and geometric normal from the raster vertex stage.
//
// Pinned to wgpu 30.0.0 / Naga. Requires `enable wgpu_ray_query;` at the
// top of the including module (NOT duplicated here — this file is
// `#include`-style concatenated by the Rust host before shader creation).

// --- Ray flags ---
// Shadow rays: skip face culling so both front and back faces occlude.
// FORCE_OPAQUE (bit 0) is NOT set here because of a wgpu 30.0.0 issue
// where OPAQUE geometry flags prevent intersection detection on some
// drivers (observed on NVIDIA RTX 3080 Ti). Instead, we explicitly
// confirm each candidate intersection.
const RAY_FLAGS: u32 = 4u;  // SKIP_CULLING

// --- Light data (matches RenderLight layout, std140-aligned) ---
struct LightData {
    position: vec3<f32>,
    // 4 bytes padding
    color: vec3<f32>,
    // 4 bytes padding
    intensity: f32,
    shadow_cull_mask: u32,
    // 8 bytes padding
}

// --- Query helper ---

/// Cast a shadow ray from `origin` toward `light_position` and return
/// `true` when the light is visible (no occlusion hit).
///
/// # Parameters
/// - `tlas`:        The active-scene TLAS (bound at group 0, binding 1).
/// - `origin`:      World-space ray origin, offset along the geometric normal.
/// - `light_position`: World-space position of the point light.
/// - `shadow_cull_mask`: Instance cull mask ANDed with TlasInstance::mask.
/// - `t_min`:       Minimum ray distance (must be > 0.0 to avoid self-hit).
/// - `endpoint_bias`: Small bias subtracted from `t_max` to avoid hitting
///                   the light surface.
///
/// # Returns
/// `true` when no hit is found between `t_min` and `t_max`, meaning the
/// light is visible from `origin`.
fn shadow_ray_visible(
    tlas: acceleration_structure,
    origin: vec3<f32>,
    light_position: vec3<f32>,
    shadow_cull_mask: u32,
    t_min: f32,
    endpoint_bias: f32,
) -> bool {
    // Reject degenerate inputs before constructing the ray query.
    let to_light_vec: vec3<f32> = light_position - origin;
    let distance_sq: f32 = dot(to_light_vec, to_light_vec);
    if distance_sq <= 0.0 {
        return true; // Light at or behind origin
    }
    let distance: f32 = sqrt(distance_sq);
    let direction: vec3<f32> = to_light_vec / distance;

    let t_max: f32 = distance - endpoint_bias;
    if t_max <= t_min {
        return true; // Degenerate interval
    }

    // Reject non-finite values.
    if (!all(isFinite(origin))) || (!all(isFinite(direction))) {
        return true;
    }

    // Construct the RayDesc and initialize the query.
    var rq: ray_query;
    var ray_desc: RayDesc;
    ray_desc.origin = origin;
    ray_desc.dir = direction;
    ray_desc.tmin = t_min;
    ray_desc.tmax = t_max;
    ray_desc.flags = RAY_FLAGS;
    ray_desc.cull_mask = shadow_cull_mask;
    rayQueryInitialize(&rq, tlas, ray_desc);

    // Traverse: confirm each candidate intersection.
    // With geometry flags empty (not OPAQUE), every candidate must be
    // explicitly confirmed.
    var hit: bool = false;
    loop {
        if (rayQueryProceed(&rq)) {
            rayQueryConfirmIntersection(&rq);
            hit = true;
            break; // Shadow: first hit is enough to occlude.
        } else {
            break;
        }
    }

    // true = no occlusion (light visible), false = occluded
    return !hit;
}
