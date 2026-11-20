//! The texture mirror: egui's delta stream, kept as whole images a rebuilt
//! painter can upload from.
//!
//! # Responsibilities
//!
//! - [`TextureMirror`]: one image per egui texture, kept up to date by the
//!   deltas each pass reports, with a revision on every image.
//! - [`TextureMirror::apply`]: fold a pass's [`TexturesDelta`] into the
//!   mirror and remember which images changed and which were freed.
//! - [`TextureMirror::carry`]: turn what changed into the frame's
//!   [`UiTextureSet`], as whole images.
//!
//! # Design
//!
//! egui sends a whole image when a texture is new or changes size, and a
//! small patch when only part of it changes (the font atlas gets a patch for
//! every new glyph). A painter that is rebuilt mid-run starts with no
//! textures, and a patch cannot give it the parts that did not change. So the
//! producer keeps a full copy of every texture and sends that copy again
//! whenever the texture changes. The mirror is that copy: one `Vec<u8>` per
//! texture, patched by the deltas and read out as the frame's uploads.
//!
//! The bytes are straight-alpha sRGB RGBA8: every channel is stored on its
//! own, and alpha is not multiplied into the colour. That is what
//! `pill_ui_frame::Color` and the painter's shader expect. egui's `Color32`
//! pixels are premultiplied (the colour is already multiplied by the alpha),
//! so they are converted on the way in.

// Standard library
use std::collections::HashMap;
use std::sync::Arc;

// External crates
use egui::{ImageData, TextureId as EguiTextureId, TexturesDelta};
use pill_core::telemetry::telemetry_target;
use pill_ui_frame::{TextureId, UiTextureFilter, UiTextureSet};

// Current crate
use crate::convert::external_id;

// =============================================================================
// MirroredTexture
// =============================================================================

/// The whole image of one texture, as a painter that starts with no textures
/// needs it.
#[derive(Clone, Debug)]
struct MirroredTexture {
    width: u32,
    height: u32,
    filter: UiTextureFilter,
    revision: u64,
    /// Straight-alpha sRGB RGBA8, `width * height * 4` bytes.
    pixels: Vec<u8>,
}

// =============================================================================
// TextureMirror
// =============================================================================

/// Every texture egui handed this UI, kept whole.
#[derive(Default)]
pub struct TextureMirror {
    textures: HashMap<EguiTextureId, MirroredTexture>,
    /// The textures to send on the next [`Self::carry`]: the ones whose
    /// content changed, or every held one after [`Self::resend_all`].
    dirty_texture_ids: Vec<EguiTextureId>,
    /// The textures egui freed since the last [`Self::carry`].
    freed_texture_ids: Vec<EguiTextureId>,
    /// The revision number the next change will get. Every change adds one to
    /// it, so revisions grow over time and two changes never share a number.
    revision_counter: u64,
}

impl TextureMirror {
    /// An empty mirror.
    pub fn new() -> TextureMirror {
        TextureMirror::default()
    }

    /// How many textures it holds.
    pub fn len(&self) -> usize {
        self.textures.len()
    }

    /// Whether it holds no textures.
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
    }

    /// Returns the revision `texture_id` is at, or `None` when the mirror
    /// does not hold it.
    pub fn revision(&self, texture_id: EguiTextureId) -> Option<u64> {
        self.textures
            .get(&texture_id)
            .map(|mirrored_texture| mirrored_texture.revision)
    }

    /// Folds one pass's texture deltas into the mirror.
    ///
    /// A delta with no position replaces the whole image: the texture is new
    /// or its size changed. A delta with a position changes part of the image
    /// in place. A patch for a texture the mirror has never seen cannot be
    /// placed, so it is logged and dropped. egui always sends the whole image
    /// before any patch.
    pub fn apply(&mut self, textures_delta: &TexturesDelta) {
        // Step 1: fold the image deltas of every changed texture into the
        // mirror.
        for (texture_id, deltas) in &textures_delta.set {
            for patch in deltas {
                let ImageData::Color(image) = &patch.image;
                let pixels = to_unpremultiplied_rgba8(&image.pixels);
                match patch.pos {
                    None => {
                        self.revision_counter += 1;
                        self.textures.insert(
                            *texture_id,
                            MirroredTexture {
                                width: image.size[0] as u32,
                                height: image.size[1] as u32,
                                filter: to_ui_texture_filter(&patch.options),
                                revision: self.revision_counter,
                                pixels,
                            },
                        );
                    }
                    Some([x, y]) => {
                        let Some(mirrored_texture) = self.textures.get_mut(texture_id) else {
                            pill_core::warn!(
                                target: telemetry_target::UI,
                                "[ui] a texture patch arrived before its whole image"
                            );
                            continue;
                        };
                        let (patch_width, patch_height) =
                            (image.size[0] as u32, image.size[1] as u32);
                        if x as u32 + patch_width > mirrored_texture.width
                            || y as u32 + patch_height > mirrored_texture.height
                        {
                            pill_core::warn!(
                                target: telemetry_target::UI,
                                "[ui] a texture patch fell outside the mirrored image"
                            );
                            continue;
                        }
                        let stride = mirrored_texture.width as usize * 4;
                        for row in 0..patch_height as usize {
                            let to = (y + row) * stride + x * 4;
                            let from = row * patch_width as usize * 4;
                            let end = from + patch_width as usize * 4;
                            mirrored_texture.pixels[to..to + patch_width as usize * 4]
                                .copy_from_slice(&pixels[from..end]);
                        }
                        self.revision_counter += 1;
                        mirrored_texture.revision = self.revision_counter;
                    }
                }
                if !self.dirty_texture_ids.contains(texture_id) {
                    self.dirty_texture_ids.push(*texture_id);
                }
            }
        }
        // Step 2: drop the textures egui freed, and remember them for the
        // next carry.
        for texture_id in &textures_delta.free {
            self.textures.remove(texture_id);
            self.dirty_texture_ids.retain(|id| id != texture_id);
            if !self.freed_texture_ids.contains(texture_id) {
                self.freed_texture_ids.push(*texture_id);
            }
        }
    }

    /// Marks every held image for sending again.
    ///
    /// A painter that is rebuilt holds no textures, and nothing marks the
    /// existing images as changed, so the next [`carry`](Self::carry) would
    /// send nothing. This marks every image instead.
    pub fn resend_all(&mut self) {
        self.dirty_texture_ids.clear();
        self.dirty_texture_ids.extend(self.textures.keys().copied());
    }

    /// Appends the changed images to `texture_set`, each as a whole image, and
    /// the textures egui freed as frees. Both lists are then emptied.
    pub fn carry(&mut self, texture_set: &mut UiTextureSet) {
        // Step 1: send every changed image whole.
        for texture_id in self.dirty_texture_ids.drain(..) {
            let Some(mirrored_texture) = self.textures.get(&texture_id) else {
                continue;
            };
            let pixels: Arc<[u8]> = Arc::from(mirrored_texture.pixels.as_slice());
            texture_set.push_full(
                TextureId(external_id(texture_id)),
                mirrored_texture.width,
                mirrored_texture.height,
                mirrored_texture.revision,
                mirrored_texture.filter,
                pixels,
            );
        }
        // Step 2: tell the painter which textures egui freed.
        for texture_id in self.freed_texture_ids.drain(..) {
            texture_set.push_free(TextureId(external_id(texture_id)));
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Converts egui's premultiplied sRGB pixels to straight-alpha RGBA8.
fn to_unpremultiplied_rgba8(pixels: &[egui::Color32]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels.len() * 4);
    for pixel in pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    rgba
}

/// Returns the filter mode `texture_options` asks for.
///
/// The frame carries one filter per texture, so only the magnification
/// setting is read.
fn to_ui_texture_filter(texture_options: &egui::TextureOptions) -> UiTextureFilter {
    if texture_options.magnification == egui::TextureFilter::Nearest {
        UiTextureFilter::Nearest
    } else {
        UiTextureFilter::Linear
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::ImageDelta;
    use egui::{Color32, ColorImage};

    /// A whole-image delta of a square image `side_pixels` wide, filled with
    /// `color`.
    fn whole_image_delta(side_pixels: usize, color: Color32) -> ImageDelta {
        ImageDelta::full(
            ColorImage {
                size: [side_pixels, side_pixels],
                source_size: egui::vec2(side_pixels as f32, side_pixels as f32),
                pixels: vec![color; side_pixels * side_pixels],
            },
            egui::TextureOptions::NEAREST,
        )
    }

    /// A patch delta of a square image `side_pixels` wide, filled with
    /// `color`, whose top-left pixel sits at `position`.
    fn patch_image_delta(side_pixels: usize, position: [usize; 2], color: Color32) -> ImageDelta {
        ImageDelta {
            image: ImageData::Color(Arc::new(ColorImage {
                size: [side_pixels, side_pixels],
                source_size: egui::vec2(side_pixels as f32, side_pixels as f32),
                pixels: vec![color; side_pixels * side_pixels],
            })),
            options: egui::TextureOptions::NEAREST,
            pos: Some(position),
        }
    }

    /// A delta carrying one whole image for `texture_id`.
    fn whole_image_textures_delta(
        texture_id: EguiTextureId,
        side_pixels: usize,
        color: Color32,
    ) -> TexturesDelta {
        let mut textures_delta = TexturesDelta::default();
        textures_delta.push(texture_id, whole_image_delta(side_pixels, color));
        textures_delta
    }

    /// A delta carrying one patch for `texture_id`.
    fn patch_textures_delta(
        texture_id: EguiTextureId,
        side_pixels: usize,
        position: [usize; 2],
        color: Color32,
    ) -> TexturesDelta {
        let mut textures_delta = TexturesDelta::default();
        textures_delta.push(texture_id, patch_image_delta(side_pixels, position, color));
        textures_delta
    }

    #[test]
    fn a_whole_image_is_carried_whole() {
        let mut mirror = TextureMirror::new();
        let mut textures_delta =
            whole_image_textures_delta(EguiTextureId::Managed(0), 2, Color32::RED);
        mirror.apply(&textures_delta);
        textures_delta.clear();
        let mut texture_set = UiTextureSet::new();
        mirror.carry(&mut texture_set);
        assert_eq!(texture_set.textures.len(), 1);
        let texture = &texture_set.textures[0];
        assert_eq!(
            texture.id,
            TextureId(external_id(EguiTextureId::Managed(0)))
        );
        assert_eq!((texture.width, texture.height), (2, 2));
        assert_eq!(texture.pixels.len(), 16);
        assert_eq!(texture.filter, UiTextureFilter::Nearest);

        // Carrying the same mirror again with nothing changed produces
        // nothing.
        let mut empty = UiTextureSet::new();
        mirror.carry(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_patch_updates_the_mirrored_image_in_place() {
        let mut mirror = TextureMirror::new();
        let mut textures_delta =
            whole_image_textures_delta(EguiTextureId::Managed(0), 4, Color32::BLACK);
        mirror.apply(&textures_delta);
        textures_delta.clear();
        let mut first = UiTextureSet::new();
        mirror.carry(&mut first);
        let revision = first.textures[0].revision;

        let mut patch_delta =
            patch_textures_delta(EguiTextureId::Managed(0), 2, [1, 1], Color32::WHITE);
        mirror.apply(&patch_delta);
        patch_delta.clear();
        let mut second = UiTextureSet::new();
        mirror.carry(&mut second);
        let texture = &second.textures[0];
        assert!(texture.revision > revision, "the patch bumps the revision");
        // A patched texture still carries its whole image.
        let full = &texture.pixels;
        // The 2x2 patch covers rows 1 and 2 and columns 1 and 2. The checks
        // below read two pixels inside it and two outside.
        let at = |x: usize, y: usize| full[(y * 4 + x) * 4..(y * 4 + x) * 4 + 4].to_vec();
        assert_eq!(at(1, 1), vec![255, 255, 255, 255]);
        assert_eq!(at(2, 1), vec![255, 255, 255, 255]);
        assert_eq!(at(0, 0), vec![0, 0, 0, 255]);
        assert_eq!(at(1, 0), vec![0, 0, 0, 255]);
    }

    #[test]
    fn a_free_is_carried_and_forgets_the_image() {
        let mut mirror = TextureMirror::new();
        let mut textures_delta =
            whole_image_textures_delta(EguiTextureId::User(3), 1, Color32::WHITE);
        mirror.apply(&textures_delta);
        textures_delta.clear();
        let mut carried = UiTextureSet::new();
        mirror.carry(&mut carried);

        let mut free_textures_delta = TexturesDelta::default();
        free_textures_delta.free(EguiTextureId::User(3));
        mirror.apply(&free_textures_delta);
        free_textures_delta.clear();
        assert!(mirror.is_empty());
        let mut texture_set = UiTextureSet::new();
        mirror.carry(&mut texture_set);
        assert_eq!(texture_set.frees, vec![TextureId(3)]);
        assert!(texture_set.textures.is_empty());
    }

    #[test]
    fn a_patch_before_its_whole_image_is_dropped() {
        let mut mirror = TextureMirror::new();
        let mut textures_delta =
            patch_textures_delta(EguiTextureId::Managed(1), 1, [0, 0], Color32::WHITE);
        mirror.apply(&textures_delta);
        textures_delta.clear();
        assert!(mirror.is_empty());
        let mut texture_set = UiTextureSet::new();
        mirror.carry(&mut texture_set);
        assert!(texture_set.is_empty());
    }

    /// Checks that a resend sends every held image again.
    #[test]
    fn resend_all_carries_every_held_image_again() {
        let mut mirror = TextureMirror::new();
        for id in [EguiTextureId::Managed(0), EguiTextureId::User(7)] {
            let mut textures_delta = whole_image_textures_delta(id, 2, Color32::WHITE);
            mirror.apply(&textures_delta);
            textures_delta.clear();
        }

        // The first carry sends both images and empties the changed list, so
        // a second carry sends nothing. This is why a painter that starts
        // empty needs a resend.
        let mut first = UiTextureSet::new();
        mirror.carry(&mut first);
        assert_eq!(first.textures.len(), 2);
        let mut quiet = UiTextureSet::new();
        mirror.carry(&mut quiet);
        assert!(quiet.is_empty(), "nothing changed, so nothing carries");

        mirror.resend_all();
        let mut resent = UiTextureSet::new();
        mirror.carry(&mut resent);
        assert_eq!(
            resent.textures.len(),
            2,
            "a painter that starts empty gets every image back"
        );
        assert_eq!(
            resent.textures[0].pixels.len(),
            2 * 2 * 4,
            "and each one is the whole image, not a patch"
        );
    }
}
