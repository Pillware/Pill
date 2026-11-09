// Procedural pill textures (from the demo's tex_gen.rs), generated at startup.
// Pixel functions operate in object space (mesh-interpolated 3D surface position), not
// texture-space UV, so patterns follow the pill's actual geometry.
//
// Output format: RTEX v1 (b"RTEX" | u32 version=1 | u32 width | u32 height | raw RGBA bytes)
// This is what Texture::from_bytes() expects.

use crate::mesh::make_pill_mesh_bytes;
use std::sync::OnceLock;

const SIZE: u32 = 512;

pub(crate) const CAP_STACKS: u32 = 12;
pub(crate) const SLICES: u32 = 24;
pub(crate) const HEIGHT: f32 = 4.0;
pub(crate) const EXTRUDE: f32 = 0.01;

// --- Public generators ---

pub(crate) fn roughness() -> Vec<u8> {
    gen_tex(SIZE, [255, 64, 255, 255], roughness_pixel)
}

pub(crate) fn albedo() -> Vec<u8> {
    gen_tex(SIZE, [128, 128, 128, 255], albedo_pixel)
}

pub(crate) fn normal_map() -> Vec<u8> {
    gen_tex(SIZE, [128, 128, 255, 255], normal_pixel)
}

// --- Object-space pixel functions (obj = mesh surface position, axis/split = long-axis split) ---

fn albedo_pixel(obj: [f32; 3], axis: usize, split: f32) -> [u8; 4] {
    let base = if obj[axis] < split {
        [0.10f32, 0.10, 0.72] // cobalt
    } else {
        [0.42f32, 0.04, 0.04] // dark crimson
    };
    // High noise = greasy = smoother + brighter albedo (grease fills micro-scratches,
    // locally increasing reflectance). Same FBM as roughness_pixel.
    let noise = fbm([obj[0] * 2.0, obj[1] * 2.0, obj[2] * 2.0], 3);
    let bright_amp = if obj[axis] < split { 0.20 } else { 0.35 };
    let brighten = 1.0 + noise * bright_amp;
    [
        ((base[0] * brighten).clamp(0.0, 1.0) * 255.0) as u8,
        ((base[1] * brighten).clamp(0.0, 1.0) * 255.0) as u8,
        ((base[2] * brighten).clamp(0.0, 1.0) * 255.0) as u8,
        255,
    ]
}

fn roughness_pixel(obj: [f32; 3], _axis: usize, _split: f32) -> [u8; 4] {
    let p0 = [obj[0] * 2.0, obj[1] * 2.0, obj[2] * 2.0];
    let r0 = smoothstep(fbm(p0, 1), 0.1, 0.9);
    let p1 = [obj[0] * 5.11, obj[1] * 5.11, obj[2] * 5.11];
    let r1 = smoothstep(fbm(p1, 5), 0.2, 0.9);
    let roughness = ((2.0 * r0).clamp(0.0, 1.0) * 1.5 * r1).clamp(0.0, 1.0);
    // glTF metallic-roughness: G = roughness, B = metallic (full, so Material::metallic applies)
    [255, (roughness * 255.0) as u8, 255, 255]
}

fn normal_pixel(obj: [f32; 3], axis: usize, split: f32) -> [u8; 4] {
    if obj[axis] >= split {
        return [128, 128, 255, 255];
    }
    let p = [obj[0] * 4.0, obj[1] * 4.0, obj[2] * 4.0];
    let amp = 0.015f32;
    let [gx, gy, _] = fbm_grad(p, 3);
    let nx = (-gx * amp).clamp(-1.0, 1.0);
    let ny = (-gy * amp).clamp(-1.0, 1.0);
    let nz = (1.0 - nx * nx - ny * ny).sqrt().max(0.0);
    [
        ((nx * 0.5 + 0.5) * 255.0) as u8,
        ((ny * 0.5 + 0.5) * 255.0) as u8,
        ((nz * 0.5 + 0.5) * 255.0) as u8,
        255,
    ]
}

fn smoothstep(v: f32, a: f32, b: f32) -> f32 {
    let x = ((v - a) / (b - a)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

// --- Rasterizer (rasterizes the actual pill mesh into object-space pixel positions) ---

/// Pill vertices, indices, long axis and its split point.
type PillMesh = (Vec<[f32; 14]>, Vec<u32>, usize, f32);

static PILL_MESH: OnceLock<PillMesh> = OnceLock::new();

fn gen_tex(
    size: u32,
    default_rgba: [u8; 4],
    pixel_fn: impl Fn([f32; 3], usize, f32) -> [u8; 4],
) -> Vec<u8> {
    let (verts, indices, axis, split) = PILL_MESH.get_or_init(|| {
        let mesh = make_pill_mesh_bytes(CAP_STACKS, SLICES, HEIGHT, EXTRUDE);
        let (verts, indices) = parse_rmsh(&mesh);
        let (axis, split) = bounding_axis_split(&verts);
        (verts, indices, axis, split)
    });
    let (axis, split) = (*axis, *split);

    let mut rgba = vec![0u8; (size * size) as usize * 4];
    for px in rgba.chunks_exact_mut(4) {
        px.copy_from_slice(&default_rgba);
    }

    let n_tris = indices.len() / 3;
    for t in 0..n_tris {
        let i = [
            indices[t * 3] as usize,
            indices[t * 3 + 1] as usize,
            indices[t * 3 + 2] as usize,
        ];
        let uv = [
            [verts[i[0]][3], verts[i[0]][4]],
            [verts[i[1]][3], verts[i[1]][4]],
            [verts[i[2]][3], verts[i[2]][4]],
        ];
        let pos = [
            [verts[i[0]][0], verts[i[0]][1], verts[i[0]][2]],
            [verts[i[1]][0], verts[i[1]][1], verts[i[1]][2]],
            [verts[i[2]][0], verts[i[2]][1], verts[i[2]][2]],
        ];
        rasterize_triangle(&mut rgba, size, uv, pos, |obj| pixel_fn(obj, axis, split));
    }

    let mut out = Vec::with_capacity(16 + rgba.len());
    out.extend_from_slice(b"RTEX");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&rgba);
    out
}

fn parse_rmsh(data: &[u8]) -> (Vec<[f32; 14]>, Vec<u32>) {
    let vtx_count = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let idx_count = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;
    let mut off = 16usize;
    let mut verts = Vec::with_capacity(vtx_count);
    for _ in 0..vtx_count {
        let mut v = [0f32; 14];
        for f in v.iter_mut() {
            *f = f32::from_le_bytes(data[off..off + 4].try_into().unwrap());
            off += 4;
        }
        verts.push(v);
    }
    let mut indices = Vec::with_capacity(idx_count);
    for _ in 0..idx_count {
        indices.push(u32::from_le_bytes(data[off..off + 4].try_into().unwrap()));
        off += 4;
    }
    (verts, indices)
}

// Long axis + split point from the bounding box (Y is dominant, split ≈ 0).
fn bounding_axis_split(verts: &[[f32; 14]]) -> (usize, f32) {
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in verts {
        for a in 0..3 {
            mn[a] = mn[a].min(v[a]);
            mx[a] = mx[a].max(v[a]);
        }
    }
    let spans = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
    let axis = spans
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .unwrap()
        .0;
    (axis, (mn[axis] + mx[axis]) * 0.5)
}

fn rasterize_triangle<F>(
    rgba: &mut [u8],
    size: u32,
    uv: [[f32; 2]; 3],
    pos: [[f32; 3]; 3],
    pixel_fn: F,
) where
    F: Fn([f32; 3]) -> [u8; 4],
{
    let rf = size as f32;
    // RMSH stores pre-flipped V (engine convention). Un-flip to get image-space rows (top=0).
    let to_px = |u: [f32; 2]| -> [f32; 2] { [u[0] * rf, (1.0 - u[1]) * rf] };
    let p = [to_px(uv[0]), to_px(uv[1]), to_px(uv[2])];

    let min_x = (p[0][0].min(p[1][0]).min(p[2][0]).floor().max(0.0) as u32).min(size - 1);
    let max_x = (p[0][0].max(p[1][0]).max(p[2][0]).ceil() as u32).min(size - 1);
    let min_y = (p[0][1].min(p[1][1]).min(p[2][1]).floor().max(0.0) as u32).min(size - 1);
    let max_y = (p[0][1].max(p[1][1]).max(p[2][1]).ceil() as u32).min(size - 1);

    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if let Some(bary) = barycentric([x as f32 + 0.5, y as f32 + 0.5], p[0], p[1], p[2]) {
                let obj = [
                    bary[0] * pos[0][0] + bary[1] * pos[1][0] + bary[2] * pos[2][0],
                    bary[0] * pos[0][1] + bary[1] * pos[1][1] + bary[2] * pos[2][1],
                    bary[0] * pos[0][2] + bary[1] * pos[1][2] + bary[2] * pos[2][2],
                ];
                let idx = ((y * size + x) * 4) as usize;
                rgba[idx..idx + 4].copy_from_slice(&pixel_fn(obj));
            }
        }
    }
}

fn barycentric(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<[f32; 3]> {
    let edge = |p1: [f32; 2], p2: [f32; 2], q: [f32; 2]| -> f32 {
        (p2[0] - p1[0]) * (q[1] - p1[1]) - (p2[1] - p1[1]) * (q[0] - p1[0])
    };
    let denom = edge(a, b, c);
    if denom.abs() < f32::EPSILON {
        return None;
    }
    let wa = edge(b, c, p) / denom;
    let wb = edge(c, a, p) / denom;
    let wc = edge(a, b, p) / denom;
    if wa >= 0.0 && wb >= 0.0 && wc >= 0.0 {
        Some([wa, wb, wc])
    } else {
        None
    }
}

// --- Noise ---

fn fbm(p: [f32; 3], octaves: u32) -> f32 {
    let mut v = 0.0f32;
    let mut amp = 0.5f32;
    let mut q = p;
    for _ in 0..octaves {
        v += amp * value_noise3(q);
        q = [q[0] * 2.02, q[1] * 2.02, q[2] * 2.02];
        amp *= 0.5;
    }
    v
}

fn fbm_grad(p: [f32; 3], octaves: u32) -> [f32; 3] {
    let h = 0.005f32;
    let v = fbm(p, octaves);
    [
        (fbm([p[0] + h, p[1], p[2]], octaves) - v) / h,
        (fbm([p[0], p[1] + h, p[2]], octaves) - v) / h,
        (fbm([p[0], p[1], p[2] + h], octaves) - v) / h,
    ]
}

fn value_noise3(p: [f32; 3]) -> f32 {
    let ix = p[0].floor() as i32;
    let iy = p[1].floor() as i32;
    let iz = p[2].floor() as i32;
    let fx = p[0] - ix as f32;
    let fy = p[1] - iy as f32;
    let fz = p[2] - iz as f32;
    let ux = fx * fx * (3.0 - 2.0 * fx);
    let uy = fy * fy * (3.0 - 2.0 * fy);
    let uz = fz * fz * (3.0 - 2.0 * fz);

    let hash = |x: i32, y: i32, z: i32| -> f32 {
        let n = x
            .wrapping_mul(1619)
            .wrapping_add(y.wrapping_mul(31337))
            .wrapping_add(z.wrapping_mul(1013904223));
        let n = n.wrapping_mul(n.wrapping_mul(n).wrapping_mul(60493));
        (n as f32) / (i32::MAX as f32) * 0.5 + 0.5
    };
    let lerp = |a: f32, b: f32, t: f32| a + t * (b - a);

    lerp(
        lerp(
            lerp(hash(ix, iy, iz), hash(ix + 1, iy, iz), ux),
            lerp(hash(ix, iy + 1, iz), hash(ix + 1, iy + 1, iz), ux),
            uy,
        ),
        lerp(
            lerp(hash(ix, iy, iz + 1), hash(ix + 1, iy, iz + 1), ux),
            lerp(hash(ix, iy + 1, iz + 1), hash(ix + 1, iy + 1, iz + 1), ux),
            uy,
        ),
        uz,
    )
}
