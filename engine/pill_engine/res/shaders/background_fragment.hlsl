struct BackgroundCamera {
    float3 right;            float tangent_half_fov;
    float3 up;               float aspect;
    float3 forward;          float _padding;
    float3 background_color;  float _padding_2;
};
[[vk::binding(0, 0)]] ConstantBuffer<BackgroundCamera> camera;
[[vk::binding(1, 0)]] Texture2D    equirect_texture;
[[vk::binding(2, 0)]] SamplerState equirect_sampler;

#include "include/equirect.hlsl"

[shader("fragment")]
float4 fs_main(float2 normalized_device_coordinates : TEXCOORD0) : SV_Target {
    float3 direction = normalize(
        camera.right * (normalized_device_coordinates.x * camera.tangent_half_fov * camera.aspect)
      + camera.up    * (normalized_device_coordinates.y * camera.tangent_half_fov)
      + camera.forward
    );
    float2 texture_coordinates = dir_to_equirect_uv(direction);
    float3 sky_color = equirect_texture.Sample(equirect_sampler, texture_coordinates).rgb;
    return float4(sky_color * camera.background_color, 1.0);
}
