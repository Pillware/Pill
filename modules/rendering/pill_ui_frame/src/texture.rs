//! The versioned texture set a UI frame carries, so a backend rebuilt mid-run
//! can upload what it lacks instead of drawing nothing.
//!
//! # Responsibilities
//!
//! - [`UiTextureSet`]: the textures whose content changed since the producer
//!   last carried them, each with a revision number that only goes up, plus
//!   the textures to free.
//! - [`UiTexture`]: one texture's size, sampling and whole image at this
//!   revision, as owned plain data.
//!
//! # Design
//!
//! The revision decides what gets uploaded. A painter remembers the revision
//! it applied for each texture, and uploads the image again whenever the
//! carried revision is a different number. The producer therefore carries a
//! texture only when its revision changed, not every texture every frame
//! ([`UiFrame`](crate::frame::UiFrame)).
//!
//! A carried set is applied once per frame, before any surface decides whether
//! it draws. The textures belong to the frame update, not to a surface, so a
//! frame that draws nothing - a minimized window, a surface that may not
//! present yet - still hands them over, and the next drawn frame has them.
//! Because the painter remembers the revisions it applied, applying the same
//! set twice in one frame uploads nothing the second time; that is an
//! optimisation, not the rule.
//!
//! A painter that starts with no textures at all needs more than that set,
//! because nothing has changed by then. A renderer module that was just
//! rebuilt asks the producer to send every texture again through
//! [`UiFrameSlot::request_texture_resend`](crate::slot::UiFrameSlot::request_texture_resend),
//! and the producer carries its whole set again when the request number
//! changes. An entry always carries the whole image rather than a patch, so a
//! re-carried texture needs no history to apply.
//!
//! Every buffer is an [`Arc`] allocated by the code that produced it and
//! handed over as plain data. A project reload may drop the producer while a
//! renderer still holds the frame, so the bytes must be owned by the data, not
//! borrowed from the producer.

// Standard library
use std::sync::Arc;

// Current crate
use crate::draw::TextureId;

// =============================================================================
// UiTextureFilter
// =============================================================================

/// How a backend samples a UI texture.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum UiTextureFilter {
    /// One texel per pixel, keeping edges sharp. Used for glyph atlases and
    /// pixel art.
    #[default]
    Nearest,
    /// Blends between neighbouring texels, keeping gradients smooth. Used for
    /// photo-like textures and scaled drawings.
    Linear,
}

// =============================================================================
// UiTexture
// =============================================================================

/// One texture sampled by a frame's draw lists, at one revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiTexture {
    /// The id the draw lists use to sample this texture.
    pub id: TextureId,
    /// The image's width in texels.
    pub width: u32,
    /// The image's height in texels.
    pub height: u32,
    /// The revision of the content this entry carries. It goes up every time
    /// the texture's content changes.
    pub revision: u64,
    /// The whole image as tightly packed straight-alpha sRGB RGBA8 rows,
    /// `width * height * 4` bytes.
    pub pixels: Arc<[u8]>,
    /// How the texture is sampled.
    pub filter: UiTextureFilter,
}

// =============================================================================
// UiTextureSet
// =============================================================================

/// The textures a frame carries to the backend, and the ones it frees.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiTextureSet {
    /// The textures whose content changed since the producer last carried
    /// them, one entry per texture. Empty when nothing changed.
    pub textures: Vec<UiTexture>,
    /// Textures the producer no longer uses. A backend drops them.
    pub frees: Vec<TextureId>,
}

impl UiTextureSet {
    /// An empty set.
    pub fn new() -> UiTextureSet {
        UiTextureSet::default()
    }

    /// Whether there is nothing to upload and nothing to free.
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty() && self.frees.is_empty()
    }

    /// Forget the previous frame's set, keeping the allocations.
    pub fn clear(&mut self) {
        self.textures.clear();
        self.frees.clear();
    }

    /// Adds the whole image of `id` at `revision`.
    ///
    /// `pixels` is straight-alpha sRGB RGBA8 and exactly `width * height * 4`
    /// bytes. Any other length is a producer bug that the backend would read
    /// past, so it is refused here rather than at the upload.
    ///
    /// # Panics
    ///
    /// Panics when `pixels` is not exactly `width * height * 4` bytes.
    pub fn push_full(
        &mut self,
        id: TextureId,
        width: u32,
        height: u32,
        revision: u64,
        filter: UiTextureFilter,
        pixels: Arc<[u8]>,
    ) {
        assert_eq!(
            pixels.len(),
            width as usize * height as usize * 4,
            "a whole-texture upload needs exactly width * height * 4 bytes"
        );
        self.textures.push(UiTexture {
            id,
            width,
            height,
            revision,
            pixels,
            filter,
        });
    }

    /// Marks `id` to be freed in every backend that applied the set.
    pub fn push_free(&mut self, id: TextureId) {
        self.frees.push(id);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_upload_carries_the_image_and_its_revision() {
        let mut set = UiTextureSet::new();
        set.push_full(
            TextureId(4),
            2,
            2,
            7,
            UiTextureFilter::Nearest,
            Arc::from(vec![255u8; 16]),
        );
        let texture = &set.textures[0];
        assert_eq!(texture.pixels.len(), 16);
        assert_eq!((texture.id, texture.revision), (TextureId(4), 7));
    }

    #[test]
    fn a_wrongly_sized_upload_is_refused() {
        let outcome = std::panic::catch_unwind(|| {
            let mut set = UiTextureSet::new();
            set.push_full(
                TextureId(1),
                2,
                2,
                1,
                UiTextureFilter::Nearest,
                Arc::from(vec![0u8; 8]),
            );
        });
        assert!(outcome.is_err(), "a 2x2 image is 16 bytes, not 8");
    }

    #[test]
    fn freeing_a_texture_empties_the_set_only_with_no_uploads() {
        let mut set = UiTextureSet::new();
        set.push_free(TextureId(2));
        assert!(!set.is_empty());
        assert!(set.textures.is_empty());
        set.clear();
        assert!(set.is_empty());
    }
}
