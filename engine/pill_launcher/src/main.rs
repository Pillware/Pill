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
    HotReload
}

// --- Platform helpers -------------------------------------------------------

#[cfg(target_os = "windows")]
const EXEC_SUFFIX: &str = ".exe";
#[cfg(not(target_os = "windows"))]
const EXEC_SUFFIX: &str = "";            // Linux, macOS, etc. – no extension

#[cfg(target_os = "windows")]
const DYLIB_PREFIX: &str = "";           //  pill_game.dll
#[cfg(not(target_os = "windows"))]
const DYLIB_PREFIX: &str = "lib";        //  libpill_game.so / .dylib

#[cfg(target_os = "windows")]
const DYLIB_SUFFIX: &str = ".dll";
#[cfg(target_os = "linux")]
const DYLIB_SUFFIX: &str = ".so";
#[cfg(target_os = "macos")]
const DYLIB_SUFFIX: &str = ".dylib";

fn dylib(name: &str) -> String {
    format!("{DYLIB_PREFIX}{name}{DYLIB_SUFFIX}")
}

fn target_dir_for(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Release => "release",
        _ => "debug",
    }
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

fn parse_file_lines<A: FnMut(String)>(input_path: &PathBuf, mut action: A) -> Result<()> {
    // Open files from path
    let input_file = File::open(input_path).unwrap();

    // Read lines from input file
    let lines = BufReader::new(input_file).lines().map(|v| v.unwrap()).collect::<Vec<String>>();

    // Write files to output file
    for line in lines {
        action(line);
    }

    Ok(())
}

// --- Utilities ---

fn get_game_build_path(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf) -> Result<PathBuf> {
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
    let game_title = config.get_str("TITLE").context("Failed to get game config.ini")?.replace(' ', "");

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

fn remove_files_starting_with(directory_path: &PathBuf, file_name_prefix: &str) -> Result<()> {
    if !directory_path.exists() || !directory_path.is_dir() {
        return Ok(()); // Skip non-existent or non-dir
    }

    for entry in fs::read_dir(directory_path).context("Failed to read directory")? {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with(file_name_prefix) {
                    fs::remove_file(&path).with_context(|| format!("Failed to remove file: {}", path.display()))?;
                }
            }
        }
    }

    Ok(())
}


// --- Actions ---

fn create_game_project(game_project_parent_directory_path: &PathBuf, game_name: &String) -> Result<()> {
    const TEMPLATE_NAME: &str = "pill_default";

    let game_project_directory_path = game_project_parent_directory_path.join(game_name);
    if game_project_directory_path.exists() {
        return Err(Error::msg(format!("Game project directory {} already exists", game_project_directory_path.display())));
    }

    let game_resource_directory_path = game_project_directory_path.join("res");

    println!("Creating new game project {} in directory {}", game_name, game_project_directory_path.display());

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_game_project_directory_path = get_path(Location::PillLauncherCrate).join("res").join("templates");

    // Copy template
    println!("Copying project template...");

    fs_extra::dir::copy(
        &template_game_project_directory_path.join(TEMPLATE_NAME),
        &game_project_parent_directory_path,
        &CopyOptions::new().overwrite(true)
    )
    .context("Cannot copy template directory")?;

    // Rename project directory
    fs::rename(TEMPLATE_NAME, game_name)?;

    // Setup config file
    println!("Setting up config file...");
    modify_file(&game_resource_directory_path.join("config.ini"), &game_resource_directory_path.join("config.ini"), |line: String| -> String {
        if line.starts_with("TITLE") { return format!("TITLE={}", game_name) }
        if line.starts_with("WINDOW_TITLE") { return format!("WINDOW_TITLE={}", game_name) }
        line
    })?;

    // Setup cargo.toml file
    println!("Setting up manifest file...");
    modify_file(&game_project_directory_path.join("Cargo.toml"), &game_project_directory_path.join("Cargo.toml"), |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{ path = \"{}\", features = [\"game\"] }}", get_path(Location::PillEngineCrate).to_str().unwrap().replace("\\", "/")) }
        line
    })?;

    modify_file(&game_project_directory_path.join("Cargo.toml"), &game_project_directory_path.join("Cargo.toml"), |line: String| -> String {
        if line.contains("workspace") { return format!("workspace = \"{}\"", get_path(Location::EngineCrates).to_str().unwrap().replace("\\", "/")) }
        line
    })?;

    // Success
    println!("Game project creation completed!");

    Ok(())
}

fn run_game_project(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf, compile_mode: &CompileMode) -> Result<()> {
    // Build game project
    build_game_project(game_project_directory_path, output_directory_path, compile_mode)?;

    // Run game project
    println!("Running game project from {}...", output_directory_path.display());
    let game_title = get_game_title(&game_project_directory_path).context("Failed to get game title")?;
    let standalone_executable_path = output_directory_path.join(format!("{game_title}{EXEC_SUFFIX}"));

    // Run exe (capture potential IO error here)
    let status = Command::new(&standalone_executable_path)
        .current_dir(output_directory_path)
        .status()
        .context(format!(
            "Failed to launch game project executable: {}",
            standalone_executable_path.display()
        ))?;

    if !status.success() {
        // Game ran and exited with an error — don't say "failed to run" - just return Ok
        eprintln!("Game exited with error code: {}", status.code().map_or("unknown".into(), |c| c.to_string()));
    }

    Ok(())
}

fn build_game_project(game_project_directory_path: &PathBuf, output_directory_path: &PathBuf, compile_mode: &CompileMode) -> Result<()> {
    println!("Building game project from {}...", game_project_directory_path.display());

    // Check if it is valid game project directory
    check_if_game_project_validity(&game_project_directory_path).context("Game project is invalid")?;

    // Get game title
    let game_title = get_game_title(&game_project_directory_path).context("Failed to get game title")?;

    // Compilation has to be done together on pill_standalone and pill_game together in the same context.
    // For that compilation through Cargo workspace is required.
    // Otherwise, typeids of types like "Mesh" will not match what will make all generic (templated) functions work improperly
    let engine_workspace_directory_path = get_path(Location::EngineCrates);

    let workspace_manifest_path = engine_workspace_directory_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    // If game project has changed changed then previous compilation artifacts have to be removed
    let compilation_artifacts_folder_path = get_path(Location::EngineCrates).join("target").join(target_dir_for(compile_mode));
    let engine_workspace_manifest_game_project_directory_path = format!("    \"{}\", ### Game project crate (This will be changed by Pill Launcher on build to allow proper compilation of game project)", game_project_directory_path.to_str().unwrap().replace('\\', "/"));
    let mut game_project_directory_already_linked = false;
    parse_file_lines(&workspace_manifest_path, |line: String| {
        if line.contains(engine_workspace_manifest_game_project_directory_path.clone().as_str()) {
            game_project_directory_already_linked = true;
        }
    })?;

    if !game_project_directory_already_linked {
        // Remove previous compilation artifacts
        let artifact_prefix = if cfg!(target_os = "windows") { "pill_game" } else { "libpill_game" };
        remove_files_starting_with(&compilation_artifacts_folder_path, artifact_prefix)?;
        remove_files_starting_with(&compilation_artifacts_folder_path.join("deps"), artifact_prefix)?;
    }

    // Update workspace manifest file to include game project crate
    modify_file(&workspace_manifest_path, &workspace_manifest_path,  |line: String| -> String {
        if line.contains("### Game project crate") {
            return engine_workspace_manifest_game_project_directory_path.clone();
        }
        line
    })?;

    // Update workspace path in game project manifest
    modify_file(&game_project_directory_path.join("Cargo.toml"), &game_project_directory_path.join("Cargo.toml"),  |line: String| -> String {
        if line.contains("workspace") { return format!("workspace = \"{}\"", get_path(Location::EngineCrates).to_str().unwrap().replace("\\", "/")) }
        line
    })?;

    // Build standalone executable along with game dynamic library
	let mut arguments = vec![
        "build",
        "-p", "pill_game",
        "-p", "pill_standalone",
    ];
    if *compile_mode == CompileMode::Release {
        arguments.push("--release");
    }
    Command::new("cargo")
        .args(&arguments)
        .current_dir(&engine_workspace_directory_path)
        .status()
        .context("failed to run cargo build")?
        .success()
        .then_some(())
        .ok_or_else(|| Error::msg("build failed"))?;

    // Create build directory if does not exist
    fs::create_dir_all(output_directory_path.join("data").as_path()).context("Failed to create build output directories")?;

	if *compile_mode != CompileMode::HotReload {
		// Copy built standalone executable to build directory
		let standalone_output_path = compilation_artifacts_folder_path.join(format!("pill_standalone{EXEC_SUFFIX}"));
		if !standalone_output_path.exists() {
			return Err(Error::msg("Standalone executable was not built successfully"));
		}

		let destination_executable_path = output_directory_path.join(format!("{game_title}{EXEC_SUFFIX}"));
		fs::copy(&standalone_output_path, &destination_executable_path)
			.with_context(|| format!(
				"Can't copy standalone executable from {} to {}",
				standalone_output_path.display(),
				destination_executable_path.display()
			))?;

		// ensure executable bit on Unix
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			let mut perms = fs::metadata(&destination_executable_path)?.permissions();
			perms.set_mode(0o755);
			fs::set_permissions(&destination_executable_path, perms)?;
		}
	}

    // Copy built dynamic library to build directory
    let game_library_output_path = compilation_artifacts_folder_path.join(dylib("pill_game"));
    if !game_library_output_path.exists() {
        return Err(Error::msg(format!("Game dynamic library was not built successfully in {}", game_library_output_path.display())));
    }

    let game_dynamic_library_name = if *compile_mode == CompileMode::HotReload {
		dylib("pill_game_hot_reloaded")
	} else {
		dylib("pill_game")
	};
    let output_game_library_path = output_directory_path.join("data").join(game_dynamic_library_name);
    fs::copy(&game_library_output_path, &output_game_library_path)
        .context(format!("Can't copy game dynamic library from {} to {}", game_library_output_path.display(), output_game_library_path.display()))?;

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
    modify_file(&empty_example_game_path.join("Cargo.toml"), &empty_example_game_path.join("Cargo.toml"), |line: String| -> String {
        if line.contains("pill_engine") { return format!("pill_engine = {{path = \"{}\", features = [\"game\"]}}", get_path(Location::PillEngineCrate).to_str().unwrap().replace("\\", "/")) }
        line
    })?;

    // Update game project dependency in standalone's cargo.toml
    modify_file(&get_path(Location::PillStandaloneCrate).join("Cargo.toml"), &get_path(Location::PillStandaloneCrate).join("Cargo.toml"), |line: String| -> String {
        if line.contains("pill_game") { return format!("pill_game = {{path = \"{}\"}}", empty_example_game_path.to_str().unwrap().replace("\\", "/")) }
        line
    })?;

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
    let arguments = vec!["doc", "--no-deps", "--features", "game", "--manifest-path", engine_crate_manifest_path.to_str().unwrap(), "--target-dir", output_game_dev_path.to_str().unwrap(), "--release"];
    let status = Command::new("cargo")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating game dev docs")?;

	if status.success() {
        println!("Engine dev docs generated successfully!");
    }

    // Engine dev docs
    // TODO: Remove game from workspace cargo.toml
    let arguments = vec!["doc", "--no-deps", "--document-private-items", "--features", "internal game", "--manifest-path", full_engine_manifest_path.to_str().unwrap(), "--target-dir", output_engine_dev_path.to_str().unwrap(), "--release"];
    let status = Command::new("cargo")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating engine dev docs")?;

    // Success
	if status.success() {
        println!("Game dev docs generated successfully!");
    }

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
        .possible_values(&["debug", "release", "hot-reload"])
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
        "hot-reload" => CompileMode::HotReload,
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