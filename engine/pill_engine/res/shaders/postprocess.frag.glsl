#version 450

// Input texture coordinates from vertex shader
layout(location = 0) in vec2 tex_coords;

// Input scene texture
layout(set = 0, binding = 0) uniform texture2D scene_texture;
layout(set = 0, binding = 1) uniform sampler scene_sampler;

// Postprocess parameters
layout(set = 1, binding = 0) uniform PostprocessParams {
    float vignette_strength;
    float vignette_extent;
    vec2 screen_resolution;
    float tilt_shift_focus_area;    // Height of the focus area (0.0 to 1.0)
    float tilt_shift_focus_pos;     // Vertical position of focus (0.0 to 1.0)
    float tilt_shift_blur_amount;   // Blur intensity
    float abberration_strength; // Strength of the aberration effect
   //  float _padding; 
};

// Output color
layout(location = 0) out vec4 out_color;


vec3 cartoon_effect(vec2 tex_coords) {
    // Sample center color
    vec3 color = texture(sampler2D(scene_texture, scene_sampler), tex_coords).rgb;
    
    // Edge detection kernel (Sobel-like)
    float edge_threshold = 0.2;
    float lum_center = dot(color, vec3(0.299, 0.587, 0.114)); // Luminance

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
    float edge = step(edge_threshold, edge_strength);

    // Color quantization (posterization)
    float levels = 4.0;
    vec3 quant_color = floor(color * levels) / levels;

    // Mix edge outline (black) with quantized color
    return mix(quant_color, vec3(0.0), edge);
}

// Sniper crosshair function
float sniper_crosshair(vec2 tex_coords, vec2 center) {
    vec2 crosshair_pos = tex_coords - center;
    float line_thickness = 0.002; // Thickness of the crosshair lines
    float line_length = 0.05;     // Length of each crosshair segment
    float gap_size = 0.02;        // Gap in the center of the crosshair

    // Horizontal line
    float horizontal_line = 0.0;
    if (abs(crosshair_pos.y) < line_thickness) {
        if (abs(crosshair_pos.x) > gap_size && abs(crosshair_pos.x) < (gap_size + line_length)) {
            horizontal_line = 1.0;
        }
    }

    // Vertical line  
    float vertical_line = 0.0;
    if (abs(crosshair_pos.x) < line_thickness) {
        if (abs(crosshair_pos.y) > gap_size && abs(crosshair_pos.y) < (gap_size + line_length)) {
            vertical_line = 1.0;
        }
    }

    // Combine crosshair lines
    return max(horizontal_line, vertical_line);
}


// Tilt-shift effect function
vec3 tilt_shift_blur(vec2 tex_coords) {
    // Calculate distance from focus area
    float focus_top = tilt_shift_focus_pos - tilt_shift_focus_area * 0.5;
    float focus_bottom = tilt_shift_focus_pos + tilt_shift_focus_area * 0.5;
    
    float distance_from_focus = 0.0;
    if (tex_coords.y < focus_top) {
        distance_from_focus = focus_top - tex_coords.y;
    } else if (tex_coords.y > focus_bottom) {
        distance_from_focus = tex_coords.y - focus_bottom;
    }
    
    // Calculate blur strength based on distance from focus
    float blur_strength = smoothstep(0.0, 0.3, distance_from_focus) * tilt_shift_blur_amount;
    
    // Sample texture with blur
    vec3 blurred_color = vec3(0.0);
    float total_weight = 0.0;
    
    // Simple box blur with variable kernel size
    int blur_samples = int(blur_strength * 10.0) + 1;
    float blur_radius = blur_strength * 0.01;
    
    for (int x = -blur_samples; x <= blur_samples; x++) {
        for (int y = -blur_samples; y <= blur_samples; y++) {
            vec2 offset = vec2(float(x), float(y)) * blur_radius;
            vec2 sample_coords = tex_coords + offset;
            
            // Clamp to texture boundaries
            sample_coords = clamp(sample_coords, vec2(0.0), vec2(1.0));
            
            float weight = 1.0;
            blurred_color += texture(sampler2D(scene_texture, scene_sampler), sample_coords).rgb * weight;
            total_weight += weight;
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
    float offset = 1 * 1.103 * strength * abberration_strength; // Adjusted for strength

    vec2 r_uv = clamp(tex_coords + uv * offset, vec2(0.0), vec2(1.0));
    vec2 g_uv = tex_coords;
    vec2 b_uv = clamp(tex_coords - uv * offset, vec2(0.0), vec2(1.0));

    float r = processed_color(r_uv).r;
    float g = processed_color(g_uv).g;
    float b = processed_color(b_uv).b;



    return vec3(r, g, b);
}




void main() {
    // Sample the scene texture
   // vec3 scene_color = texture(sampler2D(scene_texture, scene_sampler), tex_coords).rgb;
    // vec3 blurred = tilt_shift_blur(tex_coords);
    // vec3 scene_color = cartoon_effect(tex_coords); // uses original texture
    // vec3 scene_colorx = mix(blurred, scene_color, 0.5);  // blend both effects

   // vec3 aberrated_color = chromatic_aberration(tex_coords);



   vec3  scene_color = chromatic_aberration(tex_coords); //mix(aberrated_color, scene_colorx, 0.5);

    // Calculate vignette effect
    vec2 center = vec2(0.5, 0.5);
    vec2 uv = tex_coords - center;
    
    // Calculate distance from center, adjusted for aspect ratio
    float aspect_ratio = screen_resolution.x / screen_resolution.y;
    uv.x *= aspect_ratio;
    float distance = length(uv);
    
    // Create vignette falloff
    float vignette = 1.0 - smoothstep(vignette_extent, vignette_extent + 0.3, distance);
    vignette = mix(1.0 - vignette_strength, 1.0, vignette);
    
    // Apply vignette to scene color
    vec3 final_color = scene_color * vignette;
    
    // Add sniper scope crosshair
    float crosshair = sniper_crosshair(tex_coords, center);
    final_color = mix(final_color, vec3(1.0, 1.0, 1.0), crosshair * 0.8);
    
    out_color = vec4(final_color, 1.0);
}
