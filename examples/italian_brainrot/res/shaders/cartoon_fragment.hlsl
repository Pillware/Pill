// Cartoon fragment shader for the game-defined CartoonPass. Edit here — `pill_assets` regenerates the .wgsl.
// Posterizes the color texture per channel.

[[vk::binding(0, 1)]] Texture2D    color_texture;
[[vk::binding(1, 1)]] SamplerState color_sampler;

[shader("fragment")]
float4 fs_main(
    [[vk::location(0)]]                  float2 texture_coordinates : TEXCOORD0,
    [[vk::location(1)]] nointerpolation float  posterize_level     : TEXCOORD1
) : SV_TARGET {
    float4 object_color = color_texture.Sample(color_sampler, texture_coordinates);

    // Posterize per channel: floor(rgb * levels) / levels.
    float levels = max(posterize_level, 1.0);
    float3 posterized = floor(object_color.xyz * levels) / levels;

    return float4(posterized, 1.0);
}
