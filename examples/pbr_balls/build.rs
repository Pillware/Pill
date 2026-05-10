use std::path::PathBuf;

use pill_assets::{GlbToCookedMesh, Rule};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let models = manifest.join("res/models");
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

    let cooked_mesh = models.join("MetalRoughSpheres.cooked_mesh");
    if !cooked_mesh.exists() {
        GlbToCookedMesh
            .build(&glb, &cooked_mesh)
            .expect("cook MetalRoughSpheres.glb");
    }
}
