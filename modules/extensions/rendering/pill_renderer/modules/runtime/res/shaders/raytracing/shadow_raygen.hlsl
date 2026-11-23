// Shadow ray generation: one ray per pixel of a surface, from the surface
// camera into the scene, and a shadow ray from each hit toward the key light.
// The pass that records this stage fills the body in.
//
// The stages beside it are `shadow_miss.hlsl` and `shadow_closesthit.hlsl`;
// `slangc` cooks each to its own `.spv` because a ray tracing pipeline has one
// module per stage and no single entry point that covers all three.
//
// The payload is declared here as well as in the hit and miss stages: Slang
// compiles each stage on its own, so no stage can take a type from another.

struct ShadowPayload
{
    float hit_distance;
};

[shader("raygeneration")]
void raygen_main()
{
}

