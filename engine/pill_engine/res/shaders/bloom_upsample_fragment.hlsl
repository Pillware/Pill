// Bloom upsample: 3×3 tent over the smaller level, added (blend One/One) onto the next larger one.

struct BloomUpsampleParams {
    float2 radius; // tent tap offset in UV; y scaled by aspect so the kernel stays round
    float2 _padding;
};

[[vk::binding(0, 0)]] Texture2D    source_texture;
[[vk::binding(1, 0)]] SamplerState linear_sampler;
[[vk::binding(2, 0)]] ConstantBuffer<BloomUpsampleParams> params;

float3 tap(float2 texture_coordinates) {
    return source_texture.SampleLevel(linear_sampler, texture_coordinates, 0).rgb;
}

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    float2 r  = params.radius;
    float2 uv = texture_coordinates;
    // 1 2 1 / 2 4 2 / 1 2 1, sum 16
    float3 result = tap(uv) * 4.0
                  + (tap(uv + float2(0, r.y)) + tap(uv - float2(0, r.y)) + tap(uv + float2(r.x, 0)) + tap(uv - float2(r.x, 0))) * 2.0
                  + tap(uv + r) + tap(uv - r) + tap(uv + float2(r.x, -r.y)) + tap(uv + float2(-r.x, r.y));
    return float4(result / 16.0, 1.0);
}
