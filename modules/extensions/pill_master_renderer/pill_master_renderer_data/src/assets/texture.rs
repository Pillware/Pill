//! Texture assets: the texels the renderer uploads, and how they are read.
//!
//! # Responsibilities
//!
//! - Hold one texture's texels, their dimensions, and the name the asset is
//!   known by.
//! - Say how those texels are read ([`TextureType`]): as colour, as a normal
//!   map, or as an environment (an equirectangular panorama or a cubemap) for
//!   a skybox.
//! - Build a texture by decoding an image file ([`Texture::new`]), from an
//!   RGBA8 buffer ([`Texture::from_rgba`]), or from linear float texels
//!   ([`Texture::from_rgba_f32`]). A cubemap built this way is projected onto
//!   its six faces with the direction convention the skybox shaders sample
//!   by.
//! - Read the import settings from a metadata file ([`ImportedAsset`]):
//!   [`TextureImportSettings`] in `<image>.meta` says how to read the texels.
//!
//! # Design
//!
//! A texture here is plain data with no GPU state: the renderer keeps the
//! texture, view and sampler beside it and rebuilds them when the asset's
//! version changes. [`TextureType`] travels with the texels because the bytes
//! alone do not say how to read them. The type decides the texel layout, the
//! GPU format the texture is uploaded as, and how a shader may sample it.
//!
//! An environment texture holds high dynamic range values, so it is stored as
//! half floats: the widest format every backend can filter. A cubemap is
//! projected from its panorama once, at import, instead of on every frame.

// Standard library
use std::f32::consts::PI;

// External crates
use glam::Vec3;
use pill_engine::{
    asset, pill_mirror_impl, pill_mirror_method, pill_mirror_object, AssetLoadError,
    AssetLoadResult, AssetLoader, ImportedAsset, PillMirror,
};
use serde::{Deserialize, Serialize};

// Current crate
use crate::color_space::srgb_to_linear;

// =============================================================================
// Constants
// =============================================================================

/// The file the renderer's colour stand-in is read from. It holds one white
/// texel, for a slot that binds nothing.
pub const DEFAULT_COLOR_TEXTURE: &str = "textures/default_color.png";

/// The file the renderer's normal stand-in is read from. It holds one texel of
/// `(128, 128, 255)`, the flat normal.
pub const DEFAULT_NORMAL_TEXTURE: &str = "textures/default_normal.png";

// =============================================================================
// TextureType
// =============================================================================

/// How a texture's texels are meant to be read.
///
/// The type travels with the texels because the bytes alone do not say how to
/// interpret them. It decides the texel layout and the GPU format the texture
/// is uploaded as. It also decides the binding a shader receives for a texture
/// slot, which keeps colour, normal and environment data from being read the
/// wrong way.
///
/// Serialized by variant name (`"Color"`, `"Normal"`, `"Equirect"`,
/// `"Cubemap"`) in a texture's metadata file, so renaming a variant changes the
/// on-disk format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PillMirror)]
#[repr(u8)]
pub enum TextureType {
    /// A colour image: albedo, UI, or any texture sampled for its values. It is
    /// uploaded as sRGB, so the hardware converts it to linear light before a
    /// shader reads it.
    Color,
    /// A normal map: its bytes are directions, not colours. It is uploaded as
    /// linear data, so a shader reads them exactly as stored.
    Normal,
    /// An equirectangular panorama: the whole sphere of directions around a
    /// point, laid out in one 2:1 image, longitude across and latitude down.
    ///
    /// It is high dynamic range, so it is stored and uploaded as linear half
    /// floats. A Radiance `.hdr` sky then keeps the values above 1.0 where its
    /// sun is. A shader samples the panorama by turning a direction into a uv
    /// ([`equirect_uv`]).
    Equirect,
    /// A cubemap: six square faces around a point, in the order `+X`, `-X`,
    /// `+Y`, `-Y`, `+Z`, `-Z`, sampled by direction.
    ///
    /// It is made from an equirectangular panorama, which is projected onto the
    /// faces at import ([`project_equirect_to_cubemap`]). Like
    /// [`Self::Equirect`], it holds high dynamic range values. Near the poles a
    /// panorama packs many directions into few texels; a cubemap uses the same
    /// number of texels for every direction.
    Cubemap,
}

impl TextureType {
    /// How many bytes one texel takes: four 8-bit channels, or four half-float
    /// channels for an environment texture.
    pub fn bytes_per_texel(self) -> usize {
        match self {
            TextureType::Color | TextureType::Normal => 4,
            TextureType::Equirect | TextureType::Cubemap => 8,
        }
    }

    /// How many images the texture holds: six faces for a cubemap, one
    /// otherwise.
    pub fn layer_count(self) -> u32 {
        match self {
            TextureType::Cubemap => 6,
            _ => 1,
        }
    }

    /// Whether the texels are linear half floats rather than 8-bit channels.
    pub fn is_high_dynamic_range(self) -> bool {
        matches!(self, TextureType::Equirect | TextureType::Cubemap)
    }
}

// =============================================================================
// Texture
// =============================================================================

/// Decoded texels under a name, ready for the renderer to upload.
///
/// This is plain data, not a GPU resource. The renderer keeps the matching
/// texture, view and sampler, and rebuilds them when the asset's version
/// changes. A game therefore edits the texture it holds and never touches GPU
/// state.
#[asset(imported)]
#[pill_mirror_object(asset, import, shared = "pill_master_renderer::assets::Texture")]
#[derive(Clone, Debug)]
pub struct Texture {
    /// Label for the GPU resource. The logs and error messages use it too.
    pub name: String,
    /// The texels in RGBA order, laid out as [`Self::texture_type`] says.
    /// Colour and normal maps use four bytes per texel, an environment uses
    /// four little-endian half floats, and a cubemap holds its six faces one
    /// after another. The constructors check the length against the dimensions
    /// before a texture exists.
    pub rgba: Vec<u8>,
    /// Width in texels; for a cubemap, of each face.
    pub width: u32,
    /// Height in texels; for a cubemap, of each face.
    pub height: u32,
    /// How a shader should read the texels.
    pub texture_type: TextureType,
}

#[pill_mirror_impl]
impl Texture {
    /// Builds a texture by decoding an image file.
    ///
    /// The loader supplies the bytes, and the texture keeps the name it is
    /// given, so a decode failure names the asset being loaded instead of an
    /// anonymous buffer. A colour or normal map becomes RGBA8. An environment
    /// becomes linear half floats, and a cubemap is first projected from the
    /// image as a panorama.
    ///
    /// # Errors
    ///
    /// Returns whatever the loader reports when the file is missing or
    /// unreadable, and [`AssetLoadError::Decode`] when the bytes are not an
    /// image this crate can decode.
    #[pill_mirror_method]
    pub fn new(
        name: impl Into<String>,
        texture_type: TextureType,
        loader: AssetLoader,
    ) -> AssetLoadResult<Self> {
        let bytes = loader.load()?;
        Self::decode(name.into(), texture_type, &bytes)
    }

    /// Decodes an encoded image (PNG, JPEG or Radiance HDR) into a texture of
    /// `texture_type`.
    ///
    /// This is the one decode path, shared by [`Self::new`] and the metadata
    /// import.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when the bytes are not an image this
    /// crate can decode.
    fn decode(name: String, texture_type: TextureType, bytes: &[u8]) -> AssetLoadResult<Self> {
        // Step 1: Decode the file, keeping the texture's name for the error.
        let image = image::load_from_memory(bytes).map_err(|error| AssetLoadError::Decode {
            label: name.clone(),
            detail: error.to_string(),
        })?;
        // Step 2: An 8-bit colour or normal map is stored as RGBA8 texels.
        if !texture_type.is_high_dynamic_range() {
            let image = image.to_rgba8();
            let (width, height) = image.dimensions();
            return Self::from_rgba(name, texture_type, image.into_raw(), width, height);
        }

        // Step 3: An environment is stored as linear half floats. An 8-bit
        // image holds sRGB-encoded values, and reading those as linear light
        // would brighten every midtone, so the colour channels are converted. A
        // float image (Radiance HDR) already holds linear values.
        let is_float_source = matches!(
            image.color(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        );
        let image = image.to_rgba32f();
        let (width, height) = image.dimensions();
        let mut texels = image.into_raw();
        if !is_float_source {
            for texel in texels.chunks_exact_mut(4) {
                for channel in &mut texel[..3] {
                    *channel = srgb_to_linear(*channel);
                }
            }
        }
        Self::from_rgba_f32(name, texture_type, &texels, width, height)
    }

    /// Wraps a texel buffer as a texture and checks it against the declared
    /// dimensions.
    ///
    /// Use this for texels that were made rather than loaded, such as a
    /// procedural image or a single-colour stand-in, where no file exists to
    /// decode. The buffer is laid out as [`Self::rgba`] describes for
    /// `texture_type`.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when the buffer's length is not what
    /// the dimensions and the type call for, or when a cubemap's faces are not
    /// square. The GPU upload checks the length again, but it can only report a
    /// mismatch as an upload failure. Checking here names the texture while its
    /// own numbers are still at hand.
    #[pill_mirror_method]
    pub fn from_rgba(
        name: impl Into<String>,
        texture_type: TextureType,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    ) -> AssetLoadResult<Self> {
        let name = name.into();
        let expected_bytes = expected_byte_count(texture_type, width, height);
        if rgba.len() != expected_bytes {
            return Err(AssetLoadError::Decode {
                label: name,
                detail: format!(
                    "declares {width}x{height} {texture_type:?} but carries {} bytes (expected {expected_bytes})",
                    rgba.len()
                ),
            });
        }
        if texture_type == TextureType::Cubemap && width != height {
            return Err(AssetLoadError::Decode {
                label: name,
                detail: format!("a cubemap's faces are square, but this one is {width}x{height}"),
            });
        }
        Ok(Self {
            name,
            rgba,
            width,
            height,
            texture_type,
        })
    }

    /// Builds an environment texture from linear RGBA float texels of an
    /// equirectangular panorama.
    ///
    /// A [`TextureType::Equirect`] texture keeps the panorama as it is. A
    /// [`TextureType::Cubemap`] is projected from it, with faces a quarter of
    /// the panorama's width.
    ///
    /// # Errors
    ///
    /// Returns [`AssetLoadError::Decode`] when `texture_type` is not an
    /// environment, or when `rgba` does not hold `width * height` texels of
    /// four channels each.
    #[pill_mirror_method]
    pub fn from_rgba_f32(
        name: impl Into<String>,
        texture_type: TextureType,
        rgba: &[f32],
        width: u32,
        height: u32,
    ) -> AssetLoadResult<Self> {
        let name = name.into();
        if !texture_type.is_high_dynamic_range() {
            return Err(AssetLoadError::Decode {
                label: name,
                detail: format!("float texels make an Equirect or a Cubemap, not {texture_type:?}"),
            });
        }
        let expected_floats = width as usize * height as usize * 4;
        if rgba.len() != expected_floats {
            return Err(AssetLoadError::Decode {
                label: name,
                detail: format!(
                    "declares a {width}x{height} panorama but carries {} floats (expected {expected_floats})",
                    rgba.len()
                ),
            });
        }

        let (texels, width, height) = match texture_type {
            TextureType::Cubemap => {
                let face_size = (width / 4).max(1);
                let faces = project_equirect_to_cubemap(rgba, width, height, face_size);
                (faces, face_size, face_size)
            }
            _ => (rgba.to_vec(), width, height),
        };
        let bytes = texels
            .iter()
            .flat_map(|value| half::f16::from_f32(*value).to_le_bytes())
            .collect();
        Self::from_rgba(name, texture_type, bytes, width, height)
    }
}

// The metadata type name defaults to the shared name declared above, so every
// binary's copy of `Texture` reads the same files.
impl ImportedAsset for Texture {
    type ImportSettings = TextureImportSettings;
    // The formats this crate's `image` features decode.
    const SOURCE_EXTENSIONS: &'static [&'static str] = &["png", "jpg", "jpeg", "hdr"];

    fn import(
        name: &str,
        source_bytes: &[u8],
        settings: &TextureImportSettings,
    ) -> AssetLoadResult<Self> {
        Self::decode(name.to_owned(), settings.texture_type, source_bytes)
    }
}

// =============================================================================
// TextureImportSettings
// =============================================================================

/// How to read an image file as a texture: what the `.meta` file beside it
/// holds.
///
/// Every field is part of the metadata file format, so renaming one breaks
/// existing files. `#[serde(default)]` lets a file that leaves a field out
/// still load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureImportSettings {
    /// How the texels are read ([`TextureType`]). A file that names `Depth` is
    /// refused: depth is a slot's binding kind, and no image file holds depth.
    /// `Equirect` and `Cubemap` read the image as a panorama, usually a
    /// Radiance `.hdr` sky.
    pub texture_type: TextureType,
}

impl Default for TextureImportSettings {
    fn default() -> Self {
        Self {
            texture_type: TextureType::Color,
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The number of bytes a texture of this type and size holds.
fn expected_byte_count(texture_type: TextureType, width: u32, height: u32) -> usize {
    width as usize
        * height as usize
        * texture_type.bytes_per_texel()
        * texture_type.layer_count() as usize
}

/// Returns the equirectangular uv that a direction samples, in `[0, 1]`.
///
/// Looking down `-Z` maps to the middle of the panorama (`u = 0.5`), `+X` maps
/// three quarters of the way across, and straight up maps to the top row. The
/// skybox shaders use the same formula, so a cubemap projected here shows the
/// same sky as the panorama it came from.
pub fn equirect_uv(direction: Vec3) -> (f32, f32) {
    let direction = direction.normalize_or(Vec3::NEG_Z);
    let u = 0.5 + direction.x.atan2(-direction.z) / (2.0 * PI);
    let v = 0.5 - direction.y.clamp(-1.0, 1.0).asin() / PI;
    (u, v)
}

/// Returns the direction a texel of a cubemap face looks along.
///
/// `face` is the layer index: `+X`, `-X`, `+Y`, `-Y`, `+Z`, `-Z`. `s` and `t`
/// run from -1 to 1 across the face, left to right and top to bottom. This is
/// the cube face table GPUs sample by, so a texel stored here is the one a
/// shader reads for that direction.
pub fn cube_face_direction(face: usize, s: f32, t: f32) -> Vec3 {
    let direction = match face {
        0 => Vec3::new(1.0, -t, -s),
        1 => Vec3::new(-1.0, -t, s),
        2 => Vec3::new(s, 1.0, t),
        3 => Vec3::new(s, -1.0, -t),
        4 => Vec3::new(s, -t, 1.0),
        _ => Vec3::new(-s, -t, -1.0),
    };
    direction.normalize()
}

/// Projects an equirectangular panorama onto the six faces of a cubemap.
///
/// Takes linear RGBA float texels of a `width` x `height` panorama and returns
/// the faces, each `face_size` square, one after another in layer order. Every
/// face texel samples the panorama bilinearly along its own direction. The
/// sampling wraps around the panorama's seam and clamps at its poles.
pub fn project_equirect_to_cubemap(
    panorama: &[f32],
    width: u32,
    height: u32,
    face_size: u32,
) -> Vec<f32> {
    let face_size = face_size.max(1);
    let mut faces = Vec::with_capacity(6 * face_size as usize * face_size as usize * 4);
    for face in 0..6 {
        for row in 0..face_size {
            for column in 0..face_size {
                // Texel centres, mapped onto -1..1 across the face.
                let s = 2.0 * (column as f32 + 0.5) / face_size as f32 - 1.0;
                let t = 2.0 * (row as f32 + 0.5) / face_size as f32 - 1.0;
                let (u, v) = equirect_uv(cube_face_direction(face, s, t));
                faces.extend_from_slice(&sample_bilinear(panorama, width, height, u, v));
            }
        }
    }
    faces
}

/// Reads a panorama at `(u, v)` with bilinear filtering.
///
/// `u` wraps around the seam and `v` clamps at the poles, so a sample never
/// leaves the image.
fn sample_bilinear(panorama: &[f32], width: u32, height: u32, u: f32, v: f32) -> [f32; 4] {
    let x = u * width as f32 - 0.5;
    let y = (v * height as f32 - 0.5).clamp(0.0, (height - 1) as f32);
    let x_floor = x.floor();
    let y_floor = y.floor();
    let x_blend = x - x_floor;
    let y_blend = y - y_floor;

    let column = |offset: i64| (x_floor as i64 + offset).rem_euclid(width as i64) as usize;
    let row = |offset: i64| (y_floor as i64 + offset).clamp(0, height as i64 - 1) as usize;
    let texel = |column: usize, row: usize| {
        let start = (row * width as usize + column) * 4;
        &panorama[start..start + 4]
    };

    let mut result = [0.0; 4];
    for (channel, value) in result.iter_mut().enumerate() {
        let top = texel(column(0), row(0))[channel] * (1.0 - x_blend)
            + texel(column(1), row(0))[channel] * x_blend;
        let bottom = texel(column(0), row(1))[channel] * (1.0 - x_blend)
            + texel(column(1), row(1))[channel] * x_blend;
        *value = top * (1.0 - y_blend) + bottom * y_blend;
    }
    result
}

/// Decodes a texture the module ships, read from `path` under its `res` root.
///
/// The renderer's stand-in textures are content like the default material: a
/// file the module declares. They are read here because this crate's store is
/// the one that knows the module's roots. A caller that cannot use the result,
/// because the build has no such file or the image will not decode, falls back
/// to the texel it builds in code.
///
/// # Errors
///
/// Fails when the file does not resolve through the store, or when it does not
/// decode.
pub fn load_shipped_texture(path: &str, texture_type: TextureType) -> AssetLoadResult<Texture> {
    let bytes = pill_engine::asset_store::read(std::path::Path::new(path))?;
    Texture::decode(path.to_owned(), texture_type, &bytes)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 PNG, encoded in the test so no fixture file is needed.
    fn one_pixel_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// A panorama whose every texel stores the direction it looks along, so a
    /// projected face can be checked against the direction of its own texel.
    fn direction_panorama(width: u32, height: u32) -> Vec<f32> {
        let mut texels = Vec::new();
        for row in 0..height {
            for column in 0..width {
                let u = (column as f32 + 0.5) / width as f32;
                let v = (row as f32 + 0.5) / height as f32;
                // The inverse of `equirect_uv`.
                let longitude = (u - 0.5) * 2.0 * PI;
                let latitude = (0.5 - v) * PI;
                texels.extend_from_slice(&[
                    latitude.cos() * longitude.sin(),
                    latitude.sin(),
                    -latitude.cos() * longitude.cos(),
                    1.0,
                ]);
            }
        }
        texels
    }

    #[test]
    fn import_reads_the_texture_type_from_the_settings() {
        let settings = TextureImportSettings {
            texture_type: TextureType::Normal,
        };
        let texture = Texture::import("textures/n.png", &one_pixel_png(), &settings).unwrap();
        assert_eq!(texture.texture_type, TextureType::Normal);
        assert_eq!(texture.name, "textures/n.png");
        assert_eq!(texture.rgba, [10, 20, 30, 255]);
    }

    /// A `.meta` that asks for a depth buffer fails to load. Depth is a slot's
    /// binding kind, not a texture asset, so no image file can be one.
    #[test]
    fn a_depth_texture_asset_does_not_deserialize() {
        assert!(
            serde_json::from_str::<TextureImportSettings>(r#"{"texture_type":"Depth"}"#).is_err(),
            "`Depth` is not a texture asset type; an image file holds color, normal or environment data"
        );
    }

    /// The on-disk form: the variant name as written, and the default for a
    /// missing field.
    #[test]
    fn settings_serialize_by_variant_name() {
        let settings = TextureImportSettings {
            texture_type: TextureType::Cubemap,
        };
        assert_eq!(
            serde_json::to_string(&settings).unwrap(),
            r#"{"texture_type":"Cubemap"}"#
        );
        let defaulted: TextureImportSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(defaulted.texture_type, TextureType::Color);
    }

    /// An 8-bit image read as an environment is converted to linear light and
    /// stored as half floats. The conversion applies to the colour channels;
    /// the alpha channel is stored as it is.
    #[test]
    fn an_eight_bit_image_imports_as_a_linear_equirect() {
        let settings = TextureImportSettings {
            texture_type: TextureType::Equirect,
        };
        let texture = Texture::import("sky.png", &one_pixel_png(), &settings).unwrap();

        assert_eq!(texture.rgba.len(), 8);
        let channel = |index: usize| {
            half::f16::from_le_bytes([texture.rgba[index * 2], texture.rgba[index * 2 + 1]])
                .to_f32()
        };
        assert!((channel(0) - srgb_to_linear(10.0 / 255.0)).abs() < 1.0e-3);
        assert!((channel(3) - 1.0).abs() < 1.0e-3);
    }

    /// A Radiance file keeps values above 1.0, which is why skies are HDR.
    #[test]
    fn a_radiance_file_keeps_its_range() {
        let mut bytes = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&[image::Rgb([8.0f32, 0.5, 0.25]); 2], 2, 1)
            .unwrap();
        let settings = TextureImportSettings {
            texture_type: TextureType::Equirect,
        };

        let texture = Texture::import("sky.hdr", &bytes, &settings).unwrap();

        let red = half::f16::from_le_bytes([texture.rgba[0], texture.rgba[1]]).to_f32();
        assert!((red - 8.0).abs() < 0.1, "red {red}");
    }

    /// A cubemap from a 2:1 panorama has six faces a quarter as wide, and
    /// every face texel holds the direction it looks along.
    #[test]
    fn a_projected_cubemap_looks_where_its_texels_point() {
        let (width, height) = (256, 128);
        let texture = Texture::from_rgba_f32(
            "sky",
            TextureType::Cubemap,
            &direction_panorama(width, height),
            width,
            height,
        )
        .unwrap();
        assert_eq!((texture.width, texture.height), (64, 64));
        assert_eq!(texture.rgba.len(), 6 * 64 * 64 * 8);

        let faces =
            project_equirect_to_cubemap(&direction_panorama(width, height), width, height, 8);
        for face in 0..6 {
            for row in 0..8 {
                for column in 0..8 {
                    let s = 2.0 * (column as f32 + 0.5) / 8.0 - 1.0;
                    let t = 2.0 * (row as f32 + 0.5) / 8.0 - 1.0;
                    let start = ((face * 8 + row) * 8 + column) * 4;
                    let stored = Vec3::new(faces[start], faces[start + 1], faces[start + 2]);
                    let expected = cube_face_direction(face, s, t);
                    assert!(
                        stored.normalize().dot(expected) > 0.99,
                        "face {face} texel ({column}, {row}): {stored} against {expected}"
                    );
                }
            }
        }
    }

    /// Checks where the panorama's landmarks sit: forward (`-Z`) in the middle,
    /// and up on the top row.
    #[test]
    fn the_equirect_convention_matches_the_shaders() {
        assert_eq!(equirect_uv(Vec3::NEG_Z), (0.5, 0.5));
        let (u, _) = equirect_uv(Vec3::X);
        assert!((u - 0.75).abs() < 1.0e-6);
        let (_, v) = equirect_uv(Vec3::Y);
        assert!(v.abs() < 1.0e-6);
    }

    #[test]
    fn a_cubemap_with_unsquare_faces_is_refused() {
        // Two by one texels at eight bytes each, times six faces, is the right
        // length for the declared size. Only the shape is wrong.
        let bytes = vec![0; 2 * 8 * 6];
        assert!(Texture::from_rgba("sky", TextureType::Cubemap, bytes, 2, 1).is_err());
    }

    /// The stand-ins the module ships are files that decode. A renderer that
    /// reads them shows white and a flat normal instead of falling back to the
    /// texels it builds in code.
    #[test]
    fn the_shipped_stand_in_textures_decode() {
        crate::declare_crate_roots();
        let colour = load_shipped_texture(DEFAULT_COLOR_TEXTURE, TextureType::Color)
            .expect("the colour stand-in is shipped");
        assert_eq!((colour.width, colour.height), (1, 1));
        assert_eq!(colour.rgba, [255, 255, 255, 255]);

        let normal = load_shipped_texture(DEFAULT_NORMAL_TEXTURE, TextureType::Normal)
            .expect("the normal stand-in is shipped");
        assert_eq!(normal.rgba, [128, 128, 255, 255]);
    }
}
