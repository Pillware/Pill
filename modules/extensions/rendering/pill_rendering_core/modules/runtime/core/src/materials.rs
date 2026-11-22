//! Reading a material's slots by name.
//!
//! # Responsibilities
//!
//! - Return one named parameter or texture binding from a material, whatever
//!   type the shader declared the slot with ([`parameter`], [`texture`]).
//! - Read the three slots the lit shaders agree on: the base colour
//!   ([`base_colour`]), the map it multiplies ([`base_colour_texture`]) and the
//!   roughness factor ([`roughness`]).
//!
//! # Design
//!
//! A game writes a material as slot names and values; a shader declares the
//! same names in its own vocabulary. `MaterialParameter` is what a material
//! stores, and the slot names used here are the ones the engine's lit shaders
//! declare (`pbr_base`, `base_color`, `pbr_roughness`), so a renderer without a
//! GPU can shade a material the same way the shaders do.
//!
//! A slot a material does not set keeps the value the shader's own defaults
//! describe: the neutral values a new `.material` starts from. The readers
//! return those, so a renderer never has to special-case a missing slot.

// External crates
use pill_engine::Handle;
use pill_renderer_api::frame::MaterialParameter;
use pill_rendering::assets::{Material, Texture};

// =============================================================================
// Constants
// =============================================================================

/// The slot the lit shaders read the base colour tint from.
pub const BASE_COLOUR_SLOT: &str = "pbr_base";

/// The slot the lit shaders read the base colour map from.
pub const BASE_COLOUR_TEXTURE_SLOT: &str = "base_color";

/// The slot the lit shaders read the roughness factor from.
pub const ROUGHNESS_SLOT: &str = "pbr_roughness";

/// The colour a material draws with when it sets no base colour: white, which
/// leaves a bound map unchanged.
const NEUTRAL_COLOUR: [f32; 3] = [1.0, 1.0, 1.0];

/// The roughness a material draws with when it sets none.
const NEUTRAL_ROUGHNESS: f32 = 1.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the parameter stored under `slot`, or `None` when the material sets
/// none.
pub fn parameter<'a>(material: &'a Material, slot: &str) -> Option<&'a MaterialParameter> {
    material.parameters.get(slot)
}

/// Returns the texture bound to `slot`, or `None` when the slot is unbound.
pub fn texture(material: &Material, slot: &str) -> Option<Handle<Texture>> {
    material.textures.get(slot).map(|binding| binding.texture)
}

/// Returns the material's base colour: the `pbr_base` tint, or white when the
/// material sets none or sets it as another type.
pub fn base_colour(material: &Material) -> [f32; 3] {
    match material.parameters.get(BASE_COLOUR_SLOT) {
        Some(MaterialParameter::Color(colour)) => *colour,
        _ => NEUTRAL_COLOUR,
    }
}

/// Returns the texture bound to the material's base colour slot, when one is.
pub fn base_colour_texture(material: &Material) -> Option<Handle<Texture>> {
    texture(material, BASE_COLOUR_TEXTURE_SLOT)
}

/// Returns the material's roughness factor: the `pbr_roughness` scalar, or the
/// neutral value when the material sets none or sets it as another type.
pub fn roughness(material: &Material) -> f32 {
    match material.parameters.get(ROUGHNESS_SLOT) {
        Some(MaterialParameter::Scalar(roughness)) => *roughness,
        _ => NEUTRAL_ROUGHNESS,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_rendering::assets::{MaterialBuilder, TextureType};

    fn material(name: &str) -> MaterialBuilder {
        Material::builder(name)
    }

    #[test]
    fn a_slot_the_material_sets_is_read_back() {
        let mut assets = pill_engine::AssetManager::new();
        let albedo = assets
            .add_named(
                "albedo",
                Texture::from_rgba("albedo", TextureType::Color, vec![1, 2, 3, 255], 1, 1)
                    .expect("a 1x1 RGBA tile"),
            )
            .expect("a free name");
        let material = material("painted")
            .color_parameter(BASE_COLOUR_SLOT, [0.2, 0.4, 0.6])
            .texture(BASE_COLOUR_TEXTURE_SLOT, &albedo)
            .scalar_parameter(ROUGHNESS_SLOT, 0.25)
            .build();

        assert_eq!(base_colour(&material), [0.2, 0.4, 0.6]);
        assert_eq!(base_colour_texture(&material), Some(albedo));
        assert_eq!(roughness(&material), 0.25);
        assert_eq!(
            parameter(&material, ROUGHNESS_SLOT),
            Some(&MaterialParameter::Scalar(0.25))
        );
        assert_eq!(texture(&material, BASE_COLOUR_TEXTURE_SLOT), Some(albedo));
    }

    /// A material that sets nothing draws with the neutral values a new
    /// `.material` starts from, so a renderer needs no special case for it.
    #[test]
    fn an_unset_slot_reads_as_its_neutral_value() {
        let material = material("plain").build();

        assert_eq!(base_colour(&material), NEUTRAL_COLOUR);
        assert_eq!(base_colour_texture(&material), None);
        assert_eq!(roughness(&material), NEUTRAL_ROUGHNESS);
        assert_eq!(parameter(&material, BASE_COLOUR_SLOT), None);
        assert_eq!(texture(&material, BASE_COLOUR_TEXTURE_SLOT), None);
    }

    /// A slot set with another type reads as unset rather than as a value of
    /// the wrong shape: a colour slot holding a scalar is a mistake in the
    /// material, not something to reinterpret.
    #[test]
    fn a_slot_of_another_type_reads_as_its_neutral_value() {
        let material = material("miswired")
            .scalar_parameter(BASE_COLOUR_SLOT, 0.5)
            .color_parameter(ROUGHNESS_SLOT, [0.1, 0.2, 0.3])
            .build();

        assert_eq!(base_colour(&material), NEUTRAL_COLOUR);
        assert_eq!(roughness(&material), NEUTRAL_ROUGHNESS);
    }
}
