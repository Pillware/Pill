use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ibl::{equirect_from_hdr, EQUIRECT_SIZE};
use crate::util::rtex::write_rtex_hdr;
use crate::Rule;

/// Radiance HDR panorama → `{stem}_equirect.cooked_tex` (RTEX v2, Rgba32Float, `EQUIRECT_SIZE`),
/// resampled like the runtime path; EquirectToIBL then derives the IBL maps from it.
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
        let bytes = std::fs::read(input).with_context(|| format!("read {input:?}"))?;
        let equirect = equirect_from_hdr(&bytes).with_context(|| format!("decode {input:?}"))?;
        write_rtex_hdr(output, EQUIRECT_SIZE.0, EQUIRECT_SIZE.1, &equirect.pixels)
    }
}
