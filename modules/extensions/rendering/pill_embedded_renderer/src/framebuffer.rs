//! A CPU framebuffer whose pixels are packed RGB565, the format a display
//! controller reads.
//!
//! # Responsibilities
//!
//! - Owns the pixel buffer and its dimensions ([`Framebuffer`]).
//! - Converts 0.0-1.0 colour channels to packed RGB565 ([`pack_rgb565`]).
//! - Fills, blends and clears pixels.
//! - Exposes the finished frame as little-endian bytes for a display driver.
//!
//! # Design
//!
//! The buffer stores packed `u16` pixels instead of bytes. A blend reads and
//! writes a whole pixel, so storing bytes would mean shifting and masking on
//! every access. The byte form is built only when the frame is handed to the
//! driver, by [`Framebuffer::as_rgb565_le`].
//!
//! The byte order comes from the driver, not from this module: the ST7789
//! expects little-endian pixels. `to_le_bytes` gives that order, because it is
//! a no-op on a little-endian host and swaps the bytes on a big-endian one. The
//! frame is correct on both kinds of host, not only on the usual little-endian
//! target.

// =============================================================================
// Constants
// =============================================================================

/// Bytes per pixel in the RGB565 output, one `u16`.
pub const BYTES_PER_PIXEL: usize = 2;

// =============================================================================
// Framebuffer
// =============================================================================

/// A pixel buffer of packed RGB565 values that the rasterizer draws into.
///
/// Rows are stored one after another, top row first: pixel `(x, y)` is at
/// index `y * width + x`. That is the scan order the display controller
/// expects, so the finished buffer goes to the driver with no reordering.
pub struct Framebuffer {
    /// Packed RGB565 pixels, `width * height` of them, row-major.
    pixels: Vec<u16>,
    /// Row length in pixels.
    width: u32,
    /// Row count.
    height: u32,
    /// The byte view of the pixels, filled by [`Self::as_rgb565_le`].
    ///
    /// Kept between frames, so handing the driver a frame costs one copy and
    /// no allocation. A fresh allocation of this size on every frame is worth
    /// avoiding on the small CPUs this renderer targets.
    bytes: Vec<u8>,
}

impl Framebuffer {
    /// Allocates a framebuffer of `width` by `height` pixels, filled with
    /// black.
    ///
    /// Both dimensions are raised to at least one pixel. A zero-sized buffer
    /// would force a special case into every index calculation, and no display
    /// has a zero-pixel axis.
    pub fn new(width: u32, height: u32) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let pixel_count = width as usize * height as usize;
        Self {
            pixels: vec![0; pixel_count],
            width,
            height,
            bytes: vec![0; pixel_count * BYTES_PER_PIXEL],
        }
    }

    /// Row length in pixels.
    #[inline]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Row count.
    #[inline]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Resizes the buffer, reallocating only when a dimension changes.
    ///
    /// Pixels are not preserved. The next render clears and repaints every
    /// pixel anyway, so copying the old contents would only waste time.
    pub fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return;
        }
        let pixel_count = width as usize * height as usize;
        self.width = width;
        self.height = height;
        self.pixels.clear();
        self.pixels.resize(pixel_count, 0);
        self.bytes.clear();
        self.bytes.resize(pixel_count * BYTES_PER_PIXEL, 0);
    }

    /// Overwrites every pixel with one packed colour.
    #[inline]
    pub fn clear(&mut self, packed_pixel: u16) {
        self.pixels.fill(packed_pixel);
    }

    /// Reads one pixel, or returns `None` when the coordinate is outside the
    /// buffer.
    ///
    /// The tests use this. Drawing never does: [`Self::fill_span`] and
    /// [`Self::blend_span`] resolve a span once through [`Self::span_bounds`]
    /// and then write the row directly.
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> Option<u16> {
        (x < self.width && y < self.height)
            .then(|| self.pixels[y as usize * self.width as usize + x as usize])
    }

    /// Fills a horizontal span with one opaque colour.
    ///
    /// `x_start..x_end` is half-open. The range is clamped here and again by
    /// the caller, so a span that reaches past the right edge is cut short
    /// instead of wrapping onto the next row.
    #[inline]
    pub fn fill_span(&mut self, y: u32, x_start: u32, x_end: u32, packed_pixel: u16) {
        let Some((start, end)) = self.span_bounds(y, x_start, x_end) else {
            return;
        };
        self.pixels[start..end].fill(packed_pixel);
    }

    /// Blends a horizontal span over the pixels already there.
    ///
    /// `color` is RGBA in 0.0-1.0, in the same channel order a sprite instance
    /// stores it, with alpha as how much of the source shows through. Each
    /// channel is mixed as `source * alpha + destination * (1 - alpha)`, the
    /// source-over formula the wgpu backend sets through `ALPHA_BLENDING`. The
    /// channels are mixed as they are stored: this backend never converts
    /// between sRGB and linear.
    #[inline]
    pub fn blend_span(&mut self, y: u32, x_start: u32, x_end: u32, color: [f32; 4]) {
        let Some((start, end)) = self.span_bounds(y, x_start, x_end) else {
            return;
        };
        let [red, green, blue, alpha] = color;
        let destination_weight = 1.0 - alpha;
        for pixel in &mut self.pixels[start..end] {
            let (destination_red, destination_green, destination_blue) = unpack_rgb565(*pixel);
            *pixel = pack_rgb565(
                red * alpha + destination_red * destination_weight,
                green * alpha + destination_green * destination_weight,
                blue * alpha + destination_blue * destination_weight,
            );
        }
    }

    /// Turns a span into buffer indices, or returns `None` when it covers no
    /// pixel.
    ///
    /// This is the only place a span is clipped to its row, so
    /// [`Self::fill_span`] and [`Self::blend_span`] cannot disagree about what
    /// is in bounds.
    #[inline]
    fn span_bounds(&self, y: u32, x_start: u32, x_end: u32) -> Option<(usize, usize)> {
        if y >= self.height {
            return None;
        }
        let x_start = x_start.min(self.width);
        let x_end = x_end.min(self.width);
        if x_start >= x_end {
            return None;
        }
        let row_start = y as usize * self.width as usize;
        Some((row_start + x_start as usize, row_start + x_end as usize))
    }

    /// Returns the finished frame as little-endian RGB565 bytes.
    ///
    /// This is what a display driver consumes; for the ST7789 that is
    /// `draw(&[u8])`. The slice is `width * height * 2` bytes long and borrows
    /// `self`, so the framebuffer cannot change while it is in use.
    pub fn as_rgb565_le(&mut self) -> &[u8] {
        for (pixel, chunk) in self
            .pixels
            .iter()
            .zip(self.bytes.chunks_exact_mut(BYTES_PER_PIXEL))
        {
            chunk.copy_from_slice(&pixel.to_le_bytes());
        }
        &self.bytes
    }
}

// =============================================================================
// Colour packing
// =============================================================================

/// Packs three 0.0-1.0 colour channels into one RGB565 pixel.
///
/// The format gives red 5 bits, green 6 and blue 5. Green gets the extra bit
/// because the eye sees green detail best. Alpha has no place in the format,
/// so it is applied by blending before the call, never here.
///
/// Channels are clamped, not wrapped. A channel outside 0.0-1.0 is a bug in
/// the project. If the value wrapped instead, the result would be far from the
/// colour asked for. An over-bright red, for example, can end up almost black,
/// which is much harder to recognise on a small display.
#[inline]
pub fn pack_rgb565(red: f32, green: f32, blue: f32) -> u16 {
    let quantize = |channel: f32, bit_count: u32| -> u16 {
        let channel_max = ((1u32 << bit_count) - 1) as f32;
        // `clamp` returns NaN unchanged, because every comparison with NaN is
        // false. The `as u16` cast then saturates NaN to 0, so a NaN channel
        // draws black instead of an unpredictable bit pattern.
        (channel.clamp(0.0, 1.0) * channel_max + 0.5) as u16
    };
    (quantize(red, 5) << 11) | (quantize(green, 6) << 5) | quantize(blue, 5)
}

/// Unpacks one RGB565 pixel back into 0.0-1.0 channels.
///
/// This is the inverse of [`pack_rgb565`]. Blending uses it to read the
/// destination pixel, and the tests use it to check the round trip.
#[inline]
pub fn unpack_rgb565(pixel: u16) -> (f32, f32, f32) {
    let red = ((pixel >> 11) & 0x1F) as f32 / 31.0;
    let green = ((pixel >> 5) & 0x3F) as f32 / 63.0;
    let blue = (pixel & 0x1F) as f32 / 31.0;
    (red, green, blue)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The primary colours come out as the exact bit patterns the format
    /// defines.
    #[test]
    fn packing_places_channels_in_their_rgb565_fields() {
        assert_eq!(pack_rgb565(0.0, 0.0, 0.0), 0x0000);
        assert_eq!(pack_rgb565(1.0, 1.0, 1.0), 0xFFFF);
        assert_eq!(pack_rgb565(1.0, 0.0, 0.0), 0xF800);
        assert_eq!(pack_rgb565(0.0, 1.0, 0.0), 0x07E0);
        assert_eq!(pack_rgb565(0.0, 0.0, 1.0), 0x001F);
    }

    /// Out-of-range and NaN channels clamp instead of wrapping to a distant
    /// colour.
    #[test]
    fn packing_clamps_rather_than_wrapping() {
        assert_eq!(pack_rgb565(2.0, 2.0, 2.0), 0xFFFF);
        assert_eq!(pack_rgb565(-1.0, -1.0, -1.0), 0x0000);
        assert_eq!(pack_rgb565(f32::NAN, f32::NAN, f32::NAN), 0x0000);
    }

    /// Unpacking then packing returns the same pixel, up to the rounding each
    /// field applies.
    #[test]
    fn unpacking_round_trips_through_packing() {
        for pixel in [0x0000u16, 0xFFFF, 0xF800, 0x07E0, 0x001F, 0x1234] {
            let (red, green, blue) = unpack_rgb565(pixel);
            assert_eq!(pack_rgb565(red, green, blue), pixel);
        }
    }

    /// Output is little-endian and two bytes per pixel, as the driver expects.
    #[test]
    fn frame_bytes_are_little_endian_pairs() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.fill_span(0, 0, 1, 0xF800);
        framebuffer.fill_span(0, 1, 2, 0x001F);

        // Written little-endian, 0xF800 becomes [0x00, 0xF8] and 0x001F
        // becomes [0x1F, 0x00].
        assert_eq!(framebuffer.as_rgb565_le(), &[0x00, 0xF8, 0x1F, 0x00]);
    }

    /// The byte buffer is exactly two bytes per pixel at any size.
    #[test]
    fn frame_byte_length_matches_pixel_count() {
        let mut framebuffer = Framebuffer::new(240, 280);
        assert_eq!(
            framebuffer.as_rgb565_le().len(),
            240 * 280 * BYTES_PER_PIXEL
        );
    }

    /// A span reaching past the right edge truncates instead of wrapping.
    #[test]
    fn spans_clip_to_the_row_rather_than_wrapping() {
        let mut framebuffer = Framebuffer::new(4, 2);
        framebuffer.fill_span(0, 2, 99, 0xFFFF);

        assert_eq!(framebuffer.pixel(2, 0), Some(0xFFFF));
        assert_eq!(framebuffer.pixel(3, 0), Some(0xFFFF));
        // The next row must stay untouched. Wrapping would have painted it.
        assert_eq!(framebuffer.pixel(0, 1), Some(0x0000));
        assert_eq!(framebuffer.pixel(1, 1), Some(0x0000));
    }

    /// Out-of-range rows and empty ranges are dropped, not clamped into view.
    #[test]
    fn spans_outside_the_buffer_draw_nothing() {
        let mut framebuffer = Framebuffer::new(2, 2);
        framebuffer.fill_span(9, 0, 2, 0xFFFF);
        framebuffer.fill_span(0, 2, 1, 0xFFFF);
        framebuffer.fill_span(0, 1, 1, 0xFFFF);

        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(framebuffer.pixel(x, y), Some(0x0000));
            }
        }
    }

    /// Full alpha replaces the destination and zero alpha leaves it alone.
    #[test]
    fn blending_endpoints_replace_and_preserve() {
        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.clear(pack_rgb565(0.0, 0.0, 1.0));

        framebuffer.blend_span(0, 0, 1, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(framebuffer.pixel(0, 0), Some(pack_rgb565(1.0, 0.0, 0.0)));

        framebuffer.blend_span(0, 1, 2, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(framebuffer.pixel(1, 0), Some(pack_rgb565(0.0, 0.0, 1.0)));
    }

    /// Half-alpha white over black comes out near the middle of every channel.
    #[test]
    fn blending_mixes_source_and_destination() {
        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.clear(0x0000);
        framebuffer.blend_span(0, 0, 1, [1.0, 1.0, 1.0, 0.5]);

        let (red, green, blue) = unpack_rgb565(framebuffer.pixel(0, 0).expect("in bounds"));
        for channel in [red, green, blue] {
            assert!(
                (channel - 0.5).abs() < 0.05,
                "channel {channel} should sit near 0.5"
            );
        }
    }

    /// Resizing takes the new dimensions and updates the byte buffer to match.
    #[test]
    fn resizing_updates_dimensions_and_byte_length() {
        let mut framebuffer = Framebuffer::new(4, 4);
        framebuffer.resize(8, 2);

        assert_eq!((framebuffer.width(), framebuffer.height()), (8, 2));
        assert_eq!(framebuffer.as_rgb565_le().len(), 8 * 2 * BYTES_PER_PIXEL);
    }

    /// Zero dimensions become one pixel instead of an empty buffer.
    #[test]
    fn zero_dimensions_are_promoted_to_one_pixel() {
        let framebuffer = Framebuffer::new(0, 0);
        assert_eq!((framebuffer.width(), framebuffer.height()), (1, 1));
    }
}
