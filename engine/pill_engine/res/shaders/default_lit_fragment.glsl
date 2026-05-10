#version 450

// Input vertex data
layout(location=0) in vec3 in_vertex_position;
layout(location=1) in vec2 in_vertex_texture_coordinates;
layout(location=2) in vec3 in_TBN_tangent;
layout(location=3) in vec3 in_TBN_bitangent;
layout(location=4) in vec3 in_TBN_normal;
layout(location=5) in vec3 in_world_position;

// Input engine parameters
layout(set=0, binding=0) uniform engine {
    float delta_time;
    float fog_density;
    vec3  fog_color;
};

// Input camera parameters
layout(set=1, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

// Input material parameters
layout(set=2, binding=0) uniform material {
    vec3 tint;
    float specularity;
};

// Input material textures
layout(set=3, binding=0) uniform texture2D diffuse_texture;
layout(set=3, binding=1) uniform sampler diffuse_sampler;
layout(set=3, binding=2) uniform texture2D normal_texture;
layout(set=3, binding=3) uniform sampler normal_sampler;

// Output data
layout(location=0) out vec4 out_final_color;

void main() {

    // Settings
    float ambient_light_strength = 0.02;
    vec3 light_position = vec3(-10.0, 10.0, -10.0);
    vec3 light_color = vec3(1.0, 1.0, 1.0);

    // Texture
    vec4 object_color = texture(sampler2D(diffuse_texture, diffuse_sampler), in_vertex_texture_coordinates);
    vec4 object_normal = texture(sampler2D(normal_texture, normal_sampler), in_vertex_texture_coordinates);

    // Reconstruct TBN matrix from individual components
    mat3 TBN_matrix = mat3(in_TBN_tangent, in_TBN_bitangent, in_TBN_normal);

    // Ambient lighting
    vec3 ambient_light_factor = light_color * ambient_light_strength;

    // Diffuse lighting
    vec3 normal = normalize(object_normal.rgb * 2.0 - 1.0); // Transform normal vector to (-1,1) range 
    vec3 light_direction = normalize(TBN_matrix * light_position - in_vertex_position);

    float diffuse_light_strength = max(dot(normal, light_direction), 0.0);
    vec3 diffuse_light_factor = light_color * diffuse_light_strength;

    // Specular lighting
    vec3 view_direction = normalize(TBN_matrix * camera_position - in_vertex_position);
    vec3 half_direction = normalize(view_direction + light_direction);
    float specular_light_strength = pow(max(dot(normal, half_direction), 0.0), 32) * specularity;
    vec3 specular_light_factor = light_color * specular_light_strength;

    // Final color
    vec3 final_color = (ambient_light_factor + diffuse_light_factor + specular_light_factor) * object_color.xyz * tint;

    // Exponential-squared depth fog. density = 0 → no fog, bit-identical output.
    float fog_distance = length(camera_position - in_world_position);
    float fog_factor = clamp(1.0 - exp(-fog_density * fog_density * fog_distance * fog_distance), 0.0, 1.0);
    final_color = mix(final_color, fog_color, fog_factor);

    out_final_color = vec4(final_color, 1.0);
}