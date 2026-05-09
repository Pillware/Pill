struct engine {
    fog_color: vec3<f32>,
    fog_density: f32,
}

struct camera {
    camera_position: vec3<f32>,
    camera_view_projection: mat4x4<f32>,
}

struct material {
    tint: vec3<f32>,
}

struct FragmentOutput {
    @location(0) out_final_color: vec4<f32>,
}

var<private> in_vertex_position_1: vec3<f32>;
var<private> in_vertex_texture_coordinates_1: vec2<f32>;
var<private> in_TBN_tangent_1: vec3<f32>;
var<private> in_TBN_bitangent_1: vec3<f32>;
var<private> in_TBN_normal_1: vec3<f32>;
var<private> in_world_position_1: vec3<f32>;
@group(0) @binding(0) 
var<uniform> global: engine;
@group(1) @binding(0) 
var<uniform> global_1: camera;
@group(2) @binding(0) 
var<uniform> global_2: material;
@group(3) @binding(0) 
var diffuse_texture: texture_2d<f32>;
@group(3) @binding(1) 
var diffuse_sampler: sampler;
var<private> out_final_color: vec4<f32>;

fn main_1() {
    var object_color: vec4<f32>;
    var final_color: vec3<f32>;
    var fog_dist: f32;
    var fog_factor: f32;

    let _e20 = in_vertex_texture_coordinates_1;
    let _e21 = textureSample(diffuse_texture, diffuse_sampler, _e20);
    object_color = _e21;
    let _e23 = object_color;
    let _e25 = global_2.tint;
    final_color = (_e23.xyz * _e25);
    let _e28 = global_1.camera_position;
    let _e29 = in_world_position_1;
    let _e31 = global_1.camera_position;
    let _e32 = in_world_position_1;
    fog_dist = length((_e31 - _e32));
    let _e37 = global.fog_density;
    let _e39 = global.fog_density;
    let _e41 = fog_dist;
    let _e43 = fog_dist;
    let _e45 = global.fog_density;
    let _e47 = global.fog_density;
    let _e49 = fog_dist;
    let _e51 = fog_dist;
    let _e58 = global.fog_density;
    let _e60 = global.fog_density;
    let _e62 = fog_dist;
    let _e64 = fog_dist;
    let _e66 = global.fog_density;
    let _e68 = global.fog_density;
    let _e70 = fog_dist;
    let _e72 = fog_dist;
    fog_factor = clamp((1f - exp((((-(_e66) * _e68) * _e70) * _e72))), 0f, 1f);
    let _e83 = final_color;
    let _e84 = global.fog_color;
    let _e85 = fog_factor;
    final_color = mix(_e83, _e84, vec3(_e85));
    let _e88 = final_color;
    out_final_color = vec4<f32>(_e88.x, _e88.y, _e88.z, 1f);
    return;
}

@fragment 
fn main(@location(0) in_vertex_position: vec3<f32>, @location(1) in_vertex_texture_coordinates: vec2<f32>, @location(2) in_TBN_tangent: vec3<f32>, @location(3) in_TBN_bitangent: vec3<f32>, @location(4) in_TBN_normal: vec3<f32>, @location(5) in_world_position: vec3<f32>) -> FragmentOutput {
    in_vertex_position_1 = in_vertex_position;
    in_vertex_texture_coordinates_1 = in_vertex_texture_coordinates;
    in_TBN_tangent_1 = in_TBN_tangent;
    in_TBN_bitangent_1 = in_TBN_bitangent;
    in_TBN_normal_1 = in_TBN_normal;
    in_world_position_1 = in_world_position;
    main_1();
    let _e41 = out_final_color;
    return FragmentOutput(_e41);
}
