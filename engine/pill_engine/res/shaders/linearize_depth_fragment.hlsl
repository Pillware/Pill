// Depth buffer → view-space distance (R16Float), for the depth of field passes.

struct DepthUnpack {
    float depth_mul; // distance = depth_mul / (depth_add - depth)
    float depth_add;
    float2 _padding;
};

[[vk::binding(0, 0)]] Texture2D<float> depth_texture;
[[vk::binding(1, 0)]] ConstantBuffer<DepthUnpack> params;

[shader("fragment")]
float4 fs_main(float2 texture_coordinates : TEXCOORD0) : SV_Target {
    uint width, height;
    depth_texture.GetDimensions(width, height);
    int2  texel = clamp(int2(texture_coordinates * float2(width, height)), int2(0, 0), int2(width, height) - 1);
    float depth = depth_texture.Load(int3(texel, 0));
    return float4(params.depth_mul / (params.depth_add - depth), 0.0, 0.0, 1.0);
}
