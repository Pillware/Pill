// Scene + bloom × strength.

struct BloomCompositeParams {
    float  strength;
    float  _padding0, _padding1, _padding2; // scalars: a float3 would align to 16 bytes
};

[[vk::binding(0, 0)]] Texture2D    scene_texture;
[[vk::binding(1, 0)]] Texture2D    bloom_texture;
[[vk::binding(2, 0)]] SamplerState linear_sampler;
[[vk::binding(3, 0)]] ConstantBuffer<BloomCompositeParams> params;

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    float3 scene = scene_texture.SampleLevel(linear_sampler, texture_coordinates, 0).rgb;
    float3 bloom = bloom_texture.SampleLevel(linear_sampler, texture_coordinates, 0).rgb;
    return float4(scene + bloom * params.strength, 1.0);
}
