// Joint Bilateral Upsampling [Kopf et al. 2007] of the half-res AO, applied to the indirect term
// only: out = hdr - ambient * (1 - ao). AO on the finished colour would occlude the direct lights
// too. The sky writes no ambient, so this is a no-op there. Runs before the translucent pass, so
// translucent surfaces are not darkened by the opaque ambient behind them.

// Mirrors SsaoApplyParams in pass_ssao.rs
struct SsaoApplyParams {
    float strength;         // 0 = pass the colour through untouched
    float sigma_depth_rel;  // upsample depth sigma as a fraction of view depth
    float depth_mul;
    float depth_add;
};

[[vk::binding(0, 0)]] Texture2D<float4> t_color;    // full-res hdr
[[vk::binding(1, 0)]] Texture2D<float4> t_ambient;  // full-res indirect term
[[vk::binding(2, 0)]] Texture2D<float4> t_ssao;     // half-res (ao, viewZ)
[[vk::binding(3, 0)]] Texture2D<float>  t_depth;    // full-res Depth32Float
[[vk::binding(4, 0)]] ConstantBuffer<SsaoApplyParams> params;

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    uint fw, fh;
    t_color.GetDimensions(fw, fh);
    int2 fmax = int2((int)fw - 1, (int)fh - 1);
    int2 fc   = clamp((int2)(uv * float2((float)fw, (float)fh)), int2(0, 0), fmax);

    float4 colour  = t_color.Load(int3(fc, 0));
    float3 ambient = t_ambient.Load(int3(fc, 0)).rgb;

    if (params.strength <= 0.0)
        return colour;

    float raw = t_depth.Load(int3(fc, 0));
    float z   = params.depth_mul / (params.depth_add - raw);
    if (z <= 0.0 || z > 1e4)
        return colour;

    uint hw, hh;
    t_ssao.GetDimensions(hw, hh);
    int2 hmax = int2((int)hw - 1, (int)hh - 1);

    // Half-res sample position in texel space; f is the bilinear fraction.
    float2 hp   = uv * float2((float)hw, (float)hh) - 0.5;
    int2   base = (int2)floor(hp);
    float2 f    = hp - floor(hp);

    float sigma_v = max(params.sigma_depth_rel * z, 1e-3);
    float inv_2v2 = 1.0 / (2.0 * sigma_v * sigma_v);

    float acc  = 0.0;
    float wsum = 0.0;

    [unroll]
    for (int dy = 0; dy < 2; dy++) {
        [unroll]
        for (int dx = 0; dx < 2; dx++) {
            int2   tc  = clamp(base + int2(dx, dy), int2(0, 0), hmax);
            float4 tap = t_ssao.Load(int3(tc, 0));

            float bw = (dx == 0 ? 1.0 - f.x : f.x) * (dy == 0 ? 1.0 - f.y : f.y);
            float dz = tap.y - z;
            float wt = bw * exp(-dz * dz * inv_2v2);

            acc  += tap.x * wt;
            wsum += wt;
        }
    }

    // All four taps rejected == thin silhouette, every neighbour on another surface. Unoccluded
    // is the safe answer; the alternative is a hard halo.
    float ao = saturate((wsum > 1e-5) ? (acc / wsum) : 1.0);

    float  ao_effective = lerp(1.0, ao, saturate(params.strength));
    float3 occluded     = ambient * (1.0 - ao_effective);

    float3 out_rgb = colour.rgb - occluded;

    return float4(max(out_rgb, float3(0.0, 0.0, 0.0)), colour.a);
}
