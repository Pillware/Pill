use std::path::PathBuf;

use pill_assets::{default_rules, Pipeline};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let res = manifest.join("res");
    let models = res.join("models");
    std::fs::create_dir_all(&models).unwrap();

    let glb = models.join("MetalRoughSpheres.glb");
    if !glb.exists() {
        let url = "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/MetalRoughSpheres/glTF-Binary/MetalRoughSpheres.glb";
        let resp = ureq::get(url)
            .call()
            .expect("download MetalRoughSpheres.glb");
        let mut f = std::fs::File::create(&glb).unwrap();
        std::io::copy(&mut resp.into_reader(), &mut f).unwrap();
    }

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
