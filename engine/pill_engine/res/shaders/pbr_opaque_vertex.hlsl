// PBR vertex shader (pill_assets regenerates the .wgsl). The model matrix is built here from
// per-draw position / rotation (radians) / scale, so the CPU uploads raw transforms.

struct Camera {
    float4                position;
    column_major float4x4 view_projection;
    float3                fog_color;
    float                 fog_density;
};
[[vk::binding(0, 0)]] ConstantBuffer<Camera> camera;

struct PerDraw {
    float4 position; // xyz
    float4 rotation; // xyz, radians
    float4 scale;    // xyz
};
[[vk::binding(0, 3)]] StructuredBuffer<PerDraw> per_draw_array;

struct VertexOutput {
    [[vk::location(0)]] float2 texture_coordinates : TEXCOORD0;
    [[vk::location(1)]] float3 world_position      : TEXCOORD1;
    [[vk::location(2)]] float3 world_normal        : TEXCOORD2;
    [[vk::location(3)]] float3 world_tangent       : TEXCOORD3;
    [[vk::location(4)]] float3 world_bitangent     : TEXCOORD4;
                        float4 sv_position         : SV_POSITION;
};

struct VertexInput {
    [[vk::location(0)]] float3 position;
    [[vk::location(4)]] float2 texture_coordinates;
    [[vk::location(5)]] float3 normal;
    [[vk::location(6)]] float3 tangent;
    [[vk::location(7)]] float3 bitangent;
};

// Row-major float3x3 constructors; mul(R, v) applies standard right-handed rotation.
float3x3 rot_x(float angle) { float cosine = cos(angle), sine = sin(angle); return float3x3(1, 0, 0,  0, cosine, -sine,  0, sine, cosine); }
float3x3 rot_y(float angle) { float cosine = cos(angle), sine = sin(angle); return float3x3(cosine, 0, sine,  0, 1, 0,  -sine, 0, cosine); }
float3x3 rot_z(float angle) { float cosine = cos(angle), sine = sin(angle); return float3x3(cosine, -sine, 0,  sine, cosine, 0,  0, 0, 1); }

[shader("vertex")]
VertexOutput vs_main(VertexInput input, uint instance_id : SV_InstanceID) {
    PerDraw per_draw = per_draw_array[instance_id];

    // model = T * (Rx*Ry*Rz) * S — matches glam from_scale_rotation_translation(Quat::x*y*z).
    float3x3 rotation = mul(rot_x(per_draw.rotation.x), mul(rot_y(per_draw.rotation.y), rot_z(per_draw.rotation.z)));
    float3 scaled_position = input.position * per_draw.scale.xyz;
    float3 world_position = per_draw.position.xyz + mul(rotation, scaled_position);
    float4 world_position_homogeneous = float4(world_position, 1.0);

    VertexOutput output;
    output.sv_position         = mul(camera.view_projection, world_position_homogeneous);
    output.texture_coordinates = input.texture_coordinates;
    output.world_position      = world_position;
    // Inverse-transpose of R*S is R*S^-1: divide by scale so non-uniform scale keeps normals perpendicular
    output.world_normal        = normalize(mul(rotation, input.normal / per_draw.scale.xyz));
    // Tangents lie in the surface, so they transform like positions (R*S)
    output.world_tangent       = normalize(mul(rotation, input.tangent * per_draw.scale.xyz));
    output.world_bitangent     = normalize(mul(rotation, input.bitangent * per_draw.scale.xyz));
    return output;
}
