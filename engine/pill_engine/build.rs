// Pre-build step: compile HLSL shaders in res/shaders/ to WGSL via slangc.
//
// HLSL is the human source (hand-edited); WGSL is the build artifact (committed
// alongside, debuggable in browser devtools, runtime-loadable for hot-reload).
// The runtime never invokes a shader compiler — `naga` is not linked.
//
// Pipeline DAG:
//   res/shaders/*.hlsl   (committed source)
//        |  build.rs invokes `slangc -target wgsl -O0 -g`
//        v
//   res/shaders/*.wgsl   (build artifact, committed for review/CI verify)
//        |  include_bytes! at compile time (engine defaults)
//        |  OR std::fs::read at runtime (game-supplied custom shaders, hot-reload)
//        v
//   wgpu::ShaderSource::Wgsl(...)
//
// slangc is required on PATH. Install from https://github.com/shader-slang/slang/releases.

use std::env;
use std::path::PathBuf;
use std::process::Command;

struct ShaderEntry {
    hlsl: &'static str,
    wgsl: &'static str,
    entry: &'static str,
    stage: &'static str,
}

const SHADERS: &[ShaderEntry] = &[
    ShaderEntry {
        hlsl: "default_vertex.hlsl",
        wgsl: "default_vertex.wgsl",
        entry: "vs_main",
        stage: "vertex",
    },
    ShaderEntry {
        hlsl: "default_lit_fragment.hlsl",
        wgsl: "default_lit_fragment.wgsl",
        entry: "fs_main",
        stage: "fragment",
    },
    ShaderEntry {
        hlsl: "default_unlit_fragment.hlsl",
        wgsl: "default_unlit_fragment.wgsl",
        entry: "fs_main",
        stage: "fragment",
    },
];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let shader_dir = manifest_dir.join("res").join("shaders");

    println!("cargo:rerun-if-changed=build.rs");

    for s in SHADERS {
        let hlsl_path = shader_dir.join(s.hlsl);
        let wgsl_path = shader_dir.join(s.wgsl);
        println!("cargo:rerun-if-changed={}", hlsl_path.display());

        // Skip when WGSL is already up-to-date.
        if let (Ok(hlsl_meta), Ok(wgsl_meta)) =
            (hlsl_path.metadata(), wgsl_path.metadata())
        {
            if let (Ok(hlsl_mtime), Ok(wgsl_mtime)) = (hlsl_meta.modified(), wgsl_meta.modified())
            {
                if wgsl_mtime >= hlsl_mtime {
                    continue;
                }
            }
        }

        let output = Command::new("slangc")
            .args([
                hlsl_path.to_str().unwrap(),
                "-target",
                "wgsl",
                "-entry",
                s.entry,
                "-stage",
                s.stage,
                // -O0 + -g: keep intermediate variables for debuggable WGSL.
                "-O0",
                "-g",
                "-o",
                wgsl_path.to_str().unwrap(),
            ])
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "slangc not found on PATH. Install Slang from https://github.com/shader-slang/slang/releases and add slangc to PATH.".to_string()
                } else {
                    format!("Failed to run slangc: {e}")
                }
            })
            .unwrap_or_else(|msg| panic!("{msg}"));

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            panic!(
                "slangc failed for {} (exit {:?}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
                s.hlsl,
                output.status.code(),
                stdout,
                stderr
            );
        }
    }
}
