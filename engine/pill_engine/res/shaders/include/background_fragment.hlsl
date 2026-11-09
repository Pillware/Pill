// Background fragment shader body, included by background_fragment.hlsl and (with POST_FX, which
// adds the sky velocity output) background_post_fx_fragment.hlsl.
struct BackgroundCamera {
    float3 right;            float tangent_half_fov;
    float3 up;               float aspect;
    float3 forward;          float _padding;
    float3 background_color;  float _padding_2;
    column_major float4x4 previous_view_projection;
};
[[vk::binding(0, 0)]] ConstantBuffer<BackgroundCamera> camera;
[[vk::binding(1, 0)]] Texture2D    equirect_texture;
[[vk::binding(2, 0)]] SamplerState equirect_sampler;

#include "equirect.hlsl"

// Same targets as the PBR pass: color and, with POST_FX, velocity (ambient is masked off).
struct FragmentOutput {
    [[vk::location(0)]] float4 color    : SV_TARGET0;
#ifdef POST_FX
    [[vk::location(1)]] float2 velocity : SV_TARGET1;
#endif
};

[shader("fragment")]
FragmentOutput fs_main(float2 normalized_device_coordinates : TEXCOORD0) {
    float3 direction = normalize(
        camera.right * (normalized_device_coordinates.x * camera.tangent_half_fov * camera.aspect)
      + camera.up    * (normalized_device_coordinates.y * camera.tangent_half_fov)
      + camera.forward
    );
    float2 texture_coordinates = dir_to_equirect_uv(direction);
    float3 sky_color = equirect_texture.Sample(equirect_sampler, texture_coordinates).rgb;
    FragmentOutput output;
    output.color    = float4(sky_color * camera.background_color, 1.0);
#ifdef POST_FX
    // The sky is at infinity: only camera rotation moves it, so project the direction (w = 0)
    float4 previous_clip = mul(camera.previous_view_projection, float4(direction, 0.0));
    float2 previous_ndc  = previous_clip.w > 1e-4 ? previous_clip.xy / previous_clip.w : normalized_device_coordinates;
    output.velocity = (normalized_device_coordinates - previous_ndc) * float2(0.5, -0.5);
#endif
    return output;
}
