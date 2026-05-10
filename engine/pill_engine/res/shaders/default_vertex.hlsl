// Default vertex shader. Edit here — `pill_assets` regenerates the .wgsl.
//
// Convention: HLSL `mul(M, v)` = matrix * column-vector (same as GLSL `M * v`).
// HLSL matrix constructors are row-major, so values here are transposed
// from the original GLSL column-major constructors.

#include "include/common.hlsl"

struct VS_OUT {
    [[vk::location(0)]] float3 vertex_position       : TEXCOORD0;
    [[vk::location(1)]] float2 vertex_texture_coords : TEXCOORD1;
    [[vk::location(2)]] float3 TBN_tangent           : TEXCOORD2;
    [[vk::location(3)]] float3 TBN_bitangent         : TEXCOORD3;
    [[vk::location(4)]] float3 TBN_normal            : TEXCOORD4;
    [[vk::location(5)]] float3 world_position        : TEXCOORD5;
                        float4 sv_position           : SV_POSITION;
};

float3x3 inverse_mat3(float3x3 m) {
    float m00 = m[0][0]; float m01 = m[0][1]; float m02 = m[0][2];
    float m10 = m[1][0]; float m11 = m[1][1]; float m12 = m[1][2];
    float m20 = m[2][0]; float m21 = m[2][1]; float m22 = m[2][2];

    float det = m00 * (m11 * m22 - m12 * m21)
              - m01 * (m10 * m22 - m12 * m20)
              + m02 * (m10 * m21 - m11 * m20);

    if (abs(det) < 1e-6) {
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

float4x4 compute_model_matrix(float3 position, float3 rotation, float3 scale) {
    float4x4 scale_matrix = float4x4(
        scale.x, 0,       0,       0,
        0,       scale.y, 0,       0,
        0,       0,       scale.z, 0,
        0,       0,       0,       1
    );

    float cx = cos(rotation.x); float sx = sin(rotation.x);
    float cy = cos(rotation.y); float sy = sin(rotation.y);
    float cz = cos(rotation.z); float sz = sin(rotation.z);

    // X-axis rotation (rotates +Y → +Z).
    float4x4 rot_x = float4x4(
        1, 0,    0,  0,
        0, cx,   sx, 0,
        0, -sx,  cx, 0,
        0, 0,    0,  1
    );

    // Y-axis rotation (rotates +Z → +X).
    float4x4 rot_y = float4x4(
        cy,  0, -sy, 0,
        0,   1, 0,   0,
        sy,  0, cy,  0,
        0,   0, 0,   1
    );

    // Z-axis rotation (rotates +X → +Y).
    float4x4 rot_z = float4x4(
        cz,  sz, 0, 0,
        -sz, cz, 0, 0,
        0,   0,  1, 0,
        0,   0,  0, 1
    );

    float4x4 rotation_matrix = mul(rot_z, mul(rot_y, rot_x));

    float4x4 translation_matrix = float4x4(
        1, 0, 0, position.x,
        0, 1, 0, position.y,
        0, 0, 1, position.z,
        0, 0, 0, 1
    );

    return mul(translation_matrix, mul(rotation_matrix, scale_matrix));
}

[shader("vertex")]
VS_OUT vs_main(
    [[vk::location(0)]] float3 in_vertex_position       : POSITION,
    [[vk::location(1)]] float2 in_vertex_texture_coords : TEXCOORD0,
    [[vk::location(2)]] float3 in_vertex_normal         : NORMAL,
    [[vk::location(3)]] float3 in_vertex_tangent        : TANGENT,
    [[vk::location(4)]] float3 in_vertex_bitangent      : BINORMAL,
    [[vk::location(5)]] float3 transform_position       : TEXCOORD1,
    [[vk::location(6)]] float3 transform_rotation       : TEXCOORD2,
    [[vk::location(7)]] float3 transform_scale          : TEXCOORD3
) {
    float4x4 model_matrix = compute_model_matrix(transform_position, transform_rotation, transform_scale);

    float3x3 model3x3 = (float3x3)model_matrix;
    float3x3 normal_matrix = transpose(inverse_mat3(model3x3));

    float3 tangent   = normalize(mul(normal_matrix, in_vertex_tangent));
    float3 bitangent = normalize(mul(normal_matrix, in_vertex_bitangent));
    float3 normal    = normalize(mul(normal_matrix, in_vertex_normal));

    // Transpose so tangent/bitangent/normal become rows. The fragment shader
    // reads each row as a separate location and reconstructs the matrix.
    float3x3 TBN_matrix = transpose(float3x3(tangent, bitangent, normal));

    float4 model_space = mul(model_matrix, float4(in_vertex_position, 1.0));

    VS_OUT o;
    o.TBN_tangent           = TBN_matrix[0];
    o.TBN_bitangent         = TBN_matrix[1];
    o.TBN_normal            = TBN_matrix[2];
    o.vertex_position       = mul(TBN_matrix, model_space.xyz);
    o.world_position        = model_space.xyz;
    o.vertex_texture_coords = in_vertex_texture_coords;
    o.sv_position           = mul(camera.camera_view_projection, model_space);
    return o;
}
