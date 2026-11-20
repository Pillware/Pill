// Shared bind-group definitions used by every default shader. Edit here, not
// per-shader. The Rust-side counterparts live in the GPU module's own crate:
// `pill_master_renderer/src/resources/engine_parameters.rs` (EngineParams) and
// `pill_master_renderer/src/resources/renderer_camera.rs` (CameraParams); any
// field change must be mirrored on both sides.

// `EngineParametersData` carries the fog pair and nothing else, in that order.
struct EngineParams
{
    float3 fog_color;
    float  fog_density;
};

[[vk::binding(0, 0)]]
ConstantBuffer<EngineParams> engine;

struct CameraParams
{
    float3                camera_position;
    column_major float4x4 camera_view_projection;
    // Clip space back to world space: how a skybox turns a pixel into a view
    // direction.
    column_major float4x4 camera_inverse_view_projection;
};

[[vk::binding(0, 1)]]
ConstantBuffer<CameraParams> camera;

// Corner `vertex_index` of the triangle a fullscreen pass draws: three vertices,
// no vertex buffer, no input struct.
//
// Oversized on purpose: the triangle covers the target without touching its
// edges, so no interpolation runs off the end of the visible area. Both
// fullscreen vertex stages in the crate draw through this, so where the corners
// are is stated once.
float2 fullscreen_triangle_corner(uint vertex_index)
{
    float2 corners[3] = {
        float2(-1.0, -3.0),
        float2( 3.0,  1.0),
        float2(-1.0,  1.0)
    };
    return corners[vertex_index];
}
