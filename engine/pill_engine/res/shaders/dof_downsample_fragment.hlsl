// [Karis13] luminance-weighted downsample, 1/(1+luma) per tap: stops one hot pixel owning a texel and flickering the far bokeh.
[[vk::binding(0, 0)]] Texture2D<float4> t_color;

float luma(float3 c) { return dot(c, float3(0.2126, 0.7152, 0.0722)); }

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    uint w, h;
    t_color.GetDimensions(w, h);

    // Map output UV to top-left of the 4x4 source block.
    int2 base = (int2)(uv * float2(w, h)) - int2(2, 2);
    int2 maxc = int2((int)w - 1, (int)h - 1);

    float3 acc  = float3(0.0, 0.0, 0.0);
    float  wsum = 0.0;

    [unroll]
    for (int dy = 0; dy < 4; dy++) {
        [unroll]
        for (int dx = 0; dx < 4; dx++) {
            int2   coord = clamp(base + int2(dx, dy), int2(0, 0), maxc);
            float3 s     = t_color.Load(int3(coord, 0)).rgb;
            float  w     = 1.0 / (1.0 + luma(s));
            acc  += s * w;
            wsum += w;
        }
    }

    return float4(acc / max(wsum, 1e-5), 1.0);
}
