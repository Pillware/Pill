use std::f32::consts::PI;

// Gradient (linear, inverse-Reinhard compensated: c/(1+c) → target on screen).
const TOP: [f32; 3] = [0.90, 0.90, 1.20]; // Reinhard → [0.47, 0.47, 0.55] cool blue ceiling
const HORIZON: [f32; 3] = [0.43, 0.43, 0.46]; // Reinhard → [0.30, 0.30, 0.32] neutral mid
const BOTTOM: [f32; 3] = [0.040, 0.040, 0.050]; // Reinhard → [0.04, 0.04, 0.05] dark floor

// Key light: warm tungsten, upper-left, ~40° above horizon.
const KEY_U: f32 = 0.10;
const KEY_V: f32 = 0.28;
const KEY_INTENSITY: [f32; 3] = [6.0, 4.8, 2.5]; // Reinhard → [0.86, 0.83, 0.71] warm gold

// Rim light: cool blue, upper-right, ~45° above horizon.
const RIM_U: f32 = 0.60;
const RIM_V: f32 = 0.25;
const RIM_INTENSITY: [f32; 3] = [1.5, 2.5, 5.0]; // Reinhard → [0.60, 0.71, 0.83] cool blue

const LIGHT_K: f32 = 8.0; // glare falloff exponent
const KEY_SKY: [f32; 3] = [0.010, 0.060, 0.200]; // 0.2 * vec3(0.05, 0.3, 1.) — cool blue haze away from key

/// Generates the studio equirect panorama as Rgba32Float pixels (512×256 linear HDR).
pub fn generate() -> (Vec<f32>, u32, u32) {
    let (w, h) = (512u32, 256u32);
    let key_dir = equirect_uv_to_dir(KEY_U, KEY_V);
    let rim_dir = equirect_uv_to_dir(RIM_U, RIM_V);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let pixel_dir = equirect_uv_to_dir(u, v);
            let t = (pixel_dir[1] + 1.0) * 0.5; // pixel_dir[1] = sin(elev) ∈ [-1, 1]
            let mut col = if t < 0.5 {
                lerp3(BOTTOM, HORIZON, t * 2.0)
            } else {
                lerp3(HORIZON, TOP, (t - 0.5) * 2.0)
            };
            // key: warm glare + cool sky haze (FF2023 sun glare formula)
            let sun = dot(pixel_dir, key_dir).clamp(0.0, 1.0);
            let sky = 1.0 - sun.powf(0.5);
            col[0] += KEY_INTENSITY[0] * sun.powf(LIGHT_K) + KEY_SKY[0] * sky;
            col[1] += KEY_INTENSITY[1] * sun.powf(LIGHT_K) + KEY_SKY[1] * sky;
            col[2] += KEY_INTENSITY[2] * sun.powf(LIGHT_K) + KEY_SKY[2] * sky;
            // rim: glare only
            let w_rim = dot(pixel_dir, rim_dir).clamp(0.0, 1.0).powf(LIGHT_K);
            col[0] += RIM_INTENSITY[0] * w_rim;
            col[1] += RIM_INTENSITY[1] * w_rim;
            col[2] += RIM_INTENSITY[2] * w_rim;
            out.push(col[0]);
            out.push(col[1]);
            out.push(col[2]);
            out.push(1.0);
        }
    }
    (out, w, h)
}

/// Bakes IBL maps from an Rgba32Float equirect panorama.
/// Returns (diffuse 32×16, specular mips [128×64 → 8×4], brdf_lut 256×256), all Rgba32Float.
pub fn bake_all(equirect: &[f32], width: u32, height: u32) -> (Vec<f32>, Vec<Vec<f32>>, Vec<f32>) {
    let eq = Equirect {
        pixels: equirect.to_vec(),
        w: width,
        h: height,
    };

    // diffuse: irradiance as linear f32 (sRGB→linear round-trip for u8 intermediate)
    let diff_u8 = compute_irradiance(&eq, 32, 16);
    let diffuse: Vec<f32> = diff_u8
        .chunks(4)
        .flat_map(|rgba| {
            [
                srgb_to_linear(rgba[0] as f32 / 255.0),
                srgb_to_linear(rgba[1] as f32 / 255.0),
                srgb_to_linear(rgba[2] as f32 / 255.0),
                1.0f32,
            ]
        })
        .collect();

    // specular: 5 mip levels, Rgba32Float HDR, roughness steps matching shader MAX_REFLECTION_LOD=4
    const MIP_ROUGHNESS: [f32; 5] = [0.04, 0.25, 0.5, 0.75, 1.0];
    let mut specular_mips: Vec<Vec<f32>> = Vec::with_capacity(5);
    for (mip, &r) in MIP_ROUGHNESS.iter().enumerate() {
        let w = (128u32 >> mip).max(1);
        let h = (64u32 >> mip).max(1);
        specular_mips.push(compute_specular_prefilter(&eq, w, h, r));
    }

    // brdf_lut: linear u8 → f32
    let lut_u8 = compute_brdf_lut(256, 256);
    let brdf_lut: Vec<f32> = lut_u8.iter().map(|&b| b as f32 / 255.0).collect();

    (diffuse, specular_mips, brdf_lut)
}

// --- Equirect ---

struct Equirect {
    pixels: Vec<f32>, // flat RGBA f32 linear
    w: u32,
    h: u32,
}

impl Equirect {
    // Sample at a world-space direction; nearest-neighbor, returns linear HDR values.
    fn sample(&self, dir: [f32; 3]) -> [f32; 3] {
        let (u, v) = dir_to_equirect_uv(dir);
        let x = ((u * self.w as f32) as u32).min(self.w - 1);
        let y = ((v * self.h as f32) as u32).min(self.h - 1);
        let i = ((y * self.w + x) * 4) as usize;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2]]
    }
}

// --- Coordinate helpers ---

fn equirect_uv_to_dir(u: f32, v: f32) -> [f32; 3] {
    let az = (u - 0.5) * 2.0 * PI; // consistent with dir_to_equirect_uv: u = 0.5 + az/(2π)
    let elev = (0.5 - v) * PI;
    let cos_e = elev.cos();
    [cos_e * az.cos(), elev.sin(), cos_e * az.sin()]
}

fn dir_to_equirect_uv(dir: [f32; 3]) -> (f32, f32) {
    let [x, y, z] = normalize(dir);
    let az = z.atan2(x);
    let elev = y.clamp(-1.0, 1.0).asin();
    let u = (0.5 + az / (2.0 * PI)).rem_euclid(1.0);
    let v = (0.5 - elev / PI).clamp(0.0, 1.0);
    (u, v)
}

// --- Vector math ---

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-8);
    [v[0] / len, v[1] / len, v[2] / len]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

// Build an orthonormal tangent frame (t, b) from normal n.
fn tangent_frame(n: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let up = if n[1].abs() < 0.9 {
        [0.0f32, 1.0, 0.0]
    } else {
        [1.0f32, 0.0, 0.0]
    };
    let t = normalize(cross(up, n));
    let b = cross(n, t);
    (t, b)
}

// Transform v from tangent space (where n=[0,0,1]) to world space.
fn tangent_to_world(v: [f32; 3], n: [f32; 3]) -> [f32; 3] {
    let (t, b) = tangent_frame(n);
    normalize([
        t[0] * v[0] + b[0] * v[1] + n[0] * v[2],
        t[1] * v[0] + b[1] * v[1] + n[1] * v[2],
        t[2] * v[0] + b[2] * v[1] + n[2] * v[2],
    ])
}

// --- Hammersley quasi-random sequence ---

fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    bits as f32 * 2.328_306_4e-10 // / 2^32
}

fn hammersley(i: u32, n: u32) -> (f32, f32) {
    (i as f32 / n as f32, radical_inverse_vdc(i))
}

// GGX importance sampling: returns half-vector in world space around n.
fn importance_sample_ggx(xi: (f32, f32), roughness: f32, n: [f32; 3]) -> [f32; 3] {
    let a = roughness * roughness;
    let phi = 2.0 * PI * xi.0;
    let cos_t = ((1.0 - xi.1) / (1.0 + (a * a - 1.0) * xi.1))
        .max(0.0)
        .sqrt();
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let h_local = [sin_t * phi.cos(), sin_t * phi.sin(), cos_t];
    tangent_to_world(h_local, n)
}

// --- Diffuse irradiance: cosine-weighted Riemann sum ---

fn compute_irradiance(eq: &Equirect, out_w: u32, out_h: u32) -> Vec<u8> {
    const N_PHI: u32 = 64;
    const N_THETA: u32 = 32;
    let dphi = 2.0 * PI / N_PHI as f32;
    let dtheta = 0.5 * PI / N_THETA as f32;

    let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
    for y in 0..out_h {
        for x in 0..out_w {
            let u = (x as f32 + 0.5) / out_w as f32;
            let v = (y as f32 + 0.5) / out_h as f32;
            let n = equirect_uv_to_dir(u, v);

            let mut irr = [0.0f32; 3];
            let mut wt = 0.0f32;

            for j in 0..N_THETA {
                for i in 0..N_PHI {
                    let phi = (i as f32 + 0.5) * dphi;
                    let theta = (j as f32 + 0.5) * dtheta;
                    let sin_t = theta.sin();
                    let cos_t = theta.cos();
                    let local = [sin_t * phi.cos(), sin_t * phi.sin(), cos_t];
                    let world = tangent_to_world(local, n);
                    let sample = eq.sample(world);
                    let w = cos_t * sin_t;
                    irr[0] += sample[0] * w;
                    irr[1] += sample[1] * w;
                    irr[2] += sample[2] * w;
                    wt += w;
                }
            }

            let s = PI / wt;
            out.push(linear_to_srgb(irr[0] * s));
            out.push(linear_to_srgb(irr[1] * s));
            out.push(linear_to_srgb(irr[2] * s));
            out.push(255u8);
        }
    }
    out
}

// --- Specular prefilter: GGX importance sampling, HDR linear output ---

fn compute_specular_prefilter(eq: &Equirect, out_w: u32, out_h: u32, roughness: f32) -> Vec<f32> {
    const N_SAMPLES: u32 = 256;

    let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
    for y in 0..out_h {
        for x in 0..out_w {
            let u = (x as f32 + 0.5) / out_w as f32;
            let v = (y as f32 + 0.5) / out_h as f32;
            let n = equirect_uv_to_dir(u, v);

            let mut col = [0.0f32; 3];
            let mut wt = 0.0f32;

            for i in 0..N_SAMPLES {
                let xi = hammersley(i, N_SAMPLES);
                let h = importance_sample_ggx(xi, roughness, n);
                let vdoth = dot(n, h).max(0.0);
                let l = normalize([
                    2.0 * vdoth * h[0] - n[0],
                    2.0 * vdoth * h[1] - n[1],
                    2.0 * vdoth * h[2] - n[2],
                ]);
                let ndotl = dot(n, l).max(0.0);
                if ndotl > 0.0 {
                    let s = eq.sample(l);
                    col[0] += s[0] * ndotl;
                    col[1] += s[1] * ndotl;
                    col[2] += s[2] * ndotl;
                    wt += ndotl;
                }
            }

            let s = 1.0 / wt.max(1e-6);
            out.push(col[0] * s);
            out.push(col[1] * s);
            out.push(col[2] * s);
            out.push(1.0f32);
        }
    }
    out
}

// --- BRDF LUT: GGX split-sum preintegration (Karis / UE4) ---
// Stored linear (not sRGB): R = F0 scale, G = F0 bias.

fn geometry_schlick_ggx_ibl(ndotv: f32, roughness: f32) -> f32 {
    let k = roughness * roughness / 2.0;
    ndotv / (ndotv * (1.0 - k) + k)
}

fn geometry_smith_ibl(ndotv: f32, ndotl: f32, roughness: f32) -> f32 {
    geometry_schlick_ggx_ibl(ndotv, roughness) * geometry_schlick_ggx_ibl(ndotl, roughness)
}

fn compute_brdf_lut(w: u32, h: u32) -> Vec<u8> {
    const N: u32 = 1024;

    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            // x = NdotV (0=grazing, 1=normal incidence)
            // y = roughness (0=smooth, 1=rough) — y=0 is top of texture
            let ndotv = (x as f32 + 0.5) / w as f32;
            let roughness = (y as f32 + 0.5) / h as f32;
            let roughness = roughness.max(0.04);

            let v = [(1.0 - ndotv * ndotv).max(0.0).sqrt(), 0.0f32, ndotv];
            let n = [0.0f32, 0.0, 1.0];

            let mut scale = 0.0f32;
            let mut bias = 0.0f32;

            for i in 0..N {
                let xi = hammersley(i, N);
                let h_v = importance_sample_ggx(xi, roughness, n);
                let vdoth = dot(v, h_v).max(0.0);
                let l = normalize([
                    2.0 * vdoth * h_v[0] - v[0],
                    2.0 * vdoth * h_v[1] - v[1],
                    2.0 * vdoth * h_v[2] - v[2],
                ]);
                let ndotl = l[2].max(0.0); // n = [0,0,1]
                let ndoth = h_v[2].max(0.0);

                if ndotl > 0.0 {
                    let g_vis = geometry_smith_ibl(ndotv, ndotl, roughness) * vdoth
                        / (ndoth * ndotv.max(0.001));
                    let fc = (1.0 - vdoth).powi(5);
                    scale += (1.0 - fc) * g_vis;
                    bias += fc * g_vis;
                }
            }

            scale = (scale / N as f32).clamp(0.0, 1.0);
            bias = (bias / N as f32).clamp(0.0, 1.0);

            // Linear bytes (not sRGB — BRDF coefficients are in linear space)
            out.push((scale * 255.0 + 0.5) as u8);
            out.push((bias * 255.0 + 0.5) as u8);
            out.push(0u8);
            out.push(255u8);
        }
    }
    out
}

// --- Color utilities ---

fn linear_to_srgb(x: f32) -> u8 {
    let c = x.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.040_45 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
