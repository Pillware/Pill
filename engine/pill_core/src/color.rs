
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

pub fn generate_color_palette() -> Vec<(f32, f32, f32)> {
    (0..100).map(|i| {
        let hue = i as f32 / 100.0; // Evenly spaced hues
        hsl_to_rgb(hue, 0.6, 0.5)   // Saturation and lightness fixed
    }).collect()
}

// Convert HSL to RGB
pub fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let a = s * l.min(1.0 - l);
    let f = |n: f32| {
        let k = (n + h * 12.0) % 12.0;
        l - a * (-((k - 3.0).abs() - 1.0).max(-1.0).min(1.0))
    };
    (f(0.0), f(8.0), f(4.0))
}

