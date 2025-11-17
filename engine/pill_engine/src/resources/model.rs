use crate::{
    engine::Engine,
    resources::{
        material::PBRMaterialHandle, texture::TextureHandle, Mesh, MeshData, MeshHandle,
        MeshVertex, PBRMaterial, Resource, ResourceLoadType, ResourceStorage, Texture, TextureType,
    },
};
use anyhow::{anyhow, Context, Result};
use cgmath::InnerSpace;
use gltf::import as gltf_import;
use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, Rgb, Rgba};
use log::info;
use pill_core::{get_type_name, PillSlotMapKey, PillTypeMapKey, Vector3f};
use std::collections::HashMap;
use std::env;
use std::io::Cursor;
use std::path::{Path, PathBuf};

pill_core::define_new_pill_slotmap_key! {
    pub struct ModelHandle;
}

#[readonly::make]
pub struct ModelMaterialSlot {
    #[readonly]
    pub mesh: MeshHandle,
    #[readonly]
    pub material: PBRMaterialHandle,
    #[readonly]
    pub translation: Vector3f,
    #[readonly]
    pub rotation_euler_deg: Vector3f, // XYZ degrees, matches TransformComponent expectations
    #[readonly]
    pub scale: Vector3f,
}

#[readonly::make]
pub struct Model {
    #[readonly]
    pub name: String,
    #[readonly]
    pub path: PathBuf,
    #[readonly]
    pub meshes: Vec<MeshHandle>,
    #[readonly]
    pub materials: Vec<PBRMaterialHandle>,
    #[readonly]
    pub material_slots: Vec<ModelMaterialSlot>,
}

impl Model {
    pub fn new(name: &str, path: PathBuf) -> Self {
        Self {
            name: name.to_string(),
            path,
            meshes: Vec::new(),
            materials: Vec::new(),
            material_slots: Vec::new(),
        }
    }
}

impl PillTypeMapKey for Model {
    type Storage = ResourceStorage<Model>;
}

impl Resource for Model {
    type Handle = ModelHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // Resolve absolute path
        let model_path = engine.game_resources_directory_path.join(&self.path);
        let (doc, buffers, images) = gltf_import(&model_path)
            .with_context(|| format!("Importing glTF '{}' failed", model_path.display()))?;

        let base_dir = model_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        // Diagnostics:
        // - To enable verbose glTF import logging, set the runtime config key:
        //     LOG_GLTF_IMPORT = true
        //   and run with environment variables:
        //     RUST_LOG=info RUST_BACKTRACE=1
        // - The logs will print scenes/nodes/meshes/materials/images and, for each
        //   material, whether baseColor/mr/normal/emissive textures are bound.
        // - If all textures show as false, your asset may use KTX2/Basis textures.
        //   Use a PNG/JPG variant (e.g., glTF-Sample-Models “glTF-Binary”) or add
        //   Basis/KTX2 transcode support before import.
        // Logging flag: enabled if LOG_GLTF_IMPORT=true in config or environment
        let log_import = engine.config.get_bool("LOG_GLTF_IMPORT").unwrap_or(false)
            || env::var("LOG_GLTF_IMPORT")
                .map(|v| {
                    v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes")
                })
                .unwrap_or(false);
        if log_import {
            info!(
                "glTF import '{}': scenes={}, nodes={}, meshes={}, materials={}, images={}",
                self.name,
                doc.scenes().len(),
                doc.nodes().len(),
                doc.meshes().len(),
                doc.materials().len(),
                images.len()
            );
            println!(
                "glTF import '{}': scenes={}, nodes={}, meshes={}, materials={}, images={}",
                self.name,
                doc.scenes().len(),
                doc.nodes().len(),
                doc.meshes().len(),
                doc.materials().len(),
                images.len()
            );
            // Print textures and their sources up-front
            for (ti, tex) in doc.textures().enumerate() {
                let src = tex.source();
                let src_desc = match src.source() {
                    gltf::image::Source::Uri { uri, .. } => format!("uri '{}'", uri),
                    gltf::image::Source::View { .. } => "embedded view".to_string(),
                };
                info!("  texture[{}] -> image[{}] ({})", ti, src.index(), src_desc);
                println!("  texture[{}] -> image[{}] ({})", ti, src.index(), src_desc);
            }
            // Print materials with bound textures and factors
            for (i, m) in doc.materials().enumerate() {
                let p = m.pbr_metallic_roughness();
                let albedo_tex = p.base_color_texture().map(|t| t.texture().source().index());
                let mr_tex = p
                    .metallic_roughness_texture()
                    .map(|t| t.texture().source().index());
                let normal_tex = m.normal_texture().map(|t| t.texture().source().index());
                let emissive_tex = m.emissive_texture().map(|t| t.texture().source().index());
                let name = m.name().unwrap_or("<unnamed>");
                let bc = p.base_color_factor();
                let ef = m.emissive_factor();
                info!(
                    "  material[{}] '{}': baseColorTex={:?}, mrTex={:?}, normalTex={:?}, emissiveTex={:?}, baseColorFactor=[{:.3},{:.3},{:.3}], metal={:.3}, rough={:.3}, emissive=[{:.3},{:.3},{:.3}]",
                    i, name, albedo_tex, mr_tex, normal_tex, emissive_tex, bc[0], bc[1], bc[2], p.metallic_factor(), p.roughness_factor(), ef[0], ef[1], ef[2]
                );
                println!(
                    "  material[{}] '{}': baseColorTex={:?}, mrTex={:?}, normalTex={:?}, emissiveTex={:?}, baseColorFactor=[{:.3},{:.3},{:.3}], metal={:.3}, rough={:.3}, emissive=[{:.3},{:.3},{:.3}]",
                    i, name, albedo_tex, mr_tex, normal_tex, emissive_tex, bc[0], bc[1], bc[2], p.metallic_factor(), p.roughness_factor(), ef[0], ef[1], ef[2]
                );
            }
            // Print meshes and primitive attribute overview
            for (mi, m) in doc.meshes().enumerate() {
                let mname = m.name().unwrap_or("<unnamed>");
                info!(
                    "  mesh[{}] '{}': {} primitives",
                    mi,
                    mname,
                    m.primitives().len()
                );
                println!(
                    "  mesh[{}] '{}': {} primitives",
                    mi,
                    mname,
                    m.primitives().len()
                );
                for (pi, prim) in m.primitives().enumerate() {
                    let attrs: Vec<_> = prim.attributes().map(|(s, _)| s.to_string()).collect();
                    info!(
                        "    prim[{}]: mode={:?}, attrs={:?}",
                        pi,
                        prim.mode(),
                        attrs
                    );
                    println!(
                        "    prim[{}]: mode={:?}, attrs={:?}",
                        pi,
                        prim.mode(),
                        attrs
                    );
                }
            }
            // Print nodes referencing meshes
            for (ni, node) in doc.nodes().enumerate() {
                if let Some(mesh) = node.mesh() {
                    let nname = node.name().unwrap_or("<unnamed>");
                    let (t, r, s) = node.transform().decomposed();
                    info!(
                        "  node[{}] '{}': mesh={}, T=[{:.3},{:.3},{:.3}]",
                        ni,
                        nname,
                        mesh.index(),
                        t[0],
                        t[1],
                        t[2]
                    );
                    println!(
                        "  node[{}] '{}': mesh={}, T=[{:.3},{:.3},{:.3}]",
                        ni,
                        nname,
                        mesh.index(),
                        t[0],
                        t[1],
                        t[2]
                    );
                }
            }
        }

        // --- Textures and Materials ---
        // Cache textures by image index to avoid duplicates (works for URI/data/embedded)
        let mut image_index_to_tex: HashMap<usize, TextureHandle> = HashMap::new();

        let mut materials: Vec<PBRMaterialHandle> = Vec::new();
        for (mat_index, mat) in doc.materials().enumerate() {
            let mat_name_owned = mat
                .name()
                .map(|s| s.to_string())
                .unwrap_or(format!("Mat{}", mat_index));
            let mut pbr = PBRMaterial::new(&format!("{}_{}", self.name, mat_name_owned));

            // Factors
            let pbrmr = mat.pbr_metallic_roughness();
            let bc = pbrmr.base_color_factor();
            pbr.set_base_color_factor(pill_core::Color::new(bc[0], bc[1], bc[2]));
            pbr.set_metallic_factor(pbrmr.metallic_factor());
            pbr.set_roughness_factor(pbrmr.roughness_factor());
            let ef = mat.emissive_factor();
            pbr.set_emissive_factor(pill_core::Color::new(ef[0], ef[1], ef[2]));

            // Helper to load a texture from a glTF texture reference via imported images
            let mut load_tex =
                |tex: gltf::Texture, tex_type: TextureType| -> Result<TextureHandle> {
                    let img = tex.source();
                    let idx = img.index();
                    if let Some(h) = image_index_to_tex.get(&idx) {
                        if log_import {
                            info!("  reuse image[{}] as {:?} texture", idx, tex_type);
                        }
                        println!("  reuse image[{}] as {:?} texture", idx, tex_type);
                        return Ok(h.clone());
                    }
                    let img_data = images.get(idx).ok_or_else(|| {
                        anyhow!("Image index {} not found in imported images", idx)
                    })?;
                    // Encode to PNG bytes for Texture::Bytes
                    let buf = encode_gltf_image_to_png_bytes(img_data)?;
                    let tex_name = format!("{}_image_{}", self.name, idx);
                    let texture = Texture::new(&tex_name, tex_type, ResourceLoadType::Bytes(buf));
                    let h = engine.add_resource::<Texture>(texture)?;
                    image_index_to_tex.insert(idx, h.clone());
                    let src_desc = match tex.source().source() {
                        gltf::image::Source::Uri { uri, .. } => format!("uri '{}'", uri),
                        gltf::image::Source::View { .. } => "embedded view".to_string(),
                    };
                    if log_import {
                        info!(
                            "  created Texture '{}' for image[{}] ({}) as {:?}",
                            tex_name, idx, src_desc, tex_type
                        );
                    }
                    println!(
                        "  created Texture '{}' for image[{}] ({}) as {:?}",
                        tex_name, idx, src_desc, tex_type
                    );
                    Ok(h)
                };

            // Base color texture
            if let Some(ti) = pbrmr.base_color_texture() {
                let th = load_tex(ti.texture(), TextureType::Gamma)?;
                pbr.set_albedo_texture(th);
            }
            // Metallic-roughness combined texture
            if let Some(ti) = pbrmr.metallic_roughness_texture() {
                let th = load_tex(ti.texture(), TextureType::Linear)?;
                pbr.set_metallic_roughness_texture(th);
            }
            // Normal map
            if let Some(ti) = mat.normal_texture() {
                let th = load_tex(ti.texture(), TextureType::Linear)?;
                pbr.set_normal_texture(th);
            }
            // Emissive map
            if let Some(ti) = mat.emissive_texture() {
                let th = load_tex(ti.texture(), TextureType::Gamma)?;
                pbr.set_emissive_texture(th);
            }

            let mh = engine.add_resource::<PBRMaterial>(pbr)?;
            materials.push(mh);
            if log_import {
                let mref = engine.get_resource::<PBRMaterial>(&mh)?;
                info!(
                    "material[{}] '{}': baseColorTex={}, mrTex={}, normalTex={}, emissiveTex={}",
                    mat_index,
                    mref.name,
                    mref.albedo_texture.is_some(),
                    mref.metallic_roughness_texture.is_some(),
                    mref.normal_texture.is_some(),
                    mref.emissive_texture.is_some()
                );
                println!(
                    "material[{}] '{}': baseColorTex={}, mrTex={}, normalTex={}, emissiveTex={}",
                    mat_index,
                    mref.name,
                    mref.albedo_texture.is_some(),
                    mref.metallic_roughness_texture.is_some(),
                    mref.normal_texture.is_some(),
                    mref.emissive_texture.is_some()
                );
            }
        }

        // --- Meshes (per primitive) ---
        // Map (mesh_index, prim_index) -> MeshHandle
        let mut prim_to_mesh: HashMap<(usize, usize), MeshHandle> = HashMap::new();
        let mut mesh_handles: Vec<MeshHandle> = Vec::new();
        for (m_i, m) in doc.meshes().enumerate() {
            for (p_i, primitive) in m.primitives().enumerate() {
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .ok_or_else(|| anyhow!("Missing POSITION in mesh {} primitive {}", m_i, p_i))?
                    .collect();
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .ok_or_else(|| anyhow!("Missing NORMAL in mesh {} primitive {}", m_i, p_i))?
                    .collect();
                let texcoords: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|tc| tc.into_f32().collect())
                    .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

                // Indices
                let indices: Vec<u32> = if let Some(read_indices) = reader.read_indices() {
                    read_indices.into_u32().collect()
                } else {
                    (0..positions.len() as u32).collect()
                };

                // Build MeshData with tangents/bitangents and AABB
                let mut vertices: Vec<MeshDataVertexTemp> = Vec::with_capacity(positions.len());
                let mut min_v = cgmath::Vector3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
                let mut max_v =
                    cgmath::Vector3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
                for i in 0..positions.len() {
                    let p = positions[i];
                    let n = normals[i];
                    let uv = texcoords.get(i).copied().unwrap_or([0.0, 0.0]);
                    let pp = cgmath::Vector3::new(p[0], p[1], p[2]);
                    min_v = cgmath::Vector3::new(
                        min_v.x.min(pp.x),
                        min_v.y.min(pp.y),
                        min_v.z.min(pp.z),
                    );
                    max_v = cgmath::Vector3::new(
                        max_v.x.max(pp.x),
                        max_v.y.max(pp.y),
                        max_v.z.max(pp.z),
                    );
                    vertices.push(MeshDataVertexTemp {
                        position: p,
                        normal: n,
                        uv,
                        tangent: [0.0; 3],
                        bitangent: [0.0; 3],
                    });
                }

                // Tangents
                let mut counts = vec![0usize; vertices.len()];
                for tri in indices.chunks(3) {
                    if tri.len() < 3 {
                        continue;
                    }
                    let v0 = vertices[tri[0] as usize];
                    let v1 = vertices[tri[1] as usize];
                    let v2 = vertices[tri[2] as usize];
                    let pos0: cgmath::Vector3<_> = v0.position.into();
                    let pos1: cgmath::Vector3<_> = v1.position.into();
                    let pos2: cgmath::Vector3<_> = v2.position.into();
                    let uv0: cgmath::Vector2<_> = v0.uv.into();
                    let uv1: cgmath::Vector2<_> = v1.uv.into();
                    let uv2: cgmath::Vector2<_> = v2.uv.into();
                    let dp1 = pos1 - pos0;
                    let dp2 = pos2 - pos0;
                    let duv1 = uv1 - uv0;
                    let duv2 = uv2 - uv0;
                    let r = 1.0 / (duv1.x * duv2.y - duv1.y * duv2.x);
                    let t = (dp1 * duv2.y - dp2 * duv1.y) * r;
                    let b = (dp2 * duv1.x - dp1 * duv2.x) * r;
                    for &idx in tri {
                        let v = &mut vertices[idx as usize];
                        v.tangent = (cgmath::Vector3::from(v.tangent) + t).into();
                        v.bitangent = (cgmath::Vector3::from(v.bitangent) + b).into();
                        counts[idx as usize] += 1;
                    }
                }
                let mut final_vertices: Vec<MeshVertex> = Vec::with_capacity(vertices.len());
                for (i, c) in counts.into_iter().enumerate() {
                    let denom = if c == 0 { 1.0 } else { 1.0 / c as f32 };
                    let v = &mut vertices[i];
                    let tan = (pill_core::Vector3f::from(v.tangent) * denom).normalize();
                    let bitan = (pill_core::Vector3f::from(v.bitangent) * denom).normalize();
                    final_vertices.push(MeshVertex::new(
                        v.position,
                        v.uv,
                        v.normal,
                        tan.into(),
                        bitan.into(),
                    ));
                }
                let data = MeshData {
                    vertices: final_vertices,
                    indices: indices.clone(),
                    aabb_min: [min_v.x, min_v.y, min_v.z],
                    aabb_max: [max_v.x, max_v.y, max_v.z],
                };

                let mesh_name = format!("{}_m{}_p{}", self.name, m_i, p_i);
                let mh = engine.add_resource::<Mesh>(Mesh::from_mesh_data(&mesh_name, data))?;
                prim_to_mesh.insert((m_i, p_i), mh.clone());
                mesh_handles.push(mh);
                if log_import {
                    info!(
                        "mesh[{}] prim[{}] -> '{}' (vtx={}, idx={})",
                        m_i,
                        p_i,
                        mesh_name,
                        positions.len(),
                        indices.len()
                    );
                }
            }
        }

        // --- Material slots from scene nodes (local TRS) ---
        let mut slots: Vec<ModelMaterialSlot> = Vec::new();
        for node in doc.nodes() {
            if let Some(m) = node.mesh() {
                // Node local TRS
                let (t, r, s) = node.transform().decomposed();
                // Convert quat -> Euler ZYX to match TransformComponent (Rz * Ry * Rx)
                let q = glam::Quat::from_xyzw(r[0], r[1], r[2], r[3]);
                let (z, y, x) = q.to_euler(glam::EulerRot::ZYX);
                let euler_deg = Vector3f::new(x.to_degrees(), y.to_degrees(), z.to_degrees());

                for (p_i, primitive) in m.primitives().enumerate() {
                    let mesh_h = prim_to_mesh
                        .get(&(m.index(), p_i))
                        .ok_or_else(|| anyhow!("Missing mesh handle for primitive"))?
                        .clone();
                    // Material mapping
                    let mat_index = primitive.material().index().unwrap_or(0);
                    let mat_h = materials
                        .get(mat_index)
                        .ok_or_else(|| anyhow!("Material index out of bounds"))?
                        .clone();

                    slots.push(ModelMaterialSlot {
                        mesh: mesh_h,
                        material: mat_h,
                        translation: Vector3f::new(t[0], t[1], t[2]),
                        rotation_euler_deg: euler_deg,
                        scale: Vector3f::new(s[0], s[1], s[2]),
                    });
                }
            }
        }

        self.meshes = mesh_handles;
        self.materials = materials;
        self.material_slots = slots;

        if log_import {
            info!("model '{}' slots: {}", self.name, self.material_slots.len());
        }

        Ok(())
    }
}

// Internal: temp vertex used to accumulate tangents
#[derive(Clone, Copy)]
struct MeshDataVertexTemp {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    tangent: [f32; 3],
    bitangent: [f32; 3],
}

fn encode_gltf_image_to_png_bytes(data: &gltf::image::Data) -> Result<Box<[u8]>> {
    let (w, h) = (data.width, data.height);
    let dyn_img: DynamicImage = match data.format {
        gltf::image::Format::R8G8B8A8 => {
            let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(w, h, data.pixels.clone())
                .ok_or_else(|| anyhow!("Invalid RGBA8 image buffer"))?;
            DynamicImage::ImageRgba8(img)
        }
        gltf::image::Format::R8G8B8 => {
            let img: ImageBuffer<Rgb<u8>, _> = ImageBuffer::from_raw(w, h, data.pixels.clone())
                .ok_or_else(|| anyhow!("Invalid RGB8 image buffer"))?;
            DynamicImage::ImageRgb8(img)
        }
        gltf::image::Format::R8 => {
            let img: ImageBuffer<Luma<u8>, _> = ImageBuffer::from_raw(w, h, data.pixels.clone())
                .ok_or_else(|| anyhow!("Invalid L8 image buffer"))?;
            DynamicImage::ImageLuma8(img)
        }
        // Fallback: expand to RGBA8
        _ => {
            // Try to interpret as RGBA8 by padding/truncation if length matches
            let expected = (w as usize) * (h as usize) * 4;
            let mut pixels = data.pixels.clone();
            pixels.resize(expected, 255u8);
            let img: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_raw(w, h, pixels)
                .ok_or_else(|| anyhow!("Invalid fallback image buffer"))?;
            DynamicImage::ImageRgba8(img)
        }
    };
    let mut buf = Vec::new();
    {
        let mut writer = Cursor::new(&mut buf);
        dyn_img.write_to(&mut writer, ImageFormat::Png)?;
    }
    Ok(buf.into_boxed_slice())
}
