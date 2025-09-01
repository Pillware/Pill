#version 450

layout(location = 0) in vec2 in_texture_coordinates;

// Input engine parameters
layout(set=0, binding=0) uniform engine {
    float delta_time; 
};

// Input camera parameters
layout(set=1, binding=0) uniform camera {
    vec3 camera_position; 
    mat4 camera_view_projection;
};

// Input material parameters
layout(set = 2, binding = 0) uniform material {
    float exposure;         // EV stops, color *= 2^exposure
    vec3  tint;              // RGB multiplier
    float white_balance;    // -1..+1 (cool..warm)
    float hue;              // hue rotation in radians
    float saturation;       // 0=gray, 1=none, >1=more color
    float contrast;         // scale around 0.5, >0 (1 = none)
    float brightness;       // additive in display space, -1..+1 (0 = none)
    int   invert_flag;      // 1 = invert final color
    float gamma;            // >0, output = pow(color, 1/gamma), 1 = none
} color_adjust;

// Input material textures
layout(set=3, binding=0) uniform texture2D scene_texture;
layout(set=3, binding=1) uniform sampler scene_sampler;

// Output data
layout(location=0) out vec4 out_color;

// -------------------- Helpers --------------------

float safe_reciprocal(float value) {
    return 1.0 / max(value, 1e-6);
}

mat3 hue_rotation_matrix(float angle_radians) {
    float cosine_value = cos(angle_radians);
    float sine_value   = sin(angle_radians);
    // YIQ-inspired RGB hue rotation (good quality, branchless)
    return mat3(
        0.299 + 0.701*cosine_value + 0.168*sine_value,  0.587 - 0.587*cosine_value + 0.330*sine_value,  0.114 - 0.114*cosine_value - 0.497*sine_value,
        0.299 - 0.299*cosine_value - 0.328*sine_value,  0.587 + 0.413*cosine_value + 0.035*sine_value,  0.114 - 0.114*cosine_value + 0.292*sine_value,
        0.299 - 0.300*cosine_value + 1.250*sine_value,  0.587 - 0.588*cosine_value - 1.050*sine_value,  0.114 + 0.886*cosine_value - 0.203*sine_value
    );
}

vec3 apply_saturation(vec3 color_rgb, float saturation) {
    const vec3 luma_rec709 = vec3(0.2126, 0.7152, 0.0722);
    float luma = dot(color_rgb, luma_rec709);
    return mix(vec3(luma), color_rgb, saturation);
}

vec3 apply_contrast_and_brightness(vec3 color_rgb, float contrast, float brightness) {
    // Contrast about 0.5 keeps mid-gray stable in display space
    return (color_rgb - 0.5) * contrast + 0.5 + brightness;
}

vec3 apply_white_balance_simple(vec3 color_rgb, float white_balance) {
    // white_balance in [-1, 1]: <0 cooler, >0 warmer. Cheap RGB gain model.
    float warm_amount = max(white_balance, 0.0);
    float cool_amount = max(-white_balance, 0.0);
    vec3 gain_rgb = vec3(
        1.0 + 0.6*warm_amount - 0.1*cool_amount,  // R
        1.0 + 0.1*warm_amount - 0.1*cool_amount,  // G
        1.0 - 0.4*warm_amount + 0.6*cool_amount   // B
    );
    return color_rgb * gain_rgb;
}

// -------------------- Main --------------------

void main() {
    vec3 input_color_rgb = texture(sampler2D(scene_texture, scene_sampler), in_texture_coordinates).rgb;

    vec3 adjusted_color_rgb = input_color_rgb;

    // 1) Exposure (assumes linear input; if in sRGB, ensure hardware decoding or de-gamma first)
    adjusted_color_rgb *= exp2(color_adjust.exposure);

    // 2) Tint (apply early to maintain color relationships)
    adjusted_color_rgb *= color_adjust.tint;

    // 3) White balance (simple gain model)
    adjusted_color_rgb = apply_white_balance_simple(adjusted_color_rgb, color_adjust.white_balance);

    // 4) Hue rotation
    adjusted_color_rgb = hue_rotation_matrix(color_adjust.hue) * adjusted_color_rgb;

    // 5) Saturation
    adjusted_color_rgb = apply_saturation(adjusted_color_rgb, color_adjust.saturation);

    // 6) Contrast & Brightness
    adjusted_color_rgb = apply_contrast_and_brightness(adjusted_color_rgb, color_adjust.contrast, color_adjust.brightness);

    // 7) Invert (optional)
    if (color_adjust.invert_flag != 0) {
        adjusted_color_rgb = 1.0 - adjusted_color_rgb;
    }

    // 8) Output gamma
    float inverse_gamma = safe_reciprocal(max(color_adjust.gamma, 1e-6));
    adjusted_color_rgb = pow(max(adjusted_color_rgb, 0.0), vec3(inverse_gamma));

    out_color = vec4(clamp(adjusted_color_rgb, 0.0, 1.0), 1.0);
}
