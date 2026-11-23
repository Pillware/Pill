// The lit surface a PBR fragment stage shades, and the shading every surface
// built this way shares.
//
// Included by `pbr_pipeline`'s fragment stage. A project shader builds its own
// `PbrSurface` and calls `shade_pbr_surface` to get the pipeline's lighting -
// the same light rig, flat ambient and fog - without restating it. Edit here;
// `pill_assets` re-cooks every stage that includes it.
//
// A project shader reaches this header with `#include "pbr_surface.hlsl"`: the
// shader cook puts the loaded modules' `include` folders (under each module's
// `res`) on its include path, so a project needs no copy of its own. A
// module's shaders are cooked when the module is built, and an edit to one
// of these headers reaches an already-cooked *project* shader at that
// project's next cook rather than at the moment the header changed.
//
// What a surface is made of, in the order the five fields are read:
//
//   albedo         the base colour, already multiplied by its material tint
//   roughness      0 mirror-smooth, 1 fully rough, from the packing map's green
//   metallic       0 dielectric, 1 metal, from the packing map's blue
//   tangent_normal tangent space, in [-1, 1] - `normalize(sample * 2 - 1)`
//   emissive       light the surface gives off, already tinted
//
// A tool that rebuilds a material elsewhere may read this text, as the
// cooked WGSL the renderer draws. A field built in the shapes below can be
// read back: a material uniform member, a texture sample, a literal, `*`,
// `+`, `-`, `saturate(...)` or `clamp(x, 0, 1)`, a single colour channel,
// and `normalize(sample * 2 - 1)` for the normal. Anything else - a call to
// a helper of its own, a mix, a projection - leaves that one field for such
// a reader to skip. The shading itself is unaffected: it runs from this
// text either way.

#include "lit_common.hlsl"
#include "light_rig.hlsl"

// What shading needs to know about a surface, resolved from a material.
//
// The fragment stage samples the material's slots into these five values; the
// shading below never reads a texture or a uniform itself.
struct PbrSurface
{
    float3 albedo;         // Base colour times the base-colour tint.
    float  roughness;      // 0 = mirror-smooth, 1 = fully rough.
    float  metallic;       // 0 = dielectric, 1 = metal.
    float3 tangent_normal; // Tangent-space normal, already in [-1, 1].
    float3 emissive;       // Light the surface gives off, already tinted.
};

static const float PI = 3.14159265359;
// Flat ambient, standing in for the IBL the reference samples.
static const float AMBIENT_STRENGTH = 0.05;

// Ray traced shadow state of the surface being shaded.
struct ShadowParams
{
    // 1 when the ray tracing pass ran for this surface, 0 when it did not.
    uint enabled;
    uint mask_width;
    uint mask_height;
    uint reserved;
};

[[vk::binding(1, 1)]]
ConstantBuffer<ShadowParams> shadow;

// One value per pixel: 1 where the key light reaches the surface, 0 where
// geometry blocks it. When the pass does not run the renderer binds a
// single-value buffer and `shadow.enabled` is 0, so this is never read.
[[vk::binding(2, 1)]]
StructuredBuffer<float> shadow_mask;

// How much of the key light reaches this pixel: 1 when nothing blocks it and
// when the pass did not run for this surface.
float key_light_visibility(float4 sv_position)
{
    if (shadow.enabled == 0u)
    {
        return 1.0;
    }
    uint x = min((uint)sv_position.x, shadow.mask_width - 1u);
    uint y = min((uint)sv_position.y, shadow.mask_height - 1u);
    return saturate(shadow_mask[y * shadow.mask_width + x]);
}

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

// Shades one surface with this pipeline's light rig, ambient and fog.
//
// The rig is a constant until the renderer reads light components: a key, a
// fill and a rim light. Ambient has no IBL behind it, so it is the flat term
// above. The result is already fogged, so an entry point returns it as is.
float3 shade_pbr_surface(PbrSurface surface, LitVaryings input)
{
    // Tangent space to world: the TBN rows are T, B, N, so a row vector times
    // the matrix is the world-space normal the BRDF needs.
    float3x3 TBN_matrix = float3x3(
        input.TBN_tangent,
        input.TBN_bitangent,
        input.TBN_normal
    );
    float3 N = normalize(mul(surface.tangent_normal, TBN_matrix));
    float3 V = normalize(camera.camera_position - input.world_position);

    float3 F0 = lerp(float3(0.04, 0.04, 0.04), surface.albedo, surface.metallic);

    // The three-point rig lives in `light_rig.hlsl`, which the ray tracing
    // shadow stage includes too: both use the same key direction. Only the key
    // light is shadowed - the mask the ray tracing pass writes is built from
    // rays traveling along `-KEY_DIRECTION`.
    float3 direct = float3(0.0, 0.0, 0.0);
    direct += accumulate_light(
        N, V, F0, surface.albedo, surface.roughness, surface.metallic,
        KEY_DIRECTION, KEY_COLOR, KEY_INTENSITY
    ) * key_light_visibility(input.sv_position);
    direct += accumulate_light(
        N, V, F0, surface.albedo, surface.roughness, surface.metallic,
        FILL_DIRECTION, FILL_COLOR, FILL_INTENSITY
    );
    direct += accumulate_light(
        N, V, F0, surface.albedo, surface.roughness, surface.metallic,
        RIM_DIRECTION, RIM_COLOR, RIM_INTENSITY
    );

    // Ambient without IBL: the diffuse albedo plus the dielectric F0 term.
    float3 ambient = surface.albedo * AMBIENT_STRENGTH + F0 * AMBIENT_STRENGTH;

    float3 color = ambient + direct + surface.emissive;
    return apply_fog(color, input.world_position);
}
