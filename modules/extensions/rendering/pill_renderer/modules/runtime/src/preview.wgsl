// The asset preview: a fullscreen triangle whose fragment shader draws a
// texture file into a 128x128 target.
//
// A material or a mesh preview is drawn by the renderer's own chain instead, so
// this shader only fits a texture into the square. The background is the
// editor's panel colour, so a transparent texture still reads as a tile rather
// than as a hole.

@group(0) @binding(0) var image_texture: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

/// The editor's panel fill, so a tile's preview sits on the same colour as the
/// panel around it.
const BACKGROUND: vec3<f32> = vec3(0.13, 0.14, 0.15);

@vertex
fn preview_vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    // One triangle covering the whole target; the uv runs 0..1 across it.
    var corners = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0),
        vec2(3.0, -1.0),
        vec2(-1.0, 3.0),
    );
    let corner = corners[index];
    var output: VertexOutput;
    output.position = vec4<f32>(corner, 0.0, 1.0);
    output.uv = corner * 0.5 + vec2<f32>(0.5, 0.5);
    return output;
}

@fragment
fn preview_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    // The asset's own pixels, fitted into the square: a wide picture keeps its
    // aspect and the panel colour fills the rest.
    let dimensions = vec2<f32>(textureDimensions(image_texture, 0));
    let scale = max(max(dimensions.x, dimensions.y), 1.0);
    let fit = dimensions / scale;
    let centred = input.uv - vec2<f32>(0.5, 0.5);
    let inside = all(abs(centred) <= fit * 0.5);
    let uv = clamp(
        centred / max(fit, vec2<f32>(1e-4)) + vec2<f32>(0.5, 0.5),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    let sampled = textureSample(image_texture, image_sampler, uv).rgb;
    return vec4<f32>(select(BACKGROUND, sampled, inside), 1.0);
}
