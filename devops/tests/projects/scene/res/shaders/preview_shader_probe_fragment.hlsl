// Probe fragment for the material preview suites.
//
// It is the PBR chain's own stage, kept identical so a material that names it
// is drawn exactly as one naming the chain's shader, and it exists only to
// give the project a second loaded shader: the suites compare two materials
// that differ in nothing but the shader they name, which is how they show a
// tile is drawn through the material's shader rather than one of the preview's
// own. The PBR fragment, and the reason each part of it differs from the
// reference renderer, is documented beside it in the renderer's `res/shaders/`.// PBR fragment shader: samples this material's slots into a surface and
// shades it. Edit here; the build script's `slangc` rule regenerates the
// `.wgsl` beside it.
//
// The lighting lives in `pbr_surface.hlsl`, which this stage includes: the
// surface below is the five values `shade_pbr_surface` consumes, so another
// shader can build a different surface and share the same shading.
//
// What differs from the reference, and why:
//
// * One output. The reference writes colour, velocity and ambient to three MRT
//   targets; this port has a single target and no velocity pass, so the BRDF
//   result is the whole output.
// * No IBL. The reference samples cooked irradiance, a prefiltered specular
//   chain and a BRDF LUT. This port has no f32 or mipped texture upload, so
//   ambient is a flat term in `pbr_surface.hlsl` instead - the one place the
//   lighting model is visibly cheaper than the reference.
// * The light rig is a constant in `pbr_surface.hlsl`, as it is in the port's
//   default shader: `DirectionalLightComponent` is registered but never read
//   by the renderer.
// * Material parameters are four `float4` slots, which is exactly how the Rust
//   side packs them - 16 bytes per slot, in slot order.

// The surface type and the shading live there; this stage only fills the type.
#include "pbr_surface.hlsl"

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

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

[shader("fragment")]
PixelOutput fs_main(LitVaryings input)
{
    PixelOutput result;

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

    PbrSurface surface;
    surface.albedo = base_sample.rgb * material.pbr_base.xyz;
    surface.roughness = roughness;
    surface.metallic = metallic;
    // The map holds a direction in [0, 1]; shading reads it as a vector.
    surface.tangent_normal = normalize(normal_sample * 2.0 - 1.0);
    surface.emissive = emissive_sample * material.pbr_emissive.xyz;

    result.output = float4(shade_pbr_surface(surface, input), 1.0);
    return result;
}
