// Default unlit fragment shader. Edit here — `pill_assets` regenerates the .wgsl.
// Diffuse texture + tint + exp-squared depth fog. No lighting math.

#include "include/common.hlsl"

struct MaterialParams {
    float3 tint;
};
[[vk::binding(0, 2)]] ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]] Texture2D    diffuse_texture;
[[vk::binding(1, 3)]] SamplerState diffuse_sampler;

[shader("fragment")]
float4 fs_main(
    [[vk::location(0)]] float3 in_vertex_position       : TEXCOORD0,
    [[vk::location(1)]] float2 in_vertex_texture_coords : TEXCOORD1,
    [[vk::location(2)]] float3 in_TBN_tangent           : TEXCOORD2,
    [[vk::location(3)]] float3 in_TBN_bitangent         : TEXCOORD3,
    [[vk::location(4)]] float3 in_TBN_normal            : TEXCOORD4,
    [[vk::location(5)]] float3 in_world_position        : TEXCOORD5
) : SV_TARGET {
    float4 object_color = diffuse_texture.Sample(diffuse_sampler, in_vertex_texture_coords);
    float3 final_color  = object_color.xyz * material.tint;

    // Exponential-squared depth fog. density = 0 → no fog, bit-identical output.
    float fog_dist   = length(camera.camera_position - in_world_position);
    float fog_factor = clamp(1.0 - exp(-engine.fog_density * engine.fog_density * fog_dist * fog_dist), 0.0, 1.0);
    final_color = lerp(final_color, engine.fog_color, fog_factor);

    return float4(final_color, 1.0);
}
