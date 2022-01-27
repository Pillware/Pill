#version 450

// Input vertex data
layout(location=0) in vec3 vertex_position;
layout(location=1) in vec2 vertex_texture_coordinates;
layout(location=2) in mat3 TBN_matrix;

// Input material data
layout(set = 0, binding = 0) uniform texture2D diffuse_texture;
layout(set = 0, binding = 1) uniform sampler diffuse_sampler;
layout(set = 0, binding = 2) uniform texture2D normal_texture;
layout(set = 0, binding = 3) uniform sampler normal_sampler;
layout(set = 1, binding = 0) uniform properties {
    vec3 tint;
    float specularity;
};

// Output data
layout(location=0) out vec4 out_final_color;

// Input camera data
layout(set=2, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

void main() {

    // Settings
    float ambient_light_strength = 0.1;
    vec3 light_position = vec3(10.0, 10.0, 10.0);
    vec3 light_color = vec3(1.0, 1.0, 1.0);

    // Texture
    vec4 object_color = texture(sampler2D(diffuse_texture, diffuse_sampler), vertex_texture_coordinates);
    vec4 object_normal = texture(sampler2D(normal_texture, normal_sampler), vertex_texture_coordinates);

    // Ambient lighting
    vec3 ambient_light_factor = light_color * ambient_light_strength;

    // Diffuse lighting
    vec3 normal = normalize(object_normal.rgb * 2.0 - 1.0); // Transform normal vector to (-1,1) range 
    vec3 light_direction = normalize(TBN_matrix * light_position - vertex_position);

    float diffuse_light_strength = max(dot(normal, light_direction), 0.0);
    vec3 diffuse_light_factor = light_color * diffuse_light_strength;

    // Specular lighting
    vec3 view_direction = normalize(TBN_matrix * camera_position - vertex_position);
    vec3 half_direction = normalize(view_direction + light_direction);
    float specular_light_strength = pow(max(dot(normal, half_direction), 0.0), 32) * specularity;
    vec3 specular_light_factor = light_color * specular_light_strength;

    // Final color
    vec3 final_color = (ambient_light_factor + diffuse_light_factor + specular_light_factor) * object_color.xyz * tint;
    out_final_color = vec4(final_color, 1.0); 
}