#version 450

// Input vertex data
layout(location=0) in vec3 vertex_position;
layout(location=1) in vec2 vertex_texture_coordinates;
layout(location=2) in vec3 vertex_normal;
//layout(location=3) in vec3 vertex_tangent;
//layout(location=4) in vec3 vertex_bitangent;

// Input model data
layout(location=5) in vec4 model_matrix_0;
layout(location=6) in vec4 model_matrix_1;
layout(location=7) in vec4 model_matrix_2;
layout(location=8) in vec4 model_matrix_3;

// Input camera data
layout(set=2, binding=0) uniform Camera {
    mat4 position; 
    mat4 view_projection;
};

// Output data
layout(location=0) out vec3 out_vertex_position;
layout(location=1) out vec2 out_vertex_texture_coordinates;
layout(location=2) out vec3 out_vertex_normal;
//layout(location=3) out vec3 out_vertex_light_position;
//layout(location=4) out vec3 out_vertex_view_position;


// NEW!
// layout(set=2, binding=0) uniform Light {
//     vec3 light_position;
//     vec3 light_color;
// };

void main() {
    mat4 model_matrix = mat4(
        model_matrix_0,
        model_matrix_1,
        model_matrix_2,
        model_matrix_3
    );

    vec4 model_space = model_matrix * vec4(vertex_position, 1.0);
    out_vertex_position = model_space.xyz;

    mat3 normal_matrix = mat3(transpose(inverse(model_matrix)));
    out_vertex_normal =  normal_matrix * vertex_normal;

    out_vertex_texture_coordinates = vertex_texture_coordinates;

    gl_Position = view_projection * model_space;

    // mat3 normal_matrix = mat3(transpose(inverse(model_matrix)));
    // vec3 normal = normalize(normal_matrix * a_normal);
    // vec3 tangent = normalize(normal_matrix * a_tangent);
    // vec3 bitangent = normalize(normal_matrix * a_bitangent);

    // // UDPATED!
    // mat3 tangent_matrix = transpose(mat3(
    //     tangent,
    //     bitangent,
    //     normal
    // ));

    // vec4 model_space = model_matrix * vec4(a_position, 1.0);
    // v_position = model_space.xyz;

    // // NEW!
    // v_position = tangent_matrix * model_space.xyz;
    // v_light_position = tangent_matrix * light_position;
    // v_view_position = tangent_matrix * u_view_position;

    // gl_Position = u_view_proj * model_space;
}