//! Per-entry codecs of the PPAK asset pack. Decoding is transparent: an entry decodes back to the
//! bytes the loaders already understand (RTEX texture, RMSH mesh, anything else verbatim).
//!
//! Encoders run at pack time on the dev machine (`encode` feature: libwebp, brotli encoder);
//! decoders are pure Rust and also build for wasm.

use std::io::Cursor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Codec {
    Raw = 0,
    Brotli = 1,
    /// RTEX header + lossy WebP: one RGB(A) image for color maps, one greyscale image per
    /// channel for data maps (normal, metallic-roughness), which chroma subsampling would smear.
    WebpTex = 2,
    /// RMSH, quantized (u16 position/UV, octahedral i16 normal/tangent/bitangent), brotli.
    QuantMesh = 3,
    // 4 and 5 are reserved (point clouds, timelines)
}

impl Codec {
    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0 => Codec::Raw,
            1 => Codec::Brotli,
            2 => Codec::WebpTex,
            3 => Codec::QuantMesh,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Codec::Raw => "raw",
            Codec::Brotli => "brotli",
            Codec::WebpTex => "webp",
            Codec::QuantMesh => "qmesh",
        }
    }
}

/// Decodes a stored entry; `raw_size` is the decoded size the pack recorded.
pub fn decode(codec: Codec, stored: &[u8], raw_size: usize) -> Option<Vec<u8>> {
    match codec {
        Codec::Raw => Some(stored.to_vec()),
        Codec::Brotli => brotli_decode(stored, raw_size),
        Codec::WebpTex => webp_tex_decode(stored),
        Codec::QuantMesh => quant_mesh_decode(&brotli_decode(stored, usize::MAX)?, raw_size),
    }
}

fn brotli_decode(stored: &[u8], size_hint: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(size_hint.min(64 << 20));
    brotli_decompressor::BrotliDecompress(&mut Cursor::new(stored), &mut out).ok()?;
    Some(out)
}

// --- small helpers ---

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn i16(&mut self) -> Option<i16> {
        Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

fn push_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

/// Octahedral decode of a unit vector; `(i16::MIN, i16::MIN)` is the zero vector.
fn oct_decode(x: i16, y: i16) -> [f32; 3] {
    if x == i16::MIN && y == i16::MIN {
        return [0.0; 3];
    }
    let (mut vx, mut vy) = (x as f32 / 32767.0, y as f32 / 32767.0);
    let vz = 1.0 - vx.abs() - vy.abs();
    let t = (-vz).max(0.0);
    vx += if vx >= 0.0 { -t } else { t };
    vy += if vy >= 0.0 { -t } else { t };
    let length = (vx * vx + vy * vy + vz * vz).sqrt().max(1e-12);
    [vx / length, vy / length, vz / length]
}

/// `value` from a u16 quantized over `[min, max]`.
fn dequantize(q: u16, min: f32, max: f32) -> f32 {
    min + (max - min) * (q as f32 / 65535.0)
}

// --- textures: RTEX v1 (b"RTEX" | u32 1 | u32 w | u32 h | RGBA) ---

//   stored = RTEX header (16 B) | u8 mode
//     mode 0: one WebP (RGB or RGBA)
//     mode 1: u8 channel_count, per channel u32 len + greyscale WebP (channel = its luma)

/// Decodes one WebP to (width, height, channels, pixels).
fn webp_decode(bytes: &[u8]) -> Option<(u32, u32, usize, Vec<u8>)> {
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes)).ok()?;
    let (width, height) = decoder.dimensions();
    let mut pixels = vec![0u8; decoder.output_buffer_size()?];
    decoder.read_image(&mut pixels).ok()?;
    let channels = if decoder.has_alpha() { 4 } else { 3 };
    Some((width, height, channels, pixels))
}

fn webp_tex_decode(stored: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader {
        bytes: stored,
        pos: 0,
    };
    let header = reader.take(16)?;
    let width = u32::from_le_bytes(header[8..12].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
    let mut rgba = vec![255u8; width * height * 4];
    match reader.u8()? {
        0 => {
            let (w, h, channels, pixels) = webp_decode(&stored[reader.pos..])?;
            if (w as usize, h as usize) != (width, height) {
                return None;
            }
            for (out, px) in rgba
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(pixels.chunks_exact(channels))
            {
                out[..channels].copy_from_slice(px);
            }
        }
        1 => {
            let channel_count = reader.u8()? as usize;
            for channel in 0..channel_count.min(4) {
                let len = reader.u32()? as usize;
                let (w, h, channels, pixels) = webp_decode(reader.take(len)?)?;
                if (w as usize, h as usize) != (width, height) {
                    return None;
                }
                for (out, px) in rgba
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(pixels.chunks_exact(channels))
                {
                    out[channel] = px[0];
                }
            }
        }
        _ => return None,
    }
    let mut out = Vec::with_capacity(16 + rgba.len());
    out.extend_from_slice(header);
    out.extend_from_slice(&rgba);
    Some(out)
}

// --- meshes: RMSH (b"RMSH" | u32 | u32 vertex_count | u32 index_count | 56 B vertices | u32 indices)
//     vertex = position[3] uv[2] normal[3] tangent[3] bitangent[3], all f32 ---

fn quant_mesh_decode(q: &[u8], raw_size: usize) -> Option<Vec<u8>> {
    let mut reader = Reader { bytes: q, pos: 0 };
    let header = reader.take(16)?;
    let vertex_count = u32::from_le_bytes(header[8..12].try_into().ok()?) as usize;
    let index_count = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
    let mut bounds = [0.0f32; 10]; // position min[3] max[3], uv min[2] max[2]
    for bound in &mut bounds {
        *bound = reader.f32()?;
    }
    let mut out = Vec::with_capacity(raw_size.min(256 << 20));
    out.extend_from_slice(header);
    for _ in 0..vertex_count {
        for axis in 0..3 {
            push_f32(
                &mut out,
                dequantize(reader.u16()?, bounds[axis], bounds[3 + axis]),
            );
        }
        for axis in 0..2 {
            push_f32(
                &mut out,
                dequantize(reader.u16()?, bounds[6 + axis], bounds[8 + axis]),
            );
        }
        for _ in 0..3 {
            let (x, y) = (reader.i16()?, reader.i16()?);
            for component in oct_decode(x, y) {
                push_f32(&mut out, component);
            }
        }
    }
    let wide = vertex_count > u16::MAX as usize + 1;
    for _ in 0..index_count {
        let index = if wide {
            reader.u32()?
        } else {
            reader.u16()? as u32
        };
        out.extend_from_slice(&index.to_le_bytes());
    }
    Some(out)
}

// --- encoders (pack time) ---

#[cfg(feature = "encode")]
pub use encode::encode;

#[cfg(feature = "encode")]
mod encode {
    use super::*;

    /// Picks and applies the codec for an entry by name. Falls back to plain brotli when a
    /// specialised codec doesn't recognise the bytes (unexpected layout or version).
    pub fn encode(name: &str, raw: &[u8]) -> anyhow::Result<(Codec, Vec<u8>)> {
        let specialised = if name.ends_with(".cooked_tex") {
            let is_color_map = ["albedo", "emissive", "color"]
                .iter()
                .any(|kind| name.contains(kind));
            webp_tex_encode(raw, is_color_map).map(|bytes| (Codec::WebpTex, bytes))
        } else if name.ends_with(".cooked_mesh") {
            quant_mesh_encode(raw).map(|q| (Codec::QuantMesh, brotli_encode(&q)))
        } else {
            None
        };
        if let Some(result) = specialised {
            return Ok(result);
        }
        let compressed = brotli_encode(raw);
        Ok(if compressed.len() < raw.len() {
            (Codec::Brotli, compressed)
        } else {
            (Codec::Raw, raw.to_vec())
        })
    }

    pub(super) fn brotli_encode(raw: &[u8]) -> Vec<u8> {
        let params = brotli::enc::BrotliEncoderParams {
            quality: 11,
            lgwin: 22,
            ..Default::default()
        };
        let mut out = Vec::new();
        brotli::BrotliCompress(&mut Cursor::new(raw), &mut out, &params)
            .expect("brotli into a Vec cannot fail");
        out
    }

    const WEBP_QUALITY: f32 = 85.0;

    fn webp_tex_encode(raw: &[u8], is_color_map: bool) -> Option<Vec<u8>> {
        if raw.len() < 16 || &raw[0..4] != b"RTEX" || raw[4..8] != 1u32.to_le_bytes() {
            return None;
        }
        let width = u32::from_le_bytes(raw[8..12].try_into().ok()?);
        let height = u32::from_le_bytes(raw[12..16].try_into().ok()?);
        let pixels = raw.get(16..16 + width as usize * height as usize * 4)?;
        let texels = pixels.as_chunks::<4>().0;
        let opaque = texels.iter().all(|px| px[3] == 255);
        let mut out = raw[..16].to_vec();
        if is_color_map {
            out.push(0);
            let webp = if opaque {
                let rgb: Vec<u8> = texels.iter().flat_map(|px| [px[0], px[1], px[2]]).collect();
                webp::Encoder::from_rgb(&rgb, width, height).encode(WEBP_QUALITY)
            } else {
                webp::Encoder::from_rgba(pixels, width, height).encode(WEBP_QUALITY)
            };
            out.extend_from_slice(&webp);
        } else {
            // Grey RGB = constant chroma, so each channel rides in the full-resolution luma.
            let channel_count = if opaque { 3 } else { 4 };
            out.push(1);
            out.push(channel_count as u8);
            for channel in 0..channel_count {
                let grey: Vec<u8> = texels.iter().flat_map(|px| [px[channel]; 3]).collect();
                let webp = webp::Encoder::from_rgb(&grey, width, height).encode(WEBP_QUALITY);
                out.extend_from_slice(&(webp.len() as u32).to_le_bytes());
                out.extend_from_slice(&webp);
            }
        }
        Some(out)
    }

    fn oct_encode(v: [f32; 3]) -> (i16, i16) {
        let l1 = v[0].abs() + v[1].abs() + v[2].abs();
        if l1 < 1e-12 {
            return (i16::MIN, i16::MIN);
        }
        let (mut x, mut y) = (v[0] / l1, v[1] / l1);
        if v[2] < 0.0 {
            let (ox, oy) = (x, y);
            x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
            y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
        }
        let q = |c: f32| (c.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        (q(x), q(y))
    }

    fn quantize(value: f32, min: f32, max: f32) -> u16 {
        let range = max - min;
        if range <= 0.0 {
            return 0;
        }
        (((value - min) / range).clamp(0.0, 1.0) * 65535.0).round() as u16
    }

    fn bounds<const N: usize>(values: impl Iterator<Item = [f32; N]>) -> ([f32; N], [f32; N]) {
        let mut min = [f32::INFINITY; N];
        let mut max = [f32::NEG_INFINITY; N];
        for value in values {
            for axis in 0..N {
                min[axis] = min[axis].min(value[axis]);
                max[axis] = max[axis].max(value[axis]);
            }
        }
        for axis in 0..N {
            if !min[axis].is_finite() {
                min[axis] = 0.0;
                max[axis] = 0.0;
            }
        }
        (min, max)
    }

    fn f32s(bytes: &[u8]) -> Vec<f32> {
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| f32::from_le_bytes(b))
            .collect()
    }

    const MESH_VERTEX_FLOATS: usize = 14;

    fn quant_mesh_encode(raw: &[u8]) -> Option<Vec<u8>> {
        if raw.len() < 16 || &raw[0..4] != b"RMSH" {
            return None;
        }
        let vertex_count = u32::from_le_bytes(raw[8..12].try_into().ok()?) as usize;
        let index_count = u32::from_le_bytes(raw[12..16].try_into().ok()?) as usize;
        let vertex_bytes = vertex_count * MESH_VERTEX_FLOATS * 4;
        if raw.len() != 16 + vertex_bytes + index_count * 4 {
            return None; // not the 56-byte vertex layout this codec knows
        }
        let floats = f32s(&raw[16..16 + vertex_bytes]);
        let vertices: Vec<&[f32; MESH_VERTEX_FLOATS]> = floats.as_chunks().0.iter().collect();
        let (position_min, position_max) = bounds(vertices.iter().map(|v| [v[0], v[1], v[2]]));
        let (uv_min, uv_max) = bounds(vertices.iter().map(|v| [v[3], v[4]]));

        let mut out = raw[..16].to_vec();
        for bound in position_min
            .iter()
            .chain(&position_max)
            .chain(&uv_min)
            .chain(&uv_max)
        {
            push_f32(&mut out, *bound);
        }
        for v in &vertices {
            for axis in 0..3 {
                out.extend_from_slice(
                    &quantize(v[axis], position_min[axis], position_max[axis]).to_le_bytes(),
                );
            }
            for axis in 0..2 {
                out.extend_from_slice(
                    &quantize(v[3 + axis], uv_min[axis], uv_max[axis]).to_le_bytes(),
                );
            }
            for start in [5, 8, 11] {
                let (x, y) = oct_encode([v[start], v[start + 1], v[start + 2]]);
                out.extend_from_slice(&x.to_le_bytes());
                out.extend_from_slice(&y.to_le_bytes());
            }
        }
        let wide = vertex_count > u16::MAX as usize + 1;
        for &index in raw[16 + vertex_bytes..].as_chunks::<4>().0 {
            let index = u32::from_le_bytes(index);
            if wide {
                out.extend_from_slice(&index.to_le_bytes());
            } else {
                out.extend_from_slice(&(index as u16).to_le_bytes());
            }
        }
        Some(out)
    }
}

#[cfg(all(test, feature = "encode"))]
mod tests {
    use super::*;

    fn f32_at(bytes: &[u8], index: usize) -> f32 {
        f32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())
    }

    #[test]
    fn mesh_round_trip_within_quantization_error() {
        let vertices: [[f32; 14]; 3] = [
            [
                0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0,
            ],
            [
                1.0, 2.0, -3.0, 0.5, 2.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0,
            ],
            [
                -1.0, 0.5, 3.0, 1.0, -1.0, 0.577, -0.577, -0.577, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0,
            ],
        ];
        let mut raw = b"RMSH".to_vec();
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&3u32.to_le_bytes());
        raw.extend_from_slice(&3u32.to_le_bytes());
        for v in &vertices {
            for &f in v {
                raw.extend_from_slice(&f.to_le_bytes());
            }
        }
        for i in [0u32, 1, 2] {
            raw.extend_from_slice(&i.to_le_bytes());
        }
        let (codec, stored) = encode("models/x.cooked_mesh", &raw).unwrap();
        assert_eq!(codec, Codec::QuantMesh);
        let decoded = decode(codec, &stored, raw.len()).unwrap();
        assert_eq!(decoded.len(), raw.len());
        assert_eq!(&decoded[..16], &raw[..16]);
        assert_eq!(&decoded[16 + 3 * 56..], &raw[16 + 3 * 56..]); // indices exact
        for (i, v) in vertices.iter().enumerate() {
            let body = &decoded[16 + i * 56..];
            for (k, &expected) in v.iter().enumerate() {
                let tolerance = if k < 5 { 1e-3 } else { 2e-3 };
                let got = f32_at(body, k);
                // Normals are renormalized; compare the normalized original.
                let expected = if (5..8).contains(&k) {
                    let n = &v[5..8];
                    expected / (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
                } else {
                    expected
                };
                assert!(
                    (got - expected).abs() < tolerance,
                    "vertex {i} float {k}: {got} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn texture_round_trip_keeps_header_and_size() {
        let (w, h) = (64u32, 32u32);
        let mut raw = b"RTEX".to_vec();
        raw.extend_from_slice(&1u32.to_le_bytes());
        raw.extend_from_slice(&w.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
        for i in 0..(w * h) {
            raw.extend_from_slice(&[(i % 251) as u8, 128, (i % 7) as u8 * 30, 255]);
        }
        for name in ["models/a_albedo.cooked_tex", "models/a_normal.cooked_tex"] {
            let (codec, stored) = encode(name, &raw).unwrap();
            assert_eq!(codec, Codec::WebpTex);
            let decoded = decode(codec, &stored, raw.len()).unwrap();
            assert_eq!(decoded.len(), raw.len());
            assert_eq!(&decoded[..16], &raw[..16]);
            assert!(decoded[16..]
                .as_chunks::<4>()
                .0
                .iter()
                .all(|px| px[3] == 255));
        }
        // Data maps keep each channel at full resolution: a constant channel stays exact-ish.
        let (codec, stored) = encode("models/a_normal.cooked_tex", &raw).unwrap();
        let decoded = decode(codec, &stored, raw.len()).unwrap();
        assert!(decoded[16..]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[1].abs_diff(128) <= 2));
    }

    #[test]
    fn generic_entries_use_brotli_or_raw() {
        let raw = vec![42u8; 10_000];
        let (codec, stored) = encode("blob.dat", &raw).unwrap();
        assert_eq!(codec, Codec::Brotli);
        assert_eq!(decode(codec, &stored, raw.len()).unwrap(), raw);
        let (codec, stored) = encode("tiny.dat", b"x").unwrap();
        assert_eq!((codec, stored.as_slice()), (Codec::Raw, &b"x"[..]));
    }
}
