// PBR vertex shader body, included by pbr_opaque_vertex.hlsl and (with POST_FX, which adds the
// motion-vector outputs) pbr_opaque_post_fx_vertex.hlsl. The model matrix is built here from
// per-draw position / rotation (radians) / scale, so the CPU uploads raw transforms.

struct Camera {
    float4                position;
    column_major float4x4 view_projection;
    column_major float4x4 previous_view_projection;
    float4                flags; // x = 1 when the pass writes motion vectors
};
[[vk::binding(0, 0)]] ConstantBuffer<Camera> camera;

struct DrawTransform {
    float4 position; // xyz
    float4 rotation; // xyz, radians
    float4 scale;    // xyz
};
// Mirrors DrawTransform in pass_pbr_opaque.rs: this frame's and last frame's (motion vectors only)
[[vk::binding(0, 3)]] StructuredBuffer<DrawTransform> per_draw_array;
#ifdef POST_FX
[[vk::binding(1, 3)]] StructuredBuffer<DrawTransform> previous_per_draw_array;
#endif

struct VertexOutput {
    [[vk::location(0)]] float2 texture_coordinates : TEXCOORD0;
    [[vk::location(1)]] float3 world_position      : TEXCOORD1;
    [[vk::location(2)]] float3 world_normal        : TEXCOORD2;
    [[vk::location(3)]] float3 world_tangent       : TEXCOORD3;
    [[vk::location(4)]] float3 world_bitangent     : TEXCOORD4;
#ifdef POST_FX
    // Clip positions of this and last frame; the fragment shader divides per pixel (dividing per
    // vertex and interpolating is wrong across a triangle, and for vertices behind the camera)
    [[vk::location(5)]] float4 clip_position          : TEXCOORD5;
    [[vk::location(6)]] float4 previous_clip_position : TEXCOORD6;
#endif
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

// Zero for (near) zero-length input, so degenerate tangents don't turn into NaN
float3 safe_normalize(float3 value) {
    float length_squared = dot(value, value);
    return length_squared > 1e-12 ? value * rsqrt(length_squared) : float3(0.0, 0.0, 0.0);
}

// model = T * (Rx*Ry*Rz) * S — matches glam from_scale_rotation_translation(Quat::x*y*z).
float3x3 rotation_matrix(DrawTransform transform) {
    return mul(rot_x(transform.rotation.x), mul(rot_y(transform.rotation.y), rot_z(transform.rotation.z)));
}

[shader("vertex")]
VertexOutput vs_main(VertexInput input, uint instance_id : SV_InstanceID) {
    DrawTransform transform = per_draw_array[instance_id];

    float3x3 rotation = rotation_matrix(transform);
    float3 world_position = transform.position.xyz + mul(rotation, input.position * transform.scale.xyz);
    float4 clip_position  = mul(camera.view_projection, float4(world_position, 1.0));

    VertexOutput output;
    output.sv_position            = clip_position;
#ifdef POST_FX
    output.clip_position          = clip_position;
    output.previous_clip_position = clip_position;
    if (camera.flags.x > 0.5) {
        DrawTransform previous = previous_per_draw_array[instance_id];
        float3 previous_world_position = previous.position.xyz + mul(rotation_matrix(previous), input.position * previous.scale.xyz);
        output.previous_clip_position  = mul(camera.previous_view_projection, float4(previous_world_position, 1.0));
    }
#endif
    output.texture_coordinates = input.texture_coordinates;
    output.world_position      = world_position;
    // Inverse-transpose of R*S is R*S^-1: divide by scale so non-uniform scale keeps normals perpendicular
    output.world_normal        = normalize(mul(rotation, input.normal / transform.scale.xyz));
    // Tangents lie in the surface, so they transform like positions (R*S)
    output.world_tangent       = safe_normalize(mul(rotation, input.tangent * transform.scale.xyz));
    output.world_bitangent     = safe_normalize(mul(rotation, input.bitangent * transform.scale.xyz));
    return output;
}
