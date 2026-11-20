//! The bitmap font the debug labels are drawn in: one 5 by 7 pixel glyph per
//! character, drawn into a single atlas, and the quads a line of text becomes.
//!
//! # Responsibilities
//!
//! - Hold the glyphs as readable pixel art ([`GLYPHS`]) and draw them into one
//!   single-channel atlas ([`build_atlas`]).
//! - Hand that atlas to the UI painter as a texture under its own reserved
//!   id ([`texture_set`], [`FONT_TEXTURE`]).
//! - Turn a label into glyph quads at a pixel position ([`layout_text`]).
//!
//! # Design
//!
//! Debug labels need to be readable, not pretty, and they must not depend on a
//! font file being found at runtime. The glyphs therefore live in the source,
//! and the atlas is built from them when the renderer starts.
//!
//! A lowercase letter draws as its uppercase form, and a character the font
//! does not have draws as `?`.

// Standard library
use std::sync::Arc;

// External crates
use pill_ui_frame::{TextureId, UiTextureFilter, UiTextureSet};

// =============================================================================
// Constants
// =============================================================================

/// Glyph width in pixels.
pub(super) const GLYPH_WIDTH: u32 = 5;
/// Glyph height in pixels.
pub(super) const GLYPH_HEIGHT: u32 = 7;
/// An atlas cell: the glyph plus one pixel of space to its right and below,
/// so sampling one glyph never bleeds into the next.
const CELL_WIDTH: u32 = GLYPH_WIDTH + 1;
const CELL_HEIGHT: u32 = GLYPH_HEIGHT + 1;
/// How many cells one atlas row holds.
const ATLAS_COLUMNS: u32 = 16;
/// How many screen pixels one glyph pixel covers.
pub(super) const GLYPH_SCALE: f32 = 2.0;

/// Every glyph the font has, as its character and its pixel art.
///
/// The art is seven rows separated by `/`, each row five characters wide,
/// with `#` for a lit pixel and `.` for a blank one.
pub(super) const GLYPHS: &[(char, &str)] = &[
    (' ', "...../...../...../...../...../...../....."),
    ('!', "..#../..#../..#../..#../..#../...../..#.."),
    ('"', ".#.#./.#.#./...../...../...../...../....."),
    ('#', ".#.#./.#.#./#####/.#.#./#####/.#.#./.#.#."),
    ('%', "##..#/##..#/...#./..#../.#.../#..##/#..##"),
    ('\'', "..#../..#../...../...../...../...../....."),
    ('(', "...#./..#../.#.../.#.../.#.../..#../...#."),
    (')', ".#.../..#../...#./...#./...#./..#../.#..."),
    ('*', "...../..#../#.#.#/.###./#.#.#/..#../....."),
    ('+', "...../..#../..#../#####/..#../..#../....."),
    (',', "...../...../...../...../.##../..#../.#..."),
    ('-', "...../...../...../#####/...../...../....."),
    ('.', "...../...../...../...../...../.##../.##.."),
    ('/', "...../....#/...#./..#../.#.../#..../....."),
    ('0', ".###./#...#/#..##/#.#.#/##..#/#...#/.###."),
    ('1', "..#../.##../..#../..#../..#../..#../.###."),
    ('2', ".###./#...#/....#/...#./..#../.#.../#####"),
    ('3', "#####/...#./..#../...#./....#/#...#/.###."),
    ('4', "...#./..##./.#.#./#..#./#####/...#./...#."),
    ('5', "#####/#..../####./....#/....#/#...#/.###."),
    ('6', "..##./.#.../#..../####./#...#/#...#/.###."),
    ('7', "#####/....#/...#./..#../.#.../.#.../.#..."),
    ('8', ".###./#...#/#...#/.###./#...#/#...#/.###."),
    ('9', ".###./#...#/#...#/.####/....#/...#./.##.."),
    (':', "...../.##../.##../...../.##../.##../....."),
    (';', "...../.##../.##../...../.##../..#../.#..."),
    ('<', "...#./..#../.#.../#..../.#.../..#../...#."),
    ('=', "...../...../#####/...../#####/...../....."),
    ('>', ".#.../..#../...#./....#/...#./..#../.#..."),
    ('?', ".###./#...#/....#/...#./..#../...../..#.."),
    ('A', ".###./#...#/#...#/#####/#...#/#...#/#...#"),
    ('B', "####./#...#/#...#/####./#...#/#...#/####."),
    ('C', ".###./#...#/#..../#..../#..../#...#/.###."),
    ('D', "####./#...#/#...#/#...#/#...#/#...#/####."),
    ('E', "#####/#..../#..../####./#..../#..../#####"),
    ('F', "#####/#..../#..../####./#..../#..../#...."),
    ('G', ".###./#...#/#..../#.###/#...#/#...#/.####"),
    ('H', "#...#/#...#/#...#/#####/#...#/#...#/#...#"),
    ('I', ".###./..#../..#../..#../..#../..#../.###."),
    ('J', "..###/...#./...#./...#./...#./#..#./.##.."),
    ('K', "#...#/#..#./#.#../##.../#.#../#..#./#...#"),
    ('L', "#..../#..../#..../#..../#..../#..../#####"),
    ('M', "#...#/##.##/#.#.#/#.#.#/#...#/#...#/#...#"),
    ('N', "#...#/#...#/##..#/#.#.#/#..##/#...#/#...#"),
    ('O', ".###./#...#/#...#/#...#/#...#/#...#/.###."),
    ('P', "####./#...#/#...#/####./#..../#..../#...."),
    ('Q', ".###./#...#/#...#/#...#/#.#.#/#..#./.##.#"),
    ('R', "####./#...#/#...#/####./#.#../#..#./#...#"),
    ('S', ".####/#..../#..../.###./....#/....#/####."),
    ('T', "#####/..#../..#../..#../..#../..#../..#.."),
    ('U', "#...#/#...#/#...#/#...#/#...#/#...#/.###."),
    ('V', "#...#/#...#/#...#/#...#/#...#/.#.#./..#.."),
    ('W', "#...#/#...#/#...#/#.#.#/#.#.#/#.#.#/.#.#."),
    ('X', "#...#/#...#/.#.#./..#../.#.#./#...#/#...#"),
    ('Y', "#...#/#...#/.#.#./..#../..#../..#../..#.."),
    ('Z', "#####/....#/...#./..#../.#.../#..../#####"),
    ('[', ".###./.#.../.#.../.#.../.#.../.#.../.###."),
    ('\\', "...../#..../.#.../..#../...#./....#/....."),
    (']', ".###./...#./...#./...#./...#./...#./.###."),
    ('_', "...../...../...../...../...../...../#####"),
    ('|', "..#../..#../..#../..#../..#../..#../..#.."),
];

/// The texture the label list samples the font from.
///
/// The id is reserved at the top of the UI's id space. `pill_project_ui` maps
/// egui's managed textures to `1 << 63 | id` and its user textures below that
/// ceiling, so the only managed texture that could reach this id is the
/// largest one.
pub(crate) const FONT_TEXTURE: TextureId = TextureId(u64::MAX);

// =============================================================================
// Atlas
// =============================================================================

/// The atlas: its size and one coverage byte per texel, row by row.
pub(super) struct Atlas {
    /// Width in texels.
    pub width: u32,
    /// Height in texels.
    pub height: u32,
    /// `255` where a glyph pixel is lit, `0` elsewhere.
    pub texels: Vec<u8>,
}

// =============================================================================
// GlyphQuad
// =============================================================================

/// One glyph quad: where it goes on screen and which atlas cell it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct GlyphQuad {
    /// Left and top of the quad on screen, in pixels.
    pub origin: [f32; 2],
    /// Width and height of the quad on screen, in pixels.
    pub size: [f32; 2],
    /// Left and top texel of the glyph's cell in the atlas.
    pub atlas_origin: [u32; 2],
}

// =============================================================================
// Free Functions
// =============================================================================

/// The atlas's width and height in texels.
///
/// The width is [`ATLAS_COLUMNS`] cells, and the height is as many rows of
/// cells as the glyph table needs.
pub(super) fn atlas_size() -> (u32, u32) {
    let rows = (GLYPHS.len() as u32).div_ceil(ATLAS_COLUMNS);
    (ATLAS_COLUMNS * CELL_WIDTH, rows * CELL_HEIGHT)
}

/// Draw every glyph of [`GLYPHS`] into its cell, in table order.
pub(super) fn build_atlas() -> Atlas {
    let (width, height) = atlas_size();
    let mut texels = vec![0u8; (width * height) as usize];
    for (index, (_, glyph_art)) in GLYPHS.iter().enumerate() {
        let (cell_x, cell_y) = cell_origin(index);
        for (row, pixel_row) in glyph_art.split('/').enumerate() {
            for (column, pixel) in pixel_row.chars().enumerate() {
                if pixel == '#' {
                    let x = cell_x + column as u32;
                    let y = cell_y + row as u32;
                    texels[(y * width + x) as usize] = 255;
                }
            }
        }
    }
    Atlas {
        width,
        height,
        texels,
    }
}

/// The atlas as the one-texture set the UI painter registers once, when the
/// renderer starts.
///
/// Every texel is white and its alpha is the glyph coverage. The texture is
/// sampled nearest, which keeps the glyphs sharp at [`GLYPH_SCALE`].
pub(crate) fn texture_set() -> UiTextureSet {
    let atlas = build_atlas();
    let pixels: Vec<u8> = atlas
        .texels
        .iter()
        .flat_map(|&coverage| [255, 255, 255, coverage])
        .collect();
    let mut font_texture_set = UiTextureSet::new();
    font_texture_set.push_full(
        FONT_TEXTURE,
        atlas.width,
        atlas.height,
        1,
        UiTextureFilter::Nearest,
        Arc::from(pixels),
    );
    font_texture_set
}

/// The top-left texel of the cell that holds the glyph at `index`.
fn cell_origin(index: usize) -> (u32, u32) {
    let index = index as u32;
    (
        (index % ATLAS_COLUMNS) * CELL_WIDTH,
        (index / ATLAS_COLUMNS) * CELL_HEIGHT,
    )
}

/// The glyph to draw for `character`.
///
/// A lowercase letter uses its uppercase form, and a character the font does
/// not have uses `?`. If the table held no `?` either, the first glyph (the
/// space) is used, so the returned index is always valid.
fn glyph_index(character: char) -> usize {
    let wanted_character = character.to_ascii_uppercase();
    let lookup = |target: char| GLYPHS.iter().position(|(glyph, _)| *glyph == target);
    lookup(wanted_character)
        .or_else(|| lookup('?'))
        .unwrap_or(0)
}

/// The quads `text` becomes, starting at pixel (`x`, `y`) as the top-left
/// corner.
///
/// A newline starts the next line under the first character of the line
/// above.
pub(super) fn layout_text(text: &str, x: f32, y: f32) -> Vec<GlyphQuad> {
    let advance = CELL_WIDTH as f32 * GLYPH_SCALE;
    let line_height = (CELL_HEIGHT + 2) as f32 * GLYPH_SCALE;
    let size = [
        GLYPH_WIDTH as f32 * GLYPH_SCALE,
        GLYPH_HEIGHT as f32 * GLYPH_SCALE,
    ];
    let mut quads = Vec::with_capacity(text.len());
    let (mut pen_x, mut pen_y) = (x, y);
    for character in text.chars() {
        if character == '\n' {
            pen_x = x;
            pen_y += line_height;
            continue;
        }
        // A space advances without drawing anything.
        if character != ' ' {
            let (atlas_x, atlas_y) = cell_origin(glyph_index(character));
            quads.push(GlyphQuad {
                origin: [pen_x, pen_y],
                size,
                atlas_origin: [atlas_x, atlas_y],
            });
        }
        pen_x += advance;
    }
    quads
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The painter's texture is the atlas widened to RGBA, with the coverage
    /// in alpha.
    #[test]
    fn the_texture_set_carries_the_atlas_as_white_coverage() {
        let atlas = build_atlas();
        let font_texture_set = texture_set();
        let texture = &font_texture_set.textures[0];
        assert_eq!((texture.width, texture.height), atlas_size());
        let pixels = &texture.pixels;
        let lit = atlas
            .texels
            .iter()
            .position(|&texel| texel == 255)
            .expect("a lit texel");
        assert_eq!(&pixels[lit * 4..lit * 4 + 4], &[255, 255, 255, 255]);
        assert_eq!(
            &pixels[..4],
            &[255, 255, 255, 0],
            "the first texel is blank"
        );
    }

    /// Every glyph is seven rows of five pixels, so no glyph draws garbage.
    #[test]
    fn every_glyph_is_five_by_seven() {
        for (character, glyph_art) in GLYPHS {
            let rows: Vec<&str> = glyph_art.split('/').collect();
            assert_eq!(rows.len(), GLYPH_HEIGHT as usize, "{character:?}");
            for row in rows {
                assert_eq!(row.len(), GLYPH_WIDTH as usize, "{character:?}");
                assert!(row.chars().all(|pixel| pixel == '#' || pixel == '.'));
            }
        }
    }

    /// Every glyph has a cell in the atlas, and a lit glyph pixel comes out
    /// lit in the atlas.
    #[test]
    fn the_atlas_holds_every_glyph() {
        let atlas = build_atlas();
        assert_eq!(atlas.texels.len(), (atlas.width * atlas.height) as usize);
        // The top-left pixel of `#` is off, the second one is on.
        let index = GLYPHS.iter().position(|(glyph, _)| *glyph == '#').unwrap();
        let (x, y) = cell_origin(index);
        assert_eq!(atlas.texels[(y * atlas.width + x) as usize], 0);
        assert_eq!(atlas.texels[(y * atlas.width + x + 1) as usize], 255);
    }

    /// Lowercase draws as uppercase, an unknown character as `?`, and a
    /// space draws nothing but still advances.
    #[test]
    fn text_lays_out_left_to_right() {
        let quads = layout_text("a b", 10.0, 20.0);
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].origin, [10.0, 20.0]);
        assert_eq!(
            quads[1].origin[0],
            10.0 + 2.0 * CELL_WIDTH as f32 * GLYPH_SCALE
        );
        assert_eq!(glyph_index('a'), glyph_index('A'));
        assert_eq!(glyph_index('~'), glyph_index('?'));
    }

    /// A newline returns to the first column on the next line.
    #[test]
    fn a_newline_starts_the_next_line() {
        let quads = layout_text("A\nB", 0.0, 0.0);
        assert_eq!(quads[1].origin[0], 0.0);
        assert!(quads[1].origin[1] > quads[0].origin[1]);
    }
}
