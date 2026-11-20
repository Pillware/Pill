//! Color palettes for debug visualization.
//!
//! # Responsibilities
//!
//! - Provide a fixed palette of visually distinct colors.
//! - Generate a palette of 100 colors from evenly spaced hues.
//!
//! # Design
//!
//! Every color is an `(r, g, b)` triple of `f32` values in the 0.0-1.0
//! range. The module keeps no state and has no dependencies.

// ============================================================================
// Constants
// ============================================================================

/// A fixed palette of visually distinct RGB colors, for debug rendering.
///
/// Each entry is an `(r, g, b)` tuple in the 0.0-1.0 range.
pub const DISTINCT_COLOR_PALETTE: &[(f32, f32, f32)] = &[
    (0.894, 0.102, 0.110), // Red
    (0.215, 0.494, 0.721), // Blue
    (0.302, 0.686, 0.290), // Green
    (0.596, 0.306, 0.639), // Purple
    (1.000, 0.498, 0.000), // Orange
    (1.000, 1.000, 0.200), // Yellow
    (0.651, 0.337, 0.157), // Brown
    (0.969, 0.506, 0.749), // Pink
    (0.600, 0.600, 0.600), // Gray
    (0.100, 0.100, 0.100), // Near-black
    (0.000, 0.447, 0.698), // Deep blue
    (0.800, 0.475, 0.655), // Mauve
    (0.337, 0.705, 0.913), // Sky blue
    (0.000, 0.619, 0.451), // Teal
    (0.941, 0.894, 0.259), // Lemon
    (0.800, 0.725, 0.454), // Tan
    (0.792, 0.698, 0.839), // Lavender
    (0.984, 0.603, 0.600), // Salmon
    (0.541, 0.168, 0.886), // Indigo
    (0.125, 0.694, 0.298), // Bright green
];

// ============================================================================
// Free Functions
// ============================================================================

/// Generates 100 colors from evenly spaced hues.
///
/// The saturation and lightness stay fixed while the hue advances in 100
/// equal steps around the circle, so the colors spread evenly.
pub fn generate_color_palette() -> Vec<(f32, f32, f32)> {
    (0..100)
        .map(|i| {
            let hue = i as f32 / 100.0; // Evenly spaced hues
            hsl_to_rgb(hue, 0.6, 0.5) // Saturation and lightness fixed
        })
        .collect()
}

/// Converts a hue, saturation and lightness triple (HSL) to an RGB triple.
///
/// `hue`, `saturation` and `lightness` are expected in the 0.0-1.0 range, and
/// every returned channel is in the same range.
///
/// # Examples
///
/// ```
/// use pill_core::color::hsl_to_rgb;
///
/// let color = hsl_to_rgb(0.5, 0.6, 0.5);
/// assert!(color.0 >= 0.0 && color.0 <= 1.0);
/// assert!(color.1 >= 0.0 && color.1 <= 1.0);
/// assert!(color.2 >= 0.0 && color.2 <= 1.0);
/// ```
pub fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> (f32, f32, f32) {
    // `channel_amplitude` is how far a channel can move away from
    // `lightness`: zero for gray and larger the more saturated the color is.
    // `channel_value` returns one channel from its hue offset; the calls
    // below pass the offsets for red, green and blue.
    let channel_amplitude = saturation * lightness.min(1.0 - lightness);
    let channel_value = |channel_offset: f32| {
        let hue_position = (channel_offset + hue * 12.0) % 12.0;
        lightness - channel_amplitude * (-((hue_position - 3.0).abs() - 1.0)).clamp(-1.0, 1.0)
    };
    (channel_value(0.0), channel_value(8.0), channel_value(4.0))
}
