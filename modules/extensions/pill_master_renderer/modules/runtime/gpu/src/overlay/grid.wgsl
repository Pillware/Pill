// The scene grid: an infinite ground plane, drawn as one fullscreen triangle
// whose fragment shader intersects each pixel's ray with y = 0 and lays the
// lines out in world space, so the grid never ends, never pops while the view
// moves, and costs one draw per surface.

// The camera uniform, in the layout of `CameraParametersData`.
struct Camera {
    position: vec4<f32>,
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
};

// The grid's spacing (in `cell.x`) and the drawing region in pixels.
struct Grid {
    cell: vec4<f32>,
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(1) @binding(0) var<uniform> grid: Grid;

const MINOR_COLOR: vec3<f32> = vec3<f32>(0.42, 0.43, 0.46);
const MAJOR_COLOR: vec3<f32> = vec3<f32>(0.58, 0.60, 0.64);
const X_AXIS_COLOR: vec3<f32> = vec3<f32>(0.72, 0.32, 0.32);
const Z_AXIS_COLOR: vec3<f32> = vec3<f32>(0.32, 0.46, 0.76);
const LINE_OPACITY: f32 = 0.55;
// Where the grid starts fading and where it is gone, measured in cells.
const FADE_START_CELLS: f32 = 24.0;
const FADE_END_CELLS: f32 = 80.0;

// One triangle that covers the whole target, pinned to the far plane: the
// depth test then keeps the grid behind whatever geometry the chain drew and
// lets it show everywhere nothing occludes it.
@vertex
fn grid_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 1.0, 1.0);
}

// How much of one grid line covers a world coordinate, anti-aliased through
// the coordinate's own screen-space derivative.
fn line_coverage(value: f32, spacing: f32) -> f32 {
    let coordinate = value / spacing;
    let width = max(fwidth(coordinate), 1e-5);
    let distance = abs(fract(coordinate - 0.5) - 0.5) / width;
    return 1.0 - min(distance, 1.0);
}

@fragment
fn grid_fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // The pixel centre, back to clip space through the drawing region.
    let region = grid.viewport;
    let local = (position.xy - region.xy) / region.zw;
    let clip = vec2<f32>(local.x * 2.0 - 1.0, 1.0 - local.y * 2.0);
    // The ray through this pixel: near and far points back in world space.
    let near_h = camera.inverse_view_projection * vec4<f32>(clip, 0.0, 1.0);
    let far_h = camera.inverse_view_projection * vec4<f32>(clip, 1.0, 1.0);
    let origin = camera.position.xyz;
    let near = near_h.xyz / near_h.w;
    let far = far_h.xyz / far_h.w;
    let direction = normalize(far - near);
    // Where the ray meets the ground plane, if it looks down at it at all.
    if abs(direction.y) < 1e-5 {
        discard;
    }
    let distance = -origin.y / direction.y;
    if distance <= 0.0 {
        discard;
    }
    let point = origin + direction * distance;
    // The lines: minor every cell, major every ten, and the two axes.
    let cell = max(grid.cell.x, 1e-5);
    let minor = max(line_coverage(point.x, cell), line_coverage(point.z, cell));
    let major = max(
        line_coverage(point.x, cell * 10.0),
        line_coverage(point.z, cell * 10.0),
    );
    // The axes: a one-pixel band around x = 0 and z = 0.
    let x_axis = 1.0 - min(abs(point.z) / max(fwidth(point.z), 1e-5), 1.0);
    let z_axis = 1.0 - min(abs(point.x) / max(fwidth(point.x), 1e-5), 1.0);
    var color = MINOR_COLOR;
    var coverage = minor;
    if major > coverage {
        color = MAJOR_COLOR;
        coverage = major;
    }
    if z_axis > coverage {
        color = Z_AXIS_COLOR;
        coverage = z_axis;
    }
    if x_axis > coverage {
        color = X_AXIS_COLOR;
        coverage = x_axis;
    }
    // The grid dissolves with distance instead of shimmering as its lines
    // fall under a pixel.
    let travelled = length(point - origin);
    let fade = 1.0 - smoothstep(cell * FADE_START_CELLS, cell * FADE_END_CELLS, travelled);
    let alpha = coverage * fade * LINE_OPACITY;
    if alpha <= 0.001 {
        discard;
    }
    return vec4<f32>(color, alpha);
}
