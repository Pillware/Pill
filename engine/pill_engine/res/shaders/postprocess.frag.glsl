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
     float _padding; 
};

// Output color
layout(location = 0) out vec4 out_color;

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

void main() {
    // Sample the scene texture
   // vec3 scene_color = texture(sampler2D(scene_texture, scene_sampler), tex_coords).rgb;
     vec3 scene_color = tilt_shift_blur(tex_coords);

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
