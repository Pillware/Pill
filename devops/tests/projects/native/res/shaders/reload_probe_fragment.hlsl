// Probe fragment for the project-shader reload suites.
//
// This is a project shader asset: the source is the fragment stage, and the
// `.hlsl.meta` beside it names the vertex stage it pairs with. The host cooks
// this file to `reload_probe_fragment.wgsl` beside it and reimports the asset
// whenever either changes.
//
// Its bindings match the simple pipeline's shader, so it carries the same
// material slots; the shading rule is a flat tint so a change is easy to spot.
// `*_fragment.hlsl` is the name convention: the cooker reads the stage from it.

struct PixelInput
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
    float4 specularity;
};

Texture2D diffuse_texture : register(t0, space3);
SamplerState diffuse_sampler : register(s1, space3);

float4 fs_main(PixelInput input) : SV_TARGET
{
    float4 albedo = diffuse_texture.Sample(diffuse_sampler, input.vertex_texture_coords);
    float3 shaded = albedo.rgb * tint.rgb * 0.75;
    return float4(shaded, 1.0);
}
