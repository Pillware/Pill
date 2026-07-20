// ray_query_canary.wgsl
//
// Minimal canary shader that exercises the `enable wgpu_ray_query;`
// directive required by wgpu 28+ / Naga for any module performing a ray
// query. This file must compile against the pinned wgpu 30.0.0 dependency.
// A compilation failure signals that a Naga upgrade has changed the
// required-extension syntax and all authored RT shaders need updating.
//
// The canary itself performs no useful work and is never used in a
// render pipeline — it exists purely as a compile-time validation gate.

enable wgpu_ray_query;

@group(0) @binding(0)
var<uniform> dummy: u32;

@compute @workgroup_size(1)
fn canary_main() {
    // Declare a ray query object to verify the type is available.
    var rq: ray_query;

    // The query must be initialized before any other operation, but
    // without a TLAS bound this would be a runtime error. For a
    // compile-only canary, just declaring the type and calling a
    // diagnostic intrinsic is sufficient to verify the extension.
    _ = &rq;
}
