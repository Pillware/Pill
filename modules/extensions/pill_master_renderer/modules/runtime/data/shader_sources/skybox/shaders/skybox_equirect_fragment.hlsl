// Skybox fragment stage for an equirectangular panorama.
//
// Turns the pixel's view direction into the panorama's uv - the convention
// `assets::texture::equirect_uv` defines, so a cubemap projected from the same
// image shows the same sky - and samples it. The `sky` slot is declared as
// `TextureType::Equirect`, so the renderer binds a linear half-float panorama.
//
// Edit here - `pill_assets` regenerates the .wgsl.

#include "skybox_common.hlsl"

[[vk::binding(0, 3)]]
Texture2D<float4> sky_texture;

[[vk::binding(1, 3)]]
SamplerState sky_sampler;

[shader("fragment")]
PixelOutput fs_main(PixelInput input)
{
    float3 direction = skybox_direction(input.clip_position);
    float2 uv = float2(
        0.5 + atan2(direction.x, -direction.z) / (2.0 * SKYBOX_PI),
        0.5 - asin(clamp(direction.y, -1.0, 1.0)) / SKYBOX_PI
    );

    // Level 0 explicitly: the uv jumps by a whole turn across the panorama's
    // seam, and an implicit level would read that jump as extreme minification.
    PixelOutput result;
    result.output = skybox_shade(sky_texture.SampleLevel(sky_sampler, uv, 0.0).rgb);
    return result;
}
