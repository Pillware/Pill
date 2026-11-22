// The selection outline, traced in screen space around the mask the
// outlined meshes drew.

// The outline's colour, and its width in pixels in `settings.x`.
struct Outline {
    color: vec4<f32>,
    settings: vec4<f32>,
};

@group(0) @binding(0) var outline_mask: texture_2d<f32>;
@group(0) @binding(1) var<uniform> outline: Outline;

// One triangle that covers the whole target.
@vertex
fn fullscreen_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The band within `settings.x` pixels of the mask, its outermost pixel faded
// by how much of it is covered, so the outline follows the silhouette
// smoothly instead of in steps.
@fragment
fn outline_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(outline_mask));
    let center = vec2<i32>(position.xy);
    if textureLoad(outline_mask, center, 0).r > 0.5 {
        discard;
    }
    let radius = i32(max(outline.settings.x, 1.0));
    var nearest = 1e9;
    for (var offset_y = -radius; offset_y <= radius; offset_y += 1) {
        for (var offset_x = -radius; offset_x <= radius; offset_x += 1) {
            let sample = clamp(center + vec2<i32>(offset_x, offset_y), vec2<i32>(0, 0), size - vec2<i32>(1, 1));
            if textureLoad(outline_mask, sample, 0).r > 0.5 {
                nearest = min(nearest, f32(offset_x * offset_x + offset_y * offset_y));
            }
        }
    }
    if nearest >= 1e9 {
        discard;
    }
    let coverage = clamp(outline.settings.x + 0.5 - sqrt(nearest), 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    return vec4<f32>(outline.color.rgb, outline.color.a * coverage);
}
