#![allow(non_snake_case, dead_code)]

mod web_dev_server;
mod size_report;
mod wasm_build;

use anyhow::*;
use clap::{App, AppSettings, Arg};
use config::Config;
use fs_extra::dir::CopyOptions;
use path_absolutize::Absolutize;
use std::{
    env,
    ffi::OsStr,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    result::Result::Ok,
};

// - Cargo commands

pub(crate) enum Location {
    EngineProjectRoot, // Main engine project directory (containing creates, examples, etc)
    EngineCrates,
    PillEngineCrate,
    PillCoreCrate,
    PillNativeCrate,
    PillLauncherCrate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompileMode {
    Debug,
    Release,
    HotReload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BuildTarget {
    Native,
    Web,
}

// --- Platform helpers -------------------------------------------------------

#[cfg(target_os = "windows")]
const EXEC_SUFFIX: &str = ".exe";
#[cfg(not(target_os = "windows"))]
const EXEC_SUFFIX: &str = ""; // Linux, macOS, etc. – no extension

#[cfg(target_os = "windows")]
const DYLIB_PREFIX: &str = ""; //  pill_game.dll
#[cfg(not(target_os = "windows"))]
const DYLIB_PREFIX: &str = "lib"; //  libpill_game.so / .dylib

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
        CompileMode::Debug => "debug",
        CompileMode::HotReload => "hot-reload",
    }
}

fn find_engine_workspace_dir() -> Result<PathBuf> {
    // 1) Explicit override (standalone will set this)
    if let Ok(v) = std::env::var("PILL_ENGINE_WORKSPACE_DIR") {
        let p = PathBuf::from(v);
        let m = p.join("Cargo.toml");
        if m.exists() {
            return Ok(p);
        }
        bail!(
            "PILL_ENGINE_WORKSPACE_DIR was set but {} does not exist",
            m.display()
        );
    }

    // 2) Search upward from current_exe and current_dir
    fn search_up(start: PathBuf) -> Option<PathBuf> {
        for a in start.ancestors() {
            let cand = a.join("engine").join("Cargo.toml");
            if cand.exists() {
                return Some(a.join("engine")); // <- workspace dir
            }
            let cand2 = a.join("Cargo.toml");
            // If someone starts from .../Pill-Engine/engine already
            if cand2.exists() && a.file_name().and_then(|s| s.to_str()) == Some("engine") {
                return Some(a.to_path_buf());
            }
        }
        None
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    if let Some(d) = exe_dir.and_then(search_up) {
        return Ok(d);
    }

    let cwd = std::env::current_dir().context("current_dir failed")?;
    if let Some(d) = search_up(cwd) {
        return Ok(d);
    }

    bail!("Cannot locate engine workspace dir (tried env + walking up from exe/cwd)");
}

// Returns absolute paths
fn get_path(location: Location) -> PathBuf {
    // engine workspace dir = .../Pill-Engine/engine
    let engine_ws =
        find_engine_workspace_dir().expect("Failed to locate engine workspace directory");

    // repo root = parent of engine/
    let repo_root = engine_ws.parent().unwrap().to_path_buf();

    match location {
        Location::EngineProjectRoot => repo_root,
        Location::EngineCrates => engine_ws,
        Location::PillEngineCrate => engine_ws.join("pill_engine"),
        Location::PillCoreCrate => engine_ws.join("pill_core"),
        Location::PillNativeCrate => engine_ws.join("pill_native"),
        Location::PillLauncherCrate => engine_ws.join("pill_launcher"),
    }
}

pub(crate) fn modify_file<A: FnMut(String) -> String>(
    input_path: &PathBuf,
    output_path: &PathBuf,
    mut action: A,
) -> Result<()> {
    // Open files from path
    let input = fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read {}", input_path.display()))?;

    // Prevent overwriting the same files
    let mut changed = false;

    // Read lines from input file
    let lines = input
        .lines()
        .map(|line| {
            let new_line = action(line.to_string());
            if new_line != line {
                changed = true;
            }
            new_line
        })
        .collect::<Vec<String>>();

    let mut out = lines.join("\n");
    if input.ends_with("\n") {
        out.push('\n');
    }

    // If input is the same and we are writing in-place - ignore
    if input_path == output_path && !changed && out == input {
        return Ok(());
    }

    // Similarly we are writing to a different file and their outputs are identical - ignore
    if input_path != output_path {
        if let Ok(existing) = fs::read_to_string(output_path) {
            if existing == out {
                return Ok(());
            }
        }
    }

    // Write files to output file
    fs::write(output_path, out)
        .with_context(|| format!("Failed to write {}", output_path.display()))?;

    Ok(())
}

fn copy_if_newer(source: &PathBuf, destination: &PathBuf) -> Result<bool> {
    // returns true if copied
    if !source.exists() {
        bail!("Source does not exist: {}", source.display());
    }

    let source_meta = fs::metadata(source)?;
    let source_mtime = source_meta.modified().ok();
    let source_len = source_meta.len();

    if let Ok(destination_meta) = fs::metadata(destination) {
        let destination_mtime = destination_meta.modified().ok();
        let destination_len = destination_meta.len();

        // If same size and destination is at least as new as source, skip copy.
        if destination_len == source_len {
            if let (Some(s), Some(d)) = (source_mtime, destination_mtime) {
                if d >= s {
                    return Ok(false);
                }
            }
        }
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("copy {} -> {}", source.display(), destination.display()))?;
    Ok(true)
}

fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        bail!("Source directory does not exist: {}", source.display());
    }
    if !source.is_dir() {
        bail!("Source path is not a directory: {}", source.display());
    }

    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create directory {}", destination.display()))?;

    for entry in fs::read_dir(source)
        .with_context(|| format!("Failed to read directory {}", source.display()))?
    {
        let entry = entry?;
        let entry_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if entry_path.is_dir() {
            copy_directory_recursive(&entry_path, &destination_path)?;
        } else if entry_path.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &destination_path).with_context(|| {
                format!(
                    "copy {} -> {}",
                    entry_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

fn stage_packaged_resources(game_project_directory_path: &Path, data_dir: &Path) -> Result<()> {
    let source_resources_dir = game_project_directory_path.join("res");
    let destination_resources_dir = data_dir.join("res");

    if !source_resources_dir.exists() {
        bail!(
            "Game resources directory does not exist: {}",
            source_resources_dir.display()
        );
    }

    fs::create_dir_all(data_dir)
        .with_context(|| format!("Failed to create data directory {}", data_dir.display()))?;
    if destination_resources_dir.exists() {
        fs::remove_dir_all(&destination_resources_dir).with_context(|| {
            format!(
                "Failed to clear destination resources directory {}",
                destination_resources_dir.display()
            )
        })?;
    }

    copy_directory_recursive(&source_resources_dir, &destination_resources_dir)?;

    let staged_config_path = destination_resources_dir.join("config.ini");
    if !staged_config_path.exists() {
        bail!(
            "Failed to stage resources into {} (missing {})",
            destination_resources_dir.display(),
            staged_config_path.display()
        );
    }

    println!(
        "Staged resources from {} to {}",
        source_resources_dir.display(),
        destination_resources_dir.display()
    );

    Ok(())
}

fn parse_file_lines<A: FnMut(String)>(input_path: &PathBuf, mut action: A) -> Result<()> {
    // Open files from path
    let input_file = File::open(input_path).unwrap();

    // Read lines from input file
    let lines = BufReader::new(input_file)
        .lines()
        .map(|v| v.unwrap())
        .collect::<Vec<String>>();

    // Write files to output file
    for line in lines {
        action(line);
    }

    Ok(())
}

// --- Utilities ---

fn output_dir_for(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Debug => "dev",
        CompileMode::Release => "release",
        CompileMode::HotReload => "hot-reload",
    }
}

fn standalone_layout_for(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Release => "packaged",
        CompileMode::Debug | CompileMode::HotReload => "development",
    }
}

fn get_game_build_path(
    game_project_directory_path: &Path,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
) -> Result<PathBuf> {
    if output_directory_path.as_os_str() == "." {
        Ok(game_project_directory_path
            .join("build")
            .join(output_dir_for(compile_mode))
            .absolutize()?
            .to_path_buf())
    } else {
        Ok(output_directory_path.absolutize()?.to_path_buf())
    }
}

fn get_game_title(game_project_directory_path: &Path) -> Result<String> {
    // Get game title
    let config_path = game_project_directory_path.join("res").join("config.ini");
    let mut config = Config::default();
    config
        .merge(config::File::with_name(config_path.to_str().unwrap()))
        .context("Failed to find config.ini file in game project \"res\" folder")?;
    let game_title = config
        .get_str("TITLE")
        .context("Failed to get game config.ini")?
        .replace(' ', "");

    Ok(game_title)
}

fn check_if_game_project_validity(game_project_directory_path: &Path) -> Result<()> {
    if !game_project_directory_path.join("Cargo.toml").exists() {
        return Err(Error::msg("Missing Cargo.toml file in game project folder"));
    }
    if !game_project_directory_path.join("res").exists() {
        return Err(Error::msg("Missing \"res\" folder in game project folder"));
    }
    if !game_project_directory_path.join("src").exists() {
        return Err(Error::msg("Missing \"src\" folder in game project folder"));
    }
    if !game_project_directory_path
        .join("res")
        .join("config.ini")
        .exists()
    {
        return Err(Error::msg(
            "Missing \"config.ini\" file in game project folder",
        ));
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
                    fs::remove_file(&path)
                        .with_context(|| format!("Failed to remove file: {}", path.display()))?;
                }
            }
        }
    }

    Ok(())
}

fn try_remove_files_starting_with(directory_path: &PathBuf, file_name_prefix: &str) {
    if let Err(error) = remove_files_starting_with(directory_path, file_name_prefix) {
        eprintln!(
            "Non-fatal cleanup failure for prefix '{}' in {}: {:#}",
            file_name_prefix,
            directory_path.display(),
            error
        );
    }
}

// Render all *.puml under <crate>/docs/uml into <crate>/docs/uml_out as SVGs
fn render_puml_for_crate(crate_dir: &Path) -> Result<()> {
    let in_dir = crate_dir.join("docs").join("uml");
    let out_dir = crate_dir.join("docs").join("uml_out");

    if !in_dir.exists() {
        return Ok(()); // Skip non-existent
    }
    fs::create_dir_all(&out_dir)?;

    // Collect input files
    let mut inputs = Vec::new();
    for entry in fs::read_dir(&in_dir)
        .with_context(|| format!("Failed to read directory: {}", in_dir.display()))?
    {
        let path = entry?.path();
        println!("Checking file: {}", path.display());
        if path.extension() == Some(OsStr::new("puml")) {
            inputs.push(path);
        }
    }

    println!(
        "Found {} PlantUML files to render in {}",
        inputs.len(),
        in_dir.display()
    );

    if inputs.is_empty() {
        return Ok(());
    }

    let have_cli = Command::new("plantuml")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();

    // Prefer "plantuml" CLI tool if available
    if !have_cli {
        bail!("Please install plantuml!");
    }

    for puml in &inputs {
        let svg_path = out_dir
            .join(puml.file_stem().unwrap())
            .with_extension("svg");

        let mut child = Command::new("plantuml")
            .arg("-tsvg")
            .arg("-pipe")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .context("Spawn plantuml -pipe")?;

        {
            let mut stdin = child.stdin.take().unwrap();
            let bytes =
                fs::read(puml).with_context(|| format!("Read PUML file {}", puml.display()))?;
            stdin.write_all(&bytes)?;
        }

        let out = child.wait_with_output().context("Wait plantuml")?;
        if !out.status.success() {
            bail!("plantuml failed with code {}", out.status);
        }
        fs::write(&svg_path, &out.stdout)
            .with_context(|| format!("Write SVG file {}", svg_path.display()))?;
    }

    // TODO: either distribute plantuml or download it automatically
    Ok(())
}

fn normalize_path(p: &PathBuf) -> Result<String> {
    Ok(p.absolutize()?
        .to_path_buf()
        .to_string_lossy()
        .replace('\\', "/"))
}

fn extract_member_path_from_line(line: &str) -> Option<String> {
    // expects:     "<path>", ### Game project crate ...
    let trimmed = line.trim();
    if !trimmed.contains("### Game project crate") {
        return None;
    }
    let first_quote = trimmed.find('"')?;
    let rest = &trimmed[first_quote + 1..];
    let second_quote = rest.find('"')?;
    Some(rest[..second_quote].to_string())
}

fn prepare_workspace_for_game(
    game_project_directory_path: &PathBuf,
    compile_mode: &CompileMode,
) -> Result<PathBuf> {
    check_if_game_project_validity(game_project_directory_path)
        .context("Game project is invalid")?;

    // Compilation has to be done together on pill_native and pill_game together in the same context.
    // For that compilation through Cargo workspace is required.
    // Otherwise, typeids of types like "Mesh" will not match what will make all generic (templated) functions work improperly
    let engine_workspace_directory_path = get_path(Location::EngineCrates);
    let workspace_manifest_path = engine_workspace_directory_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    let desired_game_path = normalize_path(game_project_directory_path)?;
    let desired_line = format!(
        "    \"{}\", ### Game project crate (This will be changed by Pill Launcher on build to allow proper compilation of game project)",
        desired_game_path
    );

    // --- read current linked path (if any)
    let manifest_text = fs::read_to_string(&workspace_manifest_path)
        .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;

    let mut current_linked: Option<String> = None;
    for line in manifest_text.lines() {
        if let Some(p) = extract_member_path_from_line(line) {
            current_linked = Some(p);
            break;
        }
    }

    let switching_game = match &current_linked {
        Some(cur) => cur != &desired_game_path,
        None => true,
    };

    // --- only clean artifacts when switching projects
    if switching_game {
        let compilation_artifacts_folder_path = get_path(Location::EngineCrates)
            .join("target")
            .join(target_dir_for(compile_mode));

        let artifact_prefix = if cfg!(target_os = "windows") {
            "pill_game"
        } else {
            "libpill_game"
        };
        remove_files_starting_with(&compilation_artifacts_folder_path, artifact_prefix)?;
        remove_files_starting_with(
            &compilation_artifacts_folder_path.join("deps"),
            artifact_prefix,
        )?;
    }

    // --- only rewrite workspace Cargo.toml if the line would change
    if switching_game {
        modify_file(
            &workspace_manifest_path,
            &workspace_manifest_path,
            |line: String| -> String {
                if line.contains("### Game project crate") {
                    return desired_line.clone();
                }
                line
            },
        )?;
    }

    // --- only rewrite game Cargo.toml workspace line if needed
    let game_manifest_path = game_project_directory_path.join("Cargo.toml");
    let engine_ws_path = normalize_path(&get_path(Location::EngineCrates))?;
    let game_manifest_text = fs::read_to_string(&game_manifest_path)
        .with_context(|| format!("Failed to read {}", game_manifest_path.display()))?;

    let workspace_line_expected = format!("workspace = \"{}\"", engine_ws_path);

    let already_has_workspace_line = game_manifest_text
        .lines()
        .any(|l| l.trim_start().starts_with("workspace") && l.contains(&engine_ws_path));

    if !already_has_workspace_line {
        modify_file(
            &game_manifest_path,
            &game_manifest_path,
            |line: String| -> String {
                if line.trim_start().starts_with("workspace") {
                    return workspace_line_expected.clone();
                }
                line
            },
        )?;
    }

    Ok(engine_workspace_directory_path)
}

// --- Actions ---

fn create_game_project(
    game_project_parent_directory_path: &PathBuf,
    game_name: &String,
) -> Result<()> {
    const TEMPLATE_NAME: &str = "pill_default";

    let game_project_directory_path = game_project_parent_directory_path.join(game_name);
    if game_project_directory_path.exists() {
        return Err(Error::msg(format!(
            "Game project directory {} already exists",
            game_project_directory_path.display()
        )));
    }

    let game_resource_directory_path = game_project_directory_path.join("res");

    println!(
        "Creating new game project {} in directory {}",
        game_name,
        game_project_directory_path.display()
    );

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_game_project_directory_path = get_path(Location::PillLauncherCrate)
        .join("res")
        .join("templates");

    // Copy template
    println!("Copying project template...");

    fs_extra::dir::copy(
        template_game_project_directory_path.join(TEMPLATE_NAME),
        game_project_parent_directory_path,
        &CopyOptions::new().overwrite(true),
    )
    .context("Cannot copy template directory")?;

    // Rename project directory
    fs::rename(TEMPLATE_NAME, game_name)?;

    // Setup config file
    println!("Setting up config file...");
    modify_file(
        &game_resource_directory_path.join("config.ini"),
        &game_resource_directory_path.join("config.ini"),
        |line: String| -> String {
            if line.starts_with("TITLE") {
                return format!("TITLE={}", game_name);
            }
            if line.starts_with("WINDOW_TITLE") {
                return format!("WINDOW_TITLE={}", game_name);
            }
            line
        },
    )?;

    // Setup cargo.toml file
    println!("Setting up manifest file...");
    modify_file(
        &game_project_directory_path.join("Cargo.toml"),
        &game_project_directory_path.join("Cargo.toml"),
        |line: String| -> String {
            if line.contains("pill_engine") {
                return format!(
                    "pill_engine = {{ path = \"{}\", features = [\"game\"] }}",
                    get_path(Location::PillEngineCrate)
                        .to_str()
                        .unwrap()
                        .replace("\\", "/")
                );
            }
            line
        },
    )?;

    modify_file(
        &game_project_directory_path.join("Cargo.toml"),
        &game_project_directory_path.join("Cargo.toml"),
        |line: String| -> String {
            if line.contains("workspace") {
                return format!(
                    "workspace = \"{}\"",
                    get_path(Location::EngineCrates)
                        .to_str()
                        .unwrap()
                        .replace("\\", "/")
                );
            }
            line
        },
    )?;

    // Success
    println!("Game project creation completed!");

    Ok(())
}

fn run_game_project(
    game_project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    game_args: &[String],
) -> Result<()> {
    // Build game project
    build_game_project(
        game_project_directory_path,
        output_directory_path,
        compile_mode,
    )?;

    // Run game project
    println!(
        "Running game project from {}...",
        output_directory_path.display()
    );
    let game_title =
        get_game_title(game_project_directory_path).context("Failed to get game title")?;
    let standalone_executable_path =
        output_directory_path.join(format!("{game_title}{EXEC_SUFFIX}"));

    let launcher_bin = std::env::current_exe().context("current_exe failed")?;
    let engine_ws = find_engine_workspace_dir()?; // .../Pill-Engine/engine

    let status = Command::new(&standalone_executable_path)
        .current_dir(output_directory_path)
        .env("PILL_LAUNCHER_BIN", &launcher_bin)
        .env("PILL_ENGINE_WORKSPACE_DIR", &engine_ws)
        .env("PILL_GAME_PROJECT_DIR", game_project_directory_path)
        .env(
            "PILL_STANDALONE_LAYOUT",
            standalone_layout_for(compile_mode),
        )
        .env(
            "PILL_ENABLE_HOT_RELOAD",
            if *compile_mode == CompileMode::HotReload {
                "1"
            } else {
                "0"
            },
        )
        .args(game_args)
        .status()
        .with_context(|| {
            format!(
                "Failed to launch game project executable: {}",
                standalone_executable_path.display()
            )
        })?;

    if !status.success() {
        // Game ran and exited with an error — don't say "failed to run" - just return Ok
        eprintln!(
            "Game exited with error code: {}",
            status.code().map_or("unknown".into(), |c| c.to_string())
        );
    }

    Ok(())
}

fn build_game_project(
    game_project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
) -> Result<()> {
    println!(
        "Building game project from {}...",
        game_project_directory_path.display()
    );

    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let engine_workspace_directory_path =
        prepare_workspace_for_game(game_project_directory_path, compile_mode)?;

    // Get game title EARLY (we need it for per-game target dir)
    let game_title =
        get_game_title(game_project_directory_path).context("Failed to get game title")?;

    // Use a per-game target dir so switching games doesn't invalidate everything
    let cargo_target_dir = engine_workspace_directory_path
        .join("target_games")
        .join(&game_title);

    // Pre-render PUML only for non-hot-reload builds
    let pill_engine_dir = get_path(Location::PillEngineCrate);
    if *compile_mode != CompileMode::HotReload {
        render_puml_for_crate(&pill_engine_dir)
            .context("Failed to render PlantUML diagrams for pill_engine")?;
    }

    let mut arguments = vec![
        "build",
        "-p",
        "pill_game",
        "-p",
        "pill_native",
        "-p",
        "pill_runtime",
    ];
    if *compile_mode == CompileMode::HotReload {
        arguments.push("--profile");
        arguments.push("hot-reload");
        arguments.push("--quiet");
    }
    if *compile_mode == CompileMode::Release {
        arguments.push("--release");
    }
    Command::new("cargo")
        .args(&arguments)
        .current_dir(&engine_workspace_directory_path)
        .env("CARGO_TARGET_DIR", &cargo_target_dir)
        .status()
        .context("failed to run cargo build")?
        .success()
        .then_some(())
        .ok_or_else(|| Error::msg("build failed"))?;

    // Where cargo artifacts actually are now:
    let compilation_artifacts_folder_path = cargo_target_dir.join(target_dir_for(compile_mode));

    // Ensure build/data exists
    fs::create_dir_all(output_directory_path.join("data").as_path())
        .context("Failed to create build output directories")?;

    // Copy standalone exe ONLY for non-hot-reload builds or hot-reload consequent reloads
    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        let standalone_output_path =
            compilation_artifacts_folder_path.join(format!("pill_native{EXEC_SUFFIX}"));
        if !standalone_output_path.exists() {
            return Err(Error::msg(
                "Standalone executable was not built successfully",
            ));
        }

        let destination_executable_path =
            output_directory_path.join(format!("{game_title}{EXEC_SUFFIX}"));

        let _copied = copy_if_newer(&standalone_output_path, &destination_executable_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&destination_executable_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&destination_executable_path, perms)?;
        }
    }

    let data_dir = output_directory_path.join("data");
    fs::create_dir_all(&data_dir)?;

    // Only packaged release builds stage resources into <build>/data/res.
    // Debug and hot-reload both use the game project directory directly.
    if *compile_mode == CompileMode::Release {
        stage_packaged_resources(game_project_directory_path, &data_dir)?;
    }

    let game_src = compilation_artifacts_folder_path.join(dylib("pill_game"));
    let runtime_src = compilation_artifacts_folder_path.join(dylib("pill_runtime"));

    if !game_src.exists() {
        return Err(Error::msg(format!(
            "Game dylib missing: {}",
            game_src.display()
        )));
    }
    if !runtime_src.exists() {
        return Err(Error::msg(format!(
            "Runtime dylib missing: {}",
            runtime_src.display()
        )));
    }

    // Copy the dylibs for the initial build only (not consecutive hot-reloads, otherwise we
    // overwrite loaded libs and crash!)
    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        if copy_if_newer(&game_src, &data_dir.join(dylib("pill_game")))? {
            println!("Copied game dylib");
        } else {
            println!("Skipping copying of game dylib");
        }
        if copy_if_newer(&runtime_src, &data_dir.join(dylib("pill_runtime")))? {
            println!("Copied runtime dylib");
        } else {
            println!("Skipping copying of runtime dylib");
        }
    }

    // In hot-reload mode, also update the hot names (watcher looks for these)
    if *compile_mode == CompileMode::HotReload {
        if copy_if_newer(&game_src, &data_dir.join(dylib("pill_game_hot_reloaded")))? {
            println!("Copied game hot-reload dylib");
        } else {
            println!("Skipping copying of game hot-reload dylib");
        }
        if copy_if_newer(
            &runtime_src,
            &data_dir.join(dylib("pill_runtime_hot_reloaded")),
        )? {
            println!("Copied runtime hot-reload dylib");
        } else {
            println!("Skipping copying of runtime hot-reload dylib");
        }
    }

    // Success
    println!("Game built successfully!");

    Ok(())
}

// Runs "cargo doc" command for engine
fn generate_docs(output_directory_path: &PathBuf) -> Result<()> {
    // Set empty project as dependency
    let empty_example_game_path = get_path(Location::EngineProjectRoot)
        .join("examples")
        .join("Empty");
    if !empty_example_game_path.exists() {
        return Err(Error::msg(
            "Cannot find Empty project in examples directory",
        ));
    }

    // Update engine project dependency in game's cargo.toml
    modify_file(
        &empty_example_game_path.join("Cargo.toml"),
        &empty_example_game_path.join("Cargo.toml"),
        |line: String| -> String {
            if line.contains("pill_engine") {
                return format!(
                    "pill_engine = {{path = \"{}\", features = [\"game\"]}}",
                    get_path(Location::PillEngineCrate)
                        .to_str()
                        .unwrap()
                        .replace("\\", "/")
                );
            }
            line
        },
    )?;

    // Update game project dependency in standalone's cargo.toml
    modify_file(
        &get_path(Location::PillNativeCrate).join("Cargo.toml"),
        &get_path(Location::PillNativeCrate).join("Cargo.toml"),
        |line: String| -> String {
            if line.contains("pill_game") {
                return format!(
                    "pill_game = {{path = \"{}\"}}",
                    empty_example_game_path.to_str().unwrap().replace("\\", "/")
                );
            }
            line
        },
    )?;

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

    // Pre-render all PUML in the engine crate
    let pill_engine_dir = get_path(Location::PillEngineCrate);
    render_puml_for_crate(&pill_engine_dir)
        .context("Failed to render PlantUML diagrams for pill_engine")?;

    // Game dev docs
    let arguments = vec![
        "doc",
        "--no-deps",
        "--features",
        "game,internal",
        "--manifest-path",
        full_engine_manifest_path.to_str().unwrap(),
        "--target-dir",
        output_game_dev_path.to_str().unwrap(),
        "--release",
    ];
    let status = Command::new("cargo")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating game dev docs")?;

    if !status.success() {
        bail!("Engine docs failed to generate (exit {:?})", status.code());
    }
    println!("Engine dev docs generated successfully!");

    // Engine dev docs
    // Generate pill_core before pill_engine and don't generate other dependencies
    let core_crate_manifest_path = get_path(Location::PillCoreCrate).join("Cargo.toml");
    let arguments = vec![
        "doc",
        "--no-deps",
        "--document-private-items",
        "--manifest-path",
        core_crate_manifest_path.to_str().unwrap(),
        "--target-dir",
        output_engine_dev_path.to_str().unwrap(),
        "--release",
    ];
    let status = Command::new("cargo")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating core dev docs")?;

    // Success
    if status.success() {
        println!("Core dev docs generated successfully!");
    }

    let arguments = vec![
        "doc",
        "--no-deps",
        "--document-private-items",
        "--features",
        "all",
        "--manifest-path",
        engine_crate_manifest_path.to_str().unwrap(),
        "--target-dir",
        output_engine_dev_path.to_str().unwrap(),
        "--release",
    ];
    let status = Command::new("cargo")
        .args(arguments)
        .status()
        .context("Failed to execute command for generating engine dev docs")?;

    // Success
    if !status.success() {
        bail!(
            "Game dev docs failed to generate (exit {:?})",
            status.code()
        );
    }
    println!("Game dev docs generated successfully!");

    Ok(())
}

fn cargo_passthrough(
    game_project_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    cargo_args: &[String],
) -> Result<()> {
    if cargo_args.is_empty() {
        bail!("Must call cargo with at least one argument");
    }

    let engine_workspace_directory_path =
        prepare_workspace_for_game(game_project_directory_path, compile_mode)?;

    println!(
        "Running cargo {:?} in workspace {}...",
        cargo_args,
        engine_workspace_directory_path.display()
    );

    let status = Command::new("cargo")
        .args(cargo_args)
        .current_dir(engine_workspace_directory_path)
        .status()
        .context("Failed to run cargo passthrough")?;

    if !status.success() {
        bail!(
            "Cargo command failed: cargo {:?} (exit {:?})",
            cargo_args,
            status.code()
        );
    }

    Ok(())
}

fn run_app() -> Result<()> {
    let app = App::new("Pill Engine Launcher").about("Tool for managing Pill Engine game projects");

    // Definition of the options for the CLI
    let action_option = Arg::with_name("action")
        .short("a")
        .long("action")
        .takes_value(true)
        .possible_values(&["create", "run", "build", "docs", "cargo", "assets"])
        .required(true)
        .help("Specify action to perform: creating/running/building the game project, generating docs, running cargo passthrough, or rebuilding assets (HLSL→WGSL etc.)");

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

    let target_option = Arg::with_name("target")
        .short("t")
        .long("target")
        .takes_value(true)
        .possible_values(&["native", "web"])
        .default_value("native")
        .required(false)
        .help("Build/run target: native standalone executable or WASM+WebGPU for the browser");

    let game_args = Arg::with_name("game-args")
        .help("Arguments passed through to cargo/game (use `--` to separate them)")
        .multiple(true)
        .last(true)
        .allow_hyphen_values(true);

    // Addition of the options to the CLI
    let app = app
        .arg(action_option)
        .arg(name_option)
        .arg(path_option)
        .arg(output_path_option)
        .arg(compile_mode_option)
        .arg(target_option)
        .arg(game_args)
        .setting(AppSettings::TrailingVarArg);

    // Extraction of the arguments
    let matches = app.get_matches();

    // Arguments
    let passthrough_args: Vec<String> = matches
        .values_of("game-args")
        .map(|vals| vals.map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let action_argument = matches
        .value_of("action")
        .expect("Action has to be specified");
    let directory_path_argument = matches.value_of("path");
    let game_name_argument = matches.value_of("name");
    let output_directory_path_argument = matches.value_of("output-path");
    let compile_mode_argument = matches.value_of("compile-mode").unwrap_or("debug");

    let compile_mode: CompileMode = match compile_mode_argument {
        "release" => CompileMode::Release,
        "hot-reload" => CompileMode::HotReload,
        _ => CompileMode::Debug,
    };

    let target: BuildTarget = match matches.value_of("target").unwrap_or("native") {
        "web" => BuildTarget::Web,
        _ => BuildTarget::Native,
    };

    match action_argument {
        "create" => {
            let game_parent_directory_path = PathBuf::from(directory_path_argument.expect("Game project parent directory path has to be specified using --path flag. For example: --path <PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project parent directory path")?
                .to_path_buf();
            let game_name = String::from(game_name_argument.expect("Game name has to be specified using --name flag. For example: --name <MY_GAME_NAME>"));

            create_game_project(&game_parent_directory_path, &game_name)
                .context("Failed to create new game project")?;
        }
        "run" => {
            let game_project_directory_path = PathBuf::from(directory_path_argument.expect("Game project directory path has to be specified using --path flag. For example: --path <GAME_PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project directory path")?
                .to_path_buf();

            match target {
                BuildTarget::Native => {
                    let mut output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"));
                    output_directory_path = get_game_build_path(
                        &game_project_directory_path,
                        &output_directory_path,
                        &compile_mode,
                    )
                    .unwrap();
                    run_game_project(
                        &game_project_directory_path,
                        &output_directory_path,
                        &compile_mode,
                        &passthrough_args,
                    )
                    .context("Failed to run game project")?;
                }
                BuildTarget::Web => {
                    web_dev_server::run(&game_project_directory_path, &compile_mode)
                        .context("Failed to run game project for wasm")?;
                }
            }
        }
        "build" => {
            let game_project_directory_path = PathBuf::from(directory_path_argument.expect("Game project directory path has to be specified using --path flag. For example: --path <GAME_PROJECT_DIR>"))
                .absolutize().context("Failed to absolutize game project directory path")?
                .to_path_buf();

            match target {
                BuildTarget::Native => {
                    let mut output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"));
                    output_directory_path = get_game_build_path(
                        &game_project_directory_path,
                        &output_directory_path,
                        &compile_mode,
                    )?;
                    build_game_project(
                        &game_project_directory_path,
                        &output_directory_path,
                        &compile_mode,
                    )
                    .context("Failed to build game project")?;
                }
                BuildTarget::Web => {
                    if matches.occurrences_of("output-path") > 0 {
                        println!(
                            "Note: `-o/--output-path` is ignored with `-t wasm`; output is fixed at <game>/build/wasm/"
                        );
                    }
                    wasm_build::build(&game_project_directory_path, &compile_mode)
                        .context("Failed to build game project for wasm")?;
                }
            }
        }
        "docs" => {
            let output_directory_path = PathBuf::from(output_directory_path_argument.expect("Output directory path has to be specified using --output-path flag. For example: --output-path <OUTPUT_DIR>"))
                .absolutize().context("Failed to absolutize output directory path")?
                .to_path_buf();

            generate_docs(&output_directory_path).context("Failed to generate docs")?;
        }
        "cargo" => {
            let game_project_directory_path = PathBuf::from(
                directory_path_argument
                    .expect("Game project must be specified when running cargo commands."),
            )
            .absolutize()
            .context("Failed to absolutize game project directory path")?
            .to_path_buf();

            cargo_passthrough(
                &game_project_directory_path,
                &compile_mode,
                &passthrough_args,
            )
            .context("Cargo passthrough failed")?;
        }
        "assets" => {
            let project_dir = PathBuf::from(directory_path_argument.expect(
                "Project directory must be specified for asset rebuild. Example: --path <PROJECT_DIR>",
            ))
            .absolutize()
            .context("Failed to absolutize project directory path")?
            .to_path_buf();

            let pipeline = pill_assets::Pipeline {
                root: project_dir.join("res"),
                rules: pill_assets::default_rules(),
            };
            let stats = pipeline.run().context("Asset pipeline failed")?;
            println!(
                "Assets: discovered={} rebuilt={} skipped={} (root: {})",
                stats.discovered.len(),
                stats.rebuilt.len(),
                stats.skipped.len(),
                pipeline.root.display()
            );
        }
        _ => {
            println!("Undefined action");
        }
    };
    Ok(())
}

fn main() {
    if let Err(e) = run_app() {
        eprintln!("{:#}", e);
        std::process::exit(1);
    }
}
