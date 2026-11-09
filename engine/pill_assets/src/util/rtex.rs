//! RTEX (cooked texture) writers. Header: b"RTEX" | u32 version | u32 width | u32 height | ...
//! All multi-byte integers are little-endian.

use std::path::Path;

use anyhow::Result;

use super::write_atomic;

// RTEX v2: Rgba32Float (4 bytes per channel, f32 LE), linear HDR.
pub(crate) fn write_rtex_hdr(path: &Path, width: u32, height: u32, rgba_f32: &[f32]) -> Result<()> {
    write_atomic(
        path,
        &crate::convert::rtex_hdr_bytes(width, height, rgba_f32),
    )
}

// RTEX v4: Rgba32Float mip chain for HDR specular prefilter.
pub(crate) fn write_rtex_hdr_mip(
    path: &Path,
    width: u32,
    height: u32,
    mips: &[Vec<f32>],
) -> Result<()> {
    write_atomic(
        path,
        &crate::convert::rtex_hdr_mip_bytes(width, height, mips),
    )
}

// RTEX v1: RGBA8 texels, sRGB-encoded color (linear data such as LUTs uses v2).
pub(crate) fn write_rtex(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    write_atomic(path, &crate::convert::rtex_bytes(width, height, rgba))
}
