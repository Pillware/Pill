// Bakes the IBL of a synthesized dark studio panorama (the demo's precalc/ibl.rs studio) into
// OUT_DIR as RTEX: ibl_diffuse.rtex (v2, Rgba32Float) and ibl_specular.rtex (v4, mipped).

use std::env;
use std::path::PathBuf;

use pill_assets::convert::{rtex_hdr_bytes, rtex_hdr_mip_bytes};
use pill_assets::ibl::{bake_all, Equirect, DIFFUSE_SIZE, SPECULAR_SIZE};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let (diffuse, specular_mips) = bake_all(&synthesize_studio(256, 128));

    let (diffuse_width, diffuse_height) = DIFFUSE_SIZE;
    std::fs::write(
        out_dir.join("ibl_diffuse.rtex"),
        rtex_hdr_bytes(diffuse_width, diffuse_height, &diffuse),
    )
    .unwrap();
    let (specular_width, specular_height) = SPECULAR_SIZE;
    std::fs::write(
        out_dir.join("ibl_specular.rtex"),
        rtex_hdr_mip_bytes(specular_width, specular_height, &specular_mips),
    )
    .unwrap();
}

/// Dark studio: cool sky-to-floor gradient, a warm key and a cool top-back rim. RGBA f32.
fn synthesize_studio(width: u32, height: u32) -> Equirect {
    use std::f32::consts::PI;
    let key_direction = normalize([-0.35, 0.40, -0.85]);
    let rim_direction = normalize([0.00, 0.45, 0.89]);
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            // Inverse of the shaders' dir_to_equirect_uv: u = 0.5 + azimuth/2π, v = 0.5 - elevation/π
            let azimuth = ((x as f32 + 0.5) / width as f32 - 0.5) * 2.0 * PI;
            let elevation = (0.5 - (y as f32 + 0.5) / height as f32) * PI;
            let direction = [
                elevation.cos() * azimuth.cos(),
                elevation.sin(),
                elevation.cos() * azimuth.sin(),
            ];
            let up = direction[1].max(0.0);
            let mut color = [0.022 + 0.085 * up, 0.030 + 0.110 * up, 0.045 + 0.150 * up];
            let key_cosine = dot(direction, key_direction).max(0.0);
            let key = key_cosine.powf(8.0) * 1.5;
            let core = key_cosine.powf(220.0) * 8.0;
            let rim = dot(direction, rim_direction).max(0.0).powf(7.0) * 3.0;
            color[0] += key + core + rim * 0.38;
            color[1] += key * 0.90 + core * 0.95 + rim * 0.55;
            color[2] += key * 0.72 + core * 0.85 + rim;
            pixels.extend_from_slice(&[color[0], color[1], color[2], 1.0]);
        }
    }
    Equirect {
        pixels,
        width,
        height,
    }
}

fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = dot(vector, vector).sqrt().max(1e-8);
    [vector[0] / length, vector[1] / length, vector[2] / length]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
