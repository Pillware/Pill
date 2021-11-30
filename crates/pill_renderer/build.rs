// This script is run by cargo on build

use anyhow::*;
use fs_extra::copy_items;
use fs_extra::dir::CopyOptions;
use std::env;
use std::process::Command;

fn main() -> Result<()> {

    // Compile GLSL shaders into SPIR-V
    let shader_compiler_path = "res\\shaders\\glslc.exe";
    let shader_directory_path = "res\\shaders";
    let built_shader_directory_path = "res\\shaders\\built";

    let shaders_to_compile = [
        "master.vert", 
        "master.frag",
        ];

    for shader_to_compile in shaders_to_compile.iter() {
        //println!("Compiling shader: {}", command);

        let compiled_shader_file_name = format!("{}{}", shader_to_compile, ".spv");
        let command = format!("{} {}\\{} -o {}\\{}", shader_compiler_path, shader_directory_path, shader_to_compile, built_shader_directory_path, compiled_shader_file_name);

        Command::new("cmd")
        .args(["/C", command.as_ref()])
        .output()
        .expect(format!("Failed to compile shader: {}", shader_to_compile).as_ref());
    }


    // This tells cargo to rerun this script if something in /res/ changes.
    println!("cargo:rerun-if-changed=res/*");

    let out_dir = env::var("OUT_DIR")?; // Environment variable that cargo uses to specify where application will be built
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    let mut paths_to_copy = Vec::new();
    paths_to_copy.push("res/");
    copy_items(&paths_to_copy, out_dir, &copy_options)?;

    Ok(())
}