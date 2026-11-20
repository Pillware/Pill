// Fullscreen triangle: three vertices, no vertex buffer, no inputs.
//
// The clip-space corner comes from SV_VertexID and the uv is derived from it,
// which is the shape every post-processing pass in this engine draws. A pass
// that declares `PassKind::Fullscreen` must have a `vs_main` like this one; the
// renderer draws 0..3 with no vertex buffers and no depth.
//
// Edit here - `pill_assets` regenerates the .wgsl.

#include "common.hlsl"

// No input struct: the only stage input is SV_VertexID, and wrapping a system
// value in a struct would invent an interface that does not exist.
struct VertexOutput
{
    float4 position : SV_Position;

    [[vk::location(0)]] float2 texture_coords : TEXCOORD0;
};

[shader("vertex")]
VertexOutput vs_main(uint vertex_index : SV_VertexID)
{
    float2 position = fullscreen_triangle_corner(vertex_index);

    VertexOutput output;
    output.position = float4(position, 0.0, 1.0);
    // v flips because clip space counts up and a texture counts down.
    output.texture_coords = float2(
        position.x * 0.5 + 0.5,
        -position.y * 0.5 + 0.5
    );

    return output;
}
