// Joint-bilateral Gaussian over the half-res AO, run twice (H then V) via `blur_dir`. Depth only.
//
// The weight is slope-based, not a fixed |dz| threshold: a constant rejection is tuned for one
// distance and one orientation, so on a steep slope it rejects the whole kernel and leaves raw
// noise, while at range it accepts across genuine silhouettes. Each tap is weighted on its
// residual against the plane extrapolated from the two immediate neighbours.

// Mirrors SsaoBlurParams in pass_ssao.rs
struct SsaoBlurParams {
    float2 blur_dir;        // (1,0) horizontal, (0,1) vertical
    float  sigma_px;        // spatial sigma, half-res pixels
    float  sigma_depth_rel; // depth sigma as a fraction of view depth
};

[[vk::binding(0, 0)]] Texture2D<float4> t_ssao;   // (ao, viewZ)
[[vk::binding(1, 0)]] ConstantBuffer<SsaoBlurParams> params;

static const int TAP_HALF = 4;   // 9 taps, matching the 4x4 noise cell the AO pass writes

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    uint w, h;
    t_ssao.GetDimensions(w, h);
    int2 maxc = int2((int)w - 1, (int)h - 1);
    int2 c    = clamp((int2)(uv * float2((float)w, (float)h)), int2(0, 0), maxc);
    int2 step = (int2)params.blur_dir;

    float4 centre = t_ssao.Load(int3(c, 0));
    float  z0     = centre.y;

    if (z0 <= 0.0)
        return centre;

    // Local gradient along the blur axis, one texel either side.
    float zp = t_ssao.Load(int3(clamp(c + step, int2(0, 0), maxc), 0)).y;
    float zm = t_ssao.Load(int3(clamp(c - step, int2(0, 0), maxc), 0)).y;
    float dzdt = 0.5 * (zp - zm);

    // Tolerance grows with distance — the same world-depth slab spans fewer pixels far away.
    float sigma_v = max(params.sigma_depth_rel * z0, 1e-3);
    float inv_2s2 = 1.0 / (2.0 * params.sigma_px * params.sigma_px);
    float inv_2v2 = 1.0 / (2.0 * sigma_v * sigma_v);

    float acc  = 0.0;
    float wsum = 0.0;

    [loop]
    for (int i = -TAP_HALF; i <= TAP_HALF; i++) {
        int2   tc  = clamp(c + step * i, int2(0, 0), maxc);
        float4 tap = t_ssao.Load(int3(tc, 0));

        float fi = (float)i;
        // Residual against the plane through the centre, not against the centre depth itself.
        float predicted = z0 + dzdt * fi;
        float residual  = tap.y - predicted;

        float ws = exp(-fi * fi * inv_2s2);
        float wv = exp(-residual * residual * inv_2v2);
        float wt = ws * wv;

        acc  += tap.x * wt;
        wsum += wt;
    }

    return float4(acc / max(wsum, 1e-5), z0, 0.0, 0.0);
}
