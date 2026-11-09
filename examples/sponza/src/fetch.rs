//! HTTP byte fetch. WASM uses the browser `fetch` API (cross-origin is allowed because
//! raw.githubusercontent.com serves `access-control-allow-origin: *`); native uses `ureq`.

/// Base URL of the multi-file Sponza glTF. Buffer (`Sponza.bin`) and texture URIs in the
/// document are relative to this, exactly as three.js's GLTFLoader resolves them.
pub const SPONZA_BASE: &str =
    "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/Sponza/glTF/";

/// CC0 studio HDR for image-based lighting (Poly Haven also serves permissive CORS headers).
pub const HDR_URL: &str =
    "https://dl.polyhaven.org/file/ph-assets/HDRIs/hdr/1k/studio_small_08_1k.hdr";

/// Fetches all `urls` concurrently; results are in `urls` order.
#[cfg(target_arch = "wasm32")]
pub async fn get_all(urls: &[String]) -> Result<Vec<Vec<u8>>, String> {
    futures::future::join_all(urls.iter().map(|url| get(url)))
        .await
        .into_iter()
        .collect()
}

/// Fetches all `urls` concurrently (one thread each); results are in `urls` order.
#[cfg(not(target_arch = "wasm32"))]
pub async fn get_all(urls: &[String]) -> Result<Vec<Vec<u8>>, String> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = urls
            .iter()
            .map(|url| scope.spawn(move || get(url)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "fetch thread panicked".to_string())?
            })
            .collect()
    })
}

#[cfg(target_arch = "wasm32")]
async fn get(url: &str) -> Result<Vec<u8>, String> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{Request, Response};

    // A plain GET request; cross-origin requests default to CORS mode, which is what we want.
    let request = Request::new_with_str(url).map_err(|e| format!("build request {url}: {e:?}"))?;
    let window = web_sys::window().ok_or_else(|| "no window".to_string())?;
    let resp_value = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("fetch {url}: {e:?}"))?;
    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| format!("fetch {url}: response was not a Response"))?;
    if !resp.ok() {
        return Err(format!("HTTP {} for {url}", resp.status()));
    }
    let buf = JsFuture::from(
        resp.array_buffer()
            .map_err(|e| format!("array_buffer {url}: {e:?}"))?,
    )
    .await
    .map_err(|e| format!("array_buffer await {url}: {e:?}"))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

#[cfg(not(target_arch = "wasm32"))]
fn get(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .map_err(|e| format!("read {url}: {e}"))?;
    Ok(buf)
}
