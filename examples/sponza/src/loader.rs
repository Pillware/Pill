//! Fetches the multi-file Sponza glTF over HTTP and hands the bytes to the engine's runtime glTF
//! loader. Mirrors three.js's GLTFLoader: fetch `Sponza.gltf`, then resolve+fetch `Sponza.bin` and
//! every texture relative to the base URL.
//!
//! All glTF parsing, image decoding and mesh/texture byte-format conversion lives in the engine
//! (`pill_engine`'s `gltf_loading` feature). This module is just I/O + orchestration: the fetch is
//! the only platform-specific part (browser `fetch` on WASM, `ureq` threads on native).

use pill_engine::game::{gltf_resource_uris, ibl, parse_gltf_scene, GltfSceneData};

use crate::fetch;

pub struct SponzaCpu {
    pub scene: GltfSceneData,
    pub equirect: ibl::Equirect, // RGBA f32 background panorama (from fetched HDR)
    pub ibl: (Vec<f32>, Vec<Vec<f32>>), // ibl::bake_all output: diffuse, specular mips
}

/// Fetches the glTF, then its buffers, textures and the HDR concurrently; parses and bakes IBL.
pub async fn load() -> Result<SponzaCpu, String> {
    let gltf_url = format!("{}Sponza.gltf", fetch::SPONZA_BASE);
    let gltf_bytes = fetch::get_all(std::slice::from_ref(&gltf_url))
        .await?
        .remove(0);
    let uris = gltf_resource_uris(&gltf_bytes).map_err(|e| e.to_string())?;

    // `None` entries (embedded data) stay empty; the engine resolves them from the document
    let resources: Vec<&Option<String>> = uris.buffers.iter().chain(&uris.images).collect();
    let mut urls: Vec<String> = resources
        .iter()
        .filter_map(|uri| uri.as_ref())
        .map(|uri| format!("{}{}", fetch::SPONZA_BASE, uri))
        .collect();
    urls.push(fetch::HDR_URL.to_string());
    log::info!("Sponza: fetching {} files", urls.len() + 1);
    let mut fetched = fetch::get_all(&urls).await?.into_iter();
    let mut resolved: Vec<Vec<u8>> = resources
        .iter()
        .map(|uri| match uri {
            Some(_) => fetched.next().unwrap_or_default(),
            None => Vec::new(),
        })
        .collect();
    let hdr = fetched.next().unwrap_or_default();
    let images = resolved.split_off(uris.buffers.len());
    log::info!(
        "Sponza: fetched {:.1} MB",
        (gltf_bytes.len() + hdr.len() + resolved.iter().chain(&images).map(Vec::len).sum::<usize>())
            as f64
            / (1024.0 * 1024.0)
    );

    let scene =
        parse_gltf_scene("sponza", &gltf_bytes, &resolved, &images).map_err(|e| e.to_string())?;
    finish(scene, &hdr)
}

/// Bakes IBL from the fetched HDR and packages the result.
fn finish(scene: GltfSceneData, hdr_bytes: &[u8]) -> Result<SponzaCpu, String> {
    let equirect = ibl::equirect_from_hdr(hdr_bytes).map_err(|e| e.to_string())?;
    let ibl = ibl::bake_all(&equirect);
    log::info!(
        "Sponza: parsed {} meshes, {} materials, {} textures",
        scene.meshes.len(),
        scene.materials.len(),
        scene.textures.len(),
    );
    Ok(SponzaCpu {
        scene,
        equirect,
        ibl,
    })
}
