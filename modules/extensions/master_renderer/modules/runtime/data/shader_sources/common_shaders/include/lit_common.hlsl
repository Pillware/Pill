// What the lit stages share: the vertex-to-fragment interface and the fog.
//
// Included by `common_shaders/default_vertex.hlsl`, and by both lit fragment
// stages (`pbr_pipeline` and `simple_pipeline`). Edit here - `pill_assets`
// re-cooks every stage that includes it.

#include "common.hlsl"

// The lit interface, declared once: the vertex stage returns it and every lit
// fragment stage takes it as its input, so the locations on both sides cannot
// drift apart and a field added here reaches both.
//
// The position rides in it because a vertex stage has to return one. A
// fragment stage that never reads it pays nothing for its presence: the cooked
// WGSL declares the builtin and no instruction touches it.
struct LitVaryings
{
    // The world position turned into tangent space, in which the lighting is
    // computed.
    [[vk::location(0)]] float3 tangent_space_position : TEXCOORD0;
    [[vk::location(1)]] float2 vertex_texture_coords : TEXCOORD1;
    [[vk::location(2)]] float3 TBN_tangent           : TEXCOORD2;
    [[vk::location(3)]] float3 TBN_bitangent         : TEXCOORD3;
    [[vk::location(4)]] float3 TBN_normal            : TEXCOORD4;
    [[vk::location(5)]] float3 world_position        : TEXCOORD5;

    float4 sv_position : SV_POSITION;
};

// Exponential-squared depth fog, the same in every lit stage.
//
// `engine.fog_density` of 0 leaves the colour bit-identical, which is what a
// project that never sets fog gets: the factor is exactly 0 and the lerp
// returns its input.
float3 apply_fog(float3 color, float3 world_position)
{
    float fog_distance = length(camera.camera_position - world_position);
    float fog_factor = clamp(
        1.0 - exp(-engine.fog_density * engine.fog_density * fog_distance * fog_distance),
        0.0,
        1.0
    );
    return lerp(color, engine.fog_color, fog_factor);
}
