[[vk::binding(0, 0)]] Texture2D    hdr_texture;
[[vk::binding(1, 0)]] SamplerState hdr_sampler;

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    float3 hdr_color    = hdr_texture.Sample(hdr_sampler, texture_coordinates).rgb;
    float3 mapped_color = hdr_color / (hdr_color + 1.0);
    return float4(mapped_color, 1.0);
}
