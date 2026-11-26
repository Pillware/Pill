//! The RGB565 copy of every texture the frame samples.
//!
//! # Responsibilities
//!
//! - Convert a colour or normal texture's 8-bit texels into packed RGB565 once,
//!   when the asset's content version moves ([`TextureCache::ensure`]).
//! - Sample the packed copy with nearest-neighbour filtering
//!   ([`CachedTexture::sample_nearest`]).
//! - Reuse each entry's buffer, so a texture re-imported mid-run allocates no
//!   new memory.
//!
//! # Design
//!
//! A texture is converted once per content version, not once per frame: a
//! 256x256 base-colour map is 128 KiB of RGB565, and rebuilding it every frame
//! would cost more than drawing with it. The cache is keyed by the asset key
//! the frame carries, which packs the handle's index and generation, so a slot
//! reused for a new texture never reads the old texels.
//!
//! Only colour and normal textures are converted. An environment texture holds
//! half-float texels and is never a material's base-colour map, so it is left
//! out instead of being misread as 8-bit channels.

// External crates
use pill_engine::{AssetManager, Handle};
use pill_rendering::assets::Texture;

// Current crate
use crate::framebuffer::pack_rgb565;

// =============================================================================
// CachedTexture
// =============================================================================

/// One texture's packed RGB565 texels.
pub struct CachedTexture {
    /// The asset key the texels were converted from.
    key: u64,
    /// The asset's content version when they were converted.
    version: u64,
    /// Texels across.
    width: u32,
    /// Texel rows.
    height: u32,
    /// Packed RGB565 texels, `width * height` of them, row-major from the top.
    texels: Vec<u16>,
}

impl CachedTexture {
    /// Reads the texel a texture coordinate points at, with no filtering.
    ///
    /// `(0.0, 0.0)` is the top-left texel and `(1.0, 1.0)` the bottom-right
    /// one, which is the orientation the mesh decoder produces. A coordinate
    /// outside `0.0..=1.0` reads the nearest edge texel, as a clamped sampler
    /// does on the GPU.
    #[inline]
    pub fn sample_nearest(&self, u: f32, v: f32) -> u16 {
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 {
            return 0;
        }
        let x = ((u.clamp(0.0, 1.0) * width as f32) as usize).min(width - 1);
        let y = ((v.clamp(0.0, 1.0) * height as f32) as usize).min(height - 1);
        self.texels[y * width + x]
    }

    /// Texels across.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Texel rows.
    pub fn height(&self) -> u32 {
        self.height
    }
}

// =============================================================================
// TextureCache
// =============================================================================

/// The packed copies of the textures a frame samples.
///
/// Entries are never removed while the renderer draws: a texture that leaves
/// the frame is forgotten only by [`TextureCache::clear`], which
/// [`crate::EmbeddedRenderer::invalidate_assets`] calls. The list is short - a
/// scene samples the maps its materials name - so a linear search costs less
/// than the bookkeeping of a hash map.
#[derive(Default)]
pub struct TextureCache {
    entries: Vec<CachedTexture>,
}

impl TextureCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every cached texture.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns the index of `handle`'s packed copy, converting it first when
    /// the cache holds no copy at the asset's current version.
    ///
    /// `None` when the handle names no live texture, when the texture is not an
    /// 8-bit colour or normal map, or when its texel count does not match its
    /// dimensions.
    pub fn ensure(&mut self, assets: &AssetManager, handle: Handle<Texture>) -> Option<usize> {
        let version = assets.content_version(handle)?;
        let texture = assets.get(handle)?;
        let key = pill_rendering::assets::asset_key(handle);

        // Step 1: A copy converted from this version of the asset is correct
        // already.
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.key == key && entry.version == version)
        {
            return Some(index);
        }

        // Step 2: Convert the texels. A half-float environment texture and a
        // texel count that disagrees with the dimensions are refused rather
        // than misread.
        let (width, height) = (texture.width, texture.height);
        if texture.texture_type.is_high_dynamic_range() {
            return None;
        }
        let texel_count = width as usize * height as usize;
        if texel_count == 0 || texture.rgba.len() != texel_count * 4 {
            return None;
        }

        // Step 3: Reuse the entry this key already had - a re-imported texture
        // keeps its place and its buffer - or add one.
        let index = match self.entries.iter().position(|entry| entry.key == key) {
            Some(index) => index,
            None => {
                self.entries.push(CachedTexture {
                    key,
                    version,
                    width: 0,
                    height: 0,
                    texels: Vec::new(),
                });
                self.entries.len() - 1
            }
        };
        let entry = &mut self.entries[index];
        entry.version = version;
        entry.width = width;
        entry.height = height;
        entry.texels.clear();
        entry
            .texels
            .extend(texture.rgba.as_chunks::<4>().0.iter().map(|texel| {
                let channel = |index: usize| texel[index] as f32 / 255.0;
                pack_rgb565(channel(0), channel(1), channel(2))
            }));
        Some(index)
    }

    /// Returns the copy at `index`, which a previous [`Self::ensure`] returned.
    pub fn entry(&self, index: usize) -> Option<&CachedTexture> {
        self.entries.get(index)
    }

    /// How many textures are cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_rendering::assets::TextureType;

    /// A texture whose texels are distinct colours, one per corner.
    fn checkerboard(assets: &mut AssetManager) -> Handle<Texture> {
        let rgba = vec![
            255, 0, 0, 255, // top-left red
            0, 255, 0, 255, // top-right green
            0, 0, 255, 255, // bottom-left blue
            255, 255, 255, 255, // bottom-right white
        ];
        assets
            .add_named(
                "checker",
                Texture::from_rgba("checker", TextureType::Color, rgba, 2, 2)
                    .expect("a 2x2 RGBA texture"),
            )
            .expect("a free name")
    }

    /// Each corner of the texture reads the texel stored there.
    #[test]
    fn nearest_sampling_reads_the_texel_under_the_coordinate() {
        let mut assets = AssetManager::new();
        let handle = checkerboard(&mut assets);
        let mut cache = TextureCache::new();

        let index = cache.ensure(&assets, handle).expect("a colour texture");
        let texture = cache.entry(index).expect("the entry just added");

        assert_eq!(texture.sample_nearest(0.0, 0.0), pack_rgb565(1.0, 0.0, 0.0));
        assert_eq!(texture.sample_nearest(0.9, 0.0), pack_rgb565(0.0, 1.0, 0.0));
        assert_eq!(texture.sample_nearest(0.0, 0.9), pack_rgb565(0.0, 0.0, 1.0));
        assert_eq!(texture.sample_nearest(0.9, 0.9), pack_rgb565(1.0, 1.0, 1.0));
    }

    /// A coordinate outside the texture reads the edge texel instead of
    /// wrapping to the opposite side.
    #[test]
    fn sampling_clamps_to_the_edge() {
        let mut assets = AssetManager::new();
        let handle = checkerboard(&mut assets);
        let mut cache = TextureCache::new();
        let index = cache.ensure(&assets, handle).expect("a colour texture");
        let texture = cache.entry(index).expect("the entry just added");

        assert_eq!(
            texture.sample_nearest(-3.0, -3.0),
            pack_rgb565(1.0, 0.0, 0.0)
        );
        assert_eq!(texture.sample_nearest(4.0, 4.0), pack_rgb565(1.0, 1.0, 1.0));
    }

    /// Asking for the same texture twice converts it once.
    #[test]
    fn a_texture_is_converted_once_per_content_version() {
        let mut assets = AssetManager::new();
        let handle = checkerboard(&mut assets);
        let mut cache = TextureCache::new();

        let first = cache.ensure(&assets, handle).expect("a colour texture");
        let second = cache.ensure(&assets, handle).expect("a colour texture");

        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);
    }

    /// A texture that changes content is converted again, in the same entry.
    #[test]
    fn a_changed_texture_is_converted_again_in_place() {
        let mut assets = AssetManager::new();
        let handle = checkerboard(&mut assets);
        let mut cache = TextureCache::new();
        let index = cache.ensure(&assets, handle).expect("a colour texture");

        assets
            .get_mut(handle)
            .expect("still loaded")
            .rgba
            .copy_from_slice(&[9, 9, 9, 255, 9, 9, 9, 255, 9, 9, 9, 255, 9, 9, 9, 255]);
        let changed = cache.ensure(&assets, handle).expect("a colour texture");

        assert_eq!(changed, index);
        assert_eq!(cache.len(), 1);
        assert_eq!(
            cache
                .entry(changed)
                .expect("the entry")
                .sample_nearest(0.0, 0.0),
            pack_rgb565(9.0 / 255.0, 9.0 / 255.0, 9.0 / 255.0)
        );
    }

    /// An environment texture holds half floats, so it is refused instead of
    /// read as 8-bit channels.
    #[test]
    fn an_environment_texture_is_not_cached() {
        let mut assets = AssetManager::new();
        let handle = assets
            .add_named(
                "sky",
                Texture::from_rgba("sky", TextureType::Equirect, vec![0; 8], 1, 1)
                    .expect("a 1x1 half-float panorama"),
            )
            .expect("a free name");
        let mut cache = TextureCache::new();

        assert_eq!(cache.ensure(&assets, handle), None);
        assert!(cache.is_empty());
    }
}
