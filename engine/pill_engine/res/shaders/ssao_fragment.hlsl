// Half-resolution screen-space ambient obscurance. Alchemy AO [McGuire et al. 2011] in the
// Scalable Ambient Obscurance formulation [McGuire/Mara/Luebke, HPG 2012], positions and normals
// reconstructed from depth since this forward renderer has no normal G-buffer.
//
// Reads the Depth32Float scene depth, not an R16Float linear depth: half-float quantises to
// ~0.03 units at 30 units out, a large fraction of the AO radius.
//
// Outputs (ao, viewZ) — the downstream bilateral filter gets its guide channel in the same fetch.

// Mirrors SsaoParams in pass_ssao.rs
struct SsaoParams {
    float depth_mul;      // view distance = depth_mul / (depth_add - depth)
    float depth_add;
    float tan_half_x;     // tan(fov/2) * aspect
    float tan_half_y;     // tan(fov/2)
    float radius;         // world units
    float intensity;
    float power;
    float _pad;
};

[[vk::binding(0, 0)]] Texture2D<float> t_depth;   // full-res Depth32Float
[[vk::binding(1, 0)]] ConstantBuffer<SsaoParams> params;

static const int   TAP_COUNT   = 8;
// Spiral turns for 8 taps, from the SAO reference's tap-pattern table.
static const float SPIRAL_TURNS = 7.0;
// Self-occlusion reject, world units. Constant as in SAO: scaled by view depth it outgrows `vn`
// at range and erases distant occlusion.
static const float BIAS        = 0.01;
// Occlusion denominator epsilon, from SAO. Not a shrink-to-zero guard: the per-tap term goes as
// sqrt(vv)/(EPS + vv), peaking at vv == EPS with 1/(2*sqrt(EPS)). At 1e-4 that is 50 per tap
// against a 5/TAP_COUNT = 0.625 normalisation — one near tap saturates the estimate.
static const float OCC_EPS     = 0.01;
static const float EPS         = 1e-4;

float linearize(float raw) {
    return params.depth_mul / (params.depth_add - raw);
}

// Half-res Z from a 2x2 block: pick the sample closest to the block average. An average
// hallucinates a surface that exists nowhere and then fails the bilateral test on both sides of
// an edge; min/max snaps a floor texel onto a wall texel and kills the occlusion on the wall.
// `texel` returns the full-res texel picked.
float half_depth_texel(int2 half_coord, int2 maxc, out int2 texel) {
    int2  b = half_coord * 2;
    int2  t0 = clamp(b + int2(0, 0), int2(0, 0), maxc);
    int2  t1 = clamp(b + int2(1, 0), int2(0, 0), maxc);
    int2  t2 = clamp(b + int2(0, 1), int2(0, 0), maxc);
    int2  t3 = clamp(b + int2(1, 1), int2(0, 0), maxc);
    float d0 = linearize(t_depth.Load(int3(t0, 0)));
    float d1 = linearize(t_depth.Load(int3(t1, 0)));
    float d2 = linearize(t_depth.Load(int3(t2, 0)));
    float d3 = linearize(t_depth.Load(int3(t3, 0)));

    float avg  = 0.25 * (d0 + d1 + d2 + d3);
    float best = d0;
    float bestd = abs(d0 - avg);
    texel = t0;
    if (abs(d1 - avg) < bestd) { best = d1; bestd = abs(d1 - avg); texel = t1; }
    if (abs(d2 - avg) < bestd) { best = d2; bestd = abs(d2 - avg); texel = t2; }
    if (abs(d3 - avg) < bestd) { best = d3; texel = t3; }
    return best;
}

float half_depth(int2 half_coord, int2 maxc) {
    int2 texel;
    return half_depth_texel(half_coord, maxc, texel);
}

// View space, right-handed, camera down -Z. `uv` in [0,1].
float3 view_pos(float2 uv, float z) {
    float2 ndc = float2(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return float3(ndc.x * params.tan_half_x * z, ndc.y * params.tan_half_y * z, -z);
}

// View-space position of a half-res texel, reconstructed at the full-res texel its depth came
// from. Using the half-res texel centre instead puts the point up to half a full-res pixel off
// the surface, which on grazing planes exceeds BIAS and bands them with false occlusion.
float3 half_position(int2 half_coord, int2 maxc, out float z) {
    int2 texel;
    z = half_depth_texel(half_coord, maxc, texel);
    return view_pos(((float2)texel + 0.5) / (float2)(maxc + 1), z);
}

// 4x4 repeating rotation, not white noise: every direction appears within each 4x4 neighbourhood,
// so the blur downstream sees a complete set rather than a random subset. Same 4x4-noise /
// 4x4-blur pairing as the Activision GTAO paper.
float rotation_4x4(int2 p) {
    // Bit-reversed van der Corput over the cell — consecutive pixels get maximally separated
    // angles, so the tile covers the circle evenly.
    int i = (p.y & 3) * 4 + (p.x & 3);
    int r = ((i & 1) << 3) | ((i & 2) << 1) | ((i & 4) >> 1) | ((i & 8) >> 3);
    return (float)r / 16.0;
}

[shader("fragment")]
float4 fs_main(float2 uv : TEXCOORD0) : SV_Target {
    uint fw, fh;
    t_depth.GetDimensions(fw, fh);
    int2 maxc = int2((int)fw - 1, (int)fh - 1);

    int2 half_dim   = int2((int)fw / 2, (int)fh / 2);
    int2 half_coord = clamp((int2)(uv * (float2)half_dim), int2(0, 0), half_dim - int2(1, 1));

    float  z;
    float3 P = half_position(half_coord, maxc, z);

    // Far plane / sky: no geometry to occlude, and keeps the 1/z below finite.
    if (z <= 0.0 || z > 1e4)
        return float4(1.0, z, 0.0, 0.0);

    // 5-tap cross, closer neighbour per axis before the cross product [Turánszki 2019]. Naive
    // ddx/ddy straddles silhouettes and tilts the normal into the surface, letting rays go
    // below it and haloing every edge.
    int2   half_max = half_dim - int2(1, 1);
    float  zl, zr, zd, zu;
    float3 Pl = half_position(clamp(half_coord + int2(-1,  0), int2(0, 0), half_max), maxc, zl);
    float3 Pr = half_position(clamp(half_coord + int2( 1,  0), int2(0, 0), half_max), maxc, zr);
    float3 Pd = half_position(clamp(half_coord + int2( 0, -1), int2(0, 0), half_max), maxc, zd);
    float3 Pu = half_position(clamp(half_coord + int2( 0,  1), int2(0, 0), half_max), maxc, zu);

    float3 dx = (abs(zr - z) < abs(zl - z)) ? (Pr - P) : (P - Pl);
    float3 dy = (abs(zu - z) < abs(zd - z)) ? (Pu - P) : (P - Pd);

    float3 N = normalize(cross(dx, dy));
    if (N.z < 0.0) N = -N;   // visible surfaces face the camera

    // Project the world-space radius to half-res pixels.
    float proj_scale = 0.5 * (float)half_dim.y / max(params.tan_half_y, EPS);
    float radius_px  = params.radius * proj_scale / z;

    // Sub-pixel footprint: the spiral would degenerate onto one texel.
    if (radius_px < 1.0)
        return float4(1.0, z, 0.0, 0.0);

    float r2   = params.radius * params.radius;
    float ang0 = rotation_4x4(half_coord) * 6.2831853;
    float occ  = 0.0;

    [loop]
    for (int i = 0; i < TAP_COUNT; i++) {
        float alpha = ((float)i + 0.5) / (float)TAP_COUNT;
        float h     = alpha * radius_px;
        float theta = alpha * SPIRAL_TURNS * 6.2831853 + ang0;

        int2 tap = clamp(half_coord + (int2)(float2(cos(theta), sin(theta)) * h),
                         int2(0, 0), half_dim - int2(1, 1));
        // Centre texel is pure self-occlusion, and the vv -> 0 case OCC_EPS handles worst.
        if (all(tap == half_coord))
            continue;
        float  zs;
        float3 Q = half_position(tap, maxc, zs);
        if (zs <= 0.0 || zs > 1e4)
            continue;

        float3 v = Q - P;

        float vv = dot(v, v);
        float vn = dot(v, N);

        // SAO eq. 10 — the cubed falloff keeps the estimator continuous as a sample crosses
        // the radius, so nothing pops under camera motion.
        float f = max(r2 - vv, 0.0);
        occ += f * f * f * max((vn - BIAS) / (vv + OCC_EPS), 0.0);
    }

    // SAO normalisation: 5/r^6 cancels the falloff's units, keeping `intensity` independent
    // of `radius`.
    float ao = max(0.0, 1.0 - occ * params.intensity * 5.0 / (max(pow(params.radius, 6.0), EPS) * (float)TAP_COUNT));
    ao = pow(saturate(ao), max(params.power, EPS));

    return float4(ao, z, 0.0, 0.0);
}
