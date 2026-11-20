//! The post-processing half of the PBR frame: bloom, its composite, the tonemap
//! and the lens pass that writes the surface.
//!
//! # Responsibilities
//!
//! - Declare the four fullscreen passes the frame ends with, and the shaders
//!   they draw through ([`install`]).
//! - Own the offscreen target names the passes hand between each other, and the
//!   generated grain tile the last one samples.
//!
//! # Design
//!
//! Four passes in a row, each reading a target an earlier pass wrote:
//!
//! ```text
//! bloom      reads hdr         → bloom    what is brighter than the threshold, at half size
//! composite  reads hdr, bloom  → lit      bloom added back over the lit image
//! tonemap    reads lit         → ldr      the range brought down for the display
//! lens       reads ldr         → surface  grain, grade and vignette
//! ```
//!
//! The first of them reads [`HDR_TARGET`], which the frame's geometry pass
//! writes. That name is declared here rather than there because the passes that
//! consume it live here, and the geometry pass only writes it.
//!
//! # Where the shaders are
//!
//! In `shader_sources/post_processing/shaders/`: the fullscreen vertex stage
//! each pass here draws through, and the four fragment stages. The fragment
//! stages `#include` nothing. The vertex stage includes `common.hlsl`, which
//! lives in `shader_sources/common_shaders/include/`.

// External crates
use pill_engine::{AssetGuid, AssetManager, Handle};

// Current crate
use crate::assets::{MaterialParameter, PassKind, PassTarget, RenderPass, Texture, TextureType};
use crate::config::ShaderSourceRecord;

// =============================================================================
// Constants
// =============================================================================

/// Name of the target the frame's geometry pass writes, and the first pass here
/// reads.
pub(super) const HDR_TARGET: &str = "hdr";
/// Target the prefilter writes and the composite reads.
const BLOOM_TARGET: &str = "bloom";
/// Target the composite writes and the tonemap reads.
const LIT_TARGET: &str = "lit";
/// Target the tonemap writes and the lens pass reads.
const LDR_TARGET: &str = "ldr";

/// Asset name of the generated grain tile.
pub const GRAIN_TEXTURE_NAME: &str = "pill.pbr.grain";

/// The cooked fullscreen vertex stage every pass here draws through.
const FULLSCREEN_VERTEX: &str = "post_processing/shaders/fullscreen_vertex.wgsl";

/// The bloom prefilter's fragment stage, named twice: once as the `.hlsl`
/// source path it is imported from, and once as the cooked file beside it.
const PREFILTER_SHADER: &str = "post_processing/shaders/bloom_prefilter_fragment.hlsl";
const PREFILTER_FRAGMENT: &str = "post_processing/shaders/bloom_prefilter_fragment.wgsl";
/// The bloom prefilter shader's guid, which its meta repeats.
const PREFILTER_GUID: AssetGuid = AssetGuid::from_name("pill.pbr.shader.bloom.prefilter");

/// The bloom composite's fragment stage.
const COMPOSITE_SHADER: &str = "post_processing/shaders/bloom_composite_fragment.hlsl";
const COMPOSITE_FRAGMENT: &str = "post_processing/shaders/bloom_composite_fragment.wgsl";
/// The bloom composite shader's guid, which its meta repeats.
const COMPOSITE_GUID: AssetGuid = AssetGuid::from_name("pill.pbr.shader.bloom.composite");

/// The tonemap's fragment stage.
const TONEMAP_SHADER: &str = "post_processing/shaders/tonemap_fragment.hlsl";
const TONEMAP_FRAGMENT: &str = "post_processing/shaders/tonemap_fragment.wgsl";
/// The tonemap shader's guid, which its meta repeats.
const TONEMAP_GUID: AssetGuid = AssetGuid::from_name("pill.pbr.shader.tonemap");

/// The lens pass's fragment stage.
const LENS_SHADER: &str = "post_processing/shaders/lens_fragment.hlsl";
const LENS_FRAGMENT: &str = "post_processing/shaders/lens_fragment.wgsl";
/// The lens shader's guid, which its meta repeats.
const LENS_GUID: AssetGuid = AssetGuid::from_name("pill.pbr.shader.lens");

/// The shader assets [`install`] creates, and the files each is built from.
pub const SHADER_SOURCES: &[ShaderSourceRecord] = &[
    ShaderSourceRecord {
        shader_asset_name: PREFILTER_SHADER,
        vertex_path: FULLSCREEN_VERTEX,
        fragment_path: PREFILTER_FRAGMENT,
    },
    ShaderSourceRecord {
        shader_asset_name: COMPOSITE_SHADER,
        vertex_path: FULLSCREEN_VERTEX,
        fragment_path: COMPOSITE_FRAGMENT,
    },
    ShaderSourceRecord {
        shader_asset_name: TONEMAP_SHADER,
        vertex_path: FULLSCREEN_VERTEX,
        fragment_path: TONEMAP_FRAGMENT,
    },
    ShaderSourceRecord {
        shader_asset_name: LENS_SHADER,
        vertex_path: FULLSCREEN_VERTEX,
        fragment_path: LENS_FRAGMENT,
    },
];

/// Asset name of the bloom prefilter pass.
const BLOOM_PASS: &str = "pill.pbr.pass.bloom";
/// Asset name of the bloom composite pass.
const COMPOSITE_PASS: &str = "pill.pbr.pass.bloom.composite";
/// Asset name of the tonemap pass.
const TONEMAP_PASS: &str = "pill.pbr.pass.tonemap";
/// Asset name of the lens pass.
const LENS_PASS: &str = "pill.pbr.pass.lens";

// =============================================================================
// The Chain's Look
// =============================================================================

// Every value below is a look choice. They are named so the frame's look can be
// read and tuned from this file. The engine gives every parameter one 16-byte
// slot, so a pass with unrelated scalars carries them in one `Color`, in the
// order its shader's `MaterialParams` declares them. Each constant that fills a
// slot is named after the field it fills.

/// Brightness a pixel has to exceed before it blooms.
const BLOOM_THRESHOLD: f32 = 1.0;
/// How softly the bloom ramps in above [`BLOOM_THRESHOLD`].
const BLOOM_KNEE: f32 = 0.6;
/// How much of the bloom is added back over the lit image.
const BLOOM_STRENGTH: f32 = 0.6;

/// The tonemap curve's contrast exponent.
const TONEMAP_CONTRAST: f32 = 1.6;
/// The exponent that shapes how the tonemap bends its highlights down.
const TONEMAP_SHOULDER: f32 = 0.977;
/// The range the display has to fit, in scene units.
const HDR_MAX: f32 = 8.0;
/// Where mid grey enters the curve, in scene units.
const MID_GREY_IN: f32 = 0.18;
/// The display value that mid grey is mapped to.
const MID_GREY_OUT: f32 = 0.267;

/// Barrel distortion. A negative value bows the sides outward; a positive
/// value pinches them inward.
const LENS_BARREL_K1: f32 = -0.06;
/// How far the vignette pulls the corners down.
///
/// The shader multiplies the radius by this value before the `smoothstep`. At
/// 1.0 the whole falloff fits inside the frame and the corners go black. At 0.5
/// the falloff stops short of the corners: they lose about 40 percent of their
/// light, and the edges almost none.
const LENS_VIGNETTE_STRENGTH: f32 = 0.5;
/// How far red and blue split at the edge, as a scale of the radius squared.
const LENS_CHROMATIC_ABERRATION: f32 = 0.004;
/// Per-channel exponent applied before the grade's ramp.
const LENS_GAMMA: [f32; 3] = [1.0, 1.0, 1.0];
/// The grade's ramp: where it starts, where it ends, and how much of the graded
/// image is mixed in. A descending ramp inverts the grade.
const LENS_GRADE: [f32; 3] = [1.0, 0.0, 0.0];
/// Grain amplitude.
///
/// At zero, the shader's `min(colour + black_limit, grain_amount)` gives no
/// offset, so the tile is sampled and ignored. Grain is off because the pattern
/// is dense at 1:1 pixels and reads as noise on a still image. The rest of the
/// pass stays in place, so turning grain back on means changing this number
/// alone.
const LENS_GRAIN_AMPLITUDE: f32 = 0.0;
/// Screen pixels per grain texel.
const GRAIN_PIXELS_PER_TEXEL: f32 = 1.0;

/// Edge of the grain tile, in texels.
///
/// The square tile is generated at this size, so one number sets both sides.
/// The lens pass also receives the size as a parameter, so the shader can map
/// screen pixels into the tile without repeating a literal of its own.
const GRAIN_TILE_SIZE: u32 = 256;

// =============================================================================
// Free Functions
// =============================================================================

/// Install the four post-processing passes.
///
/// The passes come back in the order the frame runs them. The grain tile the
/// last pass samples is not returned: the lens pass already holds that texture,
/// and a second handle would let the two disagree.
///
/// A store that already holds all four passes gets the same handles back
/// instead of a second copy, so this is safe to call once per generation.
///
/// # Errors
///
/// Returns an error when the store already holds an asset of the same kind
/// under one of these names, or when one of the shaders fails to build.
pub fn install(
    assets: &mut AssetManager,
) -> Result<Vec<Handle<RenderPass>>, Box<dyn std::error::Error>> {
    // Step 1: Hand back the passes when the store already holds all four.
    if let (Some(bloom_pass), Some(composite_pass), Some(tonemap_pass), Some(lens_pass)) = (
        assets.handle_by_name::<RenderPass>(BLOOM_PASS),
        assets.handle_by_name::<RenderPass>(COMPOSITE_PASS),
        assets.handle_by_name::<RenderPass>(TONEMAP_PASS),
        assets.handle_by_name::<RenderPass>(LENS_PASS),
    ) {
        return Ok(vec![bloom_pass, composite_pass, tonemap_pass, lens_pass]);
    }

    // Step 2: Generate the grain tile the lens pass samples. The pattern only
    // has to make neighbouring texels differ, not be random, and generating it
    // keeps a binary out of the repository.
    let grain_texture_handle = assets.add_named(GRAIN_TEXTURE_NAME, grain_texture())?;

    // Step 3: Build the four passes in the order the frame runs them.
    let prefilter_shader = crate::config::import_shader(assets, PREFILTER_SHADER, PREFILTER_GUID)?;
    let bloom_pass = RenderPass::new(BLOOM_PASS)
        .with_shader(prefilter_shader)
        .with_kind(PassKind::Fullscreen)
        .with_input("hdr", HDR_TARGET)
        .with_parameter(
            "threshold",
            MaterialParameter::Color([BLOOM_THRESHOLD, BLOOM_KNEE, 0.0]),
        )
        .with_target(PassTarget::Offscreen(BLOOM_TARGET.to_owned()))
        // Half the surface on each side costs a quarter of the pixels, and that
        // saving is why a pass can ask for its own target scale.
        .with_target_scale(2)
        .with_order(1);
    let bloom_pass = assets.add_named_with_guid(
        BLOOM_PASS,
        pill_engine::AssetGuid::from_name(BLOOM_PASS),
        bloom_pass,
    )?;

    let composite_shader = crate::config::import_shader(assets, COMPOSITE_SHADER, COMPOSITE_GUID)?;
    let composite_pass = RenderPass::new(COMPOSITE_PASS)
        .with_shader(composite_shader)
        .with_kind(PassKind::Fullscreen)
        .with_input("hdr", HDR_TARGET)
        .with_input("bloom", BLOOM_TARGET)
        .with_parameter(
            "bloom",
            MaterialParameter::Color([BLOOM_STRENGTH, 0.0, 0.0]),
        )
        .with_target(PassTarget::Offscreen(LIT_TARGET.to_owned()))
        .with_order(2);
    let composite_pass = assets.add_named_with_guid(
        COMPOSITE_PASS,
        pill_engine::AssetGuid::from_name(COMPOSITE_PASS),
        composite_pass,
    )?;

    // Work out the two curve constants from the look values above: an artist
    // tunes the look, and the constants follow.
    let (b, c) = lottes_bc(
        TONEMAP_CONTRAST,
        TONEMAP_SHOULDER,
        HDR_MAX,
        MID_GREY_IN,
        MID_GREY_OUT,
    );
    let tonemap_shader = crate::config::import_shader(assets, TONEMAP_SHADER, TONEMAP_GUID)?;
    let tonemap_pass = RenderPass::new(TONEMAP_PASS)
        .with_shader(tonemap_shader)
        .with_kind(PassKind::Fullscreen)
        .with_input("hdr", LIT_TARGET)
        .with_parameter("contrast", MaterialParameter::Scalar(TONEMAP_CONTRAST))
        .with_parameter("shoulder", MaterialParameter::Scalar(TONEMAP_SHOULDER))
        .with_parameter("b", MaterialParameter::Scalar(b))
        .with_parameter("c", MaterialParameter::Scalar(c))
        .with_target(PassTarget::Offscreen(LDR_TARGET.to_owned()))
        .with_order(3);
    let tonemap_pass = assets.add_named_with_guid(
        TONEMAP_PASS,
        pill_engine::AssetGuid::from_name(TONEMAP_PASS),
        tonemap_pass,
    )?;

    // The last pass writes the surface, so the chain ends here.
    let lens_shader = crate::config::import_shader(assets, LENS_SHADER, LENS_GUID)?;
    let lens_pass = RenderPass::new(LENS_PASS)
        .with_shader(lens_shader)
        .with_kind(PassKind::Fullscreen)
        .with_input("source", LDR_TARGET)
        .with_texture("grain", grain_texture_handle)
        .with_parameter(
            "shape",
            MaterialParameter::Color([
                LENS_BARREL_K1,
                LENS_VIGNETTE_STRENGTH,
                LENS_CHROMATIC_ABERRATION,
            ]),
        )
        .with_parameter(
            "gamma",
            MaterialParameter::Color([LENS_GAMMA[0], LENS_GAMMA[1], LENS_GAMMA[2]]),
        )
        .with_parameter(
            "grade",
            MaterialParameter::Color([LENS_GRADE[0], LENS_GRADE[1], LENS_GRADE[2]]),
        )
        .with_parameter(
            "grain",
            MaterialParameter::Color([
                LENS_GRAIN_AMPLITUDE,
                GRAIN_PIXELS_PER_TEXEL,
                GRAIN_TILE_SIZE as f32,
            ]),
        )
        .with_target(PassTarget::Surface)
        .with_order(4);
    let lens_pass = assets.add_named_with_guid(
        LENS_PASS,
        pill_engine::AssetGuid::from_name(LENS_PASS),
        lens_pass,
    )?;

    Ok(vec![bloom_pass, composite_pass, tonemap_pass, lens_pass])
}

/// Solve the two constants that define the shape of Lottes' tonemap.
///
/// `b` and `c` are solved rather than authored: an artist says where mid grey
/// should be on the display and how much highlight the display has to fit, and
/// these two follow. See Tim Lottes' GDC 2016 talk on HDR color pipelines; the
/// fragment shader's own `lottes` function evaluates the curve these place.
fn lottes_bc(
    contrast: f32,
    shoulder: f32,
    hdr_max: f32,
    mid_grey_in: f32,
    mid_grey_out: f32,
) -> (f32, f32) {
    let hdr_a = hdr_max.powf(contrast);
    let mid_a = mid_grey_in.powf(contrast);
    let hdr_ad = hdr_a.powf(shoulder);
    let mid_ad = mid_a.powf(shoulder);
    let denominator = (hdr_ad - mid_ad) * mid_grey_out;
    let b = (-mid_a + hdr_a * mid_grey_out) / denominator;
    let c = (hdr_ad * mid_a - hdr_a * mid_ad * mid_grey_out) / denominator;
    (b, c)
}

/// A [`GRAIN_TILE_SIZE`] square grain tile, generated rather than shipped.
///
/// Each texel gets a level from an integer hash. Neighbouring texels only have
/// to differ, not be truly random, so this builds the tile at install time and
/// the crate needs no binary for it.
fn grain_texture() -> Texture {
    const SIZE: u32 = GRAIN_TILE_SIZE;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);

    for y in 0..SIZE {
        for x in 0..SIZE {
            // A cheap integer hash standing in for noise.
            let mut hash = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B);
            hash ^= hash >> 15;
            hash = hash.wrapping_mul(0x2545_F491);
            hash ^= hash >> 13;
            let noise_level = (hash & 0xFF) as u8;
            rgba.extend_from_slice(&[noise_level, noise_level, noise_level, 255]);
        }
    }

    Texture::from_rgba(GRAIN_TEXTURE_NAME, TextureType::Color, rgba, SIZE, SIZE)
        .expect("the tile is SIZE x SIZE RGBA")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_solved_curve_constants_are_positive_and_are_the_reference_values() {
        let (b, c) = lottes_bc(
            TONEMAP_CONTRAST,
            TONEMAP_SHOULDER,
            HDR_MAX,
            MID_GREY_IN,
            MID_GREY_OUT,
        );

        assert!(
            b > 0.0 && c > 0.0,
            "the curve needs both constants positive"
        );
        // The reference values for these art parameters, given to three decimal
        // places. A change here means the solved curve moved; a rounding
        // difference stays inside the 0.002 tolerance.
        assert!((b - 1.073).abs() < 0.002, "b came out {b}");
        assert!((c - 0.168).abs() < 0.002, "c came out {c}");
    }
}
