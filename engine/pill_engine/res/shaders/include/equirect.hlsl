// Equirectangular UV mapping: atan2(z,x) convention matches the bake in procedural_equirect.rs.
static const float EQUIRECT_PI = 3.14159265359;

float2 dir_to_equirect_uv(float3 direction) {
    float3 normalized_direction = normalize(direction);
    float  u_coordinate = 0.5 + atan2(normalized_direction.z, normalized_direction.x) / (2.0 * EQUIRECT_PI);
    float  v_coordinate = 0.5 - asin(clamp(normalized_direction.y, -1.0, 1.0)) / EQUIRECT_PI;
    return float2(frac(u_coordinate), clamp(v_coordinate, 0.0, 1.0));
}
