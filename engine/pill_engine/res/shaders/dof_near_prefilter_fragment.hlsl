// [Sousa13] §Near Field Processing: premultiply the near layer by CoC so only near fragments blur — cheap partial-occlusion approximation.

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

[[vk::binding(0, 0)]] Texture2D<float4> t_color;   // full-res hdr
[[vk::binding(1, 0)]] Texture2D<float4> t_depth;   // full-res linear depth (PassDepthOfField)
[[vk::binding(2, 0)]] ConstantBuffer<DofNearParams> params;

// Fade width below the cut.
static const float NEAR_CUT_WIDTH = 0.1;

// Same signed CoC as dof_bokeh_fragment.hlsl; negative == in front of the focal plane.
float coc_val(float depthLinear, float focusPoint, float focusScale) {
    return clamp((1.0 / focusPoint - 1.0 / depthLinear) * focusScale, -1.0, 1.0);
}

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    // Off-switch, matching PassDofBokeh's `focus_scale == 0` early-out.
    if (params.focus_scale == 0.0 || params.near_blur_scale == 0.0)
        return float4(0.0, 0.0, 0.0, 0.0);

    uint w, h;
    t_color.GetDimensions(w, h);
    int2 maxc = int2((int)w - 1, (int)h - 1);
    // 10x10, not 8x8: the target is floor(S/8), so an 8-wide block leaves uncovered columns.
    int2 base = (int2)(uv * float2(w, h)) - int2(5, 5);

    float  focusPoint = max(params.focus_point, 1e-4);
    float3 sum_rgb    = float3(0.0, 0.0, 0.0);
    float  sum_coc    = 0.0;
    float  count      = 0.0;

    [loop]
    for (int dy = 0; dy < 10; dy++) {
        [loop]
        for (int dx = 0; dx < 10; dx++) {
            int2   coord = clamp(base + int2(dx, dy), int2(0, 0), maxc);
            float3 c     = t_color.Load(int3(coord, 0)).rgb;
            float  d     = max(t_depth.Load(int3(coord, 0)).r, 1e-4);
            float  coc   = coc_val(d, focusPoint, params.focus_scale);
            // Near side only, no gain: near_blur_scale drives sigma, not the threshold.
            float  n     = saturate(max(-coc, 0.0));
            float  mask  = smoothstep(params.near_cut - NEAR_CUT_WIDTH, params.near_cut, n);

            sum_rgb += c * mask;
            sum_coc += mask;
            count   += 1.0;
        }
    }

    // Premultiplied; the composite unpremultiplies once at the end.
    return float4(sum_rgb / count, sum_coc / count);
}
