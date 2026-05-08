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
    vec3  fog_color;
    float fog_density;
};

// Input camera parameters
layout(set=1, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

// Input material parameters
layout(set=2, binding=0) uniform material {
    vec3 tint;
};

// Input material textures
layout(set=3, binding=0) uniform texture2D diffuse_texture;
layout(set=3, binding=1) uniform sampler diffuse_sampler;

// Output data
layout(location=0) out vec4 out_final_color;

void main() {

    // Texture
    vec4 object_color = texture(sampler2D(diffuse_texture, diffuse_sampler), in_vertex_texture_coordinates);

    // Final color
    vec3 final_color = object_color.xyz * tint;

    // Exponential-squared depth fog. density = 0 → no fog, bit-identical output.
    float fog_dist = length(camera_position - in_world_position);
    float fog_factor = clamp(1.0 - exp(-fog_density * fog_density * fog_dist * fog_dist), 0.0, 1.0);
    final_color = mix(final_color, fog_color, fog_factor);

    out_final_color = vec4(final_color, 1.0);
}