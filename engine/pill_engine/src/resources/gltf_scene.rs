//! Runtime glTF loader (feature `gltf_loading`).
//!
//! The single wasm-safe path for loading a glTF/glb obtained as bytes at runtime, however you got
//! them: fetched (multi-file `.gltf` + external `.bin`/textures) or dropped/embedded (self-contained
//! `.glb`). Opt-in: games that cook-and-embed at build time don't enable it and pay no binary cost.
//!
//! Parsing uses `goth-gltf` (nanoserde, no serde_json) to keep the wasm binary small; it's a
//! low-level reader, so attribute/index accessors are read out of the buffers here. The
//! vertex/tangent/matrix math and `RMSH`/`RTEX` writers are shared with the build-time cooker via
//! `pill_assets::convert`. Textures decode with the lean `zune-jpeg`/`zune-png` codecs. The caller
//! fetches external resources (see [`gltf_resource_uris`]) and uploads the result with
//! `Mesh::from_cooked_mesh_bytes` / `Texture::from_bytes_owned`.

use goth_gltf::{ComponentType, NodeTransform, PrimitiveMode};
use pill_assets::convert::{self, PrimitiveAttributes};
use pill_core::{PillError, Result};

use crate::resources::TextureType;

type Doc = goth_gltf::Gltf<goth_gltf::default_extensions::Extensions>;

/// A texture ready to upload via `Texture::from_bytes_owned(name, kind, rtex)`.
pub struct GltfTextureData {
    pub name: String,
    pub kind: TextureType,
    pub rtex: Vec<u8>,
}

/// A material's factors and texture references (indices into [`GltfSceneData::textures`]).
pub struct GltfMaterialData {
    pub name: String,
    pub albedo: Option<usize>,
    pub normal: Option<usize>,
    pub metallic_roughness: Option<usize>,
    pub emissive: Option<usize>,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive_color: [f32; 3],
}

/// A mesh primitive ready to upload via `Mesh::from_cooked_mesh_bytes(name, &rmsh)`.
pub struct GltfMeshData {
    pub name: String,
    pub rmsh: Vec<u8>,
    pub material: usize,
}

/// The whole scene, flattened: one mesh entry per primitive (world transform baked into vertices).
pub struct GltfSceneData {
    pub textures: Vec<GltfTextureData>,
    pub materials: Vec<GltfMaterialData>,
    pub meshes: Vec<GltfMeshData>,
}

/// External resources a glTF document references, in document (index) order. An entry is `None`
/// when nothing needs fetching: an embedded BIN buffer, a buffer-view image or a `data:` URI.
pub struct GltfResourceUris {
    pub buffers: Vec<Option<String>>,
    pub images: Vec<Option<String>>,
}

/// Lists external buffer/image URIs to fetch, in document order. URIs are returned verbatim
/// (relative to the `.gltf`); the caller resolves them against its base URL.
pub fn gltf_resource_uris(gltf_bytes: &[u8]) -> Result<GltfResourceUris> {
    let (doc, _) =
        Doc::from_bytes(gltf_bytes).map_err(|e| PillError::from(format!("parse glTF: {e:?}")))?;
    let external = |uri: &Option<String>| uri.clone().filter(|uri| !uri.starts_with("data:"));
    Ok(GltfResourceUris {
        buffers: doc.buffers.iter().map(|b| external(&b.uri)).collect(),
        images: doc.images.iter().map(|i| external(&i.uri)).collect(),
    })
}

/// Parses a glTF/glb into uploadable scene data; resource names start with `name_prefix`, so
/// several scenes can be loaded into one engine.
///
/// `external_buffers` / `external_images` are indexed 1:1 with the document's buffers/images (the
/// order from [`gltf_resource_uris`]); entries that were `None` there may be empty.
pub fn parse_gltf_scene(
    name_prefix: &str,
    gltf_bytes: &[u8],
    external_buffers: &[Vec<u8>],
    external_images: &[Vec<u8>],
) -> Result<GltfSceneData> {
    use std::borrow::Cow;
    use std::collections::{HashMap, HashSet};

    let (doc, blob) =
        Doc::from_bytes(gltf_bytes).map_err(|e| PillError::from(format!("parse glTF: {e:?}")))?;

    // Resolve every buffer: embedded BIN blob, data: URI, or caller-supplied external bytes.
    let mut buffers: Vec<Cow<[u8]>> = Vec::with_capacity(doc.buffers.len());
    for (i, buffer) in doc.buffers.iter().enumerate() {
        let bytes =
            match &buffer.uri {
                Some(uri) if uri.starts_with("data:") => Cow::Owned(decode_data_uri(uri)?),
                Some(_) => {
                    Cow::Borrowed(external_buffers.get(i).map(|v| v.as_slice()).ok_or_else(
                        || PillError::from(format!("missing fetched bytes for buffer {i}")),
                    )?)
                }
                None => Cow::Borrowed(blob.ok_or_else(|| {
                    PillError::from("glTF has an embedded buffer but no blob".to_string())
                })?),
            };
        buffers.push(bytes);
    }
    let buffers: Vec<&[u8]> = buffers.iter().map(|b| b.as_ref()).collect();

    // Image index per material texture slot; textures without a source (extension-only) are skipped
    const KINDS: [TextureType; 4] = [
        TextureType::Color,
        TextureType::Normal,
        TextureType::MetallicRoughness,
        TextureType::Emissive,
    ];
    let image_of = |tex_index: Option<usize>| -> Result<Option<usize>> {
        let Some(tex_index) = tex_index else {
            return Ok(None);
        };
        let image = get(&doc.textures, tex_index, "texture")?.source;
        if let Some(image) = image {
            get(&doc.images, image, "image")?;
        }
        Ok(image)
    };
    let mut material_images: Vec<[Option<usize>; 4]> = Vec::with_capacity(doc.materials.len());
    for material in &doc.materials {
        let pbr = &material.pbr_metallic_roughness;
        material_images.push([
            image_of(pbr.base_color_texture.as_ref().map(|t| t.index))?,
            image_of(material.normal_texture.as_ref().map(|t| t.index))?,
            image_of(pbr.metallic_roughness_texture.as_ref().map(|t| t.index))?,
            image_of(material.emissive_texture.as_ref().map(|t| t.index))?,
        ]);
    }

    // Decode only referenced images; an image bound as two kinds (sRGB color vs linear) is two GPU
    // textures, so its pixels are cloned only for the extra kinds.
    let mut kinds_left: HashMap<usize, usize> = HashMap::new();
    let mut seen: HashSet<(usize, TextureType)> = HashSet::new();
    for images in &material_images {
        for (image, kind) in images.iter().zip(KINDS) {
            if let Some(image) = *image {
                if seen.insert((image, kind)) {
                    *kinds_left.entry(image).or_default() += 1;
                }
            }
        }
    }
    let mut decoded: HashMap<usize, DecodedImage> = HashMap::new();
    let mut textures: Vec<GltfTextureData> = Vec::new();
    let mut texture_index: HashMap<(usize, TextureType), usize> = HashMap::new();

    let mut materials: Vec<GltfMaterialData> = Vec::with_capacity(doc.materials.len() + 1);
    for (idx, (material, images)) in doc.materials.iter().zip(&material_images).enumerate() {
        let mut slots = [None; 4];
        for (slot, (image, kind)) in slots.iter_mut().zip(images.iter().zip(KINDS)) {
            let Some(image) = *image else {
                continue;
            };
            if let Some(&existing) = texture_index.get(&(image, kind)) {
                *slot = Some(existing);
                continue;
            }
            let pixels = match decoded.remove(&image) {
                Some(pixels) => pixels,
                None => decode_image(&image_bytes(&doc, &buffers, external_images, image)?)?,
            };
            let left = kinds_left.get_mut(&image).expect("counted above");
            *left -= 1;
            let (w, h) = (pixels.w, pixels.h);
            let rgba = if *left > 0 {
                let copy = pixels.rgba.clone();
                decoded.insert(image, pixels);
                copy
            } else {
                pixels.rgba
            };
            texture_index.insert((image, kind), textures.len());
            *slot = Some(textures.len());
            textures.push(GltfTextureData {
                name: format!("{name_prefix}_{}_{image}", role(kind)),
                kind,
                rtex: convert::rtex_from_rgba(w, h, rgba),
            });
        }
        let pbr = &material.pbr_metallic_roughness;
        let [albedo, normal, metallic_roughness, emissive] = slots;
        materials.push(GltfMaterialData {
            name: format!("{name_prefix}_mat_{idx}"),
            albedo,
            normal,
            metallic_roughness,
            emissive,
            base_color: pbr.base_color_factor,
            metallic: pbr.metallic_factor,
            roughness: pbr.roughness_factor,
            emissive_color: material.emissive_factor,
        });
    }

    // Fallback for primitives without a material: the glTF spec default (rough metal)
    let default_material = materials.len();
    materials.push(GltfMaterialData {
        name: format!("{name_prefix}_mat_default"),
        albedo: None,
        normal: None,
        metallic_roughness: None,
        emissive: None,
        base_color: [1.0; 4],
        metallic: 1.0,
        roughness: 1.0,
        emissive_color: [0.0; 3],
    });

    // (name label, mesh index, world transform): walk the scene graph, or, without a scene, every
    // mesh untransformed (as the build-time cooker does)
    let mut instances: Vec<(String, usize, convert::Mat4)> = Vec::new();
    match doc.scenes.get(doc.scene).or_else(|| doc.scenes.first()) {
        Some(scene) => {
            let mut visited = vec![false; doc.nodes.len()];
            let mut stack: Vec<(usize, convert::Mat4)> = scene
                .nodes
                .iter()
                .map(|&n| (n, convert::IDENTITY))
                .collect();
            while let Some((node_idx, parent)) = stack.pop() {
                let node = get(&doc.nodes, node_idx, "node")?;
                if std::mem::replace(&mut visited[node_idx], true) {
                    return Err(PillError::from(format!(
                        "glTF node {node_idx} is reachable twice (cycle or shared child)"
                    )));
                }
                let world = convert::mat4_mul(parent, node_transform(node));
                if let Some(mesh_idx) = node.mesh {
                    instances.push((node_idx.to_string(), mesh_idx, world));
                }
                for &child in &node.children {
                    stack.push((child, world));
                }
            }
        }
        None => {
            for mesh_idx in 0..doc.meshes.len() {
                instances.push((format!("m{mesh_idx}"), mesh_idx, convert::IDENTITY));
            }
        }
    }

    // One RMSH blob per primitive, world transform baked into the vertices
    let mut meshes: Vec<GltfMeshData> = Vec::new();
    for (label, mesh_idx, world) in instances {
        for (prim_idx, prim) in get(&doc.meshes, mesh_idx, "mesh")?
            .primitives
            .iter()
            .enumerate()
        {
            if prim.mode != PrimitiveMode::Triangles {
                log::warn!(
                    "glTF mesh {mesh_idx}: skipping {:?} primitive (only triangle lists are supported)",
                    prim.mode
                );
                continue;
            }
            let material = match prim.material {
                Some(material) => {
                    get(&doc.materials, material, "material")?;
                    material
                }
                None => default_material,
            };
            let attributes = &prim.attributes;
            let positions = to_vec3(read_floats(&doc, &buffers, attributes.position)?);
            let normals = match attributes.normal {
                Some(acc) => Some(to_vec3(read_floats_at(&doc, &buffers, acc)?)),
                None => None,
            };
            let texture_coordinates = match attributes.texcoord_0 {
                Some(acc) => Some(to_vec2(read_floats_at(&doc, &buffers, acc)?)),
                None => None,
            };
            let tangents = match attributes.tangent {
                Some(acc) => to_vec4(read_floats_at(&doc, &buffers, acc)?),
                None => Vec::new(),
            };
            let indices = match prim.indices {
                Some(acc) => Some(read_indices(&doc, &buffers, acc)?),
                None => None,
            };

            let (vertices, indices) = convert::build_primitive(
                PrimitiveAttributes {
                    positions,
                    normals,
                    texture_coordinates,
                    tangents,
                    indices,
                },
                world,
            )
            .map_err(|e| PillError::from(format!("glTF mesh {mesh_idx}: {e}")))?;
            if indices.is_empty() {
                continue;
            }
            meshes.push(GltfMeshData {
                name: format!("{name_prefix}_mesh_{label}_{prim_idx}"),
                rmsh: convert::rmsh_bytes(&vertices, &indices),
                material,
            });
        }
    }

    Ok(GltfSceneData {
        textures,
        materials,
        meshes,
    })
}

/// `slice[index]`, or an error naming what was referenced out of range.
fn get<'a, T>(slice: &'a [T], index: usize, what: &str) -> Result<&'a T> {
    slice.get(index).ok_or_else(|| {
        PillError::from(format!(
            "glTF {what} {index} out of range ({} defined)",
            slice.len()
        ))
    })
}

/// Encoded bytes of image `index`: data: URI, caller-fetched, or a buffer view.
fn image_bytes<'a>(
    doc: &Doc,
    buffers: &[&'a [u8]],
    external_images: &'a [Vec<u8>],
    index: usize,
) -> Result<std::borrow::Cow<'a, [u8]>> {
    use std::borrow::Cow;
    let image = get(&doc.images, index, "image")?;
    match (&image.uri, image.buffer_view) {
        (Some(uri), _) if uri.starts_with("data:") => Ok(Cow::Owned(decode_data_uri(uri)?)),
        (Some(_), _) => external_images
            .get(index)
            .filter(|v| !v.is_empty())
            .map(|v| Cow::Borrowed(v.as_slice()))
            .ok_or_else(|| PillError::from(format!("missing fetched bytes for image {index}"))),
        (None, Some(bv)) => {
            let view = get(&doc.buffer_views, bv, "buffer view")?;
            let buffer = *get(buffers, view.buffer, "buffer")?;
            buffer
                .get(view.byte_offset..view.byte_offset + view.byte_length)
                .map(Cow::Borrowed)
                .ok_or_else(|| PillError::from(format!("image {index} view exceeds its buffer")))
        }
        (None, None) => Err(PillError::from(format!(
            "image {index} has neither uri nor buffer view"
        ))),
    }
}

/// Payload of a base64 `data:` URI.
fn decode_data_uri(uri: &str) -> Result<Vec<u8>> {
    let payload = uri
        .split_once(";base64,")
        .map(|(_, payload)| payload)
        .ok_or_else(|| PillError::from("only base64 data: URIs are supported".to_string()))?;
    let mut out = Vec::with_capacity(payload.len() / 4 * 3);
    let (mut bits, mut bit_count) = (0u32, 0);
    for byte in payload.bytes().filter(|&b| b != b'=') {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(PillError::from("invalid base64 in data: URI".to_string())),
        };
        bits = (bits << 6) | value as u32;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            out.push((bits >> bit_count) as u8);
        }
    }
    Ok(out)
}

// --- Accessor reading (goth-gltf is low-level; we read the typed arrays out of the buffers) ---

/// Reads a required float-attribute accessor (POSITION) as a flat `Vec<f32>`.
fn read_floats(doc: &Doc, buffers: &[&[u8]], accessor: Option<usize>) -> Result<Vec<f32>> {
    let accessor = accessor
        .ok_or_else(|| PillError::from("primitive missing a required attribute".to_string()))?;
    read_floats_at(doc, buffers, accessor)
}

fn read_floats_at(doc: &Doc, buffers: &[&[u8]], accessor_idx: usize) -> Result<Vec<f32>> {
    let acc = get(&doc.accessors, accessor_idx, "accessor")?;
    let components = acc.accessor_type.num_components();
    let (buf, base, stride, comp_size) = accessor_layout(doc, buffers, accessor_idx)?;
    let mut out = Vec::with_capacity(acc.count * components);
    for i in 0..acc.count {
        let elem = base + i * stride;
        for c in 0..components {
            let o = elem + c * comp_size;
            let value = match acc.component_type {
                ComponentType::Float => f32::from_le_bytes(buf[o..o + 4].try_into().unwrap()),
                ComponentType::UnsignedByte => norm_u(buf[o] as f32, 255.0, acc.normalized),
                ComponentType::UnsignedShort => norm_u(
                    u16::from_le_bytes(buf[o..o + 2].try_into().unwrap()) as f32,
                    65535.0,
                    acc.normalized,
                ),
                ComponentType::Byte => norm_s(buf[o] as i8 as f32, 127.0, acc.normalized),
                ComponentType::Short => norm_s(
                    i16::from_le_bytes(buf[o..o + 2].try_into().unwrap()) as f32,
                    32767.0,
                    acc.normalized,
                ),
                ComponentType::UnsignedInt => {
                    u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) as f32
                }
            };
            out.push(value);
        }
    }
    Ok(out)
}

/// Reads an index accessor as `u32` (glTF allows u8/u16/u32 indices).
fn read_indices(doc: &Doc, buffers: &[&[u8]], accessor_idx: usize) -> Result<Vec<u32>> {
    let acc = get(&doc.accessors, accessor_idx, "accessor")?;
    let (buf, base, stride, _) = accessor_layout(doc, buffers, accessor_idx)?;
    let mut out = Vec::with_capacity(acc.count);
    for i in 0..acc.count {
        let o = base + i * stride;
        let value = match acc.component_type {
            ComponentType::UnsignedInt => u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()),
            ComponentType::UnsignedShort => {
                u16::from_le_bytes(buf[o..o + 2].try_into().unwrap()) as u32
            }
            ComponentType::UnsignedByte => buf[o] as u32,
            other => {
                return Err(PillError::from(format!(
                    "unsupported index component type {other:?}"
                )))
            }
        };
        out.push(value);
    }
    Ok(out)
}

/// Resolves (buffer slice, byte base offset, element stride, component byte size) for an accessor.
/// Checks that every element lies inside the buffer, so callers can index without bounds checks.
fn accessor_layout<'a>(
    doc: &Doc,
    buffers: &[&'a [u8]],
    accessor_idx: usize,
) -> Result<(&'a [u8], usize, usize, usize)> {
    let acc = get(&doc.accessors, accessor_idx, "accessor")?;
    let bv = acc
        .buffer_view
        .ok_or_else(|| PillError::from("accessor without a buffer view".to_string()))?;
    let view = get(&doc.buffer_views, bv, "buffer view")?;
    let buf = *get(buffers, view.buffer, "buffer")?;
    let comp_size = acc.component_type.byte_size();
    let element = acc.accessor_type.num_components() * comp_size;
    let stride = view.byte_stride.unwrap_or(element);
    let base = view.byte_offset + acc.byte_offset;
    let end = match acc.count {
        0 => base,
        count => (count - 1)
            .checked_mul(stride)
            .and_then(|last| last.checked_add(base + element))
            .unwrap_or(usize::MAX),
    };
    if end > buf.len() {
        return Err(PillError::from(format!(
            "accessor {accessor_idx} reads bytes up to {end}, buffer has {}",
            buf.len()
        )));
    }
    Ok((buf, base, stride, comp_size))
}

fn norm_u(v: f32, max: f32, normalized: bool) -> f32 {
    if normalized {
        v / max
    } else {
        v
    }
}

fn norm_s(v: f32, max: f32, normalized: bool) -> f32 {
    if normalized {
        // glTF signed-normalized dequant: max(c / MAX, -1.0). The divisor is the type's positive
        // max, so values can't exceed +1.0 — only the low end needs the clamp (spec 3.6.2.1.2).
        (v / max).max(-1.0)
    } else {
        v
    }
}

fn to_vec3(flat: Vec<f32>) -> Vec<[f32; 3]> {
    flat.as_chunks().0.to_vec()
}
fn to_vec2(flat: Vec<f32>) -> Vec<[f32; 2]> {
    flat.as_chunks().0.to_vec()
}
fn to_vec4(flat: Vec<f32>) -> Vec<[f32; 4]> {
    flat.as_chunks().0.to_vec()
}

/// Node local transform → row-major Mat4 (matching `pill_assets::convert`).
fn node_transform(
    node: &goth_gltf::Node<goth_gltf::default_extensions::Extensions>,
) -> convert::Mat4 {
    match node.transform() {
        NodeTransform::Matrix(m) => {
            // glTF matrices are column-major; group into columns for mat4_from_cols.
            convert::mat4_from_cols([
                [m[0], m[1], m[2], m[3]],
                [m[4], m[5], m[6], m[7]],
                [m[8], m[9], m[10], m[11]],
                [m[12], m[13], m[14], m[15]],
            ])
        }
        NodeTransform::Set {
            translation: [tx, ty, tz],
            rotation: [x, y, z, w],
            scale: [sx, sy, sz],
        } => {
            // T * R(quat) * S, row-major.
            let r00 = 1.0 - 2.0 * (y * y + z * z);
            let r01 = 2.0 * (x * y - z * w);
            let r02 = 2.0 * (x * z + y * w);
            let r10 = 2.0 * (x * y + z * w);
            let r11 = 1.0 - 2.0 * (x * x + z * z);
            let r12 = 2.0 * (y * z - x * w);
            let r20 = 2.0 * (x * z - y * w);
            let r21 = 2.0 * (y * z + x * w);
            let r22 = 1.0 - 2.0 * (x * x + y * y);
            [
                [r00 * sx, r01 * sy, r02 * sz, tx],
                [r10 * sx, r11 * sy, r12 * sz, ty],
                [r20 * sx, r21 * sy, r22 * sz, tz],
                [0.0, 0.0, 0.0, 1.0],
            ]
        }
    }
}

// --- Texture decode (lean zune codecs) ---

struct DecodedImage {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
}

fn decode_image(bytes: &[u8]) -> Result<DecodedImage> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        decode_jpeg(bytes)
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        decode_png(bytes)
    } else {
        Err(PillError::from(
            "unsupported texture image (expected JPEG or PNG)".to_string(),
        ))
    }
}

fn decode_jpeg(bytes: &[u8]) -> Result<DecodedImage> {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    let pixels = decoder
        .decode()
        .map_err(|e| PillError::from(format!("decode JPEG: {e:?}")))?;
    let (w, h) = decoder
        .dimensions()
        .ok_or_else(|| PillError::from("JPEG has no dimensions".to_string()))?;
    let components = decoder
        .get_output_colorspace()
        .map(|c| c.num_components())
        .unwrap_or(3);
    Ok(DecodedImage {
        rgba: expand_to_rgba(pixels, components),
        w: w as u32,
        h: h as u32,
    })
}

fn decode_png(bytes: &[u8]) -> Result<DecodedImage> {
    use zune_png::zune_core::{options::DecoderOptions, result::DecodingResult};
    // 16-bit PNGs (common for normal/roughness maps) are reduced to 8 bits per channel
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = zune_png::PngDecoder::new_with_options(bytes, options);
    let result = decoder
        .decode()
        .map_err(|e| PillError::from(format!("decode PNG: {e:?}")))?;
    let (w, h) = decoder
        .get_dimensions()
        .ok_or_else(|| PillError::from("PNG has no dimensions".to_string()))?;
    let components = decoder
        .get_colorspace()
        .map(|c| c.num_components())
        .unwrap_or(4);
    let px8 = match result {
        DecodingResult::U8(v) => v,
        _ => return Err(PillError::from("PNG did not decode to 8 bits".to_string())),
    };
    Ok(DecodedImage {
        rgba: expand_to_rgba(px8, components),
        w: w as u32,
        h: h as u32,
    })
}

/// Expands decoded pixels (luma / luma-alpha / RGB / RGBA) to RGBA8. Takes ownership so the
/// already-RGBA case (the common one) is a move, not a full-image copy.
fn expand_to_rgba(pixels: Vec<u8>, components: usize) -> Vec<u8> {
    match components {
        3 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, 255])
            .collect(),
        2 => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        1 => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => pixels, // already RGBA (4) or unknown — pass through
    }
}

fn role(kind: TextureType) -> &'static str {
    match kind {
        TextureType::Color => "albedo",
        TextureType::Normal => "normal",
        TextureType::MetallicRoughness => "metallic_roughness",
        TextureType::Emissive => "emissive",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Reads back the position + texcoord of vertex `i` from an RMSH blob.
    // RMSH = b"RMSH" + u32 ver + u32 vcount + u32 icount + CookedVertex[]; CookedVertex is
    // position[3] + texture_coordinates[2] + normal[3] + tangent[3] + bitangent[3] = 14 f32.
    fn vertex(rmsh: &[u8], i: usize) -> ([f32; 3], [f32; 2]) {
        let base = 16 + i * 14 * 4;
        let f =
            |o: usize| f32::from_le_bytes(rmsh[base + o * 4..base + o * 4 + 4].try_into().unwrap());
        ([f(0), f(1), f(2)], [f(3), f(4)])
    }

    // Exercises the paths Sponza never hits: a node `matrix` transform, u32 indices, and a
    // normalized-u8 TEXCOORD. Expected values are computed by hand.
    #[test]
    fn parses_matrix_node_u32_indices_normalized_uv() {
        // Buffer: positions(3×vec3 f32) | normals(3×vec3 f32) | uv(3×vec2 u8) | pad | indices(3×u32)
        let mut buf = Vec::new();
        for p in [[1f32, 0., 0.], [0., 1., 0.], [0., 0., 1.]] {
            for v in p {
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }
        for _ in 0..3 {
            for v in [0f32, 0., 1.] {
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }
        buf.extend_from_slice(&[0, 0, 255, 0, 0, 255]); // uv u8: (0,0) (255,0) (0,255)
        buf.extend_from_slice(&[0, 0]); // pad to 4-byte alignment for u32 indices
        for i in [0u32, 1, 2] {
            buf.extend_from_slice(&i.to_le_bytes());
        }
        assert_eq!(buf.len(), 92);

        // Node matrix (column-major): scale 2 + translate (10,20,30).
        let json = r#"{
          "asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[0]}],
          "nodes":[{"mesh":0,"matrix":[2,0,0,0, 0,2,0,0, 0,0,2,0, 10,20,30,1]}],
          "meshes":[{"primitives":[{"attributes":{"POSITION":0,"NORMAL":1,"TEXCOORD_0":2},"indices":3}]}],
          "buffers":[{"uri":"b.bin","byteLength":92}],
          "bufferViews":[
            {"buffer":0,"byteOffset":0,"byteLength":36},
            {"buffer":0,"byteOffset":36,"byteLength":36},
            {"buffer":0,"byteOffset":72,"byteLength":6},
            {"buffer":0,"byteOffset":80,"byteLength":12}],
          "accessors":[
            {"bufferView":0,"componentType":5126,"type":"VEC3","count":3},
            {"bufferView":1,"componentType":5126,"type":"VEC3","count":3},
            {"bufferView":2,"componentType":5121,"type":"VEC2","count":3,"normalized":true},
            {"bufferView":3,"componentType":5125,"type":"SCALAR","count":3}],
          "materials":[],"textures":[],"images":[]
        }"#;

        let scene = parse_gltf_scene("t", json.as_bytes(), &[buf], &[]).expect("parse");
        assert_eq!(scene.meshes.len(), 1);
        let rmsh = &scene.meshes[0].rmsh;
        assert_eq!(&rmsh[0..4], b"RMSH");
        assert_eq!(u32::from_le_bytes(rmsh[8..12].try_into().unwrap()), 3); // vertex count
        assert_eq!(u32::from_le_bytes(rmsh[12..16].try_into().unwrap()), 3); // index count

        let approx = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-4);
        // matrix: world = scale 2 then translate (10,20,30)
        let (p0, uv0) = vertex(rmsh, 0);
        let (p1, uv1) = vertex(rmsh, 1);
        let (p2, uv2) = vertex(rmsh, 2);
        assert!(approx(p0, [12.0, 20.0, 30.0]), "p0 = {p0:?}");
        assert!(approx(p1, [10.0, 22.0, 30.0]), "p1 = {p1:?}");
        assert!(approx(p2, [10.0, 20.0, 32.0]), "p2 = {p2:?}");
        // normalized u8 → [0,1]
        assert!(
            (uv0[0]).abs() < 1e-4 && (uv0[1]).abs() < 1e-4,
            "uv0 = {uv0:?}"
        );
        assert!(
            (uv1[0] - 1.0).abs() < 1e-4 && uv1[1].abs() < 1e-4,
            "uv1 = {uv1:?}"
        );
        assert!(
            uv2[0].abs() < 1e-4 && (uv2[1] - 1.0).abs() < 1e-4,
            "uv2 = {uv2:?}"
        );

        // indices preserved (u32 path)
        let icount = 3usize;
        let ibase = 16 + 3 * 14 * 4;
        let indices: Vec<u32> = (0..icount)
            .map(|i| u32::from_le_bytes(rmsh[ibase + i * 4..ibase + i * 4 + 4].try_into().unwrap()))
            .collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    // One triangle (3 × vec3 f32 = 36 bytes) in buffer 0
    fn triangle_buffer() -> Vec<u8> {
        [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]]
            .iter()
            .flatten()
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }

    // Document with one triangle mesh; `extra` is spliced into the top-level object
    fn doc(scene: &str, primitive: &str, buffer_uri: &str, extra: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}}{scene},
              "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}{primitive}}}]}}],
              "buffers":[{{"uri":"{buffer_uri}","byteLength":36}}],
              "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],
              "accessors":[{{"bufferView":0,"componentType":5126,"type":"VEC3","count":3}}]{extra}}}"#
        )
    }

    const ONE_NODE: &str = r#","scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0}]"#;

    #[test]
    fn truncated_buffer_is_rejected() {
        let json = doc(ONE_NODE, "", "b.bin", "");
        let buffer = triangle_buffer()[..30].to_vec();
        assert!(parse_gltf_scene("t", json.as_bytes(), &[buffer], &[]).is_err());
    }

    #[test]
    fn node_cycle_is_rejected() {
        let scene = r#","scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"children":[1]},{"children":[0]}]"#;
        let json = doc(scene, "", "b.bin", "");
        assert!(parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &[]).is_err());
    }

    #[test]
    fn material_index_out_of_range_is_rejected() {
        let json = doc(ONE_NODE, r#","material":3"#, "b.bin", "");
        assert!(parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &[]).is_err());
    }

    #[test]
    fn no_scenes_uses_every_mesh_untransformed() {
        let json = doc("", "", "b.bin", "");
        let scene = parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &[]).unwrap();
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(scene.meshes[0].name, "t_mesh_m0_0");
        let (position, _) = vertex(&scene.meshes[0].rmsh, 1);
        assert_eq!(position, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn data_uri_buffer_parses() {
        // base64 of triangle_buffer()
        const BASE64: &str = "AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA";
        let json = doc(
            ONE_NODE,
            "",
            &format!("data:application/octet-stream;base64,{BASE64}"),
            "",
        );
        assert_eq!(
            gltf_resource_uris(json.as_bytes()).unwrap().buffers,
            vec![None]
        );
        let scene = parse_gltf_scene("t", json.as_bytes(), &[], &[]).unwrap();
        let (position, _) = vertex(&scene.meshes[0].rmsh, 2);
        assert_eq!(position, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn emissive_factor_is_kept() {
        let json = doc(
            ONE_NODE,
            r#","material":0"#,
            "b.bin",
            r#","materials":[{"emissiveFactor":[1,0.5,0]}]"#,
        );
        let scene = parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &[]).unwrap();
        assert_eq!(scene.materials[0].emissive_color, [1.0, 0.5, 0.0]);
    }

    #[test]
    fn prefixes_give_disjoint_names() {
        let json = doc(ONE_NODE, "", "b.bin", "");
        let names = |prefix| {
            let scene =
                parse_gltf_scene(prefix, json.as_bytes(), &[triangle_buffer()], &[]).unwrap();
            let mut names: Vec<String> = scene.materials.into_iter().map(|m| m.name).collect();
            names.extend(scene.meshes.into_iter().map(|m| m.name));
            names
        };
        let (a, b) = (names("a"), names("b"));
        assert!(a.iter().all(|name| !b.contains(name)), "{a:?} {b:?}");
    }

    #[test]
    fn unreferenced_unsupported_image_is_not_decoded() {
        let json = doc(ONE_NODE, "", "b.bin", r#","images":[{"uri":"x.webp"}]"#);
        let images = vec![b"RIFF....WEBP".to_vec()];
        let scene = parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &images).unwrap();
        assert!(scene.textures.is_empty());
    }

    #[test]
    fn default_material_is_rough_metal() {
        let json = doc(ONE_NODE, "", "b.bin", "");
        let scene = parse_gltf_scene("t", json.as_bytes(), &[triangle_buffer()], &[]).unwrap();
        let default = &scene.materials[scene.meshes[0].material];
        assert_eq!((default.metallic, default.roughness), (1.0, 1.0));
    }
}
