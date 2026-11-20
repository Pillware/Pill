// The asset preview: a fullscreen triangle whose fragment shader draws one
// asset into a 128x128 target.
//
// A texture is sampled as a quad that keeps its aspect inside the square; a
// material is a shaded sphere in the material's colour. The background is the
// editor's panel colour, so a transparent or absent asset still reads as a
// tile rather than as a hole.

struct PreviewParameters {
    color: vec4<f32>,
    // The mode in the first word, three words keeping the struct a multiple of
    // 16 bytes as the uniform address space asks.
    mode: vec4<u32>,
};

@group(0) @binding(0) var<uniform> parameters: PreviewParameters;
@group(0) @binding(1) var image_texture: texture_2d<f32>;
@group(0) @binding(2) var image_sampler: sampler;

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
    if parameters.mode.x == 0u {
        // A texture: the asset's own pixels, fitted into the square. The
        // sample happens outside any branch - a sampler in non-uniform
        // control flow is a shader error - and the fit picks the colour.
        let dimensions = vec2<f32>(textureDimensions(image_texture, 0));
        let scale = max(max(dimensions.x, dimensions.y), 1.0);
        let fit = dimensions / scale;
        let centred = input.uv - vec2<f32>(0.5, 0.5);
        let inside = all(abs(centred) <= fit * 0.5);
        let uv = clamp(centred / max(fit, vec2<f32>(1e-4)) + vec2<f32>(0.5, 0.5), vec2<f32>(0.0), vec2<f32>(1.0));
        let sampled = textureSample(image_texture, image_sampler, uv).rgb;
        return vec4<f32>(select(BACKGROUND, sampled, inside), 1.0);
    }
    // A material: a lit sphere in the material's colour, which is what makes
    // a material tile readable without loading the scene it dresses.
    let centred = input.uv * 2.0 - vec2<f32>(1.0, 1.0);
    let distance = length(centred);
    if distance > 1.0 {
        return vec4<f32>(BACKGROUND, 1.0);
    }
    let normal = normalize(vec3<f32>(
        centred.x,
        -centred.y,
        sqrt(max(1.0 - distance * distance, 0.0)),
    ));
    let light = normalize(vec3<f32>(0.45, 0.6, 0.65));
    let lambert = max(dot(normal, light), 0.0);
    let rim = pow(1.0 - normal.z, 2.0) * 0.2;
    let colour = parameters.color.rgb * (0.18 + 0.9 * lambert) + rim * vec3<f32>(1.0);
    return vec4<f32>(colour, 1.0);
}
