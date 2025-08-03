#version 450

layout(location = 0) in vec2 tex_coords;

layout(set = 0, binding = 0) uniform texture2D scene_texture;
layout(set = 0, binding = 1) uniform sampler scene_sampler;

layout(set = 1, binding = 0) uniform PostprocessParams {
    float vignette_strength;
    float vignette_extent;
    vec2 screen_resolution;
    float tilt_shift_focus_area;
    float tilt_shift_focus_pos;
    float tilt_shift_blur_amount;
    float abberration_strength;
};

layout(location = 0) out vec4 out_color;

float bayer_dither(vec2 frag_coord) {
    int x = int(mod(frag_coord.x, 4.0));
    int y = int(mod(frag_coord.y, 4.0));
    int index = x + y * 4;

    float ditherMatrix[16] = float[16](
         0.0,  8.0,  2.0, 10.0,
        12.0,  4.0, 14.0,  6.0,
         3.0, 11.0,  1.0,  9.0,
        15.0,  7.0, 13.0,  5.0
    );
    return ditherMatrix[index] / 16.0;
}

vec3 cartoon_effect(vec2 tex_coords) {
    vec3 color = texture(sampler2D(scene_texture, scene_sampler), tex_coords).rgb;
    float dx = 1.0 / screen_resolution.x;
    float dy = 1.0 / screen_resolution.y;

    float lum_dx = 0.0;
    float lum_dy = 0.0;

    for (int i = -1; i <= 1; ++i) {
        for (int j = -1; j <= 1; ++j) {
            vec2 offset = vec2(float(i) * dx, float(j) * dy);
            vec3 sample_col = texture(sampler2D(scene_texture, scene_sampler), tex_coords + offset).rgb;
            float lum = dot(sample_col, vec3(0.299, 0.587, 0.114));
            lum_dx += lum * float(i);
            lum_dy += lum * float(j);
        }
    }

    float edge_strength = length(vec2(lum_dx, lum_dy));
    float edge_mask = smoothstep(0.2, 0.25, edge_strength);

    float levels = 18.0;
    vec3 quant_color = floor(color * levels) / levels;
    return mix(quant_color, vec3(0.0), edge_mask);
}

vec3 tilt_shift_blur(vec2 tex_coords) {
    float focus_top = tilt_shift_focus_pos - tilt_shift_focus_area * 0.5;
    float focus_bottom = tilt_shift_focus_pos + tilt_shift_focus_area * 0.5;

    float distance_from_focus = 0.0;
    if (tex_coords.y < focus_top) {
        distance_from_focus = focus_top - tex_coords.y;
    } else if (tex_coords.y > focus_bottom) {
        distance_from_focus = tex_coords.y - focus_bottom;
    }

    float blur_strength = smoothstep(0.0, 0.3, distance_from_focus) * tilt_shift_blur_amount;

    vec3 blurred_color = vec3(0.0);
    float total_weight = 0.0;

    int blur_samples = int(blur_strength * 10.0) + 1;
    float blur_radius = blur_strength * 0.01;

    for (int x = -blur_samples; x <= blur_samples; x++) {
        for (int y = -blur_samples; y <= blur_samples; y++) {
            vec2 offset = vec2(float(x), float(y)) * blur_radius;
            vec2 sample_coords = clamp(tex_coords + offset, vec2(0.0), vec2(1.0));
            blurred_color += texture(sampler2D(scene_texture, scene_sampler), sample_coords).rgb;
            total_weight += 1.0;
        }
    }

    return blurred_color / total_weight;
}

vec3 processed_color(vec2 uv) {
    vec3 blur = tilt_shift_blur(uv);
    vec3 cartoon = cartoon_effect(uv);
    return mix(blur, cartoon, 0.5);
}

vec3 chromatic_aberration(vec2 tex_coords) {
    vec2 center = vec2(0.5, 0.5);
    vec2 uv = tex_coords - center;

    float aspect_ratio = screen_resolution.x / screen_resolution.y;
    uv.x *= aspect_ratio;

    float dist = length(uv);
    float strength = smoothstep(0.4, 0.9, dist);
    float offset = strength * abberration_strength * 1.103;

    vec2 r_uv = clamp(tex_coords + uv * offset, vec2(0.0), vec2(1.0));
    vec2 g_uv = tex_coords;
    vec2 b_uv = clamp(tex_coords - uv * offset, vec2(0.0), vec2(1.0));

    float r = processed_color(r_uv).r;
    float g = processed_color(g_uv).g;
    float b = processed_color(b_uv).b;

    return vec3(r, g, b);
}

vec3 apply_horizontal_dark_lines(vec3 color, vec2 frag_coord, float threshold, float spacing) {
    float lum = dot(color, vec3(0.299, 0.587, 0.114)); // Grayscale brightness

    // Make lines thinner by using a narrow band for the line
    float line_width = 0.15; // Lower = thinner lines (in [0,1] of spacing)
    float f = fract(frag_coord.y / spacing);
    float line = smoothstep(0.0, line_width, f) * (1.0 - smoothstep(1.0 - line_width, 1.0, f));

    float mask = step(threshold, lum) * line;

    // Blend with white, and reduce the effect (e.g., 0.3 strength)
    return mix(color, vec3(1.0), mask * 0.9);
}

void main() {
    vec2 frag_coord = tex_coords * screen_resolution;

    // Final scene color with all effects
    vec3 scene_color = chromatic_aberration(tex_coords);

    // Vignette
    vec2 center = vec2(0.5, 0.5);
    vec2 uv = tex_coords - center;
    uv.x *= screen_resolution.x / screen_resolution.y;
    float distance = length(uv);
    float vignette = 1.0 - smoothstep(vignette_extent, vignette_extent + 0.3, distance);
    vignette = mix(1.0 - vignette_strength, 1.0, vignette);
    vec3 final_color = clamp(scene_color * vignette * 1.1, 0.0, 1.0);

    // Apply dithering per channel
    float d_r = bayer_dither(frag_coord + vec2(0.0, 0.0));
    float d_g = bayer_dither(frag_coord + vec2(1.0, 1.0));
    float d_b = bayer_dither(frag_coord + vec2(2.0, 2.0));
    float levels = 4.0;

    final_color.r = floor(final_color.r * levels + d_r) / levels;
    final_color.g = floor(final_color.g * levels + d_g) / levels;
    final_color.b = floor(final_color.b * levels + d_b) / levels;

    final_color = apply_horizontal_dark_lines(final_color, frag_coord, 0.4, 10.0);

    out_color = vec4(final_color, 1.0);
}
