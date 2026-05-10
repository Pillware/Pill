// PBR static fragment shader. Edit here — pill_assets regenerates the .wgsl.
// Full GGX microfacet BRDF with 3 directional lights and optional IBL fallback.

struct Camera {
    float4                position;
    column_major float4x4 view_projection;
    float3                fog_color;
    float                 fog_density;
};
[[vk::binding(0, 0)]] ConstantBuffer<Camera> camera;

// IBL resources in globals bind group (set 0, bindings 1-6).
[[vk::binding(1, 0)]] Texture2D    irradiance_texture;
[[vk::binding(2, 0)]] SamplerState irradiance_sampler;
[[vk::binding(3, 0)]] Texture2D    prefilter_texture;
[[vk::binding(4, 0)]] SamplerState prefilter_sampler;
[[vk::binding(5, 0)]] Texture2D    brdf_lut_texture;
[[vk::binding(6, 0)]] SamplerState brdf_lut_sampler;

// PBR material textures (set 1) — bindings match DEFAULT_LIT_SHADER layout (0-7).
[[vk::binding(0, 1)]] Texture2D    base_color_texture;
[[vk::binding(1, 1)]] SamplerState base_color_sampler;
[[vk::binding(2, 1)]] Texture2D    normal_texture;
[[vk::binding(3, 1)]] SamplerState normal_sampler;
[[vk::binding(4, 1)]] Texture2D    metallic_roughness_texture;
[[vk::binding(5, 1)]] SamplerState metallic_roughness_sampler;
[[vk::binding(6, 1)]] Texture2D    emissive_texture;
[[vk::binding(7, 1)]] SamplerState emissive_sampler;

// PBR params UBO (set 2) — 48 bytes: 3 × 16-byte slots.
// Each scalar slot uses float+float+float2 padding to stay 16 bytes without float3 alignment gaps.
struct MaterialParams {
    float3 base_color_factor;
    float  _padding_0;
    float  roughness_factor;
    float  _padding_1;
    float2 _padding_2;
    float  metallic_factor;
    float  _padding_3;
    float2 _padding_4;
};
[[vk::binding(0, 2)]] ConstantBuffer<MaterialParams> material;

static const float PI                 = 3.14159265359;
static const float MAX_REFLECTION_LOD = 4.0;

#include "include/equirect.hlsl"

// Camera at +Z looking -Z (glTF default). Z components flipped vs. -Z camera setup.
static const float3 LIGHT_DIR0 = float3( 0.38, -0.38, -0.84); // key: behind-camera, upper-left
static const float3 LIGHT_DIR1 = float3(-0.50,  0.50,  0.71); // rim: front upper-right
static const float3 LIGHT_DIR2 = float3( 0.00, -1.00,  0.00); // bounce: from below
static const float4 LIGHT_COL0 = float4(1.0, 0.98, 0.95,  2.2); // near-neutral key
static const float4 LIGHT_COL1 = float4(0.6, 0.65, 1.00,  0.8); // cool rim
static const float4 LIGHT_COL2 = float4(0.8, 0.80, 0.80,  0.3); // neutral bounce

float distribution_ggx(float3 normal, float3 half_vector, float roughness) {
    // Add epsilon to avoid singularities at very low roughness.
    float alpha           = max(roughness * roughness, 0.0025);
    float alpha_squared   = alpha * alpha;
    float normal_dot_half = max(dot(normal, half_vector), 0.0);
    float denominator     = (normal_dot_half * normal_dot_half * (alpha_squared - 1.0) + 1.0);
    return alpha_squared / (PI * denominator * denominator + 1e-7);
}

float geometry_schlick_ggx(float normal_dot_view, float roughness) {
    // Heitz's k for direct lighting approximation.
    float roughness_plus_one = roughness + 1.0;
    float k_factor           = (roughness_plus_one * roughness_plus_one) / 8.0;
    return normal_dot_view / (normal_dot_view * (1.0 - k_factor) + k_factor);
}

float geometry_smith(float3 normal, float3 view_direction, float3 light_direction, float roughness) {
    float normal_dot_view  = max(dot(normal, view_direction), 0.0);
    float normal_dot_light = max(dot(normal, light_direction), 0.0);
    return geometry_schlick_ggx(normal_dot_light, roughness) * geometry_schlick_ggx(normal_dot_view, roughness);
}

float3 fresnel_schlick(float cos_theta, float3 base_reflectivity) {
    return base_reflectivity + (float3(1.0, 1.0, 1.0) - base_reflectivity) * pow(1.0 - cos_theta, 5.0);
}

float3 fresnel_schlick_roughness(float cos_theta, float3 base_reflectivity, float roughness) {
    return base_reflectivity + (max(float3(1.0 - roughness, 1.0 - roughness, 1.0 - roughness), base_reflectivity) - base_reflectivity) * pow(1.0 - cos_theta, 5.0);
}


float3 accumulate_directional_light(
    float3 normal, float3 view_direction, float3 base_reflectivity,
    float3 albedo, float roughness, float metallic,
    float3 light_direction_to_surface, float4 light_color
) {
    // light_direction_to_surface is direction from light to surface; incoming light is opposite.
    float3 light_direction     = normalize(-light_direction_to_surface);
    float3 half_vector         = normalize(view_direction + light_direction);
    float3 radiance            = light_color.w * light_color.xyz;
    float  normal_distribution = distribution_ggx(normal, half_vector, roughness);
    float  geometry            = geometry_smith(normal, view_direction, light_direction, roughness);
    float3 fresnel             = fresnel_schlick(max(dot(half_vector, view_direction), 0.0), base_reflectivity);
    float3 diffuse_factor      = (float3(1.0, 1.0, 1.0) - fresnel) * (1.0 - metallic);
    float  denominator         = 4.0 * max(dot(normal, view_direction), 0.0) * max(dot(normal, light_direction), 0.0) + 0.0001;
    float3 specular            = (normal_distribution * geometry * fresnel) / float3(denominator, denominator, denominator);
    return (diffuse_factor * (albedo / PI) + specular) * radiance * max(dot(normal, light_direction), 0.0);
}

[shader("fragment")]
float4 fs_main(
    [[vk::location(0)]] float2 texture_coordinates : TEXCOORD0,
    [[vk::location(1)]] float3 world_position      : TEXCOORD1,
    [[vk::location(2)]] float3 input_normal        : TEXCOORD2
) : SV_TARGET {
    float3 albedo             = base_color_texture.Sample(base_color_sampler, texture_coordinates).rgb * material.base_color_factor;
    float2 metallic_roughness = metallic_roughness_texture.Sample(metallic_roughness_sampler, texture_coordinates).gb;
    // metallic_roughness.x = G channel (roughness 0=smooth, 1=rough); metallic_roughness.y = B channel (metallic 0=dielectric, 1=metal)
    float  roughness = clamp(metallic_roughness.x * (1.0 - material.roughness_factor), 0.045, 0.99);
    float  metallic  = metallic_roughness.y * material.metallic_factor;
    // TODO: Support normal mapping (tangent space) and AO texture.
    float3 normal            = normalize(input_normal);
    float3 view_direction    = normalize(camera.position.xyz - world_position);
    float3 base_reflectivity = lerp(float3(0.04, 0.04, 0.04), albedo, float3(metallic, metallic, metallic));
    float3 outgoing_radiance = float3(0.0, 0.0, 0.0);
    outgoing_radiance += accumulate_directional_light(normal, view_direction, base_reflectivity, albedo, roughness, metallic, LIGHT_DIR0, LIGHT_COL0);
    outgoing_radiance += accumulate_directional_light(normal, view_direction, base_reflectivity, albedo, roughness, metallic, LIGHT_DIR1, LIGHT_COL1);
    outgoing_radiance += accumulate_directional_light(normal, view_direction, base_reflectivity, albedo, roughness, metallic, LIGHT_DIR2, LIGHT_COL2);
    // Diffuse IBL ambient.
    float3 specular_factor = fresnel_schlick(max(dot(normal, view_direction), 0.0), base_reflectivity);
    float3 diffuse_factor  = (float3(1.0, 1.0, 1.0) - specular_factor) * (1.0 - metallic);
    float3 irradiance      = irradiance_texture.Sample(irradiance_sampler, dir_to_equirect_uv(normal)).rgb;
    float3 ambient_diffuse = diffuse_factor * irradiance * albedo;
    // Specular IBL.
    float3 reflection        = reflect(-view_direction, normal);
    float3 prefiltered_color = prefilter_texture.SampleLevel(prefilter_sampler, dir_to_equirect_uv(reflection), roughness * MAX_REFLECTION_LOD).rgb;
    float2 environment_brdf  = brdf_lut_texture.Sample(brdf_lut_sampler, float2(max(dot(normal, view_direction), 0.0), roughness)).rg;
    float3 fresnel           = fresnel_schlick_roughness(max(dot(normal, view_direction), 0.0), base_reflectivity, roughness);
    float3 specular_ibl      = prefiltered_color * (fresnel * environment_brdf.x + environment_brdf.y);
    float3 emissive = emissive_texture.Sample(emissive_sampler, texture_coordinates).rgb;
    float3 color = outgoing_radiance + ambient_diffuse + specular_ibl + emissive;
    float  distance = length(camera.position.xyz - world_position);
    float  fog      = 1.0 - exp(-camera.fog_density * camera.fog_density * distance * distance);
    return float4(lerp(color, camera.fog_color, fog), 1.0);
}
