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

#[cfg(target_os = "windows")]
fn cargo_build_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut arguments = vec!["/C", "cargo", "build", "--manifest-path", path.to_str().unwrap()];
    if compile_mode == "release" {
        arguments.push("--release");
    }

    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn cargo_build_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(path.to_str().unwrap());
    if compile_mode == "release" {
        cmd.arg("--release");
    }

    cmd.status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn cargo_run_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut arguments = vec!["/C", "cargo", "run", "--manifest-path", path.to_str().unwrap()];
    if compile_mode == "release" {
        arguments.push("--release");
    }

    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn cargo_run_command(path: &PathBuf, compile_mode: &String) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("run")
        .arg("--manifest-path")
        .arg(path.to_str().unwrap());

    if compile_mode == "release" {
        cmd.arg("--release");
    }

    cmd.status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn cargo_game_dev_docs_command(path: &PathBuf, output_path: &PathBuf) -> Result<()> {
    let arguments = vec!["/C", "cargo", "doc", "--no-deps", "--features", "game", "--manifest-path", path.to_str().unwrap(), "--target-dir", output_path.to_str().unwrap(), "--release"];

    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn cargo_game_dev_docs_command(path: &PathBuf, output_path: &PathBuf) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("doc")
        .arg("--no-deps")
        .arg("--features")
        .arg("game")
        .arg("--manifest-path")
        .arg(path.to_str().unwrap())
        .arg("--target-dir")
        .arg(output_path.to_str().unwrap())
        .arg("--release");
    cmd.status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(target_os = "windows")]
fn cargo_engine_dev_docs_command(path: &PathBuf, output_path: &PathBuf) -> Result<()> {
    let arguments = vec!["/C", "cargo", "doc", "--no-deps", "--document-private-items", "--features", "internal game", "--manifest-path", path.to_str().unwrap(), "--target-dir", output_path.to_str().unwrap(), "--release"];

    Command::new("cmd").args(arguments).status().context("Failed to execute command")?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn cargo_engine_dev_docs_command(path: &PathBuf, output_path: &PathBuf) -> Result<()> {
    let mut cmd = Command::new("cargo");
    cmd.arg("doc")
        .arg("--no-deps")
        .arg("--document-private-items")
        .arg("--features")
        .arg("internal game")
        .arg("--manifest-path")
        .arg(path.to_str().unwrap())
        .arg("--target-dir")
        .arg(output_path.to_str().unwrap())
        .arg("--release");

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
    const TEMPLATE_NAME: &str = "Pill-Default";

    // Prepare paths
    let display_game_path = PathBuf::from(game_path).join(game_name).absolutize().unwrap().to_path_buf();
    let game_parent_path = PathBuf::from(game_path.replace("/", "\\"));
    let game_path = game_parent_path.clone().join(game_name);

    println!("Creating new game project {} in directory {}", game_name, display_game_path.display());

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_path = get_path(Location::Launcher).join("res").join("templates");

    // Copy template
    println!("Copying project template...");

    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    fs_extra::dir::copy(&template_path.join(TEMPLATE_NAME), &game_parent_path, &copy_options).context("Cannot copy template directory")?;

    // Rename project directory
    fs::rename(TEMPLATE_NAME, game_name)?;

    // Setup config file
    println!("Setting up config file...");
    let action = |line: String| -> String {
        if line.starts_with("TITLE") { return format!("TITLE={}", game_name) }
        if line.starts_with("WINDOW_TITLE") { return format!("WINDOW_TITLE={}", game_name) }
        line
    };
    modify_file(&game_path.join("res").join("config.ini"), &game_path.join("res").join("config.ini"), action)?;

    // Setup cargo.toml file
    println!("Setting up manifest file...");
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::Engine).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&game_path.join("Cargo.toml"), &game_path.join("Cargo.toml"), action)?;

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
fn build_game_project(game_path: &String, output_path: &String, compile_mode: &String) -> Result<()> {
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
    let mut output_path = PathBuf::from(output_path);
    if output_path.to_str().unwrap() == "." { // Use current directory absolute path if no argument is specified
        fs::create_dir_all(game_path.join("build").as_path())?; // Create build directory if it is not there
        output_path = env::current_dir().unwrap().join("build");
    }
    else {
        output_path = output_path.absolutize().unwrap().to_path_buf();
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

    // Prepare build directory
    if output_path.exists() {
        fs::remove_dir_all(output_path.clone()).context(format!("Cannot clear build directory: {}", output_path.clone().to_str().unwrap()))?;
    }
    else {
        fs::create_dir(output_path.clone()).unwrap();
    }

    // Run cargo command
    cargo_build_command(&get_path(Location::Standalone).join("Cargo.toml"), &compile_mode)?;

    // Copy built executable to build directory and rename it according to variable in config file
    fs::copy(&engine_build_path, &output_path.join(game_title + ".exe"))?;

    // Copy game res directory to build directory
    let mut copy_options = CopyOptions::new();
    copy_options.overwrite = true;
    fs_extra::dir::copy(&game_project_resources_path, &output_path, &copy_options).context("Cannot copy game res directory")?;

    // Success
    println!("Game built succesully!");
    Ok(())
}

#[allow(unused_assignments)]
// Runs "cargo doc" command for engine
fn generate_docs(output_path: &String) -> Result<()> {
    // Set empty project as dependency
    let empty_example_game_path = get_path(Location::MainEngine).join("examples").join("Empty");
    if !empty_example_game_path.exists() {
        return Err(Error::msg("Cannot find Empty project in examples directory"));
    }

    // Update engine project dependency in game's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::Engine).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&empty_example_game_path.join("Cargo.toml"), &empty_example_game_path.join("Cargo.toml"), action)?;

    // Update game project dependency in standalone's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_game") { return format!("pill_game = {{path = \"{}\"}}", empty_example_game_path.to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&get_path(Location::Standalone).join("Cargo.toml"), &get_path(Location::Standalone).join("Cargo.toml"), action)?;

    let mut docs_path = PathBuf::from(".");
    let mut output_game_dev_path = PathBuf::from(".");
    let mut output_engine_dev_path = PathBuf::from(".");

    // Prepare output path
    let mut output_path = PathBuf::from(output_path);
    if output_path.to_str().unwrap() == "." { // Use current directory absolute path if no argument is specified
        // Find docs directory and recreate it
        output_path = env::current_dir().unwrap();
        docs_path = output_path.join("docs");
        if docs_path.exists() {
            fs::remove_dir_all(&docs_path).context(format!("Cannot clear output directory: {}", docs_path.clone().to_str().unwrap()))?;
        }

        // Specify concrete output paths
        output_game_dev_path = docs_path.join("game_dev");
        output_engine_dev_path = docs_path.join("engine_dev");
    }
    else {
        // Find docs directory and recreate it
        output_path = output_path.absolutize().unwrap().to_path_buf();
        docs_path = output_path.join("docs");
        if docs_path.exists() {
            fs::remove_dir_all(&docs_path).context(format!("Cannot clear output directory: {}", docs_path.clone().to_str().unwrap()))?;
        }

        output_game_dev_path = docs_path.join("game_dev");
        output_engine_dev_path = docs_path.join("engine_dev");
    }

    // Prepare output directories
    fs::create_dir_all(&docs_path)?;
    fs::create_dir_all(&output_game_dev_path)?;
    fs::create_dir_all(&output_engine_dev_path)?;

    // Run cargo command to generate game_dev docs
    cargo_game_dev_docs_command(&get_path(Location::Engine).join("Cargo.toml"), &output_game_dev_path)?;

    // Run cargo command to generate engine_dev docs
    cargo_engine_dev_docs_command(&get_path(Location::MainEngine).join("Cargo.toml"), &output_engine_dev_path)?;

    // Success
    println!("Docs generated succesully!");
    Ok(())
}


fn main() {
    let app = App::new("Pill Engine Launcher").about("Tool for managing Pill Engine game projects");

    // Definition of the options for the CLI
    let action_option = Arg::with_name("action")
        .short("a")
        .long("action")
        .takes_value(true)
        .possible_values(&["create", "run", "build", "docs"])
        .required(true)
        .help("Specify action to perform: creating/running/building the game project or generating docs");

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

    let output_path_option = Arg::with_name("output-path")
        .short("o")
        .long("output-path")
        .takes_value(true)
        .default_value(".")
        .required(false)
        .help("Specify action output directory");

    let compile_mode_option = Arg::with_name("compile-mode")
        .short("c")
        .long("compile-mode")
        .takes_value(true)
        .help("Specify compile mode")
        .possible_values(&["debug", "release"])
        .default_value("debug")
        .required(false);

    // Addition of the options to the CLI
    let app = app.arg(action_option).arg(name_option).arg(path_option).arg(output_path_option).arg(compile_mode_option);

    // Extraction of the arguments
    let matches = app.get_matches();

    // Arguments
    let action = matches.value_of("action").expect("Action has to be specified");
    let game_path = matches.value_of("path");
    let game_name = matches.value_of("name");
    let output_path = matches.value_of("output-path");
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
            let output_path = String::from(output_path.unwrap());
            let compile_mode = String::from(compile_mode.unwrap());

            build_game_project(&game_path, &output_path, &compile_mode).context("Failed to build game project").unwrap();
        },
        "docs" => {
            let output_path = String::from(output_path.unwrap());

            generate_docs(&output_path).context("Failed to generate docs").unwrap();
        },
        _ => {
            println!("Undefined action");
        }
    };
}
