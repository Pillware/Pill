// Skybox fragment stage for a cubemap.
//
// Samples the cube along the pixel's view direction. The `sky` slot is declared
// as `TextureType::Cubemap`, so the renderer binds a six-face half-float cube
// whose faces were projected from a panorama at import.
//
// Edit here - `pill_assets` regenerates the .wgsl.

#include "skybox_common.hlsl"

[[vk::binding(0, 3)]]
TextureCube<float4> sky_texture;

[[vk::binding(1, 3)]]
SamplerState sky_sampler;

[shader("fragment")]
PixelOutput fs_main(PixelInput input)
{
    float3 direction = skybox_direction(input.clip_position);

    PixelOutput result;
    result.output = skybox_shade(sky_texture.SampleLevel(sky_sampler, direction, 0.0).rgb);
    return result;
}
