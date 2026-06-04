// Shared bind-group definitions used by every default shader. Edit here, not
// per-shader. The Rust-side counterparts live in
// `engine/pill_renderer/src/resources/engine_parameters.rs` (EngineParams) and
// `engine/pill_renderer/src/resources/renderer_camera.rs` (CameraParams);
// any field change must be mirrored on both sides.

struct EngineParams {
    float3 fog_color;
    float  fog_density;
};
[[vk::binding(0, 0)]] ConstantBuffer<EngineParams> engine;

struct CameraParams {
    float3                camera_position;
    column_major float4x4 camera_view_projection;
};
[[vk::binding(0, 1)]] ConstantBuffer<CameraParams> camera;
