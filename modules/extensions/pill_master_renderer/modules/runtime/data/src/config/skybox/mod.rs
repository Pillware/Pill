//! The skybox: the two shaders a sky material draws through, and the pass that
//! draws the sky.
//!
//! # Responsibilities
//!
//! - Install the equirect and cubemap skybox shaders under fixed names and
//!   guids ([`install_shaders`]), so a `.material` file can name them.
//! - Build the pass that draws a sky material behind the scene
//!   ([`skybox_pass`]), and give the values a sky material's parameters start
//!   from ([`default_parameters`]).
//!
//! # Design
//!
//! A sky is a material, not a special asset: an ordinary `.material` file whose
//! shader is one of the two here and whose `sky` slot holds an
//! [`Equirect`](crate::TextureType::Equirect) or a
//! [`Cubemap`](crate::TextureType::Cubemap) texture. The pass is an ordinary
//! [`RenderPass`] of kind [`PassKind::Skybox`] that names the material. It
//! draws one fullscreen triangle at the far plane and tests it against the
//! depth the geometry pass wrote, so the sky shows only where no mesh drew.
//!
//! A sky material file names its shader by guid:
//! `5628c9e4d9278fec79998a7adab65bc3` ([`EQUIRECT_SHADER_GUID`]) or
//! `a6dfb611b11dfb57c5039dd8b6fc1a46` ([`CUBEMAP_SHADER_GUID`]). A material
//! file looks like this:
//!
//! ```json
//! "asset": {
//!   "shader": "5628c9e4d9278fec79998a7adab65bc3",
//!   "textures": { "sky": "<guid of an .hdr imported as Equirect>" },
//!   "parameters": {
//!     "skybox_tint": { "Color": [1.0, 1.0, 1.0] },
//!     "skybox_exposure": { "Scalar": 1.0 },
//!     "skybox_rotation": { "Scalar": 0.0 }
//!   }
//! }
//! ```
//!
//! The shaders use the camera's inverse view-projection to find the direction
//! each pixel looks along. The equirect shader then turns that direction into
//! a uv with the same rule as [`equirect_uv`](crate::assets::equirect_uv).

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_engine::{AssetGuid, AssetManager, Handle};

// Current crate
use crate::assets::{Material, MaterialParameter, PassKind, PassTarget, RenderPass, Shader};
use crate::config::ShaderSourceRecord;

// =============================================================================
// Constants
// =============================================================================

config_shader_file!(
    /// The skybox vertex stage: a fullscreen triangle at the far plane.
    SKYBOX_VERTEX, "skybox/shaders/skybox_vertex.wgsl"
);
config_shader_file!(
    /// The fragment stage that samples an equirectangular panorama.
    EQUIRECT_FRAGMENT, "skybox/shaders/skybox_equirect_fragment.wgsl"
);
config_shader_file!(
    /// The fragment stage that samples a cubemap.
    CUBEMAP_FRAGMENT, "skybox/shaders/skybox_cubemap_fragment.wgsl"
);

/// The asset name of the skybox shader that samples an equirectangular
/// panorama. It is the path the shader's `.hlsl` source is imported under.
pub const EQUIRECT_SHADER_PATH: &str = "skybox/shaders/skybox_equirect_fragment.hlsl";

/// The guid the equirect skybox shader is stored under, so a sky material file
/// can name it.
///
/// It is derived from a seed string rather than drawn at random, so every
/// checkout and every run gives the shader the same guid. The `.hlsl.meta`
/// beside the source has to carry this value, and [`install_shaders`] checks
/// that it does.
pub const EQUIRECT_SHADER_GUID: AssetGuid = AssetGuid::from_name("pill.skybox.shader.equirect");

/// The asset name of the skybox shader that samples a cubemap. It is the path
/// the shader's `.hlsl` source is imported under.
pub const CUBEMAP_SHADER_PATH: &str = "skybox/shaders/skybox_cubemap_fragment.hlsl";

/// The guid the cubemap skybox shader is stored under. See
/// [`EQUIRECT_SHADER_GUID`] for how each of the two is derived.
pub const CUBEMAP_SHADER_GUID: AssetGuid = AssetGuid::from_name("pill.skybox.shader.cubemap");

/// The texture slot both skybox shaders read the sky from.
pub const SKY_SLOT: &str = "sky";

/// The shader assets [`install_shaders`] creates, with the cooked files each
/// one was built from.
pub const SHADER_SOURCES: &[ShaderSourceRecord] = &[
    ShaderSourceRecord {
        shader_asset_name: EQUIRECT_SHADER_PATH,
        vertex_path: SKYBOX_VERTEX.relative_path,
        fragment_path: EQUIRECT_FRAGMENT.relative_path,
    },
    ShaderSourceRecord {
        shader_asset_name: CUBEMAP_SHADER_PATH,
        vertex_path: SKYBOX_VERTEX.relative_path,
        fragment_path: CUBEMAP_FRAGMENT.relative_path,
    },
];

// =============================================================================
// Free Functions
// =============================================================================

/// Install the equirect and the cubemap skybox shaders, and return them in
/// that order.
///
/// Idempotent: a store that already holds them hands the same handles back, so
/// this is safe to call once per generation. It has to run before a sky
/// material file loads, because the file names its shader by guid.
///
/// # Errors
///
/// Returns the store's import error when a source or its meta will not load.
///
/// # Panics
///
/// Panics on the call that imports a shader whose `.hlsl.meta` is missing or
/// names a different guid than the constant here declares. No material file
/// could find such a shader.
pub fn install_shaders(
    assets: &mut AssetManager,
) -> Result<(Handle<Shader>, Handle<Shader>), Box<dyn std::error::Error>> {
    let equirect =
        crate::config::import_shader(assets, EQUIRECT_SHADER_PATH, EQUIRECT_SHADER_GUID)?;
    let cubemap = crate::config::import_shader(assets, CUBEMAP_SHADER_PATH, CUBEMAP_SHADER_GUID)?;
    Ok((equirect, cubemap))
}

/// Return the neutral values of a sky material's parameters: white tint,
/// exposure 1, and no rotation (0 degrees).
pub fn default_parameters() -> BTreeMap<String, MaterialParameter> {
    BTreeMap::from([
        (
            "skybox_tint".to_owned(),
            MaterialParameter::Color([1.0, 1.0, 1.0]),
        ),
        ("skybox_exposure".to_owned(), MaterialParameter::Scalar(1.0)),
        ("skybox_rotation".to_owned(), MaterialParameter::Scalar(0.0)),
    ])
}

/// Build a skybox pass named `name` that draws `material` into `target`.
///
/// `order` places the pass among the geometry passes it sits behind. The pass
/// reads the depth those passes wrote, so `target` has to be the target they
/// wrote.
///
/// A pass with no material ([`Handle::INVALID`]) cannot be drawn, so the pass
/// starts disabled.
pub fn skybox_pass(
    name: impl Into<String>,
    material: Handle<Material>,
    target: PassTarget,
    order: u8,
) -> RenderPass {
    RenderPass::new(name)
        .with_kind(PassKind::Skybox)
        .with_material(material)
        .with_target(target)
        .with_depth_write(false)
        .with_order(order)
        .with_enabled(material != Handle::INVALID)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::ShaderTextureKind;

    /// Sky material files name the shaders by these digits, so the digits may
    /// never change. A seed string above decides each one, and editing a seed
    /// would hand that shader a different guid.
    #[test]
    fn the_shader_guids_are_the_ones_material_files_name() {
        assert_eq!(
            EQUIRECT_SHADER_GUID.to_string(),
            "5628c9e4d9278fec79998a7adab65bc3"
        );
        assert_eq!(
            CUBEMAP_SHADER_GUID.to_string(),
            "a6dfb611b11dfb57c5039dd8b6fc1a46"
        );
    }

    #[test]
    fn installing_twice_returns_the_same_shaders() {
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();

        let first = install_shaders(&mut assets).expect("free names");
        let second = install_shaders(&mut assets).expect("already installed");

        assert_eq!(first, second);
        assert_eq!(assets.guid_of(first.0), Some(EQUIRECT_SHADER_GUID));
        assert_eq!(assets.guid_of(first.1), Some(CUBEMAP_SHADER_GUID));
    }

    #[test]
    fn each_shader_declares_its_sky_slot_by_type() {
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();
        let (equirect, cubemap) = install_shaders(&mut assets).expect("free names");

        let slot_kind = |handle| assets.get::<Shader>(handle).unwrap().texture_slots[SKY_SLOT].kind;

        assert_eq!(slot_kind(equirect), ShaderTextureKind::Equirect);
        assert_eq!(slot_kind(cubemap), ShaderTextureKind::Cubemap);
    }

    #[test]
    fn a_pass_without_a_material_starts_disabled() {
        let pass = skybox_pass("sky", Handle::INVALID, PassTarget::Surface, 0);

        assert_eq!(pass.settings.kind, PassKind::Skybox);
        assert!(!pass.enabled && !pass.settings.depth_write);
    }
}
