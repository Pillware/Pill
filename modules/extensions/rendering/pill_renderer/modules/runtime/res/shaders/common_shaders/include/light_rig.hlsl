// The three-point light rig the shading uses: a key, a fill and a rim light.
//
// Included by `pbr_surface.hlsl` and by the ray tracing shadow stage, so the
// shadow rays travel toward exactly the direction the shading lights from. The
// simple pipeline has its own light and does not include this header.
//
// These are constants until the renderer reads light components. A direction
// points from the surface toward the light, which is why the shadow ray of the
// key light travels along `-key_direction`.

static const float3 KEY_DIRECTION = normalize(float3(-0.5, -0.6, -1.0));
static const float3 FILL_DIRECTION = normalize(float3(1.0, -0.2, 0.4));
static const float3 RIM_DIRECTION = normalize(float3(0.0, 0.8, 0.6));

static const float3 KEY_COLOR = float3(1.0, 0.97, 0.92);
static const float3 FILL_COLOR = float3(0.85, 0.9, 1.0);
static const float3 RIM_COLOR = float3(1.0, 0.95, 0.9);

static const float KEY_INTENSITY = 3.0;
static const float FILL_INTENSITY = 1.0;
static const float RIM_INTENSITY = 1.5;
