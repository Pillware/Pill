#![allow(non_snake_case, dead_code)]

use std::{
    path::{ PathBuf },
    process::Command,
    fs::{self, File},
    io::{ BufRead, BufReader, Write },
    env
};
use config::Config;
use fs_extra::dir::CopyOptions;
use anyhow::*;
use clap::{ Arg, App };
use path_absolutize::Absolutize;

// - Cargo commands 

enum Location {
    MainEngine, // Main engine project directory (containing creates, examples, etc)
    Engine,
    Standalone,
    Launcher,
}

// Returns absolute paths
fn get_path(location: Location) -> PathBuf {
    let main_engine_directory = env::current_exe().unwrap().parent().unwrap().to_path_buf()
        .join("..").join("..").join("..").join("..")
        .absolutize().unwrap().to_path_buf();

    match location {
        Location::MainEngine => main_engine_directory,
        Location::Engine => main_engine_directory.join("crates").join("pill_engine"),
        Location::Standalone => main_engine_directory.join("crates").join("pill_standalone"),
        Location::Launcher => main_engine_directory.join("crates").join("pill_launcher"),
    } 
}

fn cargo_build_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut arguments = vec!["/C", "cargo", "build", "--manifest-path", path.to_str().unwrap()];
    if compile_mode == "release" {
        arguments.push("--release")
    }
    
    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;
    
    Ok(())
}

fn cargo_run_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut arguments = vec!["/C", "cargo", "run", "--manifest-path", path.to_str().unwrap()];
    if compile_mode == "release" {
        arguments.push("--release")
    }
    
    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;
    
    Ok(())
}

fn modify_file<A: FnMut(String) -> String>(input_path: &PathBuf, output_path: &PathBuf, mut action: A) -> Result<()> {
    // Open files from path
    let input_file = File::open(input_path).unwrap();

    // Read lines from input file
    let lines = BufReader::new(input_file).lines().map(|v| v.unwrap()).collect::<Vec<String>>();

    // Create new file (overwrite if input and output paths are the same)
    let mut output_file = File::create(output_path).unwrap();

    // Write files to output file
    for line in lines {
        writeln!(output_file, "{}", action(line)).unwrap();
    }

    Ok(())
}

// - CLI functionalities

fn create_game_project(game_path: &String, game_name: &String) -> Result<()> {
    // Prepare paths
    let display_game_path = PathBuf::from(game_path).join(game_name).absolutize().unwrap().to_path_buf();
    let game_path = PathBuf::from(game_path.replace("/", "\\")).join(game_name);

    println!("Creating new game project {} in directory {}", game_name, display_game_path.display());

    // Starting to create directories
    println!("Creating directories...");

    // Game project
    fs::create_dir_all(game_path.as_path())?;

    // src
    fs::create_dir_all(game_path.join("src").as_path())?;

    // res
    fs::create_dir_all(game_path.join("..").as_path())?;
    fs::create_dir_all(game_path.join("res").join("textures").as_path())?;
    fs::create_dir_all(game_path.join("res").join("models").as_path())?;
    fs::create_dir_all(game_path.join("res").join("audio").as_path())?;

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_path = get_path(Location::Launcher).join("res").join("templates");
    
    // Create game files
    println!("Creating config file...");
    let action = |line: String| -> String {
        if line.starts_with("TITLE") { return format!("TITLE={}", game_name) }
        if line.starts_with("WINDOW_TITLE") { return format!("WINDOW_TITLE={}", game_name) }
        line
    };
    modify_file(&template_path.join("game_config_template.txt"), &game_path.join("res").join("config.ini"), action)?;

    println!("Creating lib file...");
    fs::copy(template_path.join("game_lib_template.txt"), game_path.join("src").join("lib.rs"))?;

    println!("Creating game file...");
    fs::copy(template_path.join("game_template.txt"), game_path.join("src").join("game.rs"))?;

    // Create correct cargo.toml file in the correct directory
    println!("Creating manifest file...");
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::Engine).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&template_path.join("game_cargo_template.txt"), &game_path.join("Cargo.toml"), action)?;

    // Success
    println!("Game project creation completed!");

    Ok(())
}

// Runs "cargo run" command on pill_standalone
fn run_game_project(game_path: &String, compile_mode: &String) -> Result<()> {
    // Prepare game path
    let mut game_path = PathBuf::from(game_path);
    if game_path.to_str().unwrap() == "." { // Use current directory absolute path if no argument is specified
        game_path = env::current_dir().unwrap();
    }
    else {
        game_path = game_path.absolutize().unwrap().to_path_buf();
        env::set_current_dir(&game_path).unwrap(); // Change current directory path cargo will think that it is in game folder and will be access to res directory
    }
    
    // Check if it is valid game project directory
    if !game_path.join("Cargo.toml").exists() {
        return Err(Error::msg("Invalid game project directory"))
    }
    if !game_path.join("res").join("config.ini").exists() {
        return Err(Error::msg("Invalid game project directory"))
    }

    // Update engine project dependency in game's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::Engine).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&game_path.join("Cargo.toml"), &game_path.join("Cargo.toml"), action)?;

    // Update game project dependency in standalone's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_game") { return format!("pill_game = {{path = \"{}\"}}", game_path.to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&get_path(Location::Standalone).join("Cargo.toml"), &get_path(Location::Standalone).join("Cargo.toml"), action)?;

    // Run cargo command
    cargo_run_command(&get_path(Location::Standalone).join("Cargo.toml"), &compile_mode)?;

    Ok(())
}

// Runs "cargo build" command on pill_standalone, clears build directory in game project folder and copy exe and res folder to it
fn build_game_project(game_path: &String, build_path: &String, compile_mode: &String) -> Result<()> {
    // Prepare game path
    let mut game_path = PathBuf::from(game_path);
    if game_path.to_str().unwrap() == "." { // Use current directory absolute path if no argument is specified
        game_path = env::current_dir().unwrap();
    }
    else {
        game_path = game_path.absolutize().unwrap().to_path_buf();
        env::set_current_dir(&game_path).unwrap(); // Change current directory path cargo will think that it is in game folder and will be access to res directory
    }

    // Prepare build path
    let mut build_path = PathBuf::from(build_path);
    if build_path.to_str().unwrap() == "." { // Use current directory absolute path if no argument is specified
        fs::create_dir_all(game_path.join("build").as_path())?; // Create build directory if it is not there
        build_path = env::current_dir().unwrap().join("build"); 
    }
    else {
        build_path = build_path.absolutize().unwrap().to_path_buf();
    }

    // Check if it is valid game project directory
    if !game_path.join("Cargo.toml").exists() {
        return Err(Error::msg("Invalid game project directory"))
    }
    if !game_path.join("res").join("config.ini").exists() {
        return Err(Error::msg("Invalid game project directory"))
    }

    // Update engine project dependency in game's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::Engine).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&game_path.join("Cargo.toml"), &game_path.join("Cargo.toml"), action)?;

    // Update game project dependency in standalone's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_game") { return format!("pill_game = {{path = \"{}\"}}", game_path.to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&get_path(Location::Standalone).join("Cargo.toml"), &get_path(Location::Standalone).join("Cargo.toml"), action)?;

    let engine_build_path = &get_path(Location::MainEngine).join("target").join(compile_mode).join("pill_standalone.exe");
    let game_project_resources_path = game_path.join("res");

    // Get game name from config file
    let mut config_file = Config::default();
    config_file.merge(config::File::with_name(game_project_resources_path.join("config.ini").to_str().unwrap())).unwrap();
    let game_title = config_file.get_str("TITLE").expect("Cannot find value for TITLE in game config file");

    // Clear build directory
    if build_path.exists() {
        fs::remove_dir_all(build_path.clone()).context(format!("Cannot clear build directory: {}", build_path.clone().to_str().unwrap()))?;
    } 
    else {
        fs::create_dir(build_path.clone()).unwrap();
    }

    // Run cargo command
    cargo_build_command(&get_path(Location::Standalone).join("Cargo.toml"), &compile_mode)?;

    // Copy built executable to build directory and rename it according to variable in config file
    fs::copy(&engine_build_path, &build_path.join(game_title + ".exe"))?;
    
    // Copy game res directory to build directory
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    fs_extra::dir::copy(&game_project_resources_path, &build_path, &copy_options).context("Cannot copy game res directory")?;

    // Success
    println!("Game built succesully!");
    Ok(())
}

fn main() {
    let app = App::new("Pill Engine Launcher").about("Tool for managing Pill Engine game projects");

    // Definition of the options for the CLI
    let action_option = Arg::with_name("action")
        .short("a")
        .long("action")
        .takes_value(true)
        .possible_values(&["create", "run", "build"]) 
        .required(true)
        .help("Specify action to perform: creating/running/building the game project");
        
    let name_option = Arg::with_name("name")
        .short("n")
        .long("name")
        .takes_value(true)
        .required_if("action", "create")
        .help("Specify name of new game project");

    let path_option = Arg::with_name("path")
        .short("p")
        .long("path")
        .takes_value(true)
        .default_value(".")
        .required(false)
        .help("Specify the path for game project creating/running/building");
        
    let build_path_option = Arg::with_name("build-path")
        .short("b")
        .long("build-path")
        .takes_value(true)
        .default_value(".")
        .required(false)
        .help("Specify game project build directory");

    let compile_mode_option = Arg::with_name("compile-mode")
        .short("c")
        .long("compile-mode")
        .takes_value(true)
        .help("Specify compile mode")
        .possible_values(&["debug", "release"]) 
        .default_value("debug")
        .required(false);

    // Addition of the options to the CLI
    let app = app.arg(action_option).arg(name_option).arg(path_option).arg(build_path_option).arg(compile_mode_option);

    // Extraction of the arguments
    let matches = app.get_matches();

    // Arguments
    let action = matches.value_of("action").expect("Action has to be specified");
    let game_path = matches.value_of("path");
    let game_name = matches.value_of("name");
    let build_path = matches.value_of("build-path");
    let compile_mode = matches.value_of("compile-mode");

    match action {
        "create" => {
            let game_name = String::from(game_name.unwrap());
            let game_path = String::from(game_path.unwrap());
            
            create_game_project(&game_path, &game_name).context("Failed to create new game project").unwrap();
        },
        "run" => {
            let game_path = String::from(game_path.unwrap());
            let compile_mode = String::from(compile_mode.unwrap());

            run_game_project(&game_path, &compile_mode).context("Failed to run game project").unwrap();
        },
        "build" => {
            let game_path = String::from(game_path.unwrap());
            let build_path = String::from(build_path.unwrap());
            let compile_mode = String::from(compile_mode.unwrap());

            build_game_project(&game_path, &build_path, &compile_mode).context("Failed to build game project").unwrap();
        },
        _ => {
            println!("Undefined action");
        }
    };
}
