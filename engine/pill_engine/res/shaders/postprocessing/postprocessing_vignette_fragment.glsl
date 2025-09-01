#version 450

layout(location=0) in vec2 in_texture_coordinates;

layout(set = 0, binding = 0) uniform texture2D scene_texture;
layout(set = 0, binding = 1) uniform sampler scene_sampler;

layout(set = 1, binding = 0) uniform material {
    vec2  screen_resolution; // xy = pixels
    float strength; // 0..1 (1 = strongest darkening at edges)
    float extent;   // 0..1 radius where falloff starts (from center)
    float falloff_width;     // 0..1 how wide the falloff is
} material;

layout(location = 0) out vec4 out_color;

void main() {
    vec3 scene_color = texture(sampler2D(scene_texture, scene_sampler), in_texture_coordinates).rgb;

    // Vignette (aspect-corrected)
    vec2 center = vec2(0.5);
    vec2 uv = in_texture_coordinates - center;
    uv.x *= material.screen_resolution.x / material.screen_resolution.y;

    float distance_from_center = length(uv);
    float mask = 1.0 - smoothstep(material.extent, material.extent + material.falloff_width, distance_from_center);

    // Map mask to [1 - strength, 1]
    float vignette = mix(1.0 - material.strength, 1.0, mask);

    vec3 final_color = clamp(scene_color * vignette, 0.0, 1.0);
    out_color = vec4(final_color, 1.0);
}
