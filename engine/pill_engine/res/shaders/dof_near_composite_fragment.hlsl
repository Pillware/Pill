// [Sousa13] §Final Composite: vertical half of the near blur, then near-over-far packed into one
// lerp factor (alpha), so PassMotionBlur composites both layers with the full-res image for free.

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

[[vk::binding(0, 0)]] Texture2D<float4> t_near;   // eighth-res, horizontally blurred
[[vk::binding(1, 0)]] Texture2D<float4> t_far;    // quarter-res far bokeh (rgb, a = CoC²)
[[vk::binding(2, 0)]] ConstantBuffer<DofNearParams> params;
[[vk::binding(3, 0)]] SamplerState s_linear;

static const int TAP_HALF = 12;   // 25 taps total, matching PassDofNearBlurH

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    uint fw, fh;
    t_far.GetDimensions(fw, fh);
    int2 far_coord = clamp((int2)(uv * float2(fw, fh)),
                           int2(0, 0), int2((int)fw - 1, (int)fh - 1));
    float4 far = t_far.Load(int3(far_coord, 0));

    float4 blurred = float4(0.0, 0.0, 0.0, 0.0);
    float  sigma   = params.near_sigma_px;

    if (params.focus_scale != 0.0 && params.near_blur_scale != 0.0) {
        uint nw, nh;
        t_near.GetDimensions(nw, nh);
        float2 texel = float2(1.0 / (float)nw, 1.0 / (float)nh) * params.blur_dir;

        if (sigma < 0.05) {
            blurred = t_near.SampleLevel(s_linear, uv, 0);
        } else {
            float step = 3.0 * sigma / (float)TAP_HALF;
            float wsum = 0.0;
            [loop]
            for (int i = -TAP_HALF; i <= TAP_HALF; i++) {
                float d = (float)i * step;
                float g = exp(-0.5 * (d / sigma) * (d / sigma));
                blurred += t_near.SampleLevel(s_linear, uv + texel * d, 0) * g;
                wsum    += g;
            }
            blurred /= max(wsum, 1e-5);
        }
    }

    float  na       = saturate(blurred.a);
    float3 near_rgb = blurred.rgb / max(blurred.a, 1e-5);   // unpremultiply

    float  fa      = far.a;
    float  out_a   = na + fa * (1.0 - na);
    float3 out_rgb = lerp(far.rgb, near_rgb, na / max(out_a, 1e-5));

    return float4(out_rgb, out_a);
}
