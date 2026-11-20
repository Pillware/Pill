// Lens: barrel distortion, lateral chromatic aberration, a per-channel grade and
// a vignette, ported from the reference's `lens_fragment.hlsl`.
//
// Edit here - `pill_assets` regenerates the .wgsl.
//
// The reference's film grain samples a 256x256 tile the reference commits as an
// asset; this project generates an equivalent tile at load time. The Halton(2,3)
// jitter is left out: it is refreshed every frame, and a pass's parameters are
// read when the chain is built rather than once a frame, so the tile is held
// still. Everything else is the reference's arithmetic, with its parameters
// grouped into slots because that is how the engine packs them.

struct MaterialParams
{
    // x = barrel k1 (negative barrels, positive pincushions), y = vignette
    // strength, z = radial chromatic aberration scale, w = unused.
    float4 shape;
    // xyz = per-channel exponent. w unused.
    float4 gamma;
    // x = edge0, y = edge1, z = mix. A descending ramp (edge0 > edge1) inverts.
    float4 grade;
    // x = grain amplitude, y = pixels of screen per grain texel, z = the grain
    // tile's edge in texels (the pass passes the size of the texture it
    // generated), w unused.
    float4 grain;
};

[[vk::binding(0, 2)]]
ConstantBuffer<MaterialParams> material;

[[vk::binding(0, 3)]]
Texture2D<float4> source_texture;

[[vk::binding(1, 3)]]
SamplerState source_sampler;

[[vk::binding(2, 3)]]
Texture2D<float4> grain_texture;

[[vk::binding(3, 3)]]
SamplerState grain_sampler;

struct PixelInput
{
    [[vk::location(0)]] float2 texture_coords : TEXCOORD0;

    float4 position : SV_Position;
};

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

[shader("fragment")]
PixelOutput fs_main(PixelInput input)
{
    PixelOutput result;

    float2 centred = input.texture_coords * 2.0 - 1.0;
    float r2 = dot(centred, centred);

    // Barrel distortion about the centre.
    float2 warped = centred * (1.0 + material.shape.x * r2) * 0.5 + 0.5;

    // Lateral chromatic aberration: red and blue are sampled along the radius,
    // so the split is zero on axis and grows with r^2. A constant offset would
    // have a direction discontinuity at the centre and displace edges there.
    float2 offset = (warped - 0.5) * (r2 * material.shape.z);
    float red = source_texture.Sample(source_sampler, warped - offset).r;
    float green = source_texture.Sample(source_sampler, warped).g;
    float blue = source_texture.Sample(source_sampler, warped + offset).b;
    float3 colour = float3(red, green, blue);

    // Per-channel gamma behind a smoothstep ramp; max() keeps pow away from a
    // negative base.
    float3 graded = smoothstep(
        material.grade.x,
        material.grade.y,
        pow(max(colour, 0.0), material.gamma.rgb)
    );
    colour = lerp(colour, graded, material.grade.z);

    // After the grade: ahead of a descending ramp, the darkened corners are what
    // come out bright.
    float vignette = 1.0 - smoothstep(
        0.45,
        1.05,
        length(centred) * material.shape.y
    );
    colour *= vignette;

    // Film grain, the reference's formulation: the tile is sampled in screen
    // pixels, so its texels stay the size the amplitude was chosen for. The
    // black limit keeps a negative grain from crushing blacks.
    float2 grain_uv = input.position.xy / (material.grain.y * material.grain.z);
    float3 grain = grain_texture.Sample(grain_sampler, grain_uv).rgb * 2.0 - 1.0;
    float black_limit = material.grain.x * 0.5;
    float grain_amount = material.grain.x * 0.75;
    colour = grain * min(colour + black_limit, grain_amount) + colour;

    result.output = float4(colour, 1.0);
    return result;
}
