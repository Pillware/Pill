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
        &models.join("MetalRoughSpheres.glb"),
        "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/MetalRoughSpheres/glTF-Binary/MetalRoughSpheres.glb",
    );

    // Cook here (the launcher's cook step runs before this script downloads the GLB):
    // GLB → mesh + textures, studio.procedural_equirect → equirect → IBL maps
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
