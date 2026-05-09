// Cargo build-script entry. Rules + runner live in `pill_assets`; the launcher
// invokes the same Pipeline via `-a assets`.

use std::env;
use std::path::PathBuf;

use pill_assets::{default_rules, Pipeline};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("res");
    let pipeline = Pipeline {
        root: root.clone(),
        rules: default_rules(),
    };

    let stats = pipeline.run().expect("asset pipeline failed");

    println!("cargo:rerun-if-changed=build.rs");
    // Header-include directory: tracked recursively so changes to shared
    // structs (common.hlsl) re-trigger the build. The glob doesn't pick these
    // up (HlslToWgsl matches top-level shaders/*.hlsl only).
    println!("cargo:rerun-if-changed={}", root.join("shaders/include").display());
    for input in &stats.discovered {
        println!("cargo:rerun-if-changed={}", input.display());
    }
}
