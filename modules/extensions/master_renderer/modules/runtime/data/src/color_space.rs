//! Converts colour channels between sRGB and linear light.
//!
//! sRGB is the encoding images and UI colours are usually stored in. Linear
//! light keeps each channel proportional to the brightness it represents,
//! which is the form lighting calculations need.
//!
//! # Responsibilities
//!
//! - [`srgb_to_linear`]: decode an sRGB-encoded channel to linear light, the
//!   way texture import in this crate reads an 8-bit image.
//! - [`linear_to_srgb`]: encode a linear channel as sRGB. The GPU module uses
//!   it to prepare a debug label's colour for the UI painter, which expects
//!   sRGB vertex colours.
//!
//! # Design
//!
//! Both directions are plain arithmetic with no GPU involved, so they live in
//! the data crate and texture import here and the GPU module share one copy.
//! The UI painter's shader has its own WGSL version of the same curve.

/// Converts one sRGB-encoded channel in `[0, 1]` to linear light.
pub fn srgb_to_linear(srgb_channel: f32) -> f32 {
    if srgb_channel <= 0.04045 {
        srgb_channel / 12.92
    } else {
        ((srgb_channel + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts one linear-light channel in `[0, 1]` to its sRGB encoding.
///
/// It reverses [`srgb_to_linear`], up to floating-point rounding.
pub fn linear_to_srgb(linear_channel: f32) -> f32 {
    if linear_channel <= 0.0031308 {
        linear_channel * 12.92
    } else {
        1.055 * linear_channel.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_curve_keeps_black_and_white() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert_eq!(linear_to_srgb(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_midtone_decodes_darker_and_round_trips() {
        let linear_channel = srgb_to_linear(0.5);
        assert!(
            (linear_channel - 0.2140).abs() < 1e-3,
            "0.5 in sRGB is about 0.214 linear"
        );
        for step in 0..=20 {
            let value = step as f32 / 20.0;
            assert!(
                (linear_to_srgb(srgb_to_linear(value)) - value).abs() < 1e-5,
                "{value} survives the round trip"
            );
        }
    }
}
