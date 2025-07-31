#![allow(non_snake_case, dead_code)]

use std::{
    env, fs::{self, File}, io::{ BufRead, BufReader, Write }, path::{ PathBuf }, process::Command
};
use config::Config;
use fs_extra::dir::CopyOptions;
use anyhow::*;
use clap::{ Arg, App };
use path_absolutize::Absolutize;

// - Cargo commands 

enum Location {
    EngineProjectRoot, // Main engine project directory (containing creates, examples, etc)
    EngineCrates,
    PillEngineCrate,
    PillStandaloneCrate,
    PillLauncherCrate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompileMode {
    Debug,
    Release,
}

// Returns absolute paths
fn get_path(location: Location) -> PathBuf {
    let main_engine_directory = env::current_exe().unwrap().parent().unwrap().to_path_buf()
        .join("..").join("..").join("..").join("..")
        .absolutize().unwrap().to_path_buf();

    match location {
        Location::EngineProjectRoot => main_engine_directory,
        Location::EngineCrates => main_engine_directory.join("engine"),
        Location::PillEngineCrate => main_engine_directory.join("engine").join("pill_engine"),
        Location::PillStandaloneCrate => main_engine_directory.join("engine").join("pill_standalone"),
        Location::PillLauncherCrate => main_engine_directory.join("engine").join("pill_launcher"),
    } 
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




// --- Utilities ---

fn get_game_build_path(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf) -> Result<PathBuf> {
    println!("Getting game project build path... {}", game_project_directory_path.display());
    println!("{}", output_directory_path.as_os_str() == ".");
    let game_project_build_path = if output_directory_path.as_os_str() == "." {
        game_project_directory_path
            .join("build")
            .join("dev")
            .absolutize()
            .context("Failed to absolutize directory path")?
            .to_path_buf()
    } else {
        output_directory_path.absolutize()?.to_path_buf()
    };

    Ok(game_project_build_path)
}

fn get_game_title(game_project_directory_path: &PathBuf) -> Result<String> {
    // Get game title
    let config_path = game_project_directory_path.join("res").join("config.ini");
    let mut config = Config::default();
    config.merge(config::File::with_name(config_path.to_str().unwrap())).context("Failed to find config.ini file in game project \"res\" folder")?;
    let game_title = config.get_str("TITLE").context("Failed to get game config.ini")?;

    Ok(game_title)
}

fn check_if_game_project_validity(game_project_directory_path: &PathBuf) -> Result<()> {
    if !game_project_directory_path.join("Cargo.toml").exists() {
        return Err(Error::msg("Missing Cargo.toml file in game project folder"))
    }
    if !game_project_directory_path.join("res").exists() {
        return Err(Error::msg("Missing \"res\" folder in game project folder"))
    }
    if !game_project_directory_path.join("src").exists() {
        return Err(Error::msg("Missing \"src\" folder in game project folder"))
    }
    if !game_project_directory_path.join("res").join("config.ini").exists() {
        return Err(Error::msg("Missing \"config.ini\" file in game project folder"))
    }

    Ok(())
}


// --- Actions ---

fn create_game_project(game_parent_directory_path: &PathBuf, game_name: &String) -> Result<()> {
    const TEMPLATE_NAME: &str = "Pill-Default";
    
    let game_directory_path = game_parent_directory_path.join(game_name);
    if game_directory_path.exists() {
        return Err(Error::msg(format!("Game project directory {} already exists", game_directory_path.display())));
    }

    let game_resource_directory_path = game_directory_path.join("res");

    println!("Creating new game project {} in directory {}", game_name, game_directory_path.display());

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_project_directory_path = get_path(Location::PillLauncherCrate).join("res").join("templates");

    // Copy template
    println!("Copying project template...");
    
    fs_extra::dir::copy(
        &template_project_directory_path.join(TEMPLATE_NAME),
        &game_parent_directory_path,
        &CopyOptions::new().overwrite(true)
    )
    .context("Cannot copy template directory")?;

    // Rename project directory
    fs::rename(TEMPLATE_NAME, game_name)?; 

    // Setup config file
    println!("Setting up config file...");
    let action = |line: String| -> String {
        if line.starts_with("TITLE") { return format!("TITLE={}", game_name) }
        if line.starts_with("WINDOW_TITLE") { return format!("WINDOW_TITLE={}", game_name) }
        line
    };
    modify_file(&game_resource_directory_path.join("config.ini"), &game_resource_directory_path.join("config.ini"), action)?;

    // Setup cargo.toml file 
    println!("Setting up manifest file...");
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::PillEngineCrate).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&game_directory_path.join("Cargo.toml"), &game_directory_path.join("Cargo.toml"), action)?;

    // Success
    println!("Game project creation completed!");

    Ok(())
}

fn run_game_project(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf, compile_mode: &CompileMode) -> Result<()> {

    // Build game project
    build_game_project(game_project_directory_path, output_directory_path, compile_mode)?;

    // Run game project
    println!("Running game project...");
    let game_title = get_game_title(&game_project_directory_path).context("Failed to get game title")?;
    let standalone_executable_path = output_directory_path.join(format!("{}.exe", game_title));

    // Run exe
    let status = Command::new(standalone_executable_path)
        .current_dir(output_directory_path)
        .status()
        .context("Failed to run game project executable")?;

    if !status.success() {
        return Err(Error::msg(format!("Run executable command failed with code: {}", status.code().unwrap_or(1))));
    }

    Ok(())
}

fn build_game_project(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf, compile_mode: &CompileMode) -> Result<()> {
    println!("Building game project...");

    // Check if it is valid game project directory
    check_if_game_project_validity(&game_project_directory_path).context("Game project is invalid")?;

    // Get game title
    let game_title = get_game_title(&game_project_directory_path).context("Failed to get game title")?;

    // Compilation has to be done together on pill_standalone and pill_game together in the same context. 
    // For that compilation through Cargo workspace is required.
    // Otherwise, typeids of types like "Mesh" will not match what will make all generic (templated) functions work improperly
    let engine_workspace_path = get_path(Location::EngineProjectRoot).join("engine");

    let workspace_manifest_path = engine_workspace_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    // Update workspace manifest file to include game project crate
    let action = |line: String| -> String {
        if line.contains("### Game project crate") {
            return format!("    \"{}\", ### Game project crate (This will be changed by Pill Launcher on build to allow proper compilation of game project)", game_project_directory_path.to_str().unwrap().replace('\\', "/"));
        }
        line
    };

    modify_file(&workspace_manifest_path, &workspace_manifest_path, action)?;

    // Build standalone executable along with game dynamic library
    let mut arguments = vec!["build", "-p", "pill_game", "-p", "pill_standalone"];
    if compile_mode == &CompileMode::Release {
        arguments.push("--release");
    }
    let status = Command::new("cargo")
        .args(arguments)
        .current_dir(engine_workspace_path)
        .status().context("Failed to run cargo build command")?;

    if !status.success() {
        return Err(Error::msg(format!("Build command failed with code: {}", status.code().unwrap_or(1))));
    }

    // Prepare build folder
    println!("Game project build path: {}", output_directory_path.display());

    // Create build directory if does not exist
    fs::create_dir_all(output_directory_path.join("data").as_path()).context("Failed to create build output directories")?; 

    // Copy built standalone executable to build directory
    let standalone_output_path = get_path(Location::EngineCrates).join("target").join("debug").join("pill_standalone.exe");
    fs::copy(&standalone_output_path, &output_directory_path.join(game_title + ".exe"))?;

    // Copy built dynamic library to build directory
    let game_library_output_path = get_path(Location::EngineCrates).join("target").join("debug").join("pill_game.dll");
    fs::copy(&game_library_output_path, &output_directory_path.join("data").join("pill_game.dll"))?;

    // Success
    println!("Game built successfully!");

    Ok(())
}

// Runs "cargo doc" command for engine
fn generate_docs(output_directory_path: &PathBuf) -> Result<()> {
    // Set empty project as dependency
    let empty_example_game_path = get_path(Location::EngineProjectRoot).join("examples").join("Empty");
    if !empty_example_game_path.exists() {
        return Err(Error::msg("Cannot find Empty project in examples directory"));
    }

    // Update engine project dependency in game's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::PillEngineCrate).to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&empty_example_game_path.join("Cargo.toml"), &empty_example_game_path.join("Cargo.toml"), action)?;

    // Update game project dependency in standalone's cargo.toml
    let action = |line: String| -> String {
        if line.contains("pill_game") { return format!("pill_game = {{path = \"{}\"}}", empty_example_game_path.to_str().unwrap().replace("\\", "/")) }
        line
    };
    modify_file(&get_path(Location::PillStandaloneCrate).join("Cargo.toml"), &get_path(Location::PillStandaloneCrate).join("Cargo.toml"), action)?;
    
    let output_path = if output_directory_path.as_os_str() == "." {
        env::current_dir().context("Failed to get current directory")?
    } else {
        output_directory_path
            .absolutize()
            .context("Failed to absolutize output path")?
            .to_path_buf()
    };

    let docs_path = output_path.join("docs");

    if docs_path.exists() {
        fs::remove_dir_all(&docs_path)
            .with_context(|| format!("Cannot clear output directory: {}", docs_path.display()))?;
    }

    let output_game_dev_path = docs_path.join("game_dev");
    let output_engine_dev_path = docs_path.join("engine_dev");

    // Prepare output directories
    fs::create_dir_all(&docs_path)?;
    fs::create_dir_all(&output_game_dev_path)?;
    fs::create_dir_all(&output_engine_dev_path)?;

    let engine_crate_manifest_path = get_path(Location::PillEngineCrate).join("Cargo.toml");
    let full_engine_manifest_path = empty_example_game_path.join("Cargo.toml");

    // Game dev docs
    let arguments = vec!["/C", "cargo", "doc", "--no-deps", "--features", "game", "--manifest-path", engine_crate_manifest_path.to_str().unwrap(), "--target-dir", output_game_dev_path.to_str().unwrap(), "--release"];
    Command::new("cmd")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating game dev docs")?;

    // Engine dev docs
    // TODO: Remove game from workspace cargo.toml
    let arguments = vec!["/C", "cargo", "doc", "--no-deps", "--document-private-items", "--features", "internal game", "--manifest-path", full_engine_manifest_path.to_str().unwrap(), "--target-dir", output_engine_dev_path.to_str().unwrap(), "--release"];
    Command::new("cmd")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating engine dev docs")?;

    // Success
    println!("Docs generated successfully!");

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
    let action_argument = matches.value_of("action").expect("Action has to be specified");
    let directory_path_argument = matches.value_of("path");
    let game_name_argument = matches.value_of("name");
    let output_directory_path_argument = matches.value_of("output-path");
    let compile_mode_argument = matches.value_of("compile-mode");

    let compile_mode: CompileMode = match compile_mode_argument.unwrap() {
        "release" => CompileMode::Release,
        _ => CompileMode::Debug,
    };

    match action_argument {
        "create" => {
            let game_parent_directory_path = PathBuf::from(directory_path_argument.expect("Game project parent directory path has to be specified using --path flag. For example: --path <PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project parent directory path").unwrap()
                .to_path_buf();
            let game_name = String::from(game_name_argument.expect("Game name has to be specified using --name flag. For example: --name <MY_GAME_NAME>"));
            
            create_game_project(&game_parent_directory_path, &game_name).context("Failed to create new game project").unwrap();
        },
        "run" => {
            let game_project_directory_path = PathBuf::from(directory_path_argument.expect("Game project directory path has to be specified using --path flag. For example: --path <GAME_PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project directory path").unwrap()
                .to_path_buf();

            let mut output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"));
            output_directory_path = get_game_build_path(&game_project_directory_path, &output_directory_path).unwrap();
            
            run_game_project(&game_project_directory_path, &output_directory_path, &compile_mode).context("Failed to run game project").unwrap();
        },
        "build" => {
            let game_project_directory_path = PathBuf::from(directory_path_argument.expect("Game project directory path has to be specified using --path flag. For example: --path <GAME_PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project directory path").unwrap()
                .to_path_buf();

            let mut output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"));
            output_directory_path = get_game_build_path(&game_project_directory_path, &output_directory_path).unwrap();
            
            build_game_project(&game_project_directory_path, &output_directory_path, &compile_mode).context("Failed to build game project").unwrap();
        },
        "docs" => {
            let output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"))
                .absolutize().context("Failed to absolutize output directory path").unwrap()
                .to_path_buf();
           
            generate_docs(&output_directory_path).context("Failed to generate docs").unwrap();
        },
        _ => {
            println!("Undefined action");
        }
    };
}
