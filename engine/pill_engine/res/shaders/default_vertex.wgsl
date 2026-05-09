struct camera {
    camera_position: vec3<f32>,
    camera_view_projection: mat4x4<f32>,
}

struct VertexOutput {
    @location(0) out_vertex_position: vec3<f32>,
    @location(1) out_vertex_texture_coordinates: vec2<f32>,
    @location(2) out_TBN_tangent: vec3<f32>,
    @location(3) out_TBN_bitangent: vec3<f32>,
    @location(4) out_TBN_normal: vec3<f32>,
    @location(5) out_world_position: vec3<f32>,
    @builtin(position) member: vec4<f32>,
}

var<private> in_vertex_position_1: vec3<f32>;
var<private> in_vertex_texture_coordinates_1: vec2<f32>;
var<private> in_vertex_normal_1: vec3<f32>;
var<private> in_vertex_tangent_1: vec3<f32>;
var<private> in_vertex_bitangent_1: vec3<f32>;
var<private> transform_position_1: vec3<f32>;
var<private> transform_rotation_1: vec3<f32>;
var<private> transform_scale_1: vec3<f32>;
@group(1) @binding(0) 
var<uniform> global: camera;
var<private> out_vertex_position: vec3<f32>;
var<private> out_vertex_texture_coordinates: vec2<f32>;
var<private> out_TBN_tangent: vec3<f32>;
var<private> out_TBN_bitangent: vec3<f32>;
var<private> out_TBN_normal: vec3<f32>;
var<private> out_world_position: vec3<f32>;
var<private> gl_Position: vec4<f32>;

fn inverse_mat3_(m: mat3x3<f32>) -> mat3x3<f32> {
    var m_1: mat3x3<f32>;
    var a00_: f32;
    var a01_: f32;
    var a02_: f32;
    var a10_: f32;
    var a11_: f32;
    var a12_: f32;
    var a20_: f32;
    var a21_: f32;
    var a22_: f32;
    var det: f32;
    var invDet: f32;
    var inv: mat3x3<f32>;

    m_1 = m;
    let _e24 = m_1[0][0];
    a00_ = _e24;
    let _e30 = m_1[0][1];
    a01_ = _e30;
    let _e36 = m_1[0][2];
    a02_ = _e36;
    let _e42 = m_1[1][0];
    a10_ = _e42;
    let _e48 = m_1[1][1];
    a11_ = _e48;
    let _e54 = m_1[1][2];
    a12_ = _e54;
    let _e60 = m_1[2][0];
    a20_ = _e60;
    let _e66 = m_1[2][1];
    a21_ = _e66;
    let _e72 = m_1[2][2];
    a22_ = _e72;
    let _e74 = a00_;
    let _e75 = a11_;
    let _e76 = a22_;
    let _e78 = a12_;
    let _e79 = a21_;
    let _e83 = a01_;
    let _e84 = a10_;
    let _e85 = a22_;
    let _e87 = a12_;
    let _e88 = a20_;
    let _e93 = a02_;
    let _e94 = a10_;
    let _e95 = a21_;
    let _e97 = a11_;
    let _e98 = a20_;
    det = (((_e74 * ((_e75 * _e76) - (_e78 * _e79))) - (_e83 * ((_e84 * _e85) - (_e87 * _e88)))) + (_e93 * ((_e94 * _e95) - (_e97 * _e98))));
    let _e105 = det;
    if (abs(_e105) < 0.000001f) {
        {
            return mat3x3<f32>(vec3<f32>(1f, 0f, 0f), vec3<f32>(0f, 1f, 0f), vec3<f32>(0f, 0f, 1f));
        }
    }
    let _e116 = det;
    invDet = (1f / _e116);
    let _e124 = a11_;
    let _e125 = a22_;
    let _e127 = a12_;
    let _e128 = a21_;
    let _e131 = invDet;
    inv[0i][0i] = (((_e124 * _e125) - (_e127 * _e128)) * _e131);
    let _e137 = a01_;
    let _e138 = a22_;
    let _e140 = a02_;
    let _e141 = a21_;
    let _e145 = invDet;
    inv[0i][1i] = (-(((_e137 * _e138) - (_e140 * _e141))) * _e145);
    let _e151 = a01_;
    let _e152 = a12_;
    let _e154 = a02_;
    let _e155 = a11_;
    let _e158 = invDet;
    inv[0i][2i] = (((_e151 * _e152) - (_e154 * _e155)) * _e158);
    let _e164 = a10_;
    let _e165 = a22_;
    let _e167 = a12_;
    let _e168 = a20_;
    let _e172 = invDet;
    inv[1i][0i] = (-(((_e164 * _e165) - (_e167 * _e168))) * _e172);
    let _e178 = a00_;
    let _e179 = a22_;
    let _e181 = a02_;
    let _e182 = a20_;
    let _e185 = invDet;
    inv[1i][1i] = (((_e178 * _e179) - (_e181 * _e182)) * _e185);
    let _e191 = a00_;
    let _e192 = a12_;
    let _e194 = a02_;
    let _e195 = a10_;
    let _e199 = invDet;
    inv[1i][2i] = (-(((_e191 * _e192) - (_e194 * _e195))) * _e199);
    let _e205 = a10_;
    let _e206 = a21_;
    let _e208 = a11_;
    let _e209 = a20_;
    let _e212 = invDet;
    inv[2i][0i] = (((_e205 * _e206) - (_e208 * _e209)) * _e212);
    let _e218 = a00_;
    let _e219 = a21_;
    let _e221 = a01_;
    let _e222 = a20_;
    let _e226 = invDet;
    inv[2i][1i] = (-(((_e218 * _e219) - (_e221 * _e222))) * _e226);
    let _e232 = a00_;
    let _e233 = a11_;
    let _e235 = a01_;
    let _e236 = a10_;
    let _e239 = invDet;
    inv[2i][2i] = (((_e232 * _e233) - (_e235 * _e236)) * _e239);
    let _e241 = inv;
    return _e241;
}

fn compute_model_matrix(position: vec3<f32>, rotation: vec3<f32>, scale: vec3<f32>) -> mat4x4<f32> {
    var position_1: vec3<f32>;
    var rotation_1: vec3<f32>;
    var scale_1: vec3<f32>;
    var scale_matrix: mat4x4<f32>;
    var cx: f32;
    var sx: f32;
    var cy: f32;
    var sy: f32;
    var cz: f32;
    var sz: f32;
    var rot_x: mat4x4<f32>;
    var rot_y: mat4x4<f32>;
    var rot_z: mat4x4<f32>;
    var rotation_matrix: mat4x4<f32>;
    var translation_matrix: mat4x4<f32>;

    position_1 = position;
    rotation_1 = rotation;
    scale_1 = scale;
    let _e24 = scale_1;
    let _e29 = vec4<f32>(_e24.x, 0f, 0f, 0f);
    let _e31 = scale_1;
    let _e35 = vec4<f32>(0f, _e31.y, 0f, 0f);
    let _e38 = scale_1;
    let _e41 = vec4<f32>(0f, 0f, _e38.z, 0f);
    scale_matrix = mat4x4<f32>(vec4<f32>(_e29.x, _e29.y, _e29.z, _e29.w), vec4<f32>(_e35.x, _e35.y, _e35.z, _e35.w), vec4<f32>(_e41.x, _e41.y, _e41.z, _e41.w), vec4<f32>(0f, 0f, 0f, 1f));
    let _e65 = rotation_1;
    let _e67 = rotation_1;
    cx = cos(_e67.x);
    let _e71 = rotation_1;
    let _e73 = rotation_1;
    sx = sin(_e73.x);
    let _e77 = rotation_1;
    let _e79 = rotation_1;
    cy = cos(_e79.y);
    let _e83 = rotation_1;
    let _e85 = rotation_1;
    sy = sin(_e85.y);
    let _e89 = rotation_1;
    let _e91 = rotation_1;
    cz = cos(_e91.z);
    let _e95 = rotation_1;
    let _e97 = rotation_1;
    sz = sin(_e97.z);
    let _e111 = cx;
    let _e112 = sx;
    let _e117 = vec4<f32>(0f, _e111, -(_e112), 0f);
    let _e119 = sx;
    let _e120 = cx;
    let _e124 = vec4<f32>(0f, _e119, _e120, 0f);
    rot_x = mat4x4<f32>(vec4<f32>(1f, 0f, 0f, 0f), vec4<f32>(_e117.x, _e117.y, _e117.z, _e117.w), vec4<f32>(_e124.x, _e124.y, _e124.z, _e124.w), vec4<f32>(0f, 0f, 0f, 1f));
    let _e148 = cy;
    let _e150 = sy;
    let _e154 = vec4<f32>(_e148, 0f, _e150, 0f);
    let _e164 = sy;
    let _e167 = cy;
    let _e171 = vec4<f32>(-(_e164), 0f, _e167, 0f);
    rot_y = mat4x4<f32>(vec4<f32>(_e154.x, _e154.y, _e154.z, _e154.w), vec4<f32>(0f, 1f, 0f, 0f), vec4<f32>(_e171.x, _e171.y, _e171.z, _e171.w), vec4<f32>(0f, 0f, 0f, 1f));
    let _e195 = cz;
    let _e196 = sz;
    let _e202 = vec4<f32>(_e195, -(_e196), 0f, 0f);
    let _e203 = sz;
    let _e204 = cz;
    let _e209 = vec4<f32>(_e203, _e204, 0f, 0f);
    rot_z = mat4x4<f32>(vec4<f32>(_e202.x, _e202.y, _e202.z, _e202.w), vec4<f32>(_e209.x, _e209.y, _e209.z, _e209.w), vec4<f32>(0f, 0f, 1f, 0f), vec4<f32>(0f, 0f, 0f, 1f));
    let _e242 = rot_z;
    let _e243 = rot_y;
    let _e245 = rot_x;
    rotation_matrix = ((_e242 * _e243) * _e245);
    let _e275 = position_1;
    let _e280 = vec4<f32>(_e275.x, _e275.y, _e275.z, 1f);
    translation_matrix = mat4x4<f32>(vec4<f32>(1f, 0f, 0f, 0f), vec4<f32>(0f, 1f, 0f, 0f), vec4<f32>(0f, 0f, 1f, 0f), vec4<f32>(_e280.x, _e280.y, _e280.z, _e280.w));
    let _e291 = translation_matrix;
    let _e292 = rotation_matrix;
    let _e294 = scale_matrix;
    return ((_e291 * _e292) * _e294);
}

fn main_1() {
    var model_matrix: mat4x4<f32>;
    var model3x3_: mat3x3<f32>;
    var normal_matrix: mat3x3<f32>;
    var tangent: vec3<f32>;
    var bitangent: vec3<f32>;
    var normal: vec3<f32>;
    var TBN_matrix: mat3x3<f32>;
    var model_space: vec4<f32>;

    let _e21 = transform_position_1;
    let _e22 = transform_rotation_1;
    let _e23 = transform_scale_1;
    let _e24 = compute_model_matrix(_e21, _e22, _e23);
    model_matrix = _e24;
    let _e26 = model_matrix;
    model3x3_ = mat3x3<f32>(_e26[0].xyz, _e26[1].xyz, _e26[2].xyz);
    let _e38 = model3x3_;
    let _e39 = inverse_mat3_(_e38);
    let _e41 = model3x3_;
    let _e42 = inverse_mat3_(_e41);
    normal_matrix = transpose(_e42);
    let _e45 = normal_matrix;
    let _e46 = in_vertex_tangent_1;
    let _e48 = normal_matrix;
    let _e49 = in_vertex_tangent_1;
    tangent = normalize((_e48 * _e49));
    let _e53 = normal_matrix;
    let _e54 = in_vertex_bitangent_1;
    let _e56 = normal_matrix;
    let _e57 = in_vertex_bitangent_1;
    bitangent = normalize((_e56 * _e57));
    let _e61 = normal_matrix;
    let _e62 = in_vertex_normal_1;
    let _e64 = normal_matrix;
    let _e65 = in_vertex_normal_1;
    normal = normalize((_e64 * _e65));
    let _e69 = tangent;
    let _e70 = bitangent;
    let _e71 = normal;
    let _e85 = tangent;
    let _e86 = bitangent;
    let _e87 = normal;
    TBN_matrix = transpose(mat3x3<f32>(vec3<f32>(_e85.x, _e85.y, _e85.z), vec3<f32>(_e86.x, _e86.y, _e86.z), vec3<f32>(_e87.x, _e87.y, _e87.z)));
    let _e105 = TBN_matrix[0];
    out_TBN_tangent = _e105;
    let _e108 = TBN_matrix[1];
    out_TBN_bitangent = _e108;
    let _e111 = TBN_matrix[2];
    out_TBN_normal = _e111;
    let _e112 = model_matrix;
    let _e113 = in_vertex_position_1;
    model_space = (_e112 * vec4<f32>(_e113.x, _e113.y, _e113.z, 1f));
    let _e121 = TBN_matrix;
    let _e122 = model_space;
    out_vertex_position = (_e121 * _e122.xyz);
    let _e125 = model_space;
    out_world_position = _e125.xyz;
    let _e127 = in_vertex_texture_coordinates_1;
    out_vertex_texture_coordinates = _e127;
    let _e129 = global.camera_view_projection;
    let _e130 = model_space;
    gl_Position = (_e129 * _e130);
    return;
}

@vertex 
fn main(@location(0) in_vertex_position: vec3<f32>, @location(1) in_vertex_texture_coordinates: vec2<f32>, @location(2) in_vertex_normal: vec3<f32>, @location(3) in_vertex_tangent: vec3<f32>, @location(4) in_vertex_bitangent: vec3<f32>, @location(5) transform_position: vec3<f32>, @location(6) transform_rotation: vec3<f32>, @location(7) transform_scale: vec3<f32>) -> VertexOutput {
    in_vertex_position_1 = in_vertex_position;
    in_vertex_texture_coordinates_1 = in_vertex_texture_coordinates;
    in_vertex_normal_1 = in_vertex_normal;
    in_vertex_tangent_1 = in_vertex_tangent;
    in_vertex_bitangent_1 = in_vertex_bitangent;
    transform_position_1 = transform_position;
    transform_rotation_1 = transform_rotation;
    transform_scale_1 = transform_scale;
    main_1();
    let _e49 = out_vertex_position;
    let _e51 = out_vertex_texture_coordinates;
    let _e53 = out_TBN_tangent;
    let _e55 = out_TBN_bitangent;
    let _e57 = out_TBN_normal;
    let _e59 = out_world_position;
    let _e61 = gl_Position;
    return VertexOutput(_e49, _e51, _e53, _e55, _e57, _e59, _e61);
}
