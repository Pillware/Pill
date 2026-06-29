use crate::constants::{CUBE_SPACING, GRID_SIZE};
use pill_engine::project::*;
use std::sync::Mutex;

// ── Sync helpers ─────────────────────────────────────────────────────────

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ── Colour ───────────────────────────────────────────────────────────────

pub fn to_grayscale(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

// ── PNG decoding ─────────────────────────────────────────────────────────

pub fn decode_height_map(png_bytes: &[u8]) -> Result<Vec<f32>> {
    let mut decoder = png::Decoder::new(png_bytes);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let raw = &buf[..info.buffer_size()];

    let count = info.width as usize * info.height as usize;
    let mut heights = Vec::with_capacity(count);

    match info.color_type {
        png::ColorType::Grayscale => {
            for &g in raw.iter().take(count) {
                heights.push(g as f32 / 255.0);
            }
        }
        png::ColorType::Rgb => {
            for px in raw.chunks(3).take(count) {
                heights.push(to_grayscale(px[0], px[1], px[2]));
            }
        }
        png::ColorType::Rgba => {
            for px in raw.chunks(4).take(count) {
                heights.push(to_grayscale(px[0], px[1], px[2]));
            }
        }
        _ => heights.resize(count, 0.0),
    }

    Ok(heights)
}

pub fn sample_height(map: &[f32], w: u32, h: u32, gx: usize, gz: usize) -> f32 {
    let px = (gx as f32 / (GRID_SIZE - 1) as f32 * (w - 1) as f32) as usize;
    let py = (gz as f32 / (GRID_SIZE - 1) as f32 * (h - 1) as f32) as usize;
    map.get(py * w as usize + px).copied().unwrap_or(0.0)
}

// ── Formatting ───────────────────────────────────────────────────────────

pub fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

// ── Grid math ────────────────────────────────────────────────────────────

pub fn world_to_grid(world_x: f32, world_z: f32) -> (isize, isize) {
    let half = (GRID_SIZE as f32 * CUBE_SPACING) / 2.0;
    let gx = ((world_x + half) / CUBE_SPACING) as isize;
    let gz = ((world_z + half) / CUBE_SPACING) as isize;
    (gx, gz)
}

pub fn in_circle(gx: usize, gz: usize, cgx: isize, cgz: isize, radius: f32) -> bool {
    let dx = gx as isize - cgx;
    let dz = gz as isize - cgz;
    ((dx * dx + dz * dz) as f32).sqrt() <= radius
}
