// Default lit fragment shader. Edit here — `pill_assets` regenerates the .wgsl.
// Diffuse + normal mapped, single point light, exp-squared depth fog.

#include "lit_common.hlsl"

// The two slots the default shader declares for this material: a `tint` colour
// and a `specularity` scalar, in that order.
//
// One `float4` per slot, because that is how the engine packs them: a `float3`
// followed by a `float` would put the scalar at offset 12, inside the colour's
// own 16-byte slot, where it reads padding - a zero - instead of the value the
// material was given. A scalar parameter lives at `.x` of its slot.
struct MaterialParams
{
    float4 tint;
    float4 specularity;
};

[[vk::binding(0, 2)]]
ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]]
Texture2D diffuse_texture;

[[vk::binding(1, 3)]]
SamplerState diffuse_sampler;

[[vk::binding(2, 3)]]
Texture2D normal_texture;

[[vk::binding(3, 3)]]
SamplerState normal_sampler;

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

[shader("fragment")]
PixelOutput fs_main(LitVaryings input)
{
    PixelOutput result;

    // Settings (single point light, hard-coded).
    const float ambient_light_strength = 0.02;
    const float3 light_position = float3(-10.0, 10.0, -10.0);
    const float3 light_color = float3(1.0, 1.0, 1.0);

    float4 object_color = diffuse_texture.Sample(
        diffuse_sampler,
        input.vertex_texture_coords
    );
    float4 object_normal = normal_texture.Sample(
        normal_sampler,
        input.vertex_texture_coords
    );

    // Reconstruct TBN from per-vertex rows.
    float3x3 TBN_matrix = float3x3(
        input.TBN_tangent,
        input.TBN_bitangent,
        input.TBN_normal
    );

    float3 ambient_light_factor = light_color * ambient_light_strength;

    // Diffuse — normal map is in [0,1]; rescale to [-1,1].
    float3 normal = normalize(object_normal.rgb * 2.0 - 1.0);
    float3 light_direction = normalize(
        mul(TBN_matrix, light_position) - input.tangent_space_position
    );

    float diffuse_light_strength = max(dot(normal, light_direction), 0.0);
    float3 diffuse_light_factor = light_color * diffuse_light_strength;

    // Specular (Blinn-Phong, fixed exponent 32).
    float3 view_direction = normalize(
        mul(TBN_matrix, camera.camera_position) - input.tangent_space_position
    );
    float3 half_direction = normalize(view_direction + light_direction);
    float specular_light_strength = pow(max(dot(normal, half_direction), 0.0), 32)
                                  * material.specularity.x;
    float3 specular_light_factor = light_color * specular_light_strength;

    float3 final_color = (ambient_light_factor + diffuse_light_factor + specular_light_factor)
                       * object_color.xyz
                       * material.tint.xyz;
    final_color = apply_fog(final_color, input.world_position);

    result.output = float4(final_color, 1.0);
    return result;
}
