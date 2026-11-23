// Shadow miss stage: a ray that reached the sky. The payload records "no
// blocker", which is what makes the pixel lit instead of shadowed.

struct ShadowPayload
{
    float hit_distance;
};

[shader("miss")]
void miss_main(inout ShadowPayload payload)
{
    payload.hit_distance = -1.0;
}
