//! Finding Blender on this machine and starting it with the link script.
//!
//! # Responsibilities
//!
//! - Resolve the Blender executable: the `PILL_BLENDER_PATH` override first,
//!   then the newest install in the platform's usual location, then `blender`
//!   on `PATH`.
//! - Resolve the link script this extension ships beside its crate.
//! - Start Blender with that script and the link's port, without waiting for
//!   it: the process belongs to the person, not to the editor.
//! - Tell whether a Blender is already running, so the editor waits for it to
//!   connect instead of starting a second one.
//! - Install the link as a Blender add-on, in a background Blender, so every
//!   Blender the person opens connects on its own.

// Standard library
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

// =============================================================================
// Constants
// =============================================================================

/// The environment variable that names the Blender executable to start,
/// overriding every search.
pub const BLENDER_PATH_ENVIRONMENT_VARIABLE: &str = "PILL_BLENDER_PATH";

/// The environment variable that turns the add-on install off when it is
/// `0`, `false`, `no` or `off`.
pub const INSTALL_ADDON_ENVIRONMENT_VARIABLE: &str = "PILL_BLENDER_INSTALL_ADDON";

/// The link script, relative to this crate's manifest directory.
const LINK_SCRIPT_RELATIVE_PATH: &str = "../../blender_addon/pill_blender_link.py";

/// The add-on installer, relative to this crate's manifest directory.
const INSTALLER_RELATIVE_PATH: &str = "../../blender_addon/install_addon.py";

/// Windows' `CREATE_NO_WINDOW`: a console program started for a check opens
/// no console window over the editor.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the path of the Blender link script this extension ships.
///
/// The editor never ships, so the crate's source directory recorded at build
/// time is where the script is.
pub fn link_script_path() -> PathBuf {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINK_SCRIPT_RELATIVE_PATH);
    script_path.canonicalize().unwrap_or(script_path)
}

/// Returns the path of the add-on installer this extension ships.
pub fn installer_script_path() -> PathBuf {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(INSTALLER_RELATIVE_PATH);
    script_path.canonicalize().unwrap_or(script_path)
}

/// Returns whether the add-on install is allowed: on unless
/// `PILL_BLENDER_INSTALL_ADDON` turns it off.
pub fn addon_install_allowed() -> bool {
    std::env::var(INSTALL_ADDON_ENVIRONMENT_VARIABLE)
        .map(|switch| {
            !matches!(
                switch.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

/// Starts a background Blender that installs and enables the link add-on in
/// the person's Blender preferences. The caller polls the returned process.
///
/// # Errors
///
/// Returns the operating system's error when Blender cannot be started.
pub fn start_addon_install() -> std::io::Result<Child> {
    let mut command = Command::new(blender_executable_path());
    command
        .arg("-b")
        // A Python error inside Blender otherwise still exits with 0.
        .arg("--python-exit-code")
        .arg("1")
        .arg("--python")
        .arg(installer_script_path())
        .arg("--")
        .arg(link_script_path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);
    command.spawn()
}

/// Counts the Blender processes running on this machine.
///
/// Asks the operating system's process list (`tasklist` on Windows, `pgrep`
/// elsewhere); a list that cannot be read counts as none, so the editor falls
/// back to starting Blender.
pub fn running_blender_count() -> usize {
    if cfg!(windows) {
        let mut command = Command::new("tasklist");
        command.args(["/FI", "IMAGENAME eq blender.exe", "/NH", "/FO", "CSV"]);
        hide_console_window(&mut command);
        return command
            .output()
            .map(|listing| {
                String::from_utf8_lossy(&listing.stdout)
                    .lines()
                    .filter(|line| line.to_ascii_lowercase().starts_with("\"blender.exe\""))
                    .count()
            })
            .unwrap_or(0);
    }
    Command::new("pgrep")
        .args(["-x", "-i", "blender"])
        .output()
        .map(|listing| String::from_utf8_lossy(&listing.stdout).lines().count())
        .unwrap_or(0)
}

/// Keeps a console program started for a check from opening a window.
pub fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Returns the Blender executable to start: the override when it is set, the
/// newest install found in the platform's usual location, or the bare
/// `blender` name for the operating system to resolve on `PATH`.
pub fn blender_executable_path() -> PathBuf {
    if let Some(override_path) = std::env::var_os(BLENDER_PATH_ENVIRONMENT_VARIABLE)
        .filter(|override_path| !override_path.is_empty())
    {
        return PathBuf::from(override_path);
    }
    newest_installed_blender().unwrap_or_else(|| PathBuf::from("blender"))
}

/// Starts Blender with the link script, pointed at the editor's `port`.
///
/// Blender runs the script after its startup file loads; the script connects
/// back to the editor and asks for the meshes waiting to open.
///
/// # Errors
///
/// Returns the operating system's error when the executable cannot be
/// started, for example because Blender is not installed where the search
/// looked.
pub fn launch_blender(port: u16) -> std::io::Result<PathBuf> {
    let executable_path = blender_executable_path();
    Command::new(&executable_path)
        .arg("--python")
        .arg(link_script_path())
        .arg("--")
        .arg("--pill-link-port")
        .arg(port.to_string())
        .spawn()?;
    Ok(executable_path)
}

/// Returns the newest Blender found in the platform's usual install location.
///
/// On Windows that is one folder per version under
/// `Program Files\Blender Foundation`; the version folders sort by name, so
/// the last one holding `blender.exe` is the newest. On macOS it is the
/// application bundle. Elsewhere Blender comes from `PATH`.
pub fn newest_installed_blender() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        let program_files =
            std::env::var_os("ProgramFiles").unwrap_or_else(|| "C:\\Program Files".into());
        let foundation_directory = Path::new(&program_files).join("Blender Foundation");
        let mut version_directories: Vec<PathBuf> = std::fs::read_dir(&foundation_directory)
            .ok()?
            .flatten()
            .map(|directory_entry| directory_entry.path())
            .filter(|version_directory| version_directory.join("blender.exe").is_file())
            .collect();
        version_directories.sort_by_key(|version_directory| version_sort_key(version_directory));
        return version_directories
            .pop()
            .map(|version_directory| version_directory.join("blender.exe"));
    }
    if cfg!(target_os = "macos") {
        let bundle_executable = PathBuf::from("/Applications/Blender.app/Contents/MacOS/Blender");
        return bundle_executable.is_file().then_some(bundle_executable);
    }
    None
}

/// Orders install folders such as `Blender 4.10` after `Blender 4.2` by
/// comparing the numbers in the name rather than its characters.
pub fn version_sort_key(version_directory: &Path) -> Vec<u32> {
    let folder_name = version_directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    folder_name
        .split(|character: char| !character.is_ascii_digit())
        .filter_map(|number_text| number_text.parse().ok())
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_folders_sort_by_their_numbers() {
        let mut folders = [
            PathBuf::from("Blender 4.10"),
            PathBuf::from("Blender 4.2"),
            PathBuf::from("Blender 3.6"),
        ];
        folders.sort_by_key(|folder| version_sort_key(folder));
        assert_eq!(folders.last().unwrap(), &PathBuf::from("Blender 4.10"));
    }

    #[test]
    fn the_link_script_ships_beside_the_crate() {
        assert!(
            link_script_path().is_file(),
            "{}",
            link_script_path().display()
        );
        assert!(
            installer_script_path().is_file(),
            "{}",
            installer_script_path().display()
        );
    }
}
