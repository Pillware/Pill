#version 450

// Input vertex data
layout(location=0) in vec3 vertex_position;
layout(location=1) in vec2 vertex_texture_coordinates;
layout(location=2) in vec3 vertex_normal;
layout(location=3) in vec3 vertex_tangent;
layout(location=4) in vec3 vertex_bitangent;

// Input model data
layout(location=5) in vec4 model_matrix_0;
layout(location=6) in vec4 model_matrix_1;
layout(location=7) in vec4 model_matrix_2;
layout(location=8) in vec4 model_matrix_3;

// Input camera data
layout(set=2, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

// Output data
layout(location=0) out vec3 out_vertex_position;
layout(location=1) out vec2 out_vertex_texture_coordinates;
layout(location=2) out mat3 out_TBN_matrix;

void main() {
    mat4 model_matrix = mat4(
        model_matrix_0,
        model_matrix_1,
        model_matrix_2,
        model_matrix_3
    );

    // Create tangent matrix
    mat3 normal_matrix = mat3(transpose(inverse(model_matrix)));
    vec3 tangent = normalize(normal_matrix * vertex_tangent);
    vec3 bitangent = normalize(normal_matrix * vertex_bitangent);
    vec3 normal = normalize(normal_matrix * vertex_normal);
    mat3 TBN_matrix = transpose(mat3(tangent, bitangent, normal));
    out_TBN_matrix = TBN_matrix;

    // Calculate vertex position in model space
    vec4 model_space = model_matrix * vec4(vertex_position, 1.0);
    out_vertex_position = TBN_matrix * model_space.xyz;

    // Just forward texture coordinates
    out_vertex_texture_coordinates = vertex_texture_coordinates;

    gl_Position = camera_view_projection * model_space;
}