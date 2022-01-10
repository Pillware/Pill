extern crate clap;
extern crate fs_extra;
extern crate ini;

use std::path::{PathBuf, Path};
use std::process::Command;
use std::fs::{self, File,};
use std::io::{BufRead, BufReader, Write};
use fs_extra::dir::{CopyOptions};
use ini::Ini;
use anyhow::*;

use clap::{Arg, App};

// - Cargo commands 

fn cargo_build_command(path: &Path, additional_bin_info: Option<&String>) -> Result<()> {

    match additional_bin_info {
        Some(bin_path) => { Command::new("cmd")
                                    .args(&["/C", "cargo", "build", "--manifest-path", path.to_str().unwrap(), "--bin", bin_path, "--release"])
                                    .status()
                                    .expect("Failed to execute command");
        }
        None => { Command::new("cmd")
                        .args(&["/C", "cargo", "build", "--manifest-path", path.to_str().unwrap()])
                        .status()
                        .expect("Failed to execute command");
        }
    }
    
    Ok(())
}

fn cargo_run_command(path: &Path, additional_bin_info: Option<&String>) -> Result<()> {
    
    match additional_bin_info {
        Some(bin_path) => { Command::new("cmd")
                                    .args(&["/C", "cargo", "run", "--manifest-path", path.to_str().unwrap(), "--bin", bin_path])
                                    .status()
                                    .expect("Failed to execute command");
        }
        None => { Command::new("cmd")
                        .args(&["/C", "cargo", "run", "--manifest-path", path.to_str().unwrap()])
                        .status()
                        .expect("Failed to execute command");
        }
    }
    Ok(())
}

// - Game config functionalities 

fn create_game_config(path: &Path, game_name: &String) -> Result<()> {
    
    // Window settings
    let window_width = "1000";
    let window_heigth = "900";
    let is_fullscreen = "false";

    // Engine settings
    let max_entity_count = "1000";
    let render_queue_capacity = "1000";
    let max_ambient_sink_count = "10";
    let max_spatial_sink_count = "10";

    // Renderer settings
    let max_pipelines_count = "10";
    let max_textures_count = "10";
    let max_materials_count = "10";
    let max_meshes_count = "10";
    let max_cameras_count = "10";

    // Engine path
    let mut engine_path = std::env::current_exe()?;
    engine_path.pop();
    engine_path.pop();

    // Create config file
    let mut config = Ini::new();
    config.with_section(Some("APPLICATION"))
            .set("NAME", game_name);
    config.with_section(Some("WINDOW"))
            .set("WINDOW_WIDTH", window_width)
            .set("WINDOW_HEIGHT", window_heigth)
            .set("FULLSCREEN", is_fullscreen.to_uppercase());
    config.with_section(Some("ENGINE"))
            .set("PATH", engine_path.to_str().unwrap())
            .set("MAX_ENTITY_COUNT", max_entity_count)
            .set("MAX_RENDER_QUEUE_CAPACITY", render_queue_capacity)
            .set("MAX_AMBIENT_SINK_COUNT", max_ambient_sink_count)
            .set("MAX_SPATIAL_SINK_COUNT", max_spatial_sink_count);
    config.with_section(Some("RENDERER"))
            .set("MAX_PIPELINES_COUNT", max_pipelines_count)
            .set("MAX_TEXTURES_COUNT", max_textures_count)
            .set("MAX_MATERIALS_COUNT", max_materials_count)
            .set("MAX_MESHES_COUNT", max_meshes_count)
            .set("MAX_CAMERAS_COUNT", max_cameras_count);
    config.with_section(Some("GAME"));

    // Write config to file
    let mut config_path = PathBuf::new();
    config_path.push(path);
    config_path.push("config.ini");
    config.write_to_file(config_path)?;
    Ok(())     
}

// - Custom file functions

fn create_file_from_template(template_path: &Path, out_path: &Path) -> Result<()> {

    // Open/Create files from path
    let template_file = File::open(template_path).unwrap();
    let mut out_file = File::create(out_path).unwrap();

    // Read lines from template file
    let template_lines = BufReader::new(template_file).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    // Write files to out file
    for line in template_lines {
        writeln!(out_file, "{}", &line)?;
    }

    Ok(())
}

fn create_cargo_from_template(template_path: &Path, out_path: &Path) -> Result<()> {

    // Open/Create files from path
    let template_file = File::open(template_path).unwrap();
    let mut out_file = File::create(out_path).unwrap();

    // Read lines from template file
    let template_lines = BufReader::new(template_file).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    // Write files to out file
    for line in template_lines {

        // If line containg information about pill-engine path, set the correct path
        // Assumption: pill_launcher.exe is located in Pill-Engine/scripts directory
        if line.contains("pill_engine") {
            
            let mut engine_path = std::env::current_exe()?;
            engine_path.pop();
            engine_path.pop();
            engine_path.push("crates");
            engine_path.push("pill_engine");
            writeln!(out_file, "pill_engine = {{path = {:?}, features = [\"game\"]}}", engine_path.to_str().unwrap())?;
        }

        // Else just rewrite the path as usual
        else {
            writeln!(out_file, "{}", &line)?;
        }
    }

    Ok(())
}

fn overwrite_cargo_from_template(cargo_path: &Path, game_path: &Path) -> Result<()> {

    // Open/Create files from path
    let from_path = File::open(cargo_path).unwrap();

    // Read lines from template file
    let template_lines = BufReader::new(from_path).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    let mut out_path = File::create(cargo_path).unwrap();
    // Write files to out file
    for line in template_lines {

        if line.contains("pill_game") {
            
            writeln!(out_path, "pill_game = {{path = {:?}}} # This needs to be pointing to the correct game directory", game_path.to_str().unwrap())?;
        }
        
        // Else just rewrite the path as usual
        else {
            writeln!(out_path, "{}", &line)?;
        }
    }

    Ok(())
}

fn overwrite_config(config_path: &Path, release_value: bool) -> Result<()> {

    // Open/Create files from path
    let from_path = File::open(config_path).unwrap();

    // Read lines from template file
    let template_lines = BufReader::new(from_path).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();

    let mut out_path = File::create(config_path).unwrap();
    // Write files to out file
    for line in template_lines {

        if line.contains("FINAL_RELEASE") {
            let release_str = match release_value {
                true => "true",
                false => "false"
            };
            writeln!(out_path, "FINAL_RELEASE={}", release_str)?;
        }
        
        // Else just rewrite the path as usual
        else {
            writeln!(out_path, "{}", &line)?;
        }
    }

    Ok(())
}

// - CLI functionalities

fn create_game_project(path: &String, game_name: &String) -> Result<()> {
    
    // Prepare path
    
    let new_path = path.replace("/", "\\");
    let mut game_path = PathBuf::new();

    // Starting to create directories
    println!("Creating directories...");
    game_path.push(new_path);
    game_path.push(game_name);

    game_path.push("src");
    fs::create_dir_all(game_path.as_path()).unwrap();
    game_path.pop();

    game_path.push("res");
    fs::create_dir_all(game_path.as_path()).unwrap();
    game_path.push("models");
    fs::create_dir_all(game_path.to_str().unwrap())?;

    game_path.pop();
    game_path.push("textures");
    fs::create_dir_all(game_path.to_str().unwrap())?;

    game_path.pop();
    game_path.push("audio");
    fs::create_dir_all(game_path.to_str().unwrap())?;
    game_path.pop();
    game_path.pop();

    // Get the path needed for fetching the templates
    // Assumed pill-launcher.exe destination: /pill_engine/scripts
    let mut template_path = std::env::current_exe()?;
    template_path.pop();
    template_path.pop();
    template_path.push("crates");
    template_path.push("pill_launcher");
    template_path.push("res");
    template_path.push("templates");
    
    // Create game file in the correct directory
    println!("Creating base game file...");
    template_path.push("game_template.txt");
    game_path.push("src");
    game_path.push("game.rs");
    create_file_from_template(&template_path, &game_path)?;
    template_path.pop();
    game_path.pop();

    // Create lib file in the correct directory
    println!("Creating base lib files...");
    template_path.push("lib_template.txt");
    game_path.push("lib.rs");
    create_file_from_template(&template_path, &game_path)?;
    template_path.pop();
    game_path.pop();
    game_path.pop();

    // Create correct cargo.toml file in the correct directory
    println!("Creating cargo.toml...");
    template_path.push("game_cargo_template.txt");
    game_path.push("Cargo.toml");
    create_cargo_from_template(&template_path, &game_path)?;

    // Create config file
    game_path.pop();
    println!("Creating game configuration file...");
    create_game_config(&game_path, game_name)?;

    // Success
    println!("Project creation completed!");
    Ok(())
}

fn run_game_project(path: &String) -> Result<()> {

    // Prepare paths for line fetch from standalone's cargo.toml
    // Assumption: pill_launcher.exe is in Pill-Engine/scripts directory

    let mut standalone_path = std::env::current_exe()?;
    standalone_path.pop();
    standalone_path.pop();
    standalone_path.push("crates");
    standalone_path.push("pill_standalone");
    standalone_path.push("cargo.toml");

    let mut game_path = PathBuf::new();
    game_path.push(path);

    // Overwrite standalone's cargo.toml dependency folder for game
    overwrite_cargo_from_template(&standalone_path, &game_path).unwrap();

    // Overwrite release version about config
    game_path.push("config.ini");
    overwrite_config(&game_path, false).unwrap();
    game_path.pop();

    // Go to Pill-Engine and run cargo run --bin pill_standalone
    let mut pill_engine_path = std::env::current_exe()?;
    pill_engine_path.pop();
    pill_engine_path.pop();
    pill_engine_path.push("Cargo.toml");
    cargo_run_command(&pill_engine_path, Some(&String::from("pill_standalone"))).unwrap();

    // Success
    println!("Game run succesully!");
    Ok(())
}

fn build_game(path: &String, out_path: &String) -> Result<()> {

    // Prepare paths
    // Assumption: pill_launcher.exe is in Pill-Engine/scripts directory

    let mut standalone_path = std::env::current_exe()?;
    standalone_path.pop();
    standalone_path.pop();
    standalone_path.push("crates");
    standalone_path.push("pill_standalone");
    standalone_path.push("cargo.toml");

    let mut game_path = PathBuf::new();
    game_path.push(path);

    let mut target_release_path = std::env::current_exe()?;
    target_release_path.pop();
    target_release_path.pop();
    target_release_path.push("target");
    target_release_path.push("release");
    target_release_path.push("pill_standalone.exe");

    let mut build_path = std::path::PathBuf::new();
    build_path.push(&out_path);

    // Overwrite standalone's cargo.toml dependency folder for game
    overwrite_cargo_from_template(&standalone_path, &game_path).unwrap();

    // Create build directory in game's folder
    game_path.push("build");
    fs::create_dir_all(&game_path)?;

    // Overwrite config file in game path
    game_path.pop();
    game_path.push("config.ini");
    overwrite_config(&game_path, true).unwrap();
    game_path.pop();

    // Go to Pill-Engine and run cargo build --bin pill_standalone --release command
    let mut pill_engine_path = std::env::current_exe()?;
    pill_engine_path.pop();
    pill_engine_path.pop();
    pill_engine_path.push("Cargo.toml");
    standalone_path.pop();

    cargo_build_command(&pill_engine_path, Some(&String::from("pill_standalone"))).unwrap();

    // Copy released .exe to build directory
    game_path.push("build");
    game_path.push("final_release.exe");
    fs::copy(&target_release_path, &game_path).unwrap();
    game_path.pop();

    // Create additional path for config.ini and resources for copying to game build directory
    let options = CopyOptions::new();
    let mut res_path = std::path::PathBuf::new();
    res_path.push(&game_path);
    res_path.pop();
    res_path.push("config.ini");
    game_path.push("config.ini");
    println!("{:?}", res_path.as_path());
    fs::copy(&res_path, &game_path)?;

    game_path.pop();
    res_path.pop();
    res_path.push("res");
    println!("{:?}", res_path.as_path());
    
    fs_extra::dir::copy(&res_path.as_path(), &game_path, &options).unwrap();
    println!("{:?}", out_path);
    // Move game path to the chosen directory
    fs_extra::dir::move_dir(&game_path, &out_path, &options).unwrap();

    // Success
    println!("Game built succesully!");
    Ok(())
}

fn main() {
    let app = App::new("Pill Engine Game Launcher")
        .about("CLI for launching or creating game project for Pill Engine")
        .author("Created By Mateusz Szymoński & Łukasz Zalewski");

    // Definition of the options for the CLI
    let action_option = Arg::new("action")
        .short('a')
        .long("action") 
        .takes_value(true)
        .help("Define taken action: creating the game, running the game, or making final build of the game")
        .required(true);

    let name_option = Arg::new("name")
        .short('n')
        .long("name")
        .takes_value(true)
        .help("Choose name of new game; usefull only during game project creation")
        .default_value("NEW_GAME")
        .required(false);

    let path_option = Arg::new("path")
        .short('p')
        .long("path")
        .takes_value(true)
        .help("Pass the path for new game, or to game meant for running or building")
        .default_value(".")
        .required(false);

    let build_path_option = Arg::new("build-path")
        .short('b')
        .long("build-path")
        .takes_value(true)
        .help("Choose the path for built files; usefull only during game building process")
        .default_value(".")
        .required(false);

    // Addition of the options to the CLI
    let app = app.arg(action_option).arg(name_option).arg(path_option).arg(build_path_option);

    // Extraction of the arguments
    let matches = app.get_matches();

    let users_action = matches.value_of("action")
        .expect("Action has to be defined");

    let users_path = matches.value_of("path");

    let users_game_name = matches.value_of("name");

    let users_build_path = matches.value_of("build-path");

    match users_action {

        // "create" action
        "create" => {  

            let game_name = String::from(users_game_name.unwrap());
            let game_path = String::from(users_path.unwrap());
            
            create_game_project(&game_path, &game_name).unwrap();
            
        },
        
        // "run" action
        "run" => {
            
            let game_path = String::from(users_path.unwrap());

            run_game_project(&game_path).unwrap();
        },

        // "build" action
        "build" => {

            let game_path = String::from(users_path.unwrap());
            let build_path = String::from(users_build_path.unwrap());

            build_game(&game_path, &build_path).unwrap();

        },
        _ => { 
            println!("Undefined action");
        }
    };
}
