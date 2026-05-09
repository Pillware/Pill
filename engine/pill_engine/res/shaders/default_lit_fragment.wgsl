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
    specularity: f32,
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
@group(3) @binding(2) 
var normal_texture: texture_2d<f32>;
@group(3) @binding(3) 
var normal_sampler: sampler;
var<private> out_final_color: vec4<f32>;

fn main_1() {
    var ambient_light_strength: f32 = 0.02f;
    var light_position: vec3<f32> = vec3<f32>(-10f, 10f, -10f);
    var light_color: vec3<f32> = vec3<f32>(1f, 1f, 1f);
    var object_color: vec4<f32>;
    var object_normal: vec4<f32>;
    var TBN_matrix: mat3x3<f32>;
    var ambient_light_factor: vec3<f32>;
    var normal: vec3<f32>;
    var light_direction: vec3<f32>;
    var diffuse_light_strength: f32;
    var diffuse_light_factor: vec3<f32>;
    var view_direction: vec3<f32>;
    var half_direction: vec3<f32>;
    var specular_light_strength: f32;
    var specular_light_factor: vec3<f32>;
    var final_color: vec3<f32>;
    var fog_dist: f32;
    var fog_factor: f32;

    let _e38 = in_vertex_texture_coordinates_1;
    let _e39 = textureSample(diffuse_texture, diffuse_sampler, _e38);
    object_color = _e39;
    let _e42 = in_vertex_texture_coordinates_1;
    let _e43 = textureSample(normal_texture, normal_sampler, _e42);
    object_normal = _e43;
    let _e45 = in_TBN_tangent_1;
    let _e46 = in_TBN_bitangent_1;
    let _e47 = in_TBN_normal_1;
    TBN_matrix = mat3x3<f32>(vec3<f32>(_e45.x, _e45.y, _e45.z), vec3<f32>(_e46.x, _e46.y, _e46.z), vec3<f32>(_e47.x, _e47.y, _e47.z));
    let _e62 = light_color;
    let _e63 = ambient_light_strength;
    ambient_light_factor = (_e62 * _e63);
    let _e66 = object_normal;
    let _e73 = object_normal;
    normal = normalize(((_e73.xyz * 2f) - vec3(1f)));
    let _e82 = TBN_matrix;
    let _e83 = light_position;
    let _e85 = in_vertex_position_1;
    let _e87 = TBN_matrix;
    let _e88 = light_position;
    let _e90 = in_vertex_position_1;
    light_direction = normalize(((_e87 * _e88) - _e90));
    let _e96 = normal;
    let _e97 = light_direction;
    let _e102 = normal;
    let _e103 = light_direction;
    diffuse_light_strength = max(dot(_e102, _e103), 0f);
    let _e108 = light_color;
    let _e109 = diffuse_light_strength;
    diffuse_light_factor = (_e108 * _e109);
    let _e112 = TBN_matrix;
    let _e113 = global_1.camera_position;
    let _e115 = in_vertex_position_1;
    let _e117 = TBN_matrix;
    let _e118 = global_1.camera_position;
    let _e120 = in_vertex_position_1;
    view_direction = normalize(((_e117 * _e118) - _e120));
    let _e124 = view_direction;
    let _e125 = light_direction;
    let _e127 = view_direction;
    let _e128 = light_direction;
    half_direction = normalize((_e127 + _e128));
    let _e134 = normal;
    let _e135 = half_direction;
    let _e140 = normal;
    let _e141 = half_direction;
    let _e148 = normal;
    let _e149 = half_direction;
    let _e154 = normal;
    let _e155 = half_direction;
    let _e162 = global_2.specularity;
    specular_light_strength = (pow(max(dot(_e154, _e155), 0f), 32f) * _e162);
    let _e165 = light_color;
    let _e166 = specular_light_strength;
    specular_light_factor = (_e165 * _e166);
    let _e169 = ambient_light_factor;
    let _e170 = diffuse_light_factor;
    let _e172 = specular_light_factor;
    let _e174 = object_color;
    let _e177 = global_2.tint;
    final_color = ((((_e169 + _e170) + _e172) * _e174.xyz) * _e177);
    let _e180 = global_1.camera_position;
    let _e181 = in_world_position_1;
    let _e183 = global_1.camera_position;
    let _e184 = in_world_position_1;
    fog_dist = length((_e183 - _e184));
    let _e189 = global.fog_density;
    let _e191 = global.fog_density;
    let _e193 = fog_dist;
    let _e195 = fog_dist;
    let _e197 = global.fog_density;
    let _e199 = global.fog_density;
    let _e201 = fog_dist;
    let _e203 = fog_dist;
    let _e210 = global.fog_density;
    let _e212 = global.fog_density;
    let _e214 = fog_dist;
    let _e216 = fog_dist;
    let _e218 = global.fog_density;
    let _e220 = global.fog_density;
    let _e222 = fog_dist;
    let _e224 = fog_dist;
    fog_factor = clamp((1f - exp((((-(_e218) * _e220) * _e222) * _e224))), 0f, 1f);
    let _e235 = final_color;
    let _e236 = global.fog_color;
    let _e237 = fog_factor;
    final_color = mix(_e235, _e236, vec3(_e237));
    let _e240 = final_color;
    out_final_color = vec4<f32>(_e240.x, _e240.y, _e240.z, 1f);
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
    let _e47 = out_final_color;
    return FragmentOutput(_e47);
}
