[[vk::binding(0, 0)]] Texture2D    hdr_texture;
[[vk::binding(1, 0)]] SamplerState hdr_sampler;

// Mirrors TonemapParams in pass_tonemap.rs (RenderStateComponent::tonemap)
struct TonemapParams {
    float reinhard;              // 1 = extended Reinhard, 0 = pass-through
    float inverse_white_squared; // 1/W², 0 for an infinite white point (plain c/(1 + c))
    float2 _padding;
};
[[vk::binding(2, 0)]] ConstantBuffer<TonemapParams> params;

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    float3 hdr_color = hdr_texture.Sample(hdr_sampler, texture_coordinates).rgb;
    if (params.reinhard > 0.5) {
        hdr_color = hdr_color * (1.0 + hdr_color * params.inverse_white_squared) / (1.0 + hdr_color);
    }
    return float4(hdr_color, 1.0);
}
