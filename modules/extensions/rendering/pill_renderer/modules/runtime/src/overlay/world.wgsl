// Overlay shaders drawn through the surface's camera: debug lines and gizmos,
// the selection mask, and the pick buffer.

// The camera uniform, in the layout of `CameraParameters`.
struct Camera {
    position: vec4<f32>,
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;

// Debug lines and gizmos arrive in clip space, already widened on screen, so
// each one covers a pixel of anti-aliasing band the fragment shaders feather
// back down to a smooth edge.

// A line vertex: the corner of the line's quad, its colour, and how many
// pixels from the line's centre it sits.
struct LineInput {
    @location(0) clip: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) across: f32,
};

struct LineOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(linear) across: f32,
};

// The half width of a drawn line, in pixels; the renderer widens the quad
// past it by half a pixel for the coverage to fade across.
const LINE_HALF_WIDTH: f32 = 1.0;

// A gizmo triangle vertex: the corner pushed out by the feather, its colour,
// and the barycentric corner it stands for.
struct TriangleInput {
    @location(0) clip: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) barycentric: vec3<f32>,
};

struct TriangleOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(linear) barycentric: vec3<f32>,
};

// How far the renderer pushes a triangle's corners out, in pixels.
const TRIANGLE_FEATHER: f32 = 1.0;

// Eight sample offsets, evenly spread inside a pixel.
const TRIANGLE_SAMPLES: array<vec2<f32>, 8> = array<vec2<f32>, 8>(
    vec2<f32>(-0.3333, -0.3333),
    vec2<f32>(0.0, -0.3333),
    vec2<f32>(0.3333, -0.3333),
    vec2<f32>(-0.3333, 0.0),
    vec2<f32>(0.3333, 0.0),
    vec2<f32>(-0.3333, 0.3333),
    vec2<f32>(0.0, 0.3333),
    vec2<f32>(0.3333, 0.3333),
);

@vertex
fn line_vertex(input: LineInput) -> LineOutput {
    var output: LineOutput;
    output.clip = input.clip;
    output.color = input.color;
    output.across = input.across;
    return output;
}

@fragment
fn line_fragment(input: LineOutput) -> @location(0) vec4<f32> {
    let coverage = clamp(LINE_HALF_WIDTH + 0.5 - abs(input.across), 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    return vec4<f32>(input.color.rgb, input.color.a * coverage);
}

@vertex
fn triangle_vertex(input: TriangleInput) -> TriangleOutput {
    var output: TriangleOutput;
    output.clip = input.clip;
    output.color = input.color;
    output.barycentric = input.barycentric;
    return output;
}

// Coverage by sampling the triangle the shape really has: the barycentric
// functions of the widened triangle are straight in screen space, so at any
// sample point they can be read off directly, and the corners' widening is
// simply how many barycentric units one feather is at each edge. Sampling
// inside the true triangle (rather than fading whole half-planes) keeps a
// sharp corner from sprouting a faded point past its tip.
@fragment
fn triangle_fragment(input: TriangleOutput) -> @location(0) vec4<f32> {
    let barycentric = input.barycentric;
    let gradient_x = vec3<f32>(
        dpdx(barycentric.x),
        dpdx(barycentric.y),
        dpdx(barycentric.z),
    );
    let gradient_y = vec3<f32>(
        dpdy(barycentric.x),
        dpdy(barycentric.y),
        dpdy(barycentric.z),
    );
    let widths = vec3<f32>(
        length(vec2<f32>(gradient_x.x, gradient_y.x)),
        length(vec2<f32>(gradient_x.y, gradient_y.y)),
        length(vec2<f32>(gradient_x.z, gradient_y.z)),
    );
    let widened = TRIANGLE_FEATHER * widths;
    var covered = 0.0;
    for (var index = 0; index < 8; index += 1) {
        let offset = TRIANGLE_SAMPLES[index];
        let sampled = barycentric + gradient_x * offset.x + gradient_y * offset.y;
        if all(sampled >= widened) {
            covered += 0.125;
        }
    }
    if covered <= 0.0 {
        discard;
    }
    return vec4<f32>(input.color.rgb, input.color.a * covered);
}

// A gizmo point vertex: the corner of the screen-aligned quad, its colour,
// the corner's offset from the point's centre in pixels, the point's half
// size in pixels, and the silhouette it draws as.
struct PointInput {
    @location(0) clip: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) offset: vec2<f32>,
    @location(3) half_size: f32,
    @location(4) shape: u32,
};

struct PointOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) @interpolate(linear) offset: vec2<f32>,
    @location(2) @interpolate(flat) half_size: f32,
    @location(3) @interpolate(flat) shape: u32,
};

// The silhouettes a gizmo point draws as; the renderer sends the number.
const POINT_SHAPE_SQUARE: u32 = 1;
const POINT_SHAPE_DIAMOND: u32 = 2;

@vertex
fn point_vertex(input: PointInput) -> PointOutput {
    var output: PointOutput;
    output.clip = input.clip;
    output.color = input.color;
    output.offset = input.offset;
    output.half_size = input.half_size;
    output.shape = input.shape;
    return output;
}

// Whether a sample, given as an offset from the centre divided by the half
// size, lies inside the point's silhouette.
fn point_shape_contains(offset: vec2<f32>, shape: u32) -> bool {
    if shape == POINT_SHAPE_SQUARE {
        return max(abs(offset.x), abs(offset.y)) <= 1.0;
    }
    if shape == POINT_SHAPE_DIAMOND {
        return abs(offset.x) + abs(offset.y) <= 1.0;
    }
    return dot(offset, offset) <= 1.0;
}

// Coverage by the same eight samples a triangle takes. A point's corners carry
// offsets in pixels, so the samples need no gradients.
@fragment
fn point_fragment(input: PointOutput) -> @location(0) vec4<f32> {
    let half_size = max(input.half_size, 0.5);
    var covered = 0.0;
    for (var index = 0; index < 8; index += 1) {
        let sampled = (input.offset + TRIANGLE_SAMPLES[index]) / half_size;
        if point_shape_contains(sampled, input.shape) {
            covered += 0.125;
        }
    }
    if covered <= 0.0 {
        discard;
    }
    return vec4<f32>(input.color.rgb, input.color.a * covered);
}

// What the pick buffer holds at a pixel.
struct ValueOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) value: u32,
};

// Gizmo lines and point quads in the pick buffer: the value is the handle
// with its top bit set, over the same geometry the handle is drawn with.
@vertex
fn handle_clip_vertex(@location(0) clip: vec4<f32>, @location(1) value: u32) -> ValueOutput {
    var output: ValueOutput;
    output.clip = clip;
    output.value = value;
    return output;
}

// Gizmo triangles in the pick buffer: the value is the handle with its top bit
// set.
@vertex
fn handle_vertex(@location(0) position: vec3<f32>, @location(1) value: u32) -> ValueOutput {
    var output: ValueOutput;
    output.clip = camera.view_projection * vec4<f32>(position, 1.0);
    output.value = value;
    return output;
}

// A mesh vertex moved by its instance's three model rows, as the chain's
// shaders do it.
fn instance_clip(position: vec3<f32>, row_0: vec4<f32>, row_1: vec4<f32>, row_2: vec4<f32>) -> vec4<f32> {
    let model_position = vec4<f32>(position, 1.0);
    let world = vec4<f32>(dot(row_0, model_position), dot(row_1, model_position), dot(row_2, model_position), 1.0);
    return camera.view_projection * world;
}

// Meshes in the pick buffer: the value is one more than the instance's queue
// position, so zero stays "nothing".
@vertex
fn mesh_pick_vertex(
    @location(0) position: vec3<f32>,
    @location(1) row_0: vec4<f32>,
    @location(2) row_1: vec4<f32>,
    @location(3) row_2: vec4<f32>,
    @builtin(instance_index) instance: u32,
) -> ValueOutput {
    var output: ValueOutput;
    output.clip = instance_clip(position, row_0, row_1, row_2);
    output.value = instance + 1u;
    return output;
}

@fragment
fn value_fragment(input: ValueOutput) -> @location(0) u32 {
    return input.value;
}

// The outlined meshes' silhouettes, for the edge pass to trace.
@vertex
fn mesh_mask_vertex(
    @location(0) position: vec3<f32>,
    @location(1) row_0: vec4<f32>,
    @location(2) row_1: vec4<f32>,
    @location(3) row_2: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    return instance_clip(position, row_0, row_1, row_2);
}

@fragment
fn mask_fragment() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 1.0);
}
