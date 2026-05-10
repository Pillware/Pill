struct _MatrixStorage_float4x4_ColMajorstd140_0
{
    @align(16) data_0 : array<vec4<f32>, i32(4)>,
};

struct CameraParams_std140_0
{
    @align(16) camera_position_0 : vec3<f32>,
    @align(16) camera_view_projection_0 : _MatrixStorage_float4x4_ColMajorstd140_0,
};

@binding(0) @group(1) var<uniform> camera_0 : CameraParams_std140_0;
fn compute_model_matrix_0( position_0 : vec3<f32>,  rotation_0 : vec3<f32>,  scale_0 : vec3<f32>) -> mat4x4<f32>
{
    var scale_matrix_0 : mat4x4<f32> = mat4x4<f32>(scale_0.x, 0.0f, 0.0f, 0.0f, 0.0f, scale_0.y, 0.0f, 0.0f, 0.0f, 0.0f, scale_0.z, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f);
    var _S1 : f32 = rotation_0.x;
    var cx_0 : f32 = cos(_S1);
    var sx_0 : f32 = sin(_S1);
    var _S2 : f32 = rotation_0.y;
    var cy_0 : f32 = cos(_S2);
    var sy_0 : f32 = sin(_S2);
    var _S3 : f32 = rotation_0.z;
    var cz_0 : f32 = cos(_S3);
    var sz_0 : f32 = sin(_S3);
    var rot_x_0 : mat4x4<f32> = mat4x4<f32>(1.0f, 0.0f, 0.0f, 0.0f, 0.0f, cx_0, sx_0, 0.0f, 0.0f, - sx_0, cx_0, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f);
    var rot_y_0 : mat4x4<f32> = mat4x4<f32>(cy_0, 0.0f, - sy_0, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, sy_0, 0.0f, cy_0, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f);
    var rot_z_0 : mat4x4<f32> = mat4x4<f32>(cz_0, sz_0, 0.0f, 0.0f, - sz_0, cz_0, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, 0.0f, 0.0f, 1.0f);
    var rotation_matrix_0 : mat4x4<f32> = ((((((rot_x_0) * (rot_y_0)))) * (rot_z_0)));
    var translation_matrix_0 : mat4x4<f32> = mat4x4<f32>(1.0f, 0.0f, 0.0f, position_0.x, 0.0f, 1.0f, 0.0f, position_0.y, 0.0f, 0.0f, 1.0f, position_0.z, 0.0f, 0.0f, 0.0f, 1.0f);
    return ((((((scale_matrix_0) * (rotation_matrix_0)))) * (translation_matrix_0)));
}

fn inverse_mat3_0( m_0 : mat3x3<f32>) -> mat3x3<f32>
{
    var _S4 : f32 = m_0[i32(1)][i32(1)] * m_0[i32(2)][i32(2)] - m_0[i32(1)][i32(2)] * m_0[i32(2)][i32(1)];
    var _S5 : f32 = m_0[i32(1)][i32(0)] * m_0[i32(2)][i32(2)] - m_0[i32(1)][i32(2)] * m_0[i32(2)][i32(0)];
    var _S6 : f32 = m_0[i32(1)][i32(0)] * m_0[i32(2)][i32(1)] - m_0[i32(1)][i32(1)] * m_0[i32(2)][i32(0)];
    var det_0 : f32 = m_0[i32(0)][i32(0)] * _S4 - m_0[i32(0)][i32(1)] * _S5 + m_0[i32(0)][i32(2)] * _S6;
    var _S7 : bool = (abs(det_0)) < 9.99999997475242708e-07f;
    if(_S7)
    {
        return mat3x3<f32>(1.0f, 0.0f, 0.0f, 0.0f, 1.0f, 0.0f, 0.0f, 0.0f, 1.0f);
    }
    var invDet_0 : f32 = 1.0f / det_0;
    var inv_0 : mat3x3<f32>;
    var _S8 : f32 = _S4 * invDet_0;
    inv_0[i32(0)][i32(0)] = _S8;
    var _S9 : f32 = - (m_0[i32(0)][i32(1)] * m_0[i32(2)][i32(2)] - m_0[i32(0)][i32(2)] * m_0[i32(2)][i32(1)]) * invDet_0;
    inv_0[i32(0)][i32(1)] = _S9;
    var _S10 : f32 = (m_0[i32(0)][i32(1)] * m_0[i32(1)][i32(2)] - m_0[i32(0)][i32(2)] * m_0[i32(1)][i32(1)]) * invDet_0;
    inv_0[i32(0)][i32(2)] = _S10;
    var _S11 : f32 = - _S5 * invDet_0;
    inv_0[i32(1)][i32(0)] = _S11;
    var _S12 : f32 = (m_0[i32(0)][i32(0)] * m_0[i32(2)][i32(2)] - m_0[i32(0)][i32(2)] * m_0[i32(2)][i32(0)]) * invDet_0;
    inv_0[i32(1)][i32(1)] = _S12;
    var _S13 : f32 = - (m_0[i32(0)][i32(0)] * m_0[i32(1)][i32(2)] - m_0[i32(0)][i32(2)] * m_0[i32(1)][i32(0)]) * invDet_0;
    inv_0[i32(1)][i32(2)] = _S13;
    var _S14 : f32 = _S6 * invDet_0;
    inv_0[i32(2)][i32(0)] = _S14;
    var _S15 : f32 = - (m_0[i32(0)][i32(0)] * m_0[i32(2)][i32(1)] - m_0[i32(0)][i32(1)] * m_0[i32(2)][i32(0)]) * invDet_0;
    inv_0[i32(2)][i32(1)] = _S15;
    var _S16 : f32 = (m_0[i32(0)][i32(0)] * m_0[i32(1)][i32(1)] - m_0[i32(0)][i32(1)] * m_0[i32(1)][i32(0)]) * invDet_0;
    inv_0[i32(2)][i32(2)] = _S16;
    return inv_0;
}

struct VS_OUT_0
{
    @location(0) vertex_position_0 : vec3<f32>,
    @location(1) vertex_texture_coords_0 : vec2<f32>,
    @location(2) TBN_tangent_0 : vec3<f32>,
    @location(3) TBN_bitangent_0 : vec3<f32>,
    @location(4) TBN_normal_0 : vec3<f32>,
    @location(5) world_position_0 : vec3<f32>,
    @builtin(position) sv_position_0 : vec4<f32>,
};

struct vertexInput_0
{
    @location(0) in_vertex_position_0 : vec3<f32>,
    @location(4) in_vertex_texture_coords_0 : vec2<f32>,
    @location(5) in_vertex_normal_0 : vec3<f32>,
    @location(6) in_vertex_tangent_0 : vec3<f32>,
    @location(7) in_vertex_bitangent_0 : vec3<f32>,
    @location(1) transform_position_0 : vec3<f32>,
    @location(2) transform_rotation_0 : vec3<f32>,
    @location(3) transform_scale_0 : vec3<f32>,
};

@vertex
fn vs_main( _S17 : vertexInput_0) -> VS_OUT_0
{
    var model_matrix_0 : mat4x4<f32> = compute_model_matrix_0(_S17.transform_position_0, _S17.transform_rotation_0, _S17.transform_scale_0);
    var _S18 : mat3x3<f32> = mat3x3<f32>(model_matrix_0[i32(0)].xyz, model_matrix_0[i32(1)].xyz, model_matrix_0[i32(2)].xyz);
    var _S19 : mat3x3<f32> = inverse_mat3_0(_S18);
    var normal_matrix_0 : mat3x3<f32> = transpose(_S19);
    var tangent_0 : vec3<f32> = normalize((((_S17.in_vertex_tangent_0) * (normal_matrix_0))));
    var bitangent_0 : vec3<f32> = normalize((((_S17.in_vertex_bitangent_0) * (normal_matrix_0))));
    var normal_0 : vec3<f32> = normalize((((_S17.in_vertex_normal_0) * (normal_matrix_0))));
    var TBN_matrix_0 : mat3x3<f32> = transpose(mat3x3<f32>(tangent_0, bitangent_0, normal_0));
    var model_space_0 : vec4<f32> = (((vec4<f32>(_S17.in_vertex_position_0, 1.0f)) * (model_matrix_0)));
    var o_0 : VS_OUT_0;
    o_0.TBN_tangent_0 = TBN_matrix_0[i32(0)];
    o_0.TBN_bitangent_0 = TBN_matrix_0[i32(1)];
    o_0.TBN_normal_0 = TBN_matrix_0[i32(2)];
    var _S20 : vec3<f32> = model_space_0.xyz;
    var _S21 : vec3<f32> = (((_S20) * (TBN_matrix_0)));
    o_0.vertex_position_0 = _S21;
    o_0.world_position_0 = _S20;
    o_0.vertex_texture_coords_0 = _S17.in_vertex_texture_coords_0;
    var _S22 : vec4<f32> = (((model_space_0) * (mat4x4<f32>(camera_0.camera_view_projection_0.data_0[i32(0)][i32(0)], camera_0.camera_view_projection_0.data_0[i32(1)][i32(0)], camera_0.camera_view_projection_0.data_0[i32(2)][i32(0)], camera_0.camera_view_projection_0.data_0[i32(3)][i32(0)], camera_0.camera_view_projection_0.data_0[i32(0)][i32(1)], camera_0.camera_view_projection_0.data_0[i32(1)][i32(1)], camera_0.camera_view_projection_0.data_0[i32(2)][i32(1)], camera_0.camera_view_projection_0.data_0[i32(3)][i32(1)], camera_0.camera_view_projection_0.data_0[i32(0)][i32(2)], camera_0.camera_view_projection_0.data_0[i32(1)][i32(2)], camera_0.camera_view_projection_0.data_0[i32(2)][i32(2)], camera_0.camera_view_projection_0.data_0[i32(3)][i32(2)], camera_0.camera_view_projection_0.data_0[i32(0)][i32(3)], camera_0.camera_view_projection_0.data_0[i32(1)][i32(3)], camera_0.camera_view_projection_0.data_0[i32(2)][i32(3)], camera_0.camera_view_projection_0.data_0[i32(3)][i32(3)]))));
    o_0.sv_position_0 = _S22;
    return o_0;
}

