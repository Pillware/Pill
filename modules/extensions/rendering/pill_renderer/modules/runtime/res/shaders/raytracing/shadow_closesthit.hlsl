// Shadow closest-hit stage: the ray met geometry, so the payload records how
// far away the blocker was. The mask the chain reads is built from that value,
// which is why the stage writes a distance and not just a flag.

struct ShadowPayload
{
    float hit_distance;
};

[shader("closesthit")]
void closesthit_main(inout ShadowPayload payload)
{
    payload.hit_distance = RayTCurrent();
}
