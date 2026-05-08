#version 450

// Input vertex data
layout(location=0) in vec3 in_vertex_position;
layout(location=1) in vec2 in_vertex_texture_coordinates;
layout(location=2) in vec3 in_vertex_normal;
layout(location=3) in vec3 in_vertex_tangent;
layout(location=4) in vec3 in_vertex_bitangent;

// Input model data (instance data)
layout(location=5) in vec3 transform_position;
layout(location=6) in vec3 transform_rotation;
layout(location=7) in vec3 transform_scale;

// Input camera parameters
layout(set=1, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

// Output data
layout(location=0) out vec3 out_vertex_position;
layout(location=1) out vec2 out_vertex_texture_coordinates;
layout(location=2) out vec3 out_TBN_tangent;
layout(location=3) out vec3 out_TBN_bitangent;
layout(location=4) out vec3 out_TBN_normal;
layout(location=5) out vec3 out_world_position;

mat3 inverse_mat3(mat3 m) {
    float a00 = m[0][0], a01 = m[0][1], a02 = m[0][2];
    float a10 = m[1][0], a11 = m[1][1], a12 = m[1][2];
    float a20 = m[2][0], a21 = m[2][1], a22 = m[2][2];

    float det = a00 * (a11 * a22 - a12 * a21)
              - a01 * (a10 * a22 - a12 * a20)
              + a02 * (a10 * a21 - a11 * a20);

    if (abs(det) < 1e-6) {
        return mat3(1.0); // fallback to identity if non-invertible
    }

    float invDet = 1.0 / det;

    mat3 inv;
    inv[0][0] =  (a11 * a22 - a12 * a21) * invDet;
    inv[0][1] = -(a01 * a22 - a02 * a21) * invDet;
    inv[0][2] =  (a01 * a12 - a02 * a11) * invDet;

    inv[1][0] = -(a10 * a22 - a12 * a20) * invDet;
    inv[1][1] =  (a00 * a22 - a02 * a20) * invDet;
    inv[1][2] = -(a00 * a12 - a02 * a10) * invDet;

    inv[2][0] =  (a10 * a21 - a11 * a20) * invDet;
    inv[2][1] = -(a00 * a21 - a01 * a20) * invDet;
    inv[2][2] =  (a00 * a11 - a01 * a10) * invDet;

    return inv;
}

mat4 compute_model_matrix(vec3 position, vec3 rotation, vec3 scale) {
    // --- Scale matrix ---
    mat4 scale_matrix = mat4(
        vec4(scale.x, 0.0,     0.0,     0.0),
        vec4(0.0,     scale.y, 0.0,     0.0),
        vec4(0.0,     0.0,     scale.z, 0.0),
        vec4(0.0,     0.0,     0.0,     1.0)
    );

    // --- Rotation matrices ---
    float cx = cos(rotation.x), sx = sin(rotation.x);
    float cy = cos(rotation.y), sy = sin(rotation.y);
    float cz = cos(rotation.z), sz = sin(rotation.z);

    mat4 rot_x = mat4(
        vec4(1, 0,  0, 0),
        vec4(0, cx, -sx, 0),
        vec4(0, sx, cx, 0),
        vec4(0, 0,  0, 1)
    );

    mat4 rot_y = mat4(
        vec4(cy, 0, sy, 0),
        vec4(0,  1, 0,  0),
        vec4(-sy,0, cy, 0),
        vec4(0,  0, 0,  1)
    );

    mat4 rot_z = mat4(
        vec4(cz, -sz, 0, 0),
        vec4(sz, cz,  0, 0),
        vec4(0,  0,   1, 0),
        vec4(0,  0,   0, 1)
    );

    // Final rotation matrix: Rz * Ry * Rx (YXZ order is common too)
    mat4 rotation_matrix = rot_z * rot_y * rot_x;

    // --- Translation matrix ---
    mat4 translation_matrix = mat4(
        vec4(1, 0, 0, 0),
        vec4(0, 1, 0, 0),
        vec4(0, 0, 1, 0),
        vec4(position, 1.0)
    );

    // Final model matrix
    return translation_matrix * rotation_matrix * scale_matrix;
}

void main() {
    mat4 model_matrix = compute_model_matrix(transform_position, transform_rotation, transform_scale);
   
    // Extract 3x3 upper-left portion of model matrix
    mat3 model3x3 = mat3(model_matrix);

    // Compute inverse transpose for normal matrix
    mat3 normal_matrix = transpose(inverse_mat3(model3x3));
    

    // Create tangent matrix
    vec3 tangent = normalize(normal_matrix * in_vertex_tangent);
    vec3 bitangent = normalize(normal_matrix * in_vertex_bitangent);
    vec3 normal = normalize(normal_matrix * in_vertex_normal);
    mat3 TBN_matrix = transpose(mat3(tangent, bitangent, normal));
    out_TBN_tangent = TBN_matrix[0];  // First row (Tangent)
    out_TBN_bitangent = TBN_matrix[1]; // Second row (Bitangent)
    out_TBN_normal = TBN_matrix[2];   // Third row (Normal)

    // Calculate vertex position in model space
    vec4 model_space = model_matrix * vec4(in_vertex_position, 1.0);
    out_vertex_position = TBN_matrix * model_space.xyz;
    out_world_position = model_space.xyz;

    // Just forward texture coordinates
    out_vertex_texture_coordinates = in_vertex_texture_coordinates;

    gl_Position = camera_view_projection * model_space;
}