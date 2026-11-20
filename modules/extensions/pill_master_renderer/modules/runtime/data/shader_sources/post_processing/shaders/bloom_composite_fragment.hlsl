// Bloom composite, ported from the reference's `bloom_composite_fragment.hlsl`.
//
// Edit here - `pill_assets` regenerates the .wgsl.
//
// The additive step: the frame the scene drew, plus the bloom it produced,
// scaled. The reference's `use_karis` switch drives its upsample chain, and that
// chain only exists because the reference builds bloom as a pyramid; with one
// level the only parameter left is the strength.
//
// Bindings follow the engine's convention - the pass's own parameters in set 2,
// the frames it reads in set 3 - rather than the reference's own numbering.

struct MaterialParams
{
    // x = strength. y, z and w are unused.
    float4 bloom;
};

[[vk::binding(0, 2)]]
ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]]
Texture2D<float4> hdr_texture;

[[vk::binding(1, 3)]]
SamplerState hdr_sampler;

[[vk::binding(2, 3)]]
Texture2D<float4> bloom_texture;

[[vk::binding(3, 3)]]
SamplerState bloom_sampler;

struct PixelInput
{
    [[vk::location(0)]] float2 texture_coords : TEXCOORD0;
};

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

[shader("fragment")]
PixelOutput fs_main(PixelInput input)
{
    PixelOutput result;

    float3 hdr = hdr_texture.Sample(hdr_sampler, input.texture_coords).rgb;
    float3 bloom = bloom_texture.Sample(bloom_sampler, input.texture_coords).rgb;

    result.output = float4(hdr + bloom * material.bloom.x, 1.0);
    return result;
}
