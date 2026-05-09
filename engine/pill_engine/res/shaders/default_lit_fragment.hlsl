// Default lit fragment shader. Edit here — `pill_assets` regenerates the .wgsl.
// Diffuse + normal mapped, single point light, exp-squared depth fog.

#include "include/common.hlsl"

struct MaterialParams {
    float3 tint;
    float  specularity;
};
[[vk::binding(0, 2)]] ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]] Texture2D    diffuse_texture;
[[vk::binding(1, 3)]] SamplerState diffuse_sampler;
[[vk::binding(2, 3)]] Texture2D    normal_texture;
[[vk::binding(3, 3)]] SamplerState normal_sampler;

[shader("fragment")]
float4 fs_main(
    [[vk::location(0)]] float3 in_vertex_position       : TEXCOORD0,
    [[vk::location(1)]] float2 in_vertex_texture_coords : TEXCOORD1,
    [[vk::location(2)]] float3 in_TBN_tangent           : TEXCOORD2,
    [[vk::location(3)]] float3 in_TBN_bitangent         : TEXCOORD3,
    [[vk::location(4)]] float3 in_TBN_normal            : TEXCOORD4,
    [[vk::location(5)]] float3 in_world_position        : TEXCOORD5
) : SV_TARGET {
    // Settings (single point light, hard-coded).
    const float  ambient_light_strength = 0.02;
    const float3 light_position         = float3(-10.0, 10.0, -10.0);
    const float3 light_color            = float3(1.0, 1.0, 1.0);

    float4 object_color  = diffuse_texture.Sample(diffuse_sampler, in_vertex_texture_coords);
    float4 object_normal = normal_texture.Sample(normal_sampler, in_vertex_texture_coords);

    // Reconstruct TBN from per-vertex rows.
    float3x3 TBN_matrix = float3x3(in_TBN_tangent, in_TBN_bitangent, in_TBN_normal);

    float3 ambient_light_factor = light_color * ambient_light_strength;

    // Diffuse — normal map is in [0,1]; rescale to [-1,1].
    float3 normal          = normalize(object_normal.rgb * 2.0 - 1.0);
    float3 light_direction = normalize(mul(TBN_matrix, light_position) - in_vertex_position);

    float  diffuse_light_strength = max(dot(normal, light_direction), 0.0);
    float3 diffuse_light_factor   = light_color * diffuse_light_strength;

    // Specular (Blinn-Phong, fixed exponent 32).
    float3 view_direction          = normalize(mul(TBN_matrix, camera.camera_position) - in_vertex_position);
    float3 half_direction          = normalize(view_direction + light_direction);
    float  specular_light_strength = pow(max(dot(normal, half_direction), 0.0), 32) * material.specularity;
    float3 specular_light_factor   = light_color * specular_light_strength;

    float3 final_color = (ambient_light_factor + diffuse_light_factor + specular_light_factor)
                       * object_color.xyz
                       * material.tint;

    // Exponential-squared depth fog. density = 0 → no fog, bit-identical output.
    float fog_dist   = length(camera.camera_position - in_world_position);
    float fog_factor = clamp(1.0 - exp(-engine.fog_density * engine.fog_density * fog_dist * fog_dist), 0.0, 1.0);
    final_color = lerp(final_color, engine.fog_color, fog_factor);

    return float4(final_color, 1.0);
}
