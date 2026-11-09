//! RTEX (cooked texture) writers. Header: b"RTEX" | u32 version | u32 width | u32 height | ...
//! All multi-byte integers are little-endian.

use std::path::Path;

use anyhow::Result;

use super::write_atomic;

// RTEX v2: Rgba32Float (4 bytes per channel, f32 LE), linear HDR.
pub(crate) fn write_rtex_hdr(path: &Path, width: u32, height: u32, rgba_f32: &[f32]) -> Result<()> {
    let mut output = Vec::with_capacity(16 + rgba_f32.len() * 4);
    output.extend_from_slice(b"RTEX");
    output.extend_from_slice(&2u32.to_le_bytes()); // version 2 = Rgba32Float HDR
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    for &value in rgba_f32 {
        output.extend_from_slice(&value.to_le_bytes());
    }
    write_atomic(path, &output)
}

// RTEX v4: Rgba32Float mip chain for HDR specular prefilter.
// Header: RTEX | 4 | w | h | mip_count | mip0_f32_pixels | mip1_f32_pixels | ...
pub(crate) fn write_rtex_hdr_mip(
    path: &Path,
    width: u32,
    height: u32,
    mips: &[Vec<f32>],
) -> Result<()> {
    let mut output = Vec::new();
    output.extend_from_slice(b"RTEX");
    output.extend_from_slice(&4u32.to_le_bytes());
    output.extend_from_slice(&width.to_le_bytes());
    output.extend_from_slice(&height.to_le_bytes());
    output.extend_from_slice(&(mips.len() as u32).to_le_bytes());
    for mip in mips {
        for &value in mip {
            output.extend_from_slice(&value.to_le_bytes());
        }
    }
    write_atomic(path, &output)
}

// RTEX v1: RGBA8 texels, sRGB-encoded color (linear data such as LUTs uses v2).
pub(crate) fn write_rtex(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    write_atomic(path, &crate::convert::rtex_bytes(width, height, rgba))
}
