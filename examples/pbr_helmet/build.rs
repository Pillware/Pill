use std::path::{Path, PathBuf};

use pill_assets::{default_rules, Pipeline};

fn download_if_missing(path: &Path, url: &str) {
    if path.exists() {
        return;
    }
    // Download to a temp file and rename on success, so an interrupted download is retried
    let partial = path.with_extension("part");
    let response = ureq::get(url)
        .call()
        .unwrap_or_else(|error| panic!("download {url}: {error}"));
    let mut file = std::fs::File::create(&partial).unwrap();
    std::io::copy(&mut response.into_reader(), &mut file)
        .unwrap_or_else(|error| panic!("download {url}: {error}"));
    std::fs::rename(&partial, path).unwrap();
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let res = manifest.join("res");
    let models = res.join("models");
    std::fs::create_dir_all(&models).unwrap();

    download_if_missing(
        &models.join("DamagedHelmet.glb"),
        "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb",
    );
    // Studio HDR (low-frequency softbox, CC0)
    download_if_missing(
        &models.join("studio_small_08.hdr"),
        "https://dl.polyhaven.org/file/ph-assets/HDRIs/hdr/1k/studio_small_08_1k.hdr",
    );

    // Cook here (the launcher's cook step runs before this script downloads the sources):
    // GLB → mesh + textures, HDR → equirect → IBL maps, all include_bytes!'d by game.rs
    let stats = Pipeline {
        root: res,
        rules: default_rules(),
    }
    .run()
    .expect("asset pipeline failed");
    for input in &stats.discovered {
        println!("cargo:rerun-if-changed={}", input.display());
    }
}
