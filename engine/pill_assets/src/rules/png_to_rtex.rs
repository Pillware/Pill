use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::Rule;

/// PNG → RTEX pre-decoded texture.
///
/// Format: `b"RTEX" | u32 version=1 | u32 width | u32 height | raw RGBA bytes`
/// All multi-byte integers are little-endian.
pub struct PngToRtex;

impl Rule for PngToRtex {
    fn name(&self) -> &'static str {
        "png_to_rtex"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.png"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("rtex")
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let bytes = std::fs::read(input).with_context(|| format!("read {input:?}"))?;
        let mut decoder = png::Decoder::new(bytes.as_slice());
        // Expand palette/indexed images to RGB and low-bit depths to 8-bit.
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().context("png read_info")?;
        let mut buf = vec![0u8; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).context("png next_frame")?;
        let width = info.width;
        let height = info.height;
        let raw = &buf[..info.buffer_size()];

        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => raw.to_vec(),
            png::ColorType::Rgb => raw
                .chunks(3)
                .flat_map(|p| [p[0], p[1], p[2], 255])
                .collect(),
            png::ColorType::Grayscale => raw.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => raw
                .chunks(2)
                .flat_map(|p| [p[0], p[0], p[0], p[1]])
                .collect(),
            _ => bail!("unsupported PNG color type: {:?}", info.color_type),
        };

        let mut out = Vec::with_capacity(16 + rgba.len());
        out.extend_from_slice(b"RTEX");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&width.to_le_bytes());
        out.extend_from_slice(&height.to_le_bytes());
        out.extend_from_slice(&rgba);

        std::fs::write(output, &out).with_context(|| format!("write {output:?}"))?;
        Ok(())
    }
}
