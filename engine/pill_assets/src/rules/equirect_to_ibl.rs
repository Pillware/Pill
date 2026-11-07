use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::ibl::{self, Equirect};
use crate::util::rtex::{write_rtex_hdr, write_rtex_hdr_mip};
use crate::Rule;

pub struct EquirectToIBL;

impl Rule for EquirectToIBL {
    fn name(&self) -> &'static str {
        "equirect_to_ibl"
    }

    fn input_glob(&self) -> &'static str {
        "**/*_equirect.cooked_tex"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        // Anchor output is the diffuse IBL; build() also writes specular IBL + BRDF LUT.
        let stem = input.file_stem().unwrap().to_str().unwrap();
        let base = stem.strip_suffix("_equirect").unwrap_or(stem);
        input.with_file_name(format!("{base}_diffuse_ibl.cooked_tex"))
    }

    fn build(&self, input: &Path, output: &Path) -> Result<()> {
        let data = std::fs::read(input).with_context(|| format!("read {input:?}"))?;
        let equirect = Equirect::from_rtex(&data).with_context(|| format!("{input:?}"))?;
        let directory = output.parent().unwrap();
        let stem = input.file_stem().unwrap().to_str().unwrap();
        let base = stem.strip_suffix("_equirect").unwrap_or(stem);

        // 1. Specular prefilter HDR mip chain (RTEX v4, Rgba32Float).
        let specular_path = directory.join(format!("{base}_specular_ibl.cooked_tex"));
        let (specular_width, specular_height) = ibl::SPECULAR_SIZE;
        write_rtex_hdr_mip(
            &specular_path,
            specular_width,
            specular_height,
            &ibl::bake_specular_mips(&equirect),
        )?;

        // 2. Diffuse irradiance (RTEX v2, float since bright sources exceed 1.0). Written
        // last: it is the mtime anchor, so a failure above reruns the rule next time.
        let (diffuse_width, diffuse_height) = ibl::DIFFUSE_SIZE;
        let irradiance = ibl::compute_irradiance(&equirect, diffuse_width, diffuse_height);
        write_rtex_hdr(output, diffuse_width, diffuse_height, &irradiance)
    }
}

/// Split-sum BRDF LUT (RTEX v2) from a `{stem}.brdf_lut` marker; environment-independent, baked
/// once by the engine at 32×32 (16 KB, embedded in every build) since it is smooth and filtered.
pub struct BrdfLut;

const BRDF_LUT_SIZE: u32 = 32;

impl Rule for BrdfLut {
    fn name(&self) -> &'static str {
        "brdf_lut"
    }

    fn input_glob(&self) -> &'static str {
        "**/*.brdf_lut"
    }

    fn output_for(&self, input: &Path) -> PathBuf {
        input.with_extension("cooked_tex")
    }

    fn build(&self, _input: &Path, output: &Path) -> Result<()> {
        write_rtex_hdr(
            output,
            BRDF_LUT_SIZE,
            BRDF_LUT_SIZE,
            &ibl::compute_brdf_lut(BRDF_LUT_SIZE, BRDF_LUT_SIZE),
        )
    }
}
