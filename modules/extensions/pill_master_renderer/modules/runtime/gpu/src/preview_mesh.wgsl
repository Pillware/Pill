// The mesh preview: a lit triangle mesh framed in the preview's square.
//
// A mesh tile asks for this when the preference is on. The model is centred on
// the origin by the transform the caller frames it with, so the same pipeline
// draws any asset; the shading is a single directional light plus a rim, which
// reads the silhouette without a material or a texture.

struct MeshParameters {
    transform: mat4x4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> parameters: MeshParameters;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};

@vertex
fn preview_mesh_vertex(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = parameters.transform * vec4<f32>(input.position, 1.0);
    output.normal = input.normal;
    return output;
}

@fragment
fn preview_mesh_fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal);
    let light = normalize(vec3<f32>(0.4, 0.7, 0.6));
    let lambert = max(dot(normal, light), 0.0);
    let rim = pow(1.0 - abs(normal.z), 2.0) * 0.18;
    let colour = parameters.color.rgb * (0.22 + 0.85 * lambert) + rim * vec3<f32>(1.0);
    return vec4<f32>(colour, 1.0);
}
