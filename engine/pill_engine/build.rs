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
    backdate_generated_shaders(&root.join("shaders"));

    println!("cargo:rerun-if-changed=build.rs");
    // Header-include directory: tracked recursively so changes to shared
    // structs (common.hlsl) re-trigger the build. The glob doesn't pick these
    // up (HlslToWgsl matches top-level shaders/*.hlsl only).
    println!(
        "cargo:rerun-if-changed={}",
        root.join("shaders/include").display()
    );
    // The directory itself, so a newly added shader re-runs the pipeline
    println!("cargo:rerun-if-changed={}", root.join("shaders").display());
    for input in &stats.discovered {
        println!("cargo:rerun-if-changed={}", input.display());
    }
}

/// Gives each generated `.wgsl` its `.hlsl` source's mtime. The shaders directory is watched (so a
/// new shader re-runs this script), and outputs written during the run would otherwise be newer
/// than the run and trigger one more rebuild. The cook's up-to-date check (output >= input) still
/// holds.
fn backdate_generated_shaders(directory: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let source = entry.path();
        if source
            .extension()
            .is_none_or(|extension| extension != "hlsl")
        {
            continue;
        }
        let output = source.with_extension("wgsl");
        let Ok(modified) = source.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if let Ok(file) = std::fs::File::options().write(true).open(&output) {
            let _ = file.set_modified(modified);
        }
    }
}
