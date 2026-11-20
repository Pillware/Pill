// Default vertex shader. Edit here — `pill_assets` regenerates the .wgsl.
//
// Convention: HLSL `mul(M, v)` = matrix * column-vector (same as GLSL `M * v`).
// HLSL matrix constructors are row-major, so values here are transposed
// from the original GLSL column-major constructors.

#include "lit_common.hlsl"

struct VertexInput
{
    [[vk::location(0)]] float3 vertex_position       : POSITION;
    [[vk::location(1)]] float2 vertex_texture_coords : TEXCOORD0;
    [[vk::location(2)]] float3 vertex_normal         : NORMAL;
    [[vk::location(3)]] float3 vertex_tangent        : TANGENT;
    [[vk::location(4)]] float3 vertex_bitangent      : BINORMAL;
    // The instance's model matrix, as the top three rows of the affine
    // world-from-model transform (the fourth row is always 0, 0, 0, 1).
    [[vk::location(5)]] float4 model_row_0           : TEXCOORD1;
    [[vk::location(6)]] float4 model_row_1           : TEXCOORD2;
    [[vk::location(7)]] float4 model_row_2           : TEXCOORD3;
};

float3x3 inverse_mat3(float3x3 m)
{
    float m00 = m[0][0]; float m01 = m[0][1]; float m02 = m[0][2];
    float m10 = m[1][0]; float m11 = m[1][1]; float m12 = m[1][2];
    float m20 = m[2][0]; float m21 = m[2][1]; float m22 = m[2][2];

    float det = m00 * (m11 * m22 - m12 * m21)
              - m01 * (m10 * m22 - m12 * m20)
              + m02 * (m10 * m21 - m11 * m20);

    if (abs(det) < 1e-6)
    {
        // Fallback to identity if non-invertible.
        return float3x3(1, 0, 0,
                        0, 1, 0,
                        0, 0, 1);
    }

    float invDet = 1.0 / det;

    float3x3 inv;
    inv[0][0] =  (m11 * m22 - m12 * m21) * invDet;
    inv[0][1] = -(m01 * m22 - m02 * m21) * invDet;
    inv[0][2] =  (m01 * m12 - m02 * m11) * invDet;
    inv[1][0] = -(m10 * m22 - m12 * m20) * invDet;
    inv[1][1] =  (m00 * m22 - m02 * m20) * invDet;
    inv[1][2] = -(m00 * m12 - m02 * m10) * invDet;
    inv[2][0] =  (m10 * m21 - m11 * m20) * invDet;
    inv[2][1] = -(m00 * m21 - m01 * m20) * invDet;
    inv[2][2] =  (m00 * m11 - m01 * m10) * invDet;
    return inv;
}

[shader("vertex")]
LitVaryings vs_main(VertexInput input)
{
    // Built once per instance on the CPU, so a vertex only assembles it.
    float4x4 model_matrix = float4x4(
        input.model_row_0,
        input.model_row_1,
        input.model_row_2,
        float4(0.0, 0.0, 0.0, 1.0)
    );

    float3x3 model3x3 = (float3x3)model_matrix;
    float3x3 normal_matrix = transpose(inverse_mat3(model3x3));

    float3 tangent = normalize(mul(normal_matrix, input.vertex_tangent));
    float3 bitangent = normalize(mul(normal_matrix, input.vertex_bitangent));
    float3 normal = normalize(mul(normal_matrix, input.vertex_normal));

    // Transpose so tangent/bitangent/normal become rows. The fragment shader
    // reads each row as a separate location and reconstructs the matrix.
    float3x3 TBN_matrix = transpose(float3x3(tangent, bitangent, normal));

    float4 world_position = mul(model_matrix, float4(input.vertex_position, 1.0));

    LitVaryings output;
    output.TBN_tangent = TBN_matrix[0];
    output.TBN_bitangent = TBN_matrix[1];
    output.TBN_normal = TBN_matrix[2];
    output.tangent_space_position = mul(TBN_matrix, world_position.xyz);
    output.world_position = world_position.xyz;
    output.vertex_texture_coords = input.vertex_texture_coords;
    output.sv_position = mul(camera.camera_view_projection, world_position);

    return output;
}
