// Shadow ray generation: one pixel per dispatch, a primary ray from the
// surface camera, and a shadow ray from whatever it hits toward the key light.
//
// The mask this stage writes has one value per pixel of the surface: 1 where
// the key light reaches the pixel and 0 where the scene blocks it. The PBR
// fragment multiplies only its key-light term by that value, so the shadow
// lands under the cube without the raster passes having to know about it.
//
// Primary rays rather than reconstructed depth: the pass runs before the
// surface's geometry passes and reads nothing they produce. A ray that misses
// the scene is sky, which the key light always reaches.
//
// The stages beside it are `shadow_miss.hlsl` and `shadow_closesthit.hlsl`;
// `slangc` cooks each to its own `.spv` because a ray tracing pipeline has one
// module per stage and no entry point that covers all three. The payload is
// declared in each stage because Slang compiles each on its own.

#include "light_rig.hlsl"

struct ShadowPayload
{
    float hit_distance;
};

// The scene's top-level structure, and the mask this dispatch fills.
[[vk::binding(0, 0)]]
RaytracingAccelerationStructure scene;

[[vk::binding(1, 0)]]
RWStructuredBuffer<float> shadow_mask;

// The surface's camera in the frame the pixels belong to, and how many pixels
// there are.
struct ShadowPush
{
    column_major float4x4 inverse_view_projection;
    uint mask_width;
    uint mask_height;
    // Fills the block to its std140 size, which a push constant range has to
    // cover exactly.
    uint2 reserved;
};

[[vk::push_constant]]
ConstantBuffer<ShadowPush> push;

static const float MAXIMUM_RAY_DISTANCE = 10000.0;
// A shadow ray starts this far behind the hit, along the primary ray, so the
// surface it came from cannot block itself.
static const float SHADOW_RAY_BACK_OFF = 0.002;
static const float SHADOW_RAY_MINIMUM_DISTANCE = 0.001;

[shader("raygeneration")]
void raygen_main()
{
    uint2 pixel = DispatchRaysIndex().xy;

    // Pixel centre to the world-space near and far points of that pixel.
    float2 ndc = (float2(pixel) + 0.5) / float2(push.mask_width, push.mask_height) * 2.0 - 1.0;
    ndc.y = -ndc.y;
    float4 near_point = mul(push.inverse_view_projection, float4(ndc, 0.0, 1.0));
    float4 far_point = mul(push.inverse_view_projection, float4(ndc, 1.0, 1.0));
    float3 origin = near_point.xyz / near_point.w;
    float3 direction = normalize(far_point.xyz / far_point.w - origin);

    RayDesc primary_ray;
    primary_ray.Origin = origin;
    primary_ray.Direction = direction;
    primary_ray.TMin = 0.0;
    primary_ray.TMax = MAXIMUM_RAY_DISTANCE;

    ShadowPayload primary_payload;
    primary_payload.hit_distance = -1.0;
    TraceRay(
        scene,
        RAY_FLAG_FORCE_OPAQUE,
        0xFF,
        0,  // first shader binding table record
        1,  // record stride
        0,  // miss shader index
        primary_ray,
        primary_payload
    );

    float visibility = 1.0;
    if (primary_payload.hit_distance >= 0.0)
    {
        float3 hit_position = origin + direction * primary_payload.hit_distance;
        RayDesc shadow_ray;
        shadow_ray.Origin = hit_position - direction * SHADOW_RAY_BACK_OFF;
        shadow_ray.Direction = normalize(-KEY_DIRECTION);
        shadow_ray.TMin = SHADOW_RAY_MINIMUM_DISTANCE;
        shadow_ray.TMax = MAXIMUM_RAY_DISTANCE;

        ShadowPayload shadow_payload;
        shadow_payload.hit_distance = -1.0;
        TraceRay(
            scene,
            RAY_FLAG_ACCEPT_FIRST_HIT_AND_END_SEARCH | RAY_FLAG_FORCE_OPAQUE,
            0xFF,
            0,
            1,
            0,
            shadow_ray,
            shadow_payload
        );
        visibility = shadow_payload.hit_distance >= 0.0 ? 0.0 : 1.0;
    }

    shadow_mask[pixel.y * push.mask_width + pixel.x] = visibility;
}