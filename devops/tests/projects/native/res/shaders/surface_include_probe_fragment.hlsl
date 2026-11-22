// Probe fragment for the module-include suites.
//
// This is a project shader that includes a header the renderer data crate
// ships: `pbr_surface.hlsl`, from the module's `pbr_pipeline/include/`
// directory. The host puts every loaded module's header directories on the
// project shader cook's include path, after the project's own, so this file
// proves a project shader can build a `PbrSurface` and call the pipeline's
// shading without copying either.
//
// Nothing has to draw it: the cook and the compile are what the suites
// observe. Its bindings are the reload probe's, so it carries the same
// material slots and the same engine and camera uniforms.

#include "pbr_surface.hlsl"

struct ProbeInput
{
    float3 vertex_position : TEXCOORD0;
    float2 vertex_texture_coords : TEXCOORD1;
    float3 tangent : TEXCOORD2;
    float3 bitangent : TEXCOORD3;
    float3 normal : TEXCOORD4;
    float3 world_position : TEXCOORD5;
};

cbuffer MaterialParams : register(b0, space2)
{
    float4 tint;
    float4 roughness;
};

float4 fs_main(ProbeInput probe_input) : SV_TARGET
{
    // Shading takes the pipeline's own varying block, so the probe fills it
    // from the fields it has under those names.
    LitVaryings input;
    input.tangent_space_position = probe_input.vertex_position;
    input.vertex_texture_coords = probe_input.vertex_texture_coords;
    input.TBN_tangent = probe_input.tangent;
    input.TBN_bitangent = probe_input.bitangent;
    input.TBN_normal = probe_input.normal;
    input.world_position = probe_input.world_position;
    input.sv_position = float4(0.0, 0.0, 0.0, 1.0);

    PbrSurface surface;
    surface.albedo = tint.rgb;
    surface.roughness = saturate(roughness.x);
    surface.metallic = 0.0;
    surface.tangent_normal = normalize(float3(0.0, 0.0, 1.0));
    surface.emissive = float3(0.0, 0.0, 0.0);

    return float4(shade_pbr_surface(surface, input), 1.0);
}
