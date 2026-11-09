// Separable Gaussian, horizontal half (the vertical half is in dof_near_composite): the near field is a soft veil, not bokeh, so 2N taps buy the smoothness N^2 would.

// Mirrors DofNearParams in pass_depth_of_field.rs
struct DofNearParams {
    float focus_point;
    float focus_scale;
    float near_blur_scale; // 0 = near layer off
    float near_cut;        // near CoC at which the layer reaches full opacity
    float near_sigma_px;   // Gaussian sigma in eighth-res pixels
    float _padding;
    float2 blur_dir;       // (1,0) horizontal, (0,1) vertical
};

[[vk::binding(0, 0)]] Texture2D<float4> t_near;   // eighth-res premultiplied near layer
[[vk::binding(1, 0)]] ConstantBuffer<DofNearParams> params;
[[vk::binding(2, 0)]] SamplerState s_linear;

static const int TAP_HALF = 12;   // 25 taps total

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    float sigma = params.near_sigma_px;
    if (params.focus_scale == 0.0 || params.near_blur_scale == 0.0 || sigma < 0.05)
        return t_near.SampleLevel(s_linear, uv, 0);

    uint w, h;
    t_near.GetDimensions(w, h);
    float2 texel = float2(1.0 / (float)w, 1.0 / (float)h) * params.blur_dir;

    float  step = 3.0 * sigma / (float)TAP_HALF;   // kernel spans +-3 sigma
    float4 acc  = float4(0.0, 0.0, 0.0, 0.0);
    float  wsum = 0.0;

    [loop]
    for (int i = -TAP_HALF; i <= TAP_HALF; i++) {
        float d = (float)i * step;
        float g = exp(-0.5 * (d / sigma) * (d / sigma));
        acc  += t_near.SampleLevel(s_linear, uv + texel * d, 0) * g;
        wsum += g;
    }

    return acc / max(wsum, 1e-5);
}
