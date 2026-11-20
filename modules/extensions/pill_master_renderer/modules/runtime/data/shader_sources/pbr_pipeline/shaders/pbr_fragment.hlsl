// PBR fragment shader - GGX microfacet BRDF, ported from the reference's
// `pbr_opaque_fragment.hlsl`. Edit here; the build script's `slangc` rule
// regenerates the `.wgsl` beside it.
//
// What differs from the reference, and why:
//
// * One output. The reference writes colour, velocity and ambient to three MRT
//   targets; this port has a single target and no velocity pass, so the BRDF
//   result is the whole output.
// * No IBL. The reference samples cooked irradiance, a prefiltered specular
//   chain and a BRDF LUT. This port has no f32 or mipped texture upload, so
//   ambient is a flat term instead - the one place the lighting model is
//   visibly cheaper than the reference.
// * The light rig is a constant here, as it is in the port's default shader:
//   `DirectionalLightComponent` is registered but never read by the renderer.
// * Material parameters are four `float4` slots, which is exactly how the Rust
//   side packs them - 16 bytes per slot, in slot order.

#include "lit_common.hlsl"

// One `float4` per slot, in slot order, because that is what the Rust side
// writes: 16 bytes per slot, a `Color` in xyz, a `Scalar` in x. Nothing may read
// a component the packer leaves as padding - a `Color`'s `w` is written as 0.0,
// which is why roughness and metallic are their own `Scalar` slots read from
// `.x` rather than a fourth component of the colour slot.
struct MaterialParams
{
    float4 pbr_base;      // xyz = base colour tint (Color slot)
    float4 pbr_roughness; // x = roughness factor (Scalar slot)
    float4 pbr_metallic;  // x = metallic factor (Scalar slot)
    float4 pbr_emissive;  // xyz = emissive tint (Color slot)
};

[[vk::binding(0, 2)]]
ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]]
Texture2D base_color_texture;

[[vk::binding(1, 3)]]
SamplerState base_color_sampler;

[[vk::binding(2, 3)]]
Texture2D normal_texture;

[[vk::binding(3, 3)]]
SamplerState normal_sampler;

[[vk::binding(4, 3)]]
Texture2D metallic_roughness_texture;

[[vk::binding(5, 3)]]
SamplerState metallic_roughness_sampler;

[[vk::binding(6, 3)]]
Texture2D emissive_texture;

[[vk::binding(7, 3)]]
SamplerState emissive_sampler;

static const float PI = 3.14159265359;
// Flat ambient, standing in for the IBL the reference samples.
static const float AMBIENT_STRENGTH = 0.05;

float distribution_ggx(float3 N, float3 H, float roughness)
{
    // Clamped so a mirror-smooth surface cannot divide by a singularity.
    float a     = max(roughness * roughness, 0.0025);
    float a2    = a * a;
    float NdotH = max(dot(N, H), 0.0);
    float denom = (NdotH * NdotH * (a2 - 1.0) + 1.0);
    return a2 / (PI * denom * denom + 1e-7);
}

// Filament's height-correlated Smith-GGX: includes the 1/(4*NoV*NoL) term.
float visibility_smith_ggx(float NoV, float NoL, float roughness)
{
    float a2   = roughness * roughness;
    float GGXV = NoL * sqrt(NoV * NoV * (1.0 - a2) + a2);
    float GGXL = NoV * sqrt(NoL * NoL * (1.0 - a2) + a2);
    return 0.5 / (GGXV + GGXL + 1e-7);
}

float3 fresnel_schlick(float cos_theta, float3 F0)
{
    return F0 + (float3(1.0, 1.0, 1.0) - F0) * pow(1.0 - cos_theta, 5.0);
}

// Filament's Burley diffuse: energy-conserving, with grazing retro-reflection.
float3 diffuse_burley(float NoV, float NoL, float LoH, float roughness, float3 albedo)
{
    float f90          = 0.5 + 2.0 * roughness * LoH * LoH;
    float light_scatter = 1.0 + (f90 - 1.0) * pow(1.0 - NoL, 5.0);
    float view_scatter  = 1.0 + (f90 - 1.0) * pow(1.0 - NoV, 5.0);
    return albedo * (light_scatter * view_scatter / PI);
}

// One directional light's contribution, `light_direction` pointing light -> surface.
float3 accumulate_light(
    float3 N, float3 V, float3 F0, float3 albedo, float roughness, float metallic,
    float3 light_direction, float3 light_color, float intensity
)
{
    float3 L   = normalize(-light_direction);
    float3 H   = normalize(V + L);
    float  NoV = max(dot(N, V), 1e-4);
    float  NoL = max(dot(N, L), 0.0);
    float  LoH = max(dot(L, H), 0.0);
    if (NoL <= 0.0)
    {
        return float3(0.0, 0.0, 0.0);
    }

    float3 radiance = light_color * intensity;
    float  D   = distribution_ggx(N, H, roughness);
    float  Vis = visibility_smith_ggx(NoV, NoL, roughness);
    float3 F   = fresnel_schlick(LoH, F0);

    float3 specular = D * Vis * F;
    float3 kD = (float3(1.0, 1.0, 1.0) - F) * (1.0 - metallic);
    return (kD * diffuse_burley(NoV, NoL, LoH, roughness, albedo) + specular) * radiance * NoL;
}

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

[shader("fragment")]
PixelOutput fs_main(LitVaryings input)
{
    PixelOutput result;

    // Three-point rig, in the shader until the renderer reads light components.
    const float3 key_direction = normalize(float3(-0.5, -0.6, -1.0));
    const float3 fill_direction = normalize(float3(1.0, -0.2, 0.4));
    const float3 rim_direction = normalize(float3(0.0, 0.8, 0.6));
    const float3 key_color = float3(1.0, 0.97, 0.92);
    const float3 fill_color = float3(0.85, 0.9, 1.0);
    const float3 rim_color = float3(1.0, 0.95, 0.9);

    float4 base_sample = base_color_texture.Sample(
        base_color_sampler,
        input.vertex_texture_coords
    );
    float4 metal_rough = metallic_roughness_texture.Sample(
        metallic_roughness_sampler,
        input.vertex_texture_coords
    );
    float3 emissive_sample = emissive_texture.Sample(
        emissive_sampler,
        input.vertex_texture_coords
    ).rgb;
    float3 normal_sample = normal_texture.Sample(
        normal_sampler,
        input.vertex_texture_coords
    ).rgb;

    // glTF packs occlusion/roughness/metallic into one map: green, blue.
    float roughness = saturate(metal_rough.g * material.pbr_roughness.x);
    float metallic = saturate(metal_rough.b * material.pbr_metallic.x);

    float3 albedo = base_sample.rgb * material.pbr_base.xyz;
    float3 emissive = emissive_sample * material.pbr_emissive.xyz;

    // Tangent space to world: the TBN rows are T, B, N, so a row vector times
    // the matrix is the world-space normal the BRDF needs.
    float3x3 TBN_matrix = float3x3(
        input.TBN_tangent,
        input.TBN_bitangent,
        input.TBN_normal
    );
    float3 tangent_normal = normalize(normal_sample * 2.0 - 1.0);
    float3 N = normalize(mul(tangent_normal, TBN_matrix));
    float3 V = normalize(camera.camera_position - input.world_position);

    float3 F0 = lerp(float3(0.04, 0.04, 0.04), albedo, metallic);

    float3 direct = float3(0.0, 0.0, 0.0);
    direct += accumulate_light(
        N, V, F0, albedo, roughness, metallic, key_direction, key_color, 3.0
    );
    direct += accumulate_light(
        N, V, F0, albedo, roughness, metallic, fill_direction, fill_color, 1.0
    );
    direct += accumulate_light(
        N, V, F0, albedo, roughness, metallic, rim_direction, rim_color, 1.5
    );

    // Ambient without IBL: the diffuse albedo plus the dielectric F0 term.
    float3 ambient = albedo * AMBIENT_STRENGTH + F0 * AMBIENT_STRENGTH;

    float3 color = ambient + direct + emissive;
    color = apply_fog(color, input.world_position);

    result.output = float4(color, 1.0);
    return result;
}
