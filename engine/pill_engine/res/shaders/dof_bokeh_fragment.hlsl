// Far-field depth of field at quarter resolution.
// [Sousa13] T. Sousa, "Graphics Gems from CryENGINE 3", SIGGRAPH 2013
// Technique: "A Plausible and Efficient DoF Reconstruction Filter" — gather at reduced
// resolution (quarter-res via PassDofDownsample), CoC-weighted samples, far-field
// background depth clamping [Scheumermann05].
// CoC formula: simplified thin-lens [Potmesil81], folded into (1/P - 1/D)*scale.
// Sampling: golden-angle spiral [Shirley97] instead of Sousa's concentric-mapped n-tap.
// Composite: CoC² blend factor written to alpha; PassMotionBlur composites with full-res
// beauty per-sample [Sousa13 §Final Composite, non-linear blend].
// Not implemented: near-field scatter-as-gather + Tile Max CoC, CoC pre-multiply
//   [Gotanda09], bilateral upscale, diaphragm shape.

// Mirrors DofParams in pass_depth_of_field.rs
struct DofParams {
    float focus_point;    // focus distance in view-space units
    float focus_scale;    // CoC gain; 0.0 disables the pass
    float squeeze_x;      // horizontal bokeh squeeze: 1.0 = circular, 0.5 = 2:1 anamorphic oval
    float _padding;
};

[[vk::binding(0, 0)]] Texture2D<float4> t_color;
[[vk::binding(1, 0)]] Texture2D<float4> t_depth;
[[vk::binding(2, 0)]] ConstantBuffer<DofParams> params;
[[vk::binding(3, 0)]] SamplerState s_color;

static const float GOLDEN_ANGLE = 2.39996323;
static const float MAX_BLUR_SIZE = 10.0;
static const float RAD_SCALE = 0.5;
static const float COC_CURVE = 0.5;

float load_linear_depth(float2 uv, uint2 dims) {
    int2 coord0 = (int2)(uv * (float2)dims);
    int2 maxc   = int2((int)dims.x - 1, (int)dims.y - 1);
    int2 coord  = clamp(coord0, int2(0, 0), maxc);
    return t_depth.Load(int3(coord, 0)).r;
}

float coc_val(float depthLinear, float focusPoint, float focusScale) {
    return clamp((1.0 / focusPoint - 1.0 / depthLinear) * focusScale, -1.0, 1.0);
}

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    if (params.focus_scale == 0.0)
        return float4(t_color.SampleLevel(s_color, uv, 0).rgb, 0.0);

    uint w, h;
    t_depth.GetDimensions(w, h);
    uint2 dims = uint2(w, h);

    float  center_depth = max(load_linear_depth(uv, dims), 1e-4);
    float  focusPoint   = max(params.focus_point, 1e-4);
    float  coc_v        = coc_val(center_depth, focusPoint, params.focus_scale);
    float  coc_abs      = pow(abs(coc_v), COC_CURVE);

    float  center_size = coc_abs * MAX_BLUR_SIZE;
    uint cw, ch;
    t_color.GetDimensions(cw, ch);
    float2 pixel_size  = float2(1.0 / (float)cw, 1.0 / (float)ch);

    // In-focus: pass through with near-zero alpha so MB composites full-res beauty.
    if (center_size < 0.5)
        return float4(t_color.SampleLevel(s_color, uv, 0).rgb, coc_abs * coc_abs);

    float3 acc    = t_color.SampleLevel(s_color, uv, 0).rgb;
    float  tot    = 1.0;
    float  radius = RAD_SCALE;
    float  ang    = 0.0;

    // [Sousa13] §Far Field Processing: weight samples by CoC [Scheumermann05];
    // background samples clamped to 2×center_size to prevent foreground bleed.
    float loop_limit = min(MAX_BLUR_SIZE, center_size + 2.0);
    [loop]
    while (radius < loop_limit) {
        float2 dir          = float2(cos(ang), sin(ang));
        float2 tc           = uv + float2(dir.x * params.squeeze_x, dir.y) * pixel_size * radius;
        float3 sample_color = t_color.SampleLevel(s_color, tc, 0).rgb;
        float  sample_depth = max(load_linear_depth(tc, dims), 1e-4);
        float  scoc         = coc_val(sample_depth, focusPoint, params.focus_scale);
        float  sample_size  = pow(abs(scoc), COC_CURVE) * MAX_BLUR_SIZE;
        if (sample_depth > center_depth)
            sample_size = clamp(sample_size, 0.0, center_size * 2.0);
        float  m = smoothstep(radius - 0.5, radius + 0.5, sample_size);
        acc    = acc + lerp(acc / tot, sample_color, m);
        tot    = tot + 1.0;
        ang    = ang + GOLDEN_ANGLE;
        radius = radius + RAD_SCALE / radius;
    }

    // Alpha = CoC² blend factor consumed by PassMotionBlur for the full-res composite.
    float blend = coc_abs * coc_abs;
    return float4(acc / tot, blend);
}
