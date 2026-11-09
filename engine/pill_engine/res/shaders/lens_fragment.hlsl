// Lens and film: barrel distortion, lateral chromatic aberration, grade, vignette and film grain.
// Writes the final image (the swapchain's sRGB view encodes it).

struct LensParams {
    float ca_scale;          // lateral CA radial coefficient (chromatic_aberration_scale in pass_lens.rs)
    float distortion;        // k1: negative = barrel, positive = pincushion
    float vignette;          // vignette falloff scale, 0 = off
    float grain_strength;    // linear amplitude: 0.03 barely visible, 0.12 medium film stock
    int   grain_offset_x;    // Halton(2,3) grain tile offset, 0-255
    int   grain_offset_y;
    float grain_pixel_scale; // screen pixels per grain texel
    float aspect_yx;         // height / width, squares the CA space
    float4 grade_gamma;      // xyz = per-channel pow exponent
    float4 grade_params;     // x = edge0, y = edge1, z = mix (0 = off)
};

// smoothstep(edge0, edge1, x) that is also defined for edge0 > edge1 (a descending ramp) and for
// equal edges: the builtin is undefined there on Metal/GLSL and divides by zero
float3 ramp(float edge0, float edge1, float3 x) {
    float  span = edge1 - edge0;
    float  denominator = abs(span) < 1e-4 ? (span < 0.0 ? -1e-4 : 1e-4) : span;
    float3 t = saturate((x - edge0) / denominator);
    return t * t * (3.0 - 2.0 * t);
}

[[vk::binding(0, 0)]] Texture2D                  source_texture;
[[vk::binding(1, 0)]] SamplerState               linear_sampler;
[[vk::binding(2, 0)]] ConstantBuffer<LensParams> params;
[[vk::binding(3, 0)]] Texture2D                  grain_texture;
[[vk::binding(4, 0)]] SamplerState               grain_sampler; // linear, repeat

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0, float4 position : SV_Position) : SV_Target {
    float2 centered = texture_coordinates * 2.0 - 1.0;
    float  radius_squared = dot(centered, centered);

    float2 distorted = centered * (1.0 + params.distortion * radius_squared) * 0.5 + 0.5;

    // Lateral CA (KinoFringe, K. Takahashi): radial 1 ∓ r² scale, zero on the axis, so edges at the
    // image center don't shift
    float2 ca_centered = (distorted - 0.5) * float2(params.aspect_yx, 1.0);
    float2 ca_offset   = (distorted - 0.5) * (dot(ca_centered, ca_centered) * params.ca_scale);
    float3 color = float3(
        source_texture.SampleLevel(linear_sampler, distorted - ca_offset, 0).r,
        source_texture.SampleLevel(linear_sampler, distorted, 0).g,
        source_texture.SampleLevel(linear_sampler, distorted + ca_offset, 0).b);

    // Per-channel gamma, then a smoothstep ramp (edge0 > edge1 inverts); max() keeps pow off NaN
    float3 graded = ramp(params.grade_params.x, params.grade_params.y,
                         pow(max(color, 0.0), params.grade_gamma.rgb));
    color = lerp(color, graded, params.grade_params.z);

    // After the grade, so a descending ramp doesn't turn dark corners bright
    color *= 1.0 - smoothstep(0.45, 1.05, length(centered) * params.vignette);

    // Film grain (Lottes, AMD GPUOpen "The Fine Art of Film Grain"): a Halton-jittered high-pass
    // noise tile; the black limit keeps grain from crushing blacks
    float2 grain_coordinates = (position.xy / params.grain_pixel_scale
                              + float2(params.grain_offset_x, params.grain_offset_y)) / 256.0;
    float3 grain = grain_texture.SampleLevel(grain_sampler, grain_coordinates, 0).rgb * 2.0 - 1.0;
    color += grain * min(color + params.grain_strength * 0.5, params.grain_strength * 0.75);

    return float4(color, 1.0);
}
