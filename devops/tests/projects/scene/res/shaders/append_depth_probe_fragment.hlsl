// Append-pass fixture: a fullscreen pass that reads the depth buffer the chain
// left on the surface and tints the picture by how far away the nearest thing
// is. Reading the chain's own depth is what shows this pass ran after the
// chain rather than inside it: a pass in the chain that attached depth to test
// against it could not sample it in the same pass.
//
// The vertex stage is the renderer's shared `fullscreen_vertex.wgsl`, so this
// file supplies only the fragment. Bindings follow the engine's convention - the
// pass's own parameters in set 2, the frames it reads in set 3.

struct MaterialParams
{
    // The colour far geometry takes.
    float4 tint;
};

[[vk::binding(0, 2)]]
ConstantBuffer<MaterialParams> material;

// A depth slot binds a depth view sampled without filtering, so a sample reads
// one texel instead of blending neighbours.
[[vk::binding(0, 3)]]
Texture2D<float> depth_texture;

[[vk::binding(1, 3)]]
SamplerState depth_sampler;

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

    // 0 is the near plane and 1 the far one, so near geometry stays dark and
    // far geometry takes the tint: the picture changes wherever the chain drew.
    float depth = depth_texture.Sample(depth_sampler, input.texture_coords);
    result.output = float4(material.tint.rgb * depth, material.tint.a);

    return result;
}
