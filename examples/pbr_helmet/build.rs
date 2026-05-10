use std::io::Write;
use std::path::PathBuf;

use pill_assets::{GlbToCookedMesh, Rule};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let models = manifest.join("res/models");
    std::fs::create_dir_all(&models).unwrap();

    let glb = models.join("DamagedHelmet.glb");
    if !glb.exists() {
        let url = "https://raw.githubusercontent.com/KhronosGroup/glTF-Sample-Assets/main/Models/DamagedHelmet/glTF-Binary/DamagedHelmet.glb";
        let resp = ureq::get(url).call().expect("download DamagedHelmet.glb");
        let mut f = std::fs::File::create(&glb).unwrap();
        std::io::copy(&mut resp.into_reader(), &mut f).unwrap();
    }

    let cooked_mesh = models.join("DamagedHelmet.cooked_mesh");
    if !cooked_mesh.exists() {
        GlbToCookedMesh
            .build(&glb, &cooked_mesh)
            .expect("cook DamagedHelmet.glb");
    }

    // Download studio HDR (low-frequency softbox, CC0) if missing.
    let hdr_path = models.join("studio_small_08.hdr");
    if !hdr_path.exists() {
        let url = "https://dl.polyhaven.org/file/ph-assets/HDRIs/hdr/1k/studio_small_08_1k.hdr";
        let resp = ureq::get(url).call().expect("download studio_small_08 HDR");
        let mut f = std::fs::File::create(&hdr_path).unwrap();
        std::io::copy(&mut resp.into_reader(), &mut f).unwrap();
    }

    // Decode HDR → resize to 512×256 → write raw f32 RGBA binary to OUT_DIR for include_bytes!.
    println!("cargo:rerun-if-changed=res/models/studio_small_08.hdr");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let f32bin = out_dir.join("equirect.f32bin");
    {
        let img = image::open(&hdr_path).expect("open HDR");
        let src = img.to_rgb32f();
        let (out_w, out_h) = (512u32, 256u32);
        let resized =
            image::imageops::resize(&src, out_w, out_h, image::imageops::FilterType::Lanczos3);
        let mut out = std::fs::File::create(&f32bin).unwrap();
        out.write_all(&out_w.to_le_bytes()).unwrap();
        out.write_all(&out_h.to_le_bytes()).unwrap();
        let mut row = Vec::with_capacity((out_w * 4 * 4) as usize);
        for px in resized.pixels() {
            row.extend_from_slice(&px.0[0].to_le_bytes());
            row.extend_from_slice(&px.0[1].to_le_bytes());
            row.extend_from_slice(&px.0[2].to_le_bytes());
            row.extend_from_slice(&1.0f32.to_le_bytes());
        }
        out.write_all(&row).unwrap();
    }
}
