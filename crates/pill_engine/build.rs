// This script is run by cargo on build
use anyhow::{bail, Context};
use std::{fs, path::Path, process::Command};

//TODO: Fix windows compilation
#[cfg(target_os = "windows")]
fn compile(shader: &Path, dst: &Path) -> anyhow::Result<()> {
    let status = Command::new("res\\shaders\\glslc.exe")
        .arg(shader)
        .arg("-o")
        .arg(dst)
        .status()
        .with_context(|| {
            format!(
                "Failed to run glslc on shader {:?}",
                shader
            )
        })?;

    if !status.success() {
        bail!("Shader compilation failed with status: {}", status);
    }
    Ok(())
}

fn compile(shader: &Path, dst: &Path) -> anyhow::Result<()> {
    let status = Command::new("glslc")
        .arg(shader)
        .arg("-o")
        .arg(dst)
        .status()
        .with_context(|| {
            format!(
                "Failed to run glslc on shader {:?}",
                shader
            )
        })?;

    if !status.success() {
        bail!("Shader compilation failed with status: {}", status);
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {

    // This tells cargo to rerun this script if something in /res/shaders changes.
    println!("cargo:rerun-if-changed=res/shaders/*");

    // Compile GLSL shaders into SPIR-V
    let shader_directory_path = Path::new("res/shaders");
    let built_shader_directory_path = shader_directory_path.join("built");
    fs::create_dir_all(&built_shader_directory_path)?;

    let shaders = [
        "master.vert",
        "master.frag",
        ];

    for name in shaders {
        let src = shader_directory_path.join(name);
        let dst = built_shader_directory_path.join(format!("{}.spv", name));
        compile(&src, &dst)?
    }
    Ok(())
}
