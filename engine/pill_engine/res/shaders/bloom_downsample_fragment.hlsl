// Bloom downsample (Jimenez, "Next Generation Post Processing in Call of Duty: AW", 2014):
// 13 bilinear taps as five overlapping 2×2 boxes. The first level (from the HDR image) weights each
// box by Karis' 1/(1 + luma) to keep single bright pixels from flickering.

struct BloomDownsampleParams {
    uint  karis_average; // 1 on the first level
    float  _padding0, _padding1, _padding2; // scalars: a float3 would align to 16 bytes
};

[[vk::binding(0, 0)]] Texture2D    source_texture;
[[vk::binding(1, 0)]] SamplerState linear_sampler;
[[vk::binding(2, 0)]] ConstantBuffer<BloomDownsampleParams> params;

// Brightest channel clamp before averaging: very hot pixels (emissive, specular) would otherwise
// dominate the whole bloom; scaling all channels keeps the hue.
static const float LUMA_CAP = 100.0;

float luma(float3 color) {
    return dot(color, float3(0.2126, 0.7152, 0.0722));
}

float3 clamp_brightness(float3 color) {
    float brightest = max(color.r, max(color.g, color.b));
    return brightest > LUMA_CAP ? color * (LUMA_CAP / brightest) : color;
}

float3 tap(float2 texture_coordinates) {
    return clamp_brightness(source_texture.SampleLevel(linear_sampler, texture_coordinates, 0).rgb);
}

float karis_weight(float3 color) {
    return 1.0 / (1.0 + luma(color) * 0.25);
}

float3 box(float3 a, float3 b, float3 c, float3 d) {
    if (params.karis_average == 0) {
        return (a + b + c + d) * 0.25;
    }
    float wa = karis_weight(a), wb = karis_weight(b), wc = karis_weight(c), wd = karis_weight(d);
    return (a * wa + b * wb + c * wc + d * wd) / max(wa + wb + wc + wd, 1e-6);
}

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    uint width, height;
    source_texture.GetDimensions(width, height);
    float2 texel = 1.0 / float2(width, height);
    float2 uv = texture_coordinates;

    float3 a = tap(uv + texel * float2(-2,  2)), b = tap(uv + texel * float2(0,  2)), c = tap(uv + texel * float2(2,  2));
    float3 d = tap(uv + texel * float2(-2,  0)), e = tap(uv),                         f = tap(uv + texel * float2(2,  0));
    float3 g = tap(uv + texel * float2(-2, -2)), h = tap(uv + texel * float2(0, -2)), i = tap(uv + texel * float2(2, -2));
    float3 j = tap(uv + texel * float2(-1,  1)), k = tap(uv + texel * float2(1,  1));
    float3 l = tap(uv + texel * float2(-1, -1)), m = tap(uv + texel * float2(1, -1));

    float3 result = (box(a, b, d, e) + box(b, c, e, f) + box(d, e, g, h) + box(e, f, h, i)) * 0.125
                  + box(j, k, l, m) * 0.5;
    return float4(max(result, 0.0001), 1.0);
}
