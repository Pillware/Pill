use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::util::rtex::write_rtex_hdr;
use crate::Rule;

/// Equirect size used for backgrounds and as EquirectToIBL input; 2:1 panorama.
const OUTPUT_WIDTH: u32 = 512;
const OUTPUT_HEIGHT: u32 = 256;

/// Radiance HDR panorama → `{stem}_equirect.cooked_tex` (RTEX v2, Rgba32Float, 512×256).
/// EquirectToIBL then derives the IBL maps from it.
pub struct HdrToEquirect;

impl Rule for HdrToEquirect {
    fn name(&self) -> &'static str {
        "hdr_to_equirect"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.hdr"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        let stem = input.file_stem().unwrap().to_str().unwrap();
        input.with_file_name(format!("{stem}_equirect.cooked_tex"))
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let image = image::open(input).with_context(|| format!("decode {input:?}"))?;
        let resized = image::imageops::resize(
            &image.to_rgb32f(),
            OUTPUT_WIDTH,
            OUTPUT_HEIGHT,
            image::imageops::FilterType::Lanczos3,
        );
        let pixels: Vec<f32> = resized
            .pixels()
            // Lanczos rings below zero next to very bright texels; radiance can't be negative
            .flat_map(|pixel| {
                [
                    pixel.0[0].max(0.0),
                    pixel.0[1].max(0.0),
                    pixel.0[2].max(0.0),
                    1.0,
                ]
            })
            .collect();
        write_rtex_hdr(output, OUTPUT_WIDTH, OUTPUT_HEIGHT, &pixels)
    }
}
