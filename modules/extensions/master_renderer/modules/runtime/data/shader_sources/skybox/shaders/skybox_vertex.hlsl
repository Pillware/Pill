// Skybox vertex stage: one fullscreen triangle at the far plane.
//
// The same oversized triangle the post-processing passes draw, but placed at
// depth 1.0 rather than left without depth: a `PassKind::Skybox` pass tests it
// against the depth the geometry passes wrote (less-or-equal, no write), so the
// sky fills only the pixels no mesh covered. The clip-space corner travels to
// the fragment stage, which turns it back into a world direction.
//
// Edit here - `pill_assets` regenerates the .wgsl.

#include "common.hlsl"

struct VertexOutput
{
    float4 position : SV_Position;

    [[vk::location(0)]] float2 clip_position : TEXCOORD0;
};

[shader("vertex")]
VertexOutput vs_main(uint vertex_index : SV_VertexID)
{
    float2 position = fullscreen_triangle_corner(vertex_index);

    VertexOutput output;
    // z = w: depth 1.0, the far plane, behind everything a geometry pass drew.
    output.position = float4(position, 1.0, 1.0);
    output.clip_position = position;
    return output;
}
