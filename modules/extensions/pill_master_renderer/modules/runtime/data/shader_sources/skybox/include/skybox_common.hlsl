// What both skybox fragment stages share: the material's parameters and the
// pixel's view direction.
//
// The parameters follow the engine's packing, one `float4` per slot:
//
//   skybox_tint      xyz = colour the sky is multiplied by (Color slot)
//   skybox_exposure  x   = brightness multiplier (Scalar slot)
//   skybox_rotation  x   = turn around the world's up axis, degrees (Scalar)
//
// Edit here - `pill_assets` re-cooks every stage that includes it.

#include "common.hlsl"

struct SkyboxParams
{
    float4 skybox_tint;
    float4 skybox_exposure;
    float4 skybox_rotation;
};

[[vk::binding(0, 2)]]
ConstantBuffer<SkyboxParams> material;

struct PixelInput
{
    [[vk::location(0)]] float2 clip_position : TEXCOORD0;
};

struct PixelOutput
{
    [[vk::location(0)]] float4 output : SV_TARGET0;
};

static const float SKYBOX_PI = 3.14159265359;

// The world direction a pixel looks along, turned by the sky's rotation.
//
// The far-plane point under the pixel, taken back to world space, minus the
// camera's position. The rotation turns the sky rather than the camera, so a
// positive angle swings the sky to the left.
float3 skybox_direction(float2 clip_position)
{
    float4 world = mul(camera.camera_inverse_view_projection, float4(clip_position, 1.0, 1.0));
    float3 direction = world.xyz / world.w - camera.camera_position;

    float angle = radians(material.skybox_rotation.x);
    float cosine = cos(angle);
    float sine = sin(angle);
    direction = float3(
        cosine * direction.x + sine * direction.z,
        direction.y,
        -sine * direction.x + cosine * direction.z
    );
    return normalize(direction);
}

// The sky's colour, tinted and exposed.
float4 skybox_shade(float3 sky)
{
    return float4(sky * material.skybox_tint.xyz * material.skybox_exposure.x, 1.0);
}
