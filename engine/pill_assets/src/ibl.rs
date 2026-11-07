//! Image-based lighting bake — the single source of the IBL math.
//!
//! Pure CPU, no filesystem: used at build time (the `EquirectToIBL` / `BrdfLut` rules) and at
//! runtime (games baking from a fetched HDR, including on WASM). Inputs/outputs are plain f32
//! RGBA buffers; callers handle I/O and GPU upload.

use std::f32::consts::PI;

use anyhow::{anyhow, bail, Result};

use crate::util::{
    color::srgb_to_linear,
    vec_math::{dot3, normalize3, tangent_frame},
};

/// Diffuse irradiance map size (Rgba32Float).
pub const DIFFUSE_SIZE: (u32, u32) = (32, 16);
/// Specular prefilter base mip size (Rgba32Float, 5 mips).
pub const SPECULAR_SIZE: (u32, u32) = (128, 64);

/// Equirect size used for backgrounds and as IBL bake input (2:1 panorama).
pub const EQUIRECT_SIZE: (u32, u32) = (512, 256);

/// Decodes a Radiance `.hdr` panorama and resamples it to `EQUIRECT_SIZE` RGBA f32: box filter
/// when shrinking (no aliasing at any ratio), bilinear when enlarging.
pub fn equirect_from_hdr(bytes: &[u8]) -> Result<Equirect> {
    let mut decoder = zune_hdr::HdrDecoder::new(bytes);
    let rgb: Vec<f32> = decoder.decode().map_err(|e| anyhow!("decode HDR: {e:?}"))?;
    let (src_w, src_h) = decoder
        .get_dimensions()
        .ok_or_else(|| anyhow!("HDR has no dimensions"))?;
    if src_w == 0 || src_h == 0 || rgb.len() != src_w * src_h * 3 {
        bail!("HDR is {src_w}x{src_h} with {} values", rgb.len());
    }

    let (out_w, out_h) = (EQUIRECT_SIZE.0 as usize, EQUIRECT_SIZE.1 as usize);
    let x_taps = resample_taps(src_w, out_w);
    let y_taps = resample_taps(src_h, out_h);
    // Horizontal pass: src_h rows of out_w RGB texels
    let mut rows = vec![0.0f32; src_h * out_w * 3];
    for y in 0..src_h {
        for (ox, taps) in x_taps.iter().enumerate() {
            for &(x, weight) in taps {
                for c in 0..3 {
                    rows[(y * out_w + ox) * 3 + c] += rgb[(y * src_w + x) * 3 + c] * weight;
                }
            }
        }
    }
    // Vertical pass into RGBA
    let mut pixels = vec![0.0f32; out_w * out_h * 4];
    for (oy, taps) in y_taps.iter().enumerate() {
        for ox in 0..out_w {
            let out = &mut pixels[(oy * out_w + ox) * 4..][..4];
            for &(y, weight) in taps {
                for c in 0..3 {
                    out[c] += rows[(y * out_w + ox) * 3 + c] * weight;
                }
            }
            out[3] = 1.0;
        }
    }
    Ok(Equirect {
        pixels,
        width: out_w as u32,
        height: out_h as u32,
    })
}

/// Per output index, the (source index, weight) taps: the covered source range when shrinking,
/// the two nearest source texels when enlarging. Weights sum to 1.
fn resample_taps(src: usize, out: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f32 / out as f32;
    (0..out)
        .map(|o| {
            if scale > 1.0 {
                let start = (o as f32 * scale) as usize;
                let end = (((o + 1) as f32 * scale).ceil() as usize).clamp(start + 1, src);
                let weight = 1.0 / (end - start) as f32;
                (start..end).map(|s| (s, weight)).collect()
            } else {
                let f = ((o as f32 + 0.5) * scale - 0.5).max(0.0);
                let s0 = (f as usize).min(src - 1);
                let s1 = (s0 + 1).min(src - 1);
                let t = f - s0 as f32;
                vec![(s0, 1.0 - t), (s1, t)]
            }
        })
        .collect()
}

/// Bakes IBL maps from an Rgba32Float equirect panorama.
/// Returns (diffuse `DIFFUSE_SIZE`, specular mips from `SPECULAR_SIZE`), all Rgba32Float. The
/// BRDF LUT is environment-independent and engine-owned.
pub fn bake_all(equirect: &Equirect) -> (Vec<f32>, Vec<Vec<f32>>) {
    let diffuse = compute_irradiance(equirect, DIFFUSE_SIZE.0, DIFFUSE_SIZE.1);
    let specular_mips = bake_specular_mips(equirect);
    (diffuse, specular_mips)
}

/// Specular prefilter mip chain (5 levels from `SPECULAR_SIZE`), Rgba32Float HDR.
/// Mip i roughness = i/4, matching shader roughness * MAX_REFLECTION_LOD(4).
pub fn bake_specular_mips(equirect: &Equirect) -> Vec<Vec<f32>> {
    const MIP_ROUGHNESS: [f32; 5] = [0.04, 0.25, 0.5, 0.75, 1.0];
    let mut specular_mips: Vec<Vec<f32>> = Vec::with_capacity(MIP_ROUGHNESS.len());
    for (mip_level, &roughness) in MIP_ROUGHNESS.iter().enumerate() {
        let width = (SPECULAR_SIZE.0 >> mip_level).max(1);
        let height = (SPECULAR_SIZE.1 >> mip_level).max(1);
        specular_mips.push(compute_specular_prefilter(
            equirect, width, height, roughness,
        ));
    }
    specular_mips
}

// --- Equirect ---

/// Equirect panorama as flat RGBA f32 (linear).
pub struct Equirect {
    pub pixels: Vec<f32>,
    pub width: u32,
    pub height: u32,
}

impl Equirect {
    /// Decodes an RTEX v2 (Rgba32Float) or v1 (sRGB RGBA8) equirect into linear f32.
    pub fn from_rtex(data: &[u8]) -> Result<Self> {
        if data.len() < 16 || &data[0..4] != b"RTEX" {
            bail!("invalid RTEX");
        }
        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        let width = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let height = u32::from_le_bytes(data[12..16].try_into().unwrap());
        let bytes_per_texel = match version {
            1 => 4,
            2 => 16,
            _ => bail!("RTEX v{version} is not an equirect (expected v1 or v2)"),
        };
        let expected = width as usize * height as usize * bytes_per_texel;
        if width == 0 || height == 0 || data.len() - 16 != expected {
            bail!(
                "RTEX {width}x{height} v{version}: payload is {} bytes, expected {expected}",
                data.len() - 16
            );
        }
        let pixels = if version == 2 {
            // v2: Rgba32Float (4 bytes per channel, f32 LE)
            let (texels, _) = data[16..].as_chunks::<4>();
            texels
                .iter()
                .map(|&bytes| f32::from_le_bytes(bytes))
                .collect()
        } else {
            // v1: Rgba8UnormSrgb — decode to linear f32 at load time
            data[16..]
                .iter()
                .map(|&byte| srgb_to_linear(byte as f32 / 255.0))
                .collect()
        };
        Ok(Self {
            pixels,
            width,
            height,
        })
    }

    // Sample at a world-space direction; nearest-neighbor, returns linear HDR values.
    pub fn sample(&self, direction: [f32; 3]) -> [f32; 3] {
        let (u_coordinate, v_coordinate) = dir_to_equirect_uv(direction);
        let x = ((u_coordinate * self.width as f32) as u32).min(self.width - 1);
        let y = ((v_coordinate * self.height as f32) as u32).min(self.height - 1);
        let pixel_index = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[pixel_index],
            self.pixels[pixel_index + 1],
            self.pixels[pixel_index + 2],
        ]
    }
}

// --- Coordinate helpers ---

fn equirect_uv_to_dir(u_coordinate: f32, v_coordinate: f32) -> [f32; 3] {
    let azimuth = (u_coordinate - 0.5) * 2.0 * PI; // consistent with dir_to_equirect_uv: u = 0.5 + azimuth/(2π)
    let elevation = (0.5 - v_coordinate) * PI;
    let cos_elevation = elevation.cos();
    [
        cos_elevation * azimuth.cos(),
        elevation.sin(),
        cos_elevation * azimuth.sin(),
    ]
}

fn dir_to_equirect_uv(direction: [f32; 3]) -> (f32, f32) {
    let [x, y, z] = normalize3(direction);
    let azimuth = z.atan2(x);
    let elevation = y.clamp(-1.0, 1.0).asin();
    let u_coordinate = (0.5 + azimuth / (2.0 * PI)).rem_euclid(1.0);
    let v_coordinate = (0.5 - elevation / PI).clamp(0.0, 1.0);
    (u_coordinate, v_coordinate)
}

// --- Vector math ---

// Transform vector from tangent space (where normal=[0,0,1]) to world space.
fn tangent_to_world(vector: [f32; 3], normal: [f32; 3]) -> [f32; 3] {
    let (tangent, bitangent) = tangent_frame(normal);
    normalize3([
        tangent[0] * vector[0] + bitangent[0] * vector[1] + normal[0] * vector[2],
        tangent[1] * vector[0] + bitangent[1] * vector[1] + normal[1] * vector[2],
        tangent[2] * vector[0] + bitangent[2] * vector[1] + normal[2] * vector[2],
    ])
}

// --- Hammersley quasi-random sequence ---

fn radical_inverse_van_der_corput(mut bits: u32) -> f32 {
    bits = bits.rotate_right(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    bits as f32 * 2.328_306_4e-10 // / 2^32
}

fn hammersley(index: u32, count: u32) -> (f32, f32) {
    (
        index as f32 / count as f32,
        radical_inverse_van_der_corput(index),
    )
}

// GGX importance sampling: returns half-vector in world space around normal.
fn importance_sample_ggx(sample_point: (f32, f32), roughness: f32, normal: [f32; 3]) -> [f32; 3] {
    let alpha = roughness * roughness;
    let phi = 2.0 * PI * sample_point.0;
    let cos_theta = ((1.0 - sample_point.1) / (1.0 + (alpha * alpha - 1.0) * sample_point.1))
        .max(0.0)
        .sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let half_vector_local = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];
    tangent_to_world(half_vector_local, normal)
}

// --- Diffuse irradiance: cosine-weighted Riemann sum ---

pub fn compute_irradiance(equirect: &Equirect, output_width: u32, output_height: u32) -> Vec<f32> {
    const PHI_SAMPLE_COUNT: u32 = 64;
    const THETA_SAMPLE_COUNT: u32 = 32;
    let delta_phi = 2.0 * PI / PHI_SAMPLE_COUNT as f32;
    let delta_theta = 0.5 * PI / THETA_SAMPLE_COUNT as f32;

    let mut output = Vec::with_capacity((output_width * output_height * 4) as usize);
    for y in 0..output_height {
        for x in 0..output_width {
            let u_coordinate = (x as f32 + 0.5) / output_width as f32;
            let v_coordinate = (y as f32 + 0.5) / output_height as f32;
            let normal = equirect_uv_to_dir(u_coordinate, v_coordinate);

            let mut irradiance = [0.0f32; 3];
            let mut total_weight = 0.0f32;

            for theta_index in 0..THETA_SAMPLE_COUNT {
                for phi_index in 0..PHI_SAMPLE_COUNT {
                    let phi = (phi_index as f32 + 0.5) * delta_phi;
                    let theta = (theta_index as f32 + 0.5) * delta_theta;
                    let sin_theta = theta.sin();
                    let cos_theta = theta.cos();
                    let local = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];
                    let world = tangent_to_world(local, normal);
                    let sample = equirect.sample(world);
                    let weight = cos_theta * sin_theta;
                    irradiance[0] += sample[0] * weight;
                    irradiance[1] += sample[1] * weight;
                    irradiance[2] += sample[2] * weight;
                    total_weight += weight;
                }
            }

            // Cosine-weighted average radiance = E/π, so the shader's irradiance * albedo is Lambert
            let normalization = 1.0 / total_weight;
            output.push(irradiance[0] * normalization);
            output.push(irradiance[1] * normalization);
            output.push(irradiance[2] * normalization);
            output.push(1.0);
        }
    }
    output
}

// --- Specular prefilter: GGX importance sampling, HDR linear output ---

pub fn compute_specular_prefilter(
    equirect: &Equirect,
    output_width: u32,
    output_height: u32,
    roughness: f32,
) -> Vec<f32> {
    const SAMPLE_COUNT: u32 = 256;

    let mut output = Vec::with_capacity((output_width * output_height * 4) as usize);
    for y in 0..output_height {
        for x in 0..output_width {
            let u_coordinate = (x as f32 + 0.5) / output_width as f32;
            let v_coordinate = (y as f32 + 0.5) / output_height as f32;
            let normal = equirect_uv_to_dir(u_coordinate, v_coordinate);

            let mut color = [0.0f32; 3];
            let mut total_weight = 0.0f32;

            for sample_index in 0..SAMPLE_COUNT {
                let sample_point = hammersley(sample_index, SAMPLE_COUNT);
                let half_vector = importance_sample_ggx(sample_point, roughness, normal);
                let normal_dot_half = dot3(normal, half_vector).max(0.0);
                let light_direction = normalize3([
                    2.0 * normal_dot_half * half_vector[0] - normal[0],
                    2.0 * normal_dot_half * half_vector[1] - normal[1],
                    2.0 * normal_dot_half * half_vector[2] - normal[2],
                ]);
                let normal_dot_light = dot3(normal, light_direction).max(0.0);
                if normal_dot_light > 0.0 {
                    let sampled = equirect.sample(light_direction);
                    color[0] += sampled[0] * normal_dot_light;
                    color[1] += sampled[1] * normal_dot_light;
                    color[2] += sampled[2] * normal_dot_light;
                    total_weight += normal_dot_light;
                }
            }

            let normalization = 1.0 / total_weight.max(1e-6);
            output.push(color[0] * normalization);
            output.push(color[1] * normalization);
            output.push(color[2] * normalization);
            output.push(1.0f32);
        }
    }
    output
}

// --- BRDF LUT: GGX split-sum preintegration (Karis / UE4) ---
// Stored linear (not sRGB): R = F0 scale, G = F0 bias.

fn geometry_schlick_ggx_ibl(normal_dot_view: f32, roughness: f32) -> f32 {
    let k = roughness * roughness / 2.0;
    normal_dot_view / (normal_dot_view * (1.0 - k) + k)
}

fn geometry_smith_ibl(normal_dot_view: f32, normal_dot_light: f32, roughness: f32) -> f32 {
    geometry_schlick_ggx_ibl(normal_dot_view, roughness)
        * geometry_schlick_ggx_ibl(normal_dot_light, roughness)
}

pub fn compute_brdf_lut(width: u32, height: u32) -> Vec<f32> {
    const SAMPLE_COUNT: u32 = 1024;

    let mut output = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // x = NdotV (0=grazing, 1=normal incidence)
            // y = roughness (0=smooth, 1=rough) — y=0 is top of texture
            let normal_dot_view = (x as f32 + 0.5) / width as f32;
            let roughness = (y as f32 + 0.5) / height as f32;
            let roughness = roughness.max(0.04);

            let view_direction = [
                (1.0 - normal_dot_view * normal_dot_view).max(0.0).sqrt(),
                0.0f32,
                normal_dot_view,
            ];
            let normal = [0.0f32, 0.0, 1.0];

            let mut scale = 0.0f32;
            let mut bias = 0.0f32;

            for sample_index in 0..SAMPLE_COUNT {
                let sample_point = hammersley(sample_index, SAMPLE_COUNT);
                let half_vector = importance_sample_ggx(sample_point, roughness, normal);
                let view_dot_half = dot3(view_direction, half_vector).max(0.0);
                let light_direction = normalize3([
                    2.0 * view_dot_half * half_vector[0] - view_direction[0],
                    2.0 * view_dot_half * half_vector[1] - view_direction[1],
                    2.0 * view_dot_half * half_vector[2] - view_direction[2],
                ]);
                let normal_dot_light = light_direction[2].max(0.0); // normal = [0,0,1]
                let normal_dot_half = half_vector[2].max(0.0);

                if normal_dot_light > 0.0 {
                    let geometry_visibility =
                        geometry_smith_ibl(normal_dot_view, normal_dot_light, roughness)
                            * view_dot_half
                            / (normal_dot_half * normal_dot_view.max(0.001));
                    let fresnel_coefficient = (1.0 - view_dot_half).powi(5);
                    scale += (1.0 - fresnel_coefficient) * geometry_visibility;
                    bias += fresnel_coefficient * geometry_visibility;
                }
            }

            scale = (scale / SAMPLE_COUNT as f32).clamp(0.0, 1.0);
            bias = (bias / SAMPLE_COUNT as f32).clamp(0.0, 1.0);

            output.push(scale);
            output.push(bias);
            output.push(0.0);
            output.push(1.0);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal Radiance file: header + RLE scanlines (each channel as literal chunks of ≤ 128)
    fn hdr(width: usize, height: usize, rgbe: &[[u8; 4]]) -> Vec<u8> {
        let mut bytes =
            format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
        for row in rgbe.chunks(width) {
            bytes.extend_from_slice(&[2, 2, (width >> 8) as u8, width as u8]);
            for channel in 0..4 {
                let values: Vec<u8> = row.iter().map(|pixel| pixel[channel]).collect();
                for chunk in values.chunks(128) {
                    bytes.push(chunk.len() as u8);
                    bytes.extend_from_slice(chunk);
                }
            }
        }
        bytes
    }

    #[test]
    fn hdr_with_zero_height_is_rejected() {
        assert!(equirect_from_hdr(&hdr(4, 0, &[])).is_err());
    }

    #[test]
    fn hdr_downscale_preserves_single_bright_texel_energy() {
        let (width, height) = (4096, 2048);
        let mut rgbe = vec![[0u8; 4]; width * height];
        rgbe[1000 * width + 3001] = [128, 128, 128, 136]; // 0.5 · 2^8 = 128.0
        let source_energy = 128.0 / (width * height) as f32;
        let equirect = equirect_from_hdr(&hdr(width, height, &rgbe)).unwrap();
        let texels = (equirect.width * equirect.height) as f32;
        let energy: f32 = equirect.pixels.chunks(4).map(|p| p[0]).sum::<f32>() / texels;
        assert!(
            (energy / source_energy - 1.0).abs() < 0.01,
            "{energy} vs {source_energy}"
        );
    }

    fn rtex(version: u32, width: u32, height: u32, payload: usize) -> Vec<u8> {
        let mut bytes = b"RTEX".to_vec();
        for value in [version, width, height] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(16 + payload, 0);
        bytes
    }

    #[test]
    fn from_rtex_v4_is_rejected() {
        assert!(Equirect::from_rtex(&rtex(4, 2, 1, 32)).is_err());
    }

    #[test]
    fn from_rtex_truncated_v2_is_rejected() {
        assert!(Equirect::from_rtex(&rtex(2, 2, 1, 31)).is_err());
    }

    #[test]
    fn from_rtex_exact_v2_parses() {
        assert!(Equirect::from_rtex(&rtex(2, 2, 1, 32)).is_ok());
    }
}
