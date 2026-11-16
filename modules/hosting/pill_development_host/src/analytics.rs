//! Build, link and hot-reload analytics. An in-process collector times each
//! phase of the pipeline (compile, stage, load, init, migrate) and prints a
//! report to the host console.
//!
//! # Responsibilities
//!
//! - Time each module's build command, staging copy, DLL load, registration
//!   and schema migration.
//! - Measure the memory used by the host process and by the cargo child
//!   process, both current and peak.
//! - Inspect each loaded DLL: its file size, its image size and the DLLs it
//!   imports.
//! - Read each module's direct cargo dependencies from the `.fingerprint`
//!   JSON.
//! - Read cargo's `--timings` HTML for the compile and link time of each
//!   crate.
//! - Print one full report at startup and one short line per hot reload.
//!
//! # Design
//!
//! The collector is process-wide because the build pipeline is spread over
//! several modules (`build_runner`, `native_library`, `reload`, `runtime`)
//! and no single one sees the whole transaction. Entries are keyed by module
//! name, which is unique across the project and its extensions. Everything
//! here is best-effort: a missing DLL, an image that cannot be parsed or an
//! absent cargo-timings report shows as `-` in the report and never fails
//! the build.

// Standard library
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

// External crates
use pill_core::info;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::telemetry::{log_block_colored, Colorize};
use serde_json::Value;

// =============================================================================
// Constants
// =============================================================================

/// How many distinct fallback reasons the tally remembers.
///
/// The list is capped instead of growing without bound. The tally's counters
/// still count every refusal and failure, so a long session loses only the
/// listing of one more distinct cause, never the totals.
const MAX_TRACKED_FALLBACK_REASONS: usize = 8;

/// How much of one explanation is kept. Compiler errors can run long, and this
/// line sits inside a reload report that has to stay readable.
const MAX_FALLBACK_DETAIL_CHARS: usize = 160;

// =============================================================================
// Build Tree Locations
// =============================================================================

/// The folder, relative to the workspace root, where cargo writes its
/// `--timings` HTML report.
///
/// The path comes from the private module build tree because that is where
/// the reports land. Every build the host starts sets `CARGO_TARGET_DIR` to
/// [`crate::config::MODULE_BUILD_TARGET_DIRECTORY`], and cargo writes the
/// report under whatever target directory it is given. A fixed
/// `target/cargo-timings` would only ever hold the reports from the
/// developer's own `cargo build` runs.
///
/// Cargo does not add the target triple here, so the path stays the same for
/// `--target` builds such as the dioxus CLI's; only the artifact folders
/// move.
fn cargo_timings_directory() -> String {
    format!(
        "{}/cargo-timings",
        crate::config::MODULE_BUILD_TARGET_DIRECTORY
    )
}

/// The folder, relative to the workspace root, where cargo writes each
/// compiled crate's fingerprint JSON.
///
/// It sits in the same private build tree as [`cargo_timings_directory`].
/// Every build the host starts sets `CARGO_TARGET_DIR` to
/// [`crate::config::MODULE_BUILD_TARGET_DIRECTORY`], so the fingerprints
/// under it belong to the builds this report describes.
fn cargo_fingerprint_directory() -> String {
    format!(
        "{}/.fingerprint",
        crate::config::module_build_artifact_directory()
    )
}

/// Which pipeline a module belongs to. It is shown in the report's `kind`
/// column.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleKind {
    /// An extension loaded from its private hot-load copy.
    Extension,
    /// The active project module.
    Project,
}

impl ModuleKind {
    /// The value printed in the report's `kind` column.
    fn label(self) -> &'static str {
        match self {
            ModuleKind::Extension => "extension",
            ModuleKind::Project => "project",
        }
    }
}

// =============================================================================
// PE Inspection
// =============================================================================

/// One imported DLL and the function names the image needs from it.
pub(crate) struct PeImport {
    /// The DLL as the import table names it (a file name, `.dll` included).
    pub dll: String,
    /// Named imports; ordinal-only entries are skipped - nothing to name.
    pub symbols: Vec<String>,
}

/// The parts of a Windows PE header that this report uses.
pub(crate) struct PeInspection {
    /// DLL names referenced by the import directory.
    pub import_dlls: Vec<String>,
    /// The same DLLs with their named imports, in table order.
    pub imports: Vec<PeImport>,
    /// Exported names, so a caller can check that an import resolves.
    pub exports: Vec<String>,
    /// `SizeOfImage` from the optional header, in bytes.
    pub image_size: u64,
}

/// Read a Windows PE file's import and export tables and its image size.
///
/// Returns `None` for anything that is not a PE32 or PE32+ image, which
/// includes every `.so` and `.dylib`. It only reads the file's bytes and uses
/// no platform-specific API, so it builds and runs everywhere even though PE
/// is a Windows format. Inside a valid image a damaged table is read only as
/// far as it parses, so a partial answer is returned instead of a failure.
pub(crate) fn inspect_pe(path: &Path) -> Option<PeInspection> {
    // Step 1: The DOS stub and the PE signature, which is all that separates a
    // PE image from any other file.
    let image_bytes = std::fs::read(path).ok()?;
    if image_bytes.len() < 0x40 || &image_bytes[0..2] != b"MZ" {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(image_bytes[0x3c..0x40].try_into().ok()?) as usize;
    if e_lfanew + 24 > image_bytes.len() || &image_bytes[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return None;
    }
    let coff_header_offset = e_lfanew + 4;
    let number_of_sections = u16::from_le_bytes(
        image_bytes[coff_header_offset + 2..coff_header_offset + 4]
            .try_into()
            .ok()?,
    ) as usize;
    let size_of_optional_header = u16::from_le_bytes(
        image_bytes[coff_header_offset + 16..coff_header_offset + 18]
            .try_into()
            .ok()?,
    ) as usize;
    let optional_header_offset = coff_header_offset + 20;
    // 60 bytes covers every fixed field read below; the deepest is the
    // `SizeOfImage` at offset 56. A header that declares fewer bytes than that
    // is refused before those fields are read, so a short header can never
    // vouch for reads that run past it.
    if size_of_optional_header < 60
        || optional_header_offset + size_of_optional_header > image_bytes.len()
    {
        return None;
    }
    let magic = u16::from_le_bytes(
        image_bytes[optional_header_offset..optional_header_offset + 2]
            .try_into()
            .ok()?,
    );
    // The data-directory table starts at a different offset in PE32 vs PE32+;
    // `SizeOfImage` sits at offset 56 in both.
    let data_directory_offset = match magic {
        0x10b => optional_header_offset + 96,
        0x20b => optional_header_offset + 112,
        _ => return None,
    };
    let image_size = u32::from_le_bytes(
        image_bytes[optional_header_offset + 56..optional_header_offset + 60]
            .try_into()
            .ok()?,
    ) as u64;
    if data_directory_offset + 16 > image_bytes.len() {
        return None;
    }
    let import_rva = u32::from_le_bytes(
        image_bytes[data_directory_offset + 8..data_directory_offset + 12]
            .try_into()
            .ok()?,
    );
    let import_size = u32::from_le_bytes(
        image_bytes[data_directory_offset + 12..data_directory_offset + 16]
            .try_into()
            .ok()?,
    );
    let export_rva = u32::from_le_bytes(
        image_bytes[data_directory_offset..data_directory_offset + 4]
            .try_into()
            .ok()?,
    );
    let export_size = u32::from_le_bytes(
        image_bytes[data_directory_offset + 4..data_directory_offset + 8]
            .try_into()
            .ok()?,
    );

    // Step 2: Map an RVA to a file offset through the section table.
    let section_table_offset = coff_header_offset + 20 + size_of_optional_header;
    let rva_to_offset = |rva: u32| -> Option<usize> {
        if rva == 0 {
            return None;
        }
        for index in 0..number_of_sections {
            let section_entry_offset = section_table_offset + index * 40;
            if section_entry_offset + 40 > image_bytes.len() {
                break;
            }
            let virtual_size = u32::from_le_bytes(
                image_bytes[section_entry_offset + 8..section_entry_offset + 12]
                    .try_into()
                    .ok()?,
            );
            let virtual_address = u32::from_le_bytes(
                image_bytes[section_entry_offset + 12..section_entry_offset + 16]
                    .try_into()
                    .ok()?,
            );
            let raw_size = u32::from_le_bytes(
                image_bytes[section_entry_offset + 16..section_entry_offset + 20]
                    .try_into()
                    .ok()?,
            );
            let raw_pointer = u32::from_le_bytes(
                image_bytes[section_entry_offset + 20..section_entry_offset + 24]
                    .try_into()
                    .ok()?,
            );
            let section_span = virtual_size.max(raw_size);
            if rva >= virtual_address && rva < virtual_address.saturating_add(section_span) {
                let file_offset = raw_pointer as usize + (rva - virtual_address) as usize;
                if file_offset < image_bytes.len() {
                    return Some(file_offset);
                }
            }
        }
        None
    };

    // Step 3: The import directory - a null-terminated list of image import
    // descriptors, each naming one imported DLL and the symbols taken from it.
    let mut import_dlls = Vec::new();
    let mut imports = Vec::new();
    if import_rva != 0 && import_size != 0 {
        if let Some(import_directory_offset) = rva_to_offset(import_rva) {
            // The named entries of one lookup table (the original thunk table
            // when present, the address table otherwise).
            let symbol_names = |lookup_rva: u32| -> Vec<String> {
                let mut names = Vec::new();
                let Some(mut offset) = rva_to_offset(lookup_rva) else {
                    return names;
                };
                let entry_size = if magic == 0x20b { 8 } else { 4 };
                while names.len() < 4096 {
                    let Some(bytes) = image_bytes.get(offset..offset + entry_size) else {
                        break;
                    };
                    let thunk_value = if entry_size == 8 {
                        u64::from_le_bytes(bytes.try_into().ok().unwrap_or_default())
                    } else {
                        u32::from_le_bytes(bytes.try_into().ok().unwrap_or_default()) as u64
                    };
                    if thunk_value == 0 {
                        break; // A null thunk terminates the table.
                    }
                    offset += entry_size;
                    let ordinal = 1u64 << (entry_size * 8 - 1);
                    if thunk_value & ordinal != 0 {
                        continue; // An ordinal import; nothing to name.
                    }
                    let Some(name_offset) = rva_to_offset(thunk_value as u32) else {
                        break;
                    };
                    // `IMAGE_IMPORT_BY_NAME`: a u16 hint, then the name.
                    let name_start = name_offset + 2;
                    if name_start >= image_bytes.len() {
                        break;
                    }
                    let end = image_bytes[name_start..]
                        .iter()
                        .position(|&byte| byte == 0)
                        .map(|position| name_start + position)
                        .unwrap_or(name_start);
                    if let Ok(name) = std::str::from_utf8(&image_bytes[name_start..end]) {
                        names.push(name.to_string());
                    }
                }
                names
            };
            let mut index = 0usize;
            while index < 4096 {
                let descriptor = import_directory_offset + index * 20;
                if descriptor + 20 > image_bytes.len() {
                    break;
                }
                let original_first_thunk =
                    u32::from_le_bytes(image_bytes[descriptor..descriptor + 4].try_into().ok()?);
                let name_rva = u32::from_le_bytes(
                    image_bytes[descriptor + 12..descriptor + 16]
                        .try_into()
                        .ok()?,
                );
                if name_rva == 0 {
                    break; // A null descriptor terminates the import table.
                }
                if let Some(offset) = rva_to_offset(name_rva) {
                    let end = image_bytes[offset..]
                        .iter()
                        .position(|&byte| byte == 0)
                        .map(|position| offset + position)
                        .unwrap_or(offset);
                    if let Ok(name) = std::str::from_utf8(&image_bytes[offset..end]) {
                        let first_thunk = u32::from_le_bytes(
                            image_bytes[descriptor + 16..descriptor + 20]
                                .try_into()
                                .ok()?,
                        );
                        let lookup_table_rva = if original_first_thunk != 0 {
                            original_first_thunk
                        } else {
                            first_thunk
                        };
                        let symbols = symbol_names(lookup_table_rva);
                        import_dlls.push(name.to_string());
                        imports.push(PeImport {
                            dll: name.to_string(),
                            symbols,
                        });
                    }
                }
                index += 1;
            }
        }
    }

    // Step 4: The export directory's name-pointer table lists every exported
    // name.
    let mut exports = Vec::new();
    if export_rva != 0 && export_size != 0 {
        if let Some(export_directory_offset) = rva_to_offset(export_rva) {
            let read_u32 = |offset: usize| -> Option<u32> {
                let bytes: [u8; 4] = image_bytes.get(offset..offset + 4)?.try_into().ok()?;
                Some(u32::from_le_bytes(bytes))
            };
            let number_of_names = read_u32(export_directory_offset + 24).unwrap_or(0) as usize;
            // `AddressOfNames` (+32) is the name-pointer table; `+28` is the
            // function-RVA table, which happens to parse as garbage names.
            let names_rva = read_u32(export_directory_offset + 32).unwrap_or(0);
            if let Some(names_offset) = rva_to_offset(names_rva) {
                for index in 0..number_of_names.min(16384) {
                    let Some(name_rva) = read_u32(names_offset + index * 4) else {
                        break;
                    };
                    let Some(name_offset) = rva_to_offset(name_rva) else {
                        continue;
                    };
                    let end = image_bytes[name_offset..]
                        .iter()
                        .position(|&byte| byte == 0)
                        .map(|position| name_offset + position)
                        .unwrap_or(name_offset);
                    if let Ok(name) = std::str::from_utf8(&image_bytes[name_offset..end]) {
                        exports.push(name.to_string());
                    }
                }
            }
        }
    }

    Some(PeInspection {
        import_dlls,
        imports,
        exports,
        image_size,
    })
}

// =============================================================================
// Process Memory
// =============================================================================

/// Current and peak working set (resident set) of a process, in bytes.
///
/// A `pid` of `None` asks about the host process itself. Returns `None` when
/// the OS exposes no usable counter, for example when Linux has no `/proc`.
pub(crate) fn process_memory(pid: Option<u32>) -> Option<(u64, u64)> {
    #[cfg(target_os = "windows")]
    {
        // Windows process-memory counters without a static dependency. The
        // three functions this needs are loaded at runtime from the OS's own
        // `kernel32.dll` and `psapi.dll` through `libloading`, which the host
        // already links. That matters: a cargo crate that shares features with
        // `colored` (through `windows-sys`) would change how shared crates are
        // feature-unified between the host build and the extension builds, and
        // that silently splits the shared `pill_core.dll` into two incompatible
        // variants.
        use libloading::{Library, Symbol};

        // A local copy of `PROCESS_MEMORY_COUNTERS`. `GetProcessMemoryInfo`
        // rejects a `cb` smaller than the documented struct, so every field is
        // declared, even the trailing ones this code never reads.
        #[repr(C)]
        struct ProcessMemoryCounters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
            private_usage: usize,
        }

        type OpenProcessFn = unsafe extern "system" fn(u32, i32, u32) -> isize;
        type CloseHandleFn = unsafe extern "system" fn(isize) -> i32;
        type GetProcessMemoryInfoFn =
            unsafe extern "system" fn(isize, *mut ProcessMemoryCounters, u32) -> i32;

        // SAFETY: `kernel32.dll` and `psapi.dll` ship with every supported
        // Windows version, and the resolved symbols are cast to their
        // documented Win32 prototypes.
        unsafe {
            let kernel32 = Library::new("kernel32.dll").ok()?;
            let psapi = Library::new("psapi.dll").ok()?;
            let open_process: Symbol<OpenProcessFn> = kernel32.get(b"OpenProcess").ok()?;
            let close_handle: Symbol<CloseHandleFn> = kernel32.get(b"CloseHandle").ok()?;
            let get_process_memory_info: Symbol<GetProcessMemoryInfoFn> =
                psapi.get(b"GetProcessMemoryInfo").ok()?;

            // `GetCurrentProcess` is a pseudo-handle of `-1` that never needs
            // closing; `OpenProcess` returns a real handle closed below.
            const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
            const PROCESS_VM_READ: u32 = 0x0010;
            let process_handle = match pid {
                Some(pid) => open_process(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid),
                None => -1isize,
            };
            if process_handle == 0 {
                return None;
            }

            let mut counters = ProcessMemoryCounters {
                cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
                page_fault_count: 0,
                peak_working_set_size: 0,
                working_set_size: 0,
                quota_peak_paged_pool_usage: 0,
                quota_paged_pool_usage: 0,
                quota_peak_non_paged_pool_usage: 0,
                quota_non_paged_pool_usage: 0,
                pagefile_usage: 0,
                peak_pagefile_usage: 0,
                private_usage: 0,
            };
            // SAFETY: `counters` is a valid out-parameter with `cb` set to its
            // own size, and `process_handle` names the target process.
            let memory_info_succeeded = get_process_memory_info(
                process_handle,
                &mut counters,
                std::mem::size_of::<ProcessMemoryCounters>() as u32,
            );
            if pid.is_some() {
                // SAFETY: `process_handle` came from `OpenProcess` and is closed
                // exactly once.
                close_handle(process_handle);
            }
            if memory_info_succeeded == 0 {
                return None;
            }
            Some((
                counters.working_set_size as u64,
                counters.peak_working_set_size as u64,
            ))
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Linux `/proc/<pid>/status` exposes `VmRSS` (current) and `VmHWM`
        // (peak high-water mark), both in kB.
        let status_path = match pid {
            Some(pid) => format!("/proc/{pid}/status"),
            None => "/proc/self/status".to_string(),
        };
        let content = std::fs::read_to_string(status_path).ok()?;
        let mut current_kb = None;
        let mut peak_kb = None;
        for line in content.lines() {
            if let Some(value) = line.strip_prefix("VmRSS:") {
                current_kb = value
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok());
            } else if let Some(value) = line.strip_prefix("VmHWM:") {
                peak_kb = value
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse::<u64>().ok());
            }
        }
        Some((current_kb? * 1024, peak_kb? * 1024))
    }
}

// =============================================================================
// Cargo `--timings` Parsing
// =============================================================================

/// Per-crate compile+link wall time from the newest cargo `--timings` report.
pub(crate) struct CargoTiming {
    /// Crate name → wall time in milliseconds, for crates that actually ran.
    pub crate_durations_ms: HashMap<String, u64>,
    /// The `DURATION = N;` constant: total wall time of that cargo invocation.
    pub total_seconds: f64,
}

/// Read the newest `.html` report under the build tree's `cargo-timings`
/// folder.
///
/// Cargo embeds a JSON `UNIT_DATA` array at the end of the HTML, with one
/// entry per unit carrying `name`, `mode`, `start`, `duration` (in seconds)
/// and `sections`. Only crates that actually compiled or linked in that run
/// have a non-zero duration, which is exactly what a report about a rebuild
/// needs. Returns `None` when there is no report yet, or when the embedded
/// JSON cannot be parsed.
pub(crate) fn parse_latest_cargo_timings(workspace_root: &Path) -> Option<CargoTiming> {
    let timings_directory = workspace_root.join(cargo_timings_directory());
    let mut reports: Vec<PathBuf> = std::fs::read_dir(&timings_directory)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "html")
        })
        .collect();
    reports.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH)
    });
    let newest_report = reports.last()?;
    let content = std::fs::read_to_string(newest_report).ok()?;

    // `DURATION = <seconds>;` holds the total wall time of that cargo run. The
    // value ends at the first `;`, and the text after it (the `UNIT_DATA`
    // script) must not be part of the parse.
    let total_seconds = content
        .find("DURATION = ")
        .map(|index| &content[index + "DURATION = ".len()..])
        .and_then(|tail| tail.split(';').next())
        .and_then(|value| value.trim().parse::<f64>().ok())
        .unwrap_or(0.0);

    // The `UNIT_DATA` array closes with `];`, and the first `];` after the
    // opening bracket is that terminator: in this pretty-printed JSON no value
    // ends with a `]` directly followed by a `;`. The slice keeps both
    // brackets so serde receives a JSON array rather than its bare contents.
    let unit_data_open_index = content.find("const UNIT_DATA = [")?;
    let array_start = unit_data_open_index + "const UNIT_DATA = [".len() - 1;
    let array_end = content[array_start..].find("];")? + array_start;
    let units: Vec<Value> = serde_json::from_str(&content[array_start..=array_end]).ok()?;

    let mut crate_durations_ms = HashMap::new();
    for unit in units {
        let Some(name) = unit.get("name").and_then(Value::as_str) else {
            continue;
        };
        let duration_seconds = unit.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
        if duration_seconds > 0.0 {
            crate_durations_ms.insert(name.to_string(), (duration_seconds * 1000.0) as u64);
        }
    }
    Some(CargoTiming {
        crate_durations_ms,
        total_seconds,
    })
}

// =============================================================================
// Cargo Dependency Reading
// =============================================================================

/// Read a crate's direct dependencies from its fingerprint JSON.
///
/// Cargo writes
/// `<artifact directory>/.fingerprint/<crate>-<hash>/lib-<crate>.json` for
/// every compiled library, whose `deps` array names each direct dependency.
/// Returns an empty vector when the fingerprint is missing (for example before
/// the first build of a fresh workspace).
pub(crate) fn read_cargo_deps(workspace_root: &Path, crate_name: &str) -> Vec<String> {
    let fingerprint_directory = workspace_root.join(cargo_fingerprint_directory());
    let Ok(entries) = std::fs::read_dir(&fingerprint_directory) else {
        return Vec::new();
    };
    let mut directories: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!("{crate_name}-")))
        })
        .collect();
    directories.sort();

    let mut dependency_names = Vec::new();
    for directory in directories {
        let lib_json_path = directory.join(format!("lib-{crate_name}.json"));
        if !lib_json_path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&lib_json_path) else {
            continue;
        };
        let Ok(fingerprint_value) = serde_json::from_str::<Value>(&content) else {
            continue;
        };
        if let Some(dependency_list) = fingerprint_value.get("deps").and_then(Value::as_array) {
            for dependency in dependency_list {
                // Cargo's fingerprint `deps` is an array of arrays, each one
                // `[fingerprint, name, is_public, local_fingerprint]`.
                let name = dependency
                    .as_array()
                    .and_then(|entry| entry.get(1))
                    .and_then(Value::as_str);
                if let Some(name) = name {
                    if !dependency_names
                        .iter()
                        .any(|existing_name: &String| existing_name == name)
                    {
                        dependency_names.push(name.to_string());
                    }
                }
            }
        }
        break;
    }
    dependency_names
}

// =============================================================================
// Collector
// =============================================================================

/// One module's cumulative analytics across startup and every reload.
struct ModuleAnalytics {
    name: String,
    kind: ModuleKind,
    /// The last cargo build's wall time; `0` until a build records one.
    build_wall_ms: u64,
    /// The phase timings carry fractional milliseconds so fast operations
    /// (stage, load, init) do not round to zero.
    stage_ms: f64,
    load_ms: f64,
    init_ms: f64,
    migrate_ms: f64,
    /// The DLL's size on disk, as the build left it.
    artifact_bytes: u64,
    /// `SizeOfImage` from the DLL's PE header, in bytes.
    image_size: u64,
    import_dlls: Vec<String>,
    /// Direct cargo dependencies, read from the crate's fingerprint.
    cargo_deps: Vec<String>,
    /// This module's own unit in the newest cargo `--timings` report, when the
    /// build ran in that invocation.
    cargo_unit_ms: Option<u64>,
    /// Completed reloads of this module.
    reloads: u32,
}

impl ModuleAnalytics {
    fn new(name: &str, kind: ModuleKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            build_wall_ms: 0,
            stage_ms: 0.0,
            load_ms: 0.0,
            init_ms: 0.0,
            migrate_ms: 0.0,
            artifact_bytes: 0,
            image_size: 0,
            import_dlls: Vec::new(),
            cargo_deps: Vec::new(),
            cargo_unit_ms: None,
            reloads: 0,
        }
    }
}

/// How a generation of code reached the running process.
///
/// Both kinds print the same `Hot reload finished ...` line, so one parser
/// reads both; the `kind=` field tells them apart. A patch is not a second
/// format. It is a reload whose phases happen to be much cheaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventKind {
    /// A module or project DLL was rebuilt, reloaded and re-registered.
    Reload,
    /// One function's implementation was replaced in place.
    Patch,
}

impl EventKind {
    /// The value printed after `kind=`.
    fn label(self) -> &'static str {
        match self {
            Self::Reload => "reload",
            Self::Patch => "patch",
        }
    }
}

/// Which of the three mechanisms delivered a patch.
///
/// `kind=patch` on its own hides a real difference in guarantees, which is why
/// the route is reported separately. A slot route is provable: the replacement
/// is one atomic pointer store, and any call that reaches the dispatcher runs
/// the new code. The prologue route is best-effort: it overwrites the first
/// bytes of a live function, so it cannot reach a caller that inlined the body,
/// and the twelve-byte write is not atomic. Averaging the two would mix a
/// guarantee with an approximation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatchRoute {
    /// A `#[pill_hot]` system, replaced through the engine's own registry. The
    /// registry lives once per process, so one install reaches every caller.
    EngineSlot,
    /// A `#[pill_hot_fn]` function, replaced through the redirect slot compiled
    /// into each artifact that links the crate.
    ArtifactSlot,
    /// An un-annotated function, redirected by overwriting its first bytes in
    /// every artifact that carries a copy.
    Prologue,
}

impl PatchRoute {
    /// The value printed after `route=`, and after `via` on the console line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::EngineSlot => "engine-slot",
            Self::ArtifactSlot => "artifact-slot",
            Self::Prologue => "prologue",
        }
    }

    /// Whether a call that reaches the function is guaranteed to run the new
    /// code, rather than possibly running an inlined or unreached copy.
    ///
    /// Only the hot-patch path asks, so a build without hot patching has no
    /// caller. That is the same reason [`EventKind`] carries the allow on its
    /// own declaration.
    pub(crate) fn is_provable(self) -> bool {
        matches!(self, Self::EngineSlot | Self::ArtifactSlot)
    }
}

/// A snapshot of one completed hot reload, printed once the frame's reloads
/// have all finished.
struct ReloadEvent {
    /// Whether this was a whole-artifact reload or an in-place patch.
    kind: EventKind,
    /// Which mechanisms delivered a patch, and how many copies they reached in
    /// total. `None` for a reload, which has no route because it replaces the
    /// whole artifact instead of redirecting a function.
    route: Option<(Vec<PatchRoute>, usize)>,
    /// The module's name, or the names of the patched functions.
    name: String,
    build_ms: u64,
    stage_ms: f64,
    load_ms: f64,
    init_ms: f64,
    migrate_ms: f64,
    artifact_bytes: u64,
    /// This event's ordinal among every reload and patch of the process, so
    /// no two events in one session print the same `number=N`.
    reload_count: u32,
    /// Crates that actually compiled or linked in this reload's cargo
    /// invocations, with their wall times, sorted by time descending. This is
    /// how a dependent crate that was relinked shows up: editing
    /// `pill_dummy_color`, for example, also relinks `pill_spline`.
    cargo_crates: Vec<(String, u64)>,
}

/// Why one change fell back to a full reload instead of patching.
///
/// Identical causes collapse into one entry with a count. The usual case is
/// the same edit failing the same way on every save, and five copies of one
/// compiler error would bury the reload line they explain.
struct PatchFallback {
    /// Qualified function, or `None` when the fast path declined before it
    /// could attribute the change to one.
    function: Option<String>,
    /// `refused` when the fast path declined, `failed` when it tried and could
    /// not finish. The difference matters. A refusal means the change did not
    /// meet a precondition; a failure means the patch itself did not build.
    outcome: &'static str,
    /// Short machine-readable cause, such as `compile` or `edited-before-arming`.
    code: String,
    /// The explanation, trimmed to its first line and capped.
    detail: String,
    /// How many times this exact cause repeated.
    count: u64,
}

/// Process-wide analytics state, guarded because builds record from worker
/// threads while the frame loop prints the reports.
struct Analytics {
    started: Instant,
    modules: Vec<ModuleAnalytics>,
    pending_reload_events: Vec<ReloadEvent>,
    /// Per-crate compile and link times collected from the cargo `--timings`
    /// reports of the builds in the current reload transaction.
    /// [`record_reload`] takes a copy and then clears them, so each reload
    /// line shows exactly which crates its own build rebuilt.
    pending_cargo_crates: HashMap<String, u64>,
    host_current_bytes: u64,
    host_peak_bytes: u64,
    cargo_child_peak_bytes: u64,
    builds: u64,
    /// Total wall time of the newest cargo `--timings` report, in seconds.
    last_cargo_total_seconds: f64,
    /// Function patches that went live in this process.
    patches: u64,
    /// Reloads and patches recorded so far in this process, which numbers
    /// each event's `number=N`.
    reload_events: u32,
    /// Changes the fast path declined to patch, which fell through to a reload.
    patch_refusals: u64,
    /// Patches that were attempted and failed, leaving the previous
    /// implementation running.
    patch_failures: u64,
    /// Why the fast path did not take, one entry per distinct cause.
    patch_fallbacks: Vec<PatchFallback>,
}

/// The process-wide collector, initialized on first use.
fn analytics() -> &'static Mutex<Analytics> {
    static ANALYTICS: OnceLock<Mutex<Analytics>> = OnceLock::new();
    ANALYTICS.get_or_init(|| {
        Mutex::new(Analytics {
            started: Instant::now(),
            modules: Vec::new(),
            pending_reload_events: Vec::new(),
            pending_cargo_crates: HashMap::new(),
            host_current_bytes: 0,
            host_peak_bytes: 0,
            cargo_child_peak_bytes: 0,
            builds: 0,
            last_cargo_total_seconds: 0.0,
            patches: 0,
            reload_events: 0,
            patch_refusals: 0,
            patch_failures: 0,
            patch_fallbacks: Vec::new(),
        })
    })
}

/// Find a module's entry by name, creating it on first use.
///
/// A caller that only knows a load or phase timing passes `Extension`. A build
/// later corrects the kind through [`record_module_artifact`], which always
/// runs before any load, init or migrate record for that module.
fn find_or_create(collector: &mut Analytics, name: &str, kind: ModuleKind) -> usize {
    if let Some(index) = collector
        .modules
        .iter()
        .position(|module| module.name == name)
    {
        return index;
    }
    collector.modules.push(ModuleAnalytics::new(name, kind));
    collector.modules.len() - 1
}

/// Record the outcome of one module's build and stage step.
///
/// `build_extension` and `build_project_module` call it after the real build
/// branch, with the path of the DLL the host actually loads (the hot copy for
/// extensions). It also inspects the DLL's PE image size and imports, reads the
/// crate's direct cargo dependencies, and, when timing reports are enabled,
/// takes the per-crate compile and link time from the newest cargo `--timings`
/// report.
pub(crate) fn record_module_artifact(
    name: &str,
    kind: ModuleKind,
    stage_ms: f64,
    workspace_root: &Path,
    artifact_path: &Path,
) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, kind);
    let module = &mut collector.modules[index];
    module.kind = kind;
    module.stage_ms = stage_ms;
    if let Ok(metadata) = std::fs::metadata(artifact_path) {
        module.artifact_bytes = metadata.len();
    }
    if let Some(inspection) = inspect_pe(artifact_path) {
        module.import_dlls = inspection.import_dlls;
        module.image_size = inspection.image_size;
    }
    module.cargo_deps = read_cargo_deps(workspace_root, name);
    // Read a timing report only when this process's builds were asked to write
    // one. `--timings` is opt-in, and a leftover HTML file from an earlier run
    // must not be reported as this build's timings.
    if crate::config::cargo_timings_enabled() {
        if let Some(timing) = parse_latest_cargo_timings(workspace_root) {
            module.cargo_unit_ms = timing.crate_durations_ms.get(name).copied();
            collector.last_cargo_total_seconds = timing.total_seconds;
            // Remember every crate this build actually ran so the next reload
            // report can show the full recompile/relink set, not just the
            // module whose transaction this was.
            for (crate_name, duration_ms) in timing.crate_durations_ms {
                collector
                    .pending_cargo_crates
                    .entry(crate_name)
                    .or_insert(duration_ms);
            }
        }
    }
    collector.builds += 1;
}

/// Record the wall time and peak memory of one cargo invocation.
///
/// `run_build_command` calls it after a successful build. The build watchdog
/// samples the cargo child process's peak working set, and the host's own
/// memory is read at the same time, so the report shows what a heavy compile
/// did to the host.
pub(crate) fn record_build_command(name: &str, elapsed_ms: u64, cargo_peak_bytes: u64) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, ModuleKind::Extension);
    collector.modules[index].build_wall_ms = elapsed_ms;
    collector.cargo_child_peak_bytes = collector.cargo_child_peak_bytes.max(cargo_peak_bytes);
    if let Some((current_bytes, peak_bytes)) = process_memory(None) {
        collector.host_current_bytes = current_bytes;
        collector.host_peak_bytes = collector.host_peak_bytes.max(peak_bytes);
    }
}

/// Record the DLL mapping time of one module.
pub(crate) fn record_load(name: &str, load_ms: f64) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, ModuleKind::Extension);
    collector.modules[index].load_ms = load_ms;
}

/// The last cargo build wall time recorded for `name`, if there is one.
///
/// The reload toasts show it ("reloaded in 820 ms"). A module that never ran a
/// recorded build, such as a load-only extension or the first reload after a
/// restart, returns `None` instead of a misleading zero.
pub(crate) fn module_build_ms(name: &str) -> Option<u64> {
    let collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    collector
        .modules
        .iter()
        .find(|module| module.name == name)
        .map(|module| module.build_wall_ms)
        .filter(|milliseconds| *milliseconds != 0)
}

/// Record the registration (`init`) time of one module.
pub(crate) fn record_init(name: &str, init_ms: f64) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, ModuleKind::Extension);
    collector.modules[index].init_ms = init_ms;
}

/// Record the schema-migration time of one module reload.
pub(crate) fn record_migrate(name: &str, migrate_ms: f64) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, ModuleKind::Extension);
    collector.modules[index].migrate_ms = migrate_ms;
}

/// Record a completed hot reload and queue its report entry.
pub(crate) fn record_reload(name: &str) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = find_or_create(&mut collector, name, ModuleKind::Extension);
    collector.modules[index].reloads += 1;
    collector.reload_events += 1;
    let reload_count = collector.reload_events;
    // Snapshot the module's latest phase timings and the crates this reload's
    // builds actually rebuilt, then clear the pending set so the next reload
    // transaction starts fresh.
    let mut cargo_crates: Vec<(String, u64)> = collector
        .pending_cargo_crates
        .iter()
        .map(|(crate_name, duration_ms)| (crate_name.clone(), *duration_ms))
        .collect();
    cargo_crates.sort_by_key(|(_, duration_ms)| std::cmp::Reverse(*duration_ms));
    collector.pending_cargo_crates.clear();
    let snapshot = {
        let module = &collector.modules[index];
        ReloadEvent {
            kind: EventKind::Reload,
            route: None,
            name: name.to_string(),
            build_ms: module.build_wall_ms,
            stage_ms: module.stage_ms,
            load_ms: module.load_ms,
            init_ms: module.init_ms,
            migrate_ms: module.migrate_ms,
            artifact_bytes: module.artifact_bytes,
            reload_count,
            cargo_crates,
        }
    };
    collector.pending_reload_events.push(snapshot);
}

/// Record one function patch in the same shape as a module reload.
///
/// The phase mapping is deliberate, not an approximation. `build` is `rustc`,
/// which plays the part of a module's cargo build. `stage` is everything that
/// prepares the build: classification, source generation and the flag capture.
/// `load` is `LoadLibrary`. `init` is the install that makes the new code live,
/// the same job `pill_module_init` does for a reload. `migrate` is always zero,
/// because a patch cannot change a component schema - that is exactly the kind
/// of edit it refuses.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_patch(
    function: &str,
    stage_ms: f64,
    build_ms: u64,
    load_ms: f64,
    activate_ms: f64,
    artifact_bytes: u64,
    routes: &[PatchRoute],
    copies: usize,
) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    collector.patches += 1;
    collector.reload_events += 1;
    let reload_count = collector.reload_events;
    collector.pending_reload_events.push(ReloadEvent {
        kind: EventKind::Patch,
        route: Some((routes.to_vec(), copies)),
        name: function.to_string(),
        build_ms,
        stage_ms,
        load_ms,
        init_ms: activate_ms,
        migrate_ms: 0.0,
        artifact_bytes,
        // Patches and reloads share one session-wide count, so the
        // `number=N` field names one event whichever kind it was.
        reload_count,
        // No cargo invocation ran, so there is no per-crate breakdown to show.
        cargo_crates: Vec::new(),
    });
}

/// Record one managed assembly swap in the same shape as a module reload.
///
/// The phase mapping follows `record_patch`'s rule of naming what each phase
/// really is here. `stage` is everything that precedes the build - the
/// watcher's debounce plus the wait for the frame loop to start building - and
/// `build` is the assembly rebuild. `load` is the loader's swap. `init` and
/// `migrate` are zero because the loader has no separate activation or schema
/// step, and the artifact size is zero because the host is never told the
/// assembly's size; the report prints that as `size=-`.
///
/// A swap whose timing was never armed records zeros, which is what the report
/// says for a phase that did not run: an assembly this host did not build has
/// no measured build to report.
pub(crate) fn record_managed_reload(name: &str, stage_ms: f64, build_ms: f64, load_ms: f64) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    collector.reload_events += 1;
    let reload_count = collector.reload_events;
    collector.pending_reload_events.push(ReloadEvent {
        kind: EventKind::Reload,
        route: None,
        name: name.to_string(),
        // A `NaN` build (no timed assembly) rounds to 0, the report's value for
        // a phase that did not run.
        build_ms: build_ms.round() as u64,
        stage_ms,
        load_ms,
        init_ms: 0.0,
        migrate_ms: 0.0,
        artifact_bytes: 0,
        reload_count,
        // No cargo invocation ran, so there is no per-crate breakdown to show.
        cargo_crates: Vec::new(),
    });
}

/// Record that the fast path declined a change and fell through to a reload.
pub(crate) fn record_patch_refusal(code: &str, detail: &str) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    collector.patch_refusals += 1;
    remember_fallback(&mut collector, None, "refused", code, detail);
}

/// Record that a patch was attempted and failed, keeping the previous code.
pub(crate) fn record_patch_failure(function: &str, code: &str, detail: &str) {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    collector.patch_failures += 1;
    remember_fallback(&mut collector, Some(function), "failed", code, detail);
}

/// Add one fallback reason to the tally, counting a repeat instead of adding a
/// second entry.
///
/// The match uses the function, the outcome and the code, not the detail text,
/// so a compiler message whose column number moves between saves is not a new
/// cause.
fn remember_fallback(
    collector: &mut Analytics,
    function: Option<&str>,
    outcome: &'static str,
    code: &str,
    detail: &str,
) {
    if let Some(existing) = collector.patch_fallbacks.iter_mut().find(|entry| {
        entry.function.as_deref() == function && entry.outcome == outcome && entry.code == code
    }) {
        existing.count += 1;
        return;
    }
    if collector.patch_fallbacks.len() >= MAX_TRACKED_FALLBACK_REASONS {
        return;
    }
    // Keep the first line of the detail only. The host already summarizes a
    // compiler failure to one line, and a multi-line entry would break the
    // report's shape.
    let first_line = detail.lines().next().unwrap_or("").trim();
    let capped_detail: String = if first_line.chars().count() > MAX_FALLBACK_DETAIL_CHARS {
        first_line
            .chars()
            .take(MAX_FALLBACK_DETAIL_CHARS)
            .chain("...".chars())
            .collect()
    } else {
        first_line.to_string()
    };
    collector.patch_fallbacks.push(PatchFallback {
        function: function.map(str::to_string),
        outcome,
        code: code.to_string(),
        detail: capped_detail,
        count: 1,
    });
}

/// Formats the running patch, refusal and failure tally as report lines.
/// Returns an empty list until the fast path has done something.
///
/// Takes its own lock instead of using the caller's, because
/// `print_reload_events` has already released the lock when it asks for these
/// lines.
fn patch_tally_lines() -> Vec<String> {
    let collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if collector.patches == 0 && collector.patch_refusals == 0 && collector.patch_failures == 0 {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "Function hot patching: patches {} | refused {} | failed {}",
        collector.patches.to_string().yellow(),
        collector.patch_refusals.to_string().yellow(),
        collector.patch_failures.to_string().yellow(),
    )];
    // The summary line above gives only counts. These lines give the reason
    // behind each one, recorded here because the moment of failure has long
    // scrolled past by the time the reload report appears.
    for fallback in &collector.patch_fallbacks {
        let repeat_suffix = if fallback.count > 1 {
            format!(" x{}", fallback.count)
        } else {
            String::new()
        };
        lines.push(format!(
            "    {} {} {}{}",
            fallback.function.as_deref().unwrap_or("(change)"),
            fallback.outcome,
            format!("({})", fallback.code).dimmed(),
            repeat_suffix,
        ));
        if !fallback.detail.is_empty() {
            lines.push(format!("        {}", fallback.detail.dimmed()));
        }
    }
    lines
}

/// Record the host process's current and peak memory (called at setup end).
pub(crate) fn record_host_memory() {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((current, peak)) = process_memory(None) {
        collector.host_current_bytes = current;
        collector.host_peak_bytes = collector.host_peak_bytes.max(peak);
    }
}

// =============================================================================
// Report Rendering
// =============================================================================

/// Format a byte count as a compact human-readable string.
fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    }
}

/// Format a millisecond count as a compact human-readable string.
fn format_ms(ms: u64) -> String {
    if ms >= 1000 {
        format!("{:.2}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

/// Print the full startup analytics report.
///
/// Called once after every module (extension + project) has been built, staged,
/// loaded and initialized. See [`render_startup_report`] for the layout.
pub(crate) fn print_startup_report() {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Startup builds left their crates in the pending set, which only the
    // reload report reads. Clear it here so the first reload does not list
    // every crate the startup built.
    collector.pending_cargo_crates.clear();
    info!(
        target: telemetry_target::HOT_RELOAD,
        "{}",
        render_startup_report(&collector)
    );
}

/// One column of the startup table. It holds the header, the alignment, one
/// cell per module row and the total row.
struct ReportColumn {
    header: &'static str,
    left_aligned: bool,
    cells: Vec<String>,
    total: String,
}

impl ReportColumn {
    /// The widest of the header, the cells and the total.
    fn width(&self) -> usize {
        self.cells
            .iter()
            .chain(std::iter::once(&self.total))
            .map(String::len)
            .chain(std::iter::once(self.header.len()))
            .max()
            .unwrap_or(0)
    }

    /// `text` padded to `width` on the side this column aligns to.
    fn pad(&self, text: &str, width: usize) -> String {
        if self.left_aligned {
            format!("{text:<width$}")
        } else {
            format!("{text:>width$}")
        }
    }
}

/// Render the startup report as one multi-line log message ([`log_block_colored`]):
/// the title, the totals, one table row per module with a total row, then each
/// module's links and direct dependencies.
///
/// The title and the `elapsed: ... builds: N    reloads: N` line are parsed by
/// the suites and the benchmark harness (`devops/`), so their text stays as it
/// is. Columns that are empty for every module, such as `migrate` at startup
/// and `cargo` without `--timings`, are left out.
fn render_startup_report(collector: &Analytics) -> String {
    let elapsed_seconds = collector.started.elapsed().as_secs_f64();
    let total_reloads: u64 = collector
        .modules
        .iter()
        .map(|module| u64::from(module.reloads))
        .sum();
    let modules = &collector.modules;

    // Step 1: Build the table columns, then drop the ones with nothing in them.
    let format_milliseconds = |value: f64| {
        if value > 0.0 {
            format!("{value:.1}ms")
        } else {
            "-".to_string()
        }
    };
    let format_count = |value: usize| {
        if value > 0 {
            value.to_string()
        } else {
            "-".to_string()
        }
    };
    let sum = |value: fn(&ModuleAnalytics) -> f64| modules.iter().map(value).sum::<f64>();
    let build_total: u64 = modules.iter().map(|module| module.build_wall_ms).sum();
    let mut columns = vec![
        ReportColumn {
            header: "module",
            left_aligned: true,
            cells: modules.iter().map(|module| module.name.clone()).collect(),
            total: "total".to_string(),
        },
        ReportColumn {
            header: "kind",
            left_aligned: true,
            cells: modules
                .iter()
                .map(|module| module.kind.label().to_string())
                .collect(),
            total: String::new(),
        },
        ReportColumn {
            header: "build",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| match module.build_wall_ms {
                    0 => "-".to_string(),
                    build_ms => format_ms(build_ms),
                })
                .collect(),
            total: format_ms(build_total),
        },
        ReportColumn {
            header: "stage",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| format_milliseconds(module.stage_ms))
                .collect(),
            total: format_milliseconds(sum(|module| module.stage_ms)),
        },
        ReportColumn {
            header: "load",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| format_milliseconds(module.load_ms))
                .collect(),
            total: format_milliseconds(sum(|module| module.load_ms)),
        },
        ReportColumn {
            header: "init",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| format_milliseconds(module.init_ms))
                .collect(),
            total: format_milliseconds(sum(|module| module.init_ms)),
        },
        ReportColumn {
            header: "migrate",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| format_milliseconds(module.migrate_ms))
                .collect(),
            total: format_milliseconds(sum(|module| module.migrate_ms)),
        },
        ReportColumn {
            header: "size",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| match module.artifact_bytes {
                    0 => "-".to_string(),
                    bytes => format_bytes(bytes),
                })
                .collect(),
            total: format_bytes(modules.iter().map(|module| module.artifact_bytes).sum()),
        },
        ReportColumn {
            header: "imports",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| format_count(module.import_dlls.len()))
                .collect(),
            total: String::new(),
        },
        ReportColumn {
            header: "cargo",
            left_aligned: false,
            cells: modules
                .iter()
                .map(|module| module.cargo_unit_ms.map_or("-".to_string(), format_ms))
                .collect(),
            total: String::new(),
        },
    ];
    columns.retain(|column| {
        column.left_aligned || column.cells.iter().any(|cell| cell.as_str() != "-")
    });
    let widths: Vec<usize> = columns.iter().map(ReportColumn::width).collect();
    let table_width = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);

    // Step 2: The title and the run totals, colored for the terminal. The log
    // formatter strips the codes for the file lane and for piped output, which
    // is what the harness reads, so the line it parses is plain text.
    let mut lines: Vec<String> = vec![
        "BUILD / LINK / HOT-RELOAD ANALYTICS · startup"
            .cyan()
            .bold()
            .to_string(),
        format!(
            "elapsed: {}    builds: {}    reloads: {}",
            format!("{elapsed_seconds:.2}s").yellow(),
            collector.builds.to_string().yellow(),
            total_reloads.to_string().yellow()
        ),
        format!(
            "{} {} {}    {} {}",
            "host RSS".dimmed(),
            format_bytes(collector.host_current_bytes).yellow(),
            format!("(peak {})", format_bytes(collector.host_peak_bytes)).dimmed(),
            "cargo child peak RSS".dimmed(),
            format_bytes(collector.cargo_child_peak_bytes).yellow(),
        ),
    ];
    if collector.last_cargo_total_seconds > 0.0 {
        lines.push(format!(
            "{} {}",
            "newest cargo --timings total".dimmed(),
            format!("{:.2}s", collector.last_cargo_total_seconds).yellow()
        ));
    }
    lines.push(String::new());

    // Step 3: The table, with a rule under the header and above the totals.
    // Cells are padded before they are colored, so codes never count towards
    // a column's width.
    let row = |cells: Vec<String>| cells.join("  ");
    let rule = "─".repeat(table_width).dimmed().to_string();
    lines.push(row(columns
        .iter()
        .zip(&widths)
        .map(|(column, width)| column.pad(column.header, *width).bold().to_string())
        .collect()));
    lines.push(rule.clone());
    for index in 0..modules.len() {
        lines.push(row(columns
            .iter()
            .zip(&widths)
            .enumerate()
            .map(|(position, (column, width))| {
                let cell = column.pad(&column.cells[index], *width);
                if position == 0 {
                    cell.cyan().to_string()
                } else {
                    cell
                }
            })
            .collect()));
    }
    lines.push(rule);
    // Columns with no total (the counts) at the end of the row are left off,
    // so the line carries no trailing padding.
    let last_total = columns
        .iter()
        .rposition(|column| !column.total.is_empty())
        .unwrap_or(0);
    lines.push(row(columns
        .iter()
        .zip(&widths)
        .take(last_total + 1)
        .map(|(column, width)| column.pad(&column.total, *width).yellow().to_string())
        .collect()));

    // Step 4: What each module links against (system DLLs are only counted,
    // not named) and its direct dependencies, wrapped under the module's
    // name.
    let name_width = widths[0];
    let indent = " ".repeat(name_width + 2);
    let wrap_width = table_width.max(60).saturating_sub(name_width + 2 + 6);
    let mut wrote_detail = false;
    for module in modules {
        let (linked, system_count) = linked_libraries(&module.import_dlls);
        let dependencies: Vec<&str> = module
            .cargo_deps
            .iter()
            .map(String::as_str)
            .filter(|dependency| *dependency != "build_script_build")
            .collect();
        let mut details = Vec::new();
        if !linked.is_empty() || system_count > 0 {
            let system = format!("(+{system_count} system)").dimmed();
            let text = match (linked.is_empty(), system_count) {
                (false, 0) => linked.join(", "),
                (false, _) => format!("{}  {system}", linked.join(", ")),
                (true, _) => system.to_string(),
            };
            details.push(format!("{} {text}", "links".dimmed()));
        }
        for (position, chunk) in wrap_list(&dependencies, wrap_width).into_iter().enumerate() {
            let label = if position == 0 { "deps " } else { "     " };
            details.push(format!("{} {chunk}", label.dimmed()));
        }
        if details.is_empty() {
            continue;
        }
        if !wrote_detail {
            lines.push(String::new());
            wrote_detail = true;
        }
        for (position, detail) in details.iter().enumerate() {
            if position == 0 {
                let name = format!("{:<name_width$}", module.name);
                lines.push(format!("{}  {detail}", name.cyan()));
            } else {
                lines.push(format!("{indent}{detail}"));
            }
        }
    }
    let mut lines = lines.into_iter();
    let heading = lines.next().unwrap_or_default();
    log_block_colored(heading, lines)
}

/// Split a module's imports into the names worth showing and a count of the
/// rest.
///
/// The names kept are the `pill_*` dylibs and Rust's `std`, with the `.dll`
/// suffix and the `std` hash removed. The rest are Windows system and runtime
/// libraries, which are only counted.
fn linked_libraries(import_dlls: &[String]) -> (Vec<String>, usize) {
    let mut linked = Vec::new();
    let mut system_count = 0;
    for dll in import_dlls {
        let stem = dll.strip_suffix(".dll").unwrap_or(dll);
        if stem.starts_with("std-") {
            linked.push("std".to_string());
        } else if stem.starts_with("pill_") {
            linked.push(stem.to_string());
        } else {
            system_count += 1;
        }
    }
    (linked, system_count)
}

/// Join the names with `, ` into lines of at most `width` characters, never
/// splitting one name. A name longer than `width` still gets its own line.
fn wrap_list(items: &[&str], width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current_line = String::new();
    for item in items {
        if !current_line.is_empty() && current_line.len() + 2 + item.len() > width {
            lines.push(format!("{current_line},"));
            current_line.clear();
        }
        if !current_line.is_empty() {
            current_line.push_str(", ");
        }
        current_line.push_str(item);
    }
    if !current_line.is_empty() {
        lines.push(current_line);
    }
    lines
}

/// Log one line per completed hot reload and a summary of the whole
/// transaction, as a single multi-line entry.
///
/// Called at the end of the reload step, once the frame's reloads are done, so
/// the console shows the build → stage → load → init → migrate breakdown as it
/// happens. `reload_started` is the moment the reload transaction began, before
/// the first build, so the total spans the whole cascade - the edited module
/// plus the queued project reload - and not only the last reload. Returns the
/// number of reload events reported.
///
/// The `Hot reload finished ...` and `crates rebuilt by cargo:` lines keep the
/// exact text the suites and the benchmark harness's line parser read.
/// Their colors reach an interactive terminal only (see `log_block_colored`).
pub(crate) fn print_reload_events(reload_started: Instant) -> usize {
    let events = drain_reload_events();
    let count = events.len();
    if count == 0 {
        return 0;
    }
    // Step 1: One line per recorded event, plus its cargo breakdown.
    let mut lines = Vec::new();
    for event in &events {
        lines.push(format!(
            "Hot reload finished | duration=(build={}ms, stage={}ms, load={}ms, init={}ms, \
             migrate={}ms) | size={} | module={} | kind={} | number={}{}",
            // The template carries the unit, so every duration prints as a
            // plain number of milliseconds; a reload that built nothing is 0.
            event.build_ms.to_string().yellow(),
            format!("{:.1}", event.stage_ms).yellow(),
            format!("{:.1}", event.load_ms).yellow(),
            format!("{:.1}", event.init_ms).yellow(),
            format!("{:.1}", event.migrate_ms).yellow(),
            if event.artifact_bytes > 0 {
                format_bytes(event.artifact_bytes)
            } else {
                "-".to_string()
            }
            .yellow(),
            event.name.yellow(),
            event.kind.label().yellow(),
            event.reload_count.to_string().yellow(),
            // Appended rather than folded into `kind=`, so the field the
            // harness already parses keeps its exact meaning and vocabulary.
            // A reload has no route, and prints none.
            match &event.route {
                Some((routes, copies)) => {
                    let joined = routes
                        .iter()
                        .map(|route| route.label())
                        .collect::<Vec<_>>()
                        .join("+");
                    format!(
                        " | route={} | copies={}",
                        joined.yellow(),
                        copies.to_string().yellow()
                    )
                }
                None => String::new(),
            },
        ));
        // The module line above is only the transaction's own timings; the
        // cargo `--timings` breakdown shows every crate the build actually
        // recompiled or relinked (dependents included).
        if !event.cargo_crates.is_empty() {
            let crate_time_texts: Vec<String> = event
                .cargo_crates
                .iter()
                .map(|(crate_name, duration_ms)| {
                    format!(
                        "{} {}",
                        crate_name.green(),
                        format_ms(*duration_ms).yellow()
                    )
                })
                .collect();
            lines.push(format!(
                "    {} {}",
                "crates rebuilt by cargo:".bold(),
                crate_time_texts.join(&" | ".dimmed().to_string())
            ));
        }
    }
    // Step 2: One total for the whole transaction (module + project for a
    // cascade, or a single reload otherwise). The recorded phases are summed
    // and subtracted, so the rest - the wait between two reloads in a
    // cascade, scheduling and rounding - is printed as its own term instead
    // of going missing. Everything is whole milliseconds, so the parts add up
    // exactly to the total.
    let total_ms = reload_started.elapsed().as_millis() as u64;
    let segment_ms: Vec<u64> = events
        .iter()
        .map(|event| {
            event.build_ms
                + (event.stage_ms + event.load_ms + event.init_ms + event.migrate_ms) as u64
        })
        .collect();
    let phases_ms: u64 = segment_ms.iter().sum();
    let gap_ms = total_ms.saturating_sub(phases_ms);
    // Step 3: The running fast-path tally, reported alongside the transaction
    // it belongs to rather than in the startup report, where these are always
    // zero. Left out entirely until the fast path has done something, so a
    // project that has not opted in sees no extra noise.
    lines.extend(patch_tally_lines());

    let label = if count > 1 {
        "Total cascaded reload duration"
    } else {
        "Total reload duration"
    };
    lines.push(format!(
        "{}: {} {}",
        label,
        format_ms(total_ms).yellow(),
        format!("({total_ms}ms)").dimmed(),
    ));
    let segment_texts: Vec<String> = events
        .iter()
        .zip(&segment_ms)
        .map(|(event, segment)| format!("{} {}ms", event.name.cyan(), segment.to_string().yellow()))
        .collect();
    lines.push(format!(
        "    = {} + {} {}ms",
        segment_texts.join(&" + ".dimmed().to_string()),
        "gap/scheduling".dimmed(),
        gap_ms.to_string().yellow(),
    ));
    info!(
        target: telemetry_target::HOT_RELOAD,
        "{}",
        log_block_colored("Reload analytics", lines)
    );
    count
}

/// Take and clear the pending reload events for the current frame.
fn drain_reload_events() -> Vec<ReloadEvent> {
    let mut collector = analytics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut collector.pending_reload_events)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    /// The startup report keeps the line the benchmark harness parses, drops
    /// columns that are empty for every module, lines its rows up, and names
    /// the engine's `pill_*` libraries and `std` among the imports while the
    /// rest are only counted.
    #[test]
    fn startup_report_layout() {
        let mut extension =
            ModuleAnalytics::new("pill_master_renderer_data", ModuleKind::Extension);
        extension.build_wall_ms = 5830;
        extension.load_ms = 178.2;
        extension.artifact_bytes = 3 * 1024 * 1024;
        extension.import_dlls = vec![
            "pill_core.dll".to_string(),
            "std-0cebe7c42cd80226.dll".to_string(),
            "KERNEL32.dll".to_string(),
            "api-ms-win-crt-heap-l1-1-0.dll".to_string(),
        ];
        extension.cargo_deps = vec!["build_script_build".to_string(), "glam".to_string()];
        let mut project = ModuleAnalytics::new("project", ModuleKind::Project);
        project.build_wall_ms = 3220;
        project.init_ms = 2.8;
        let collector = Analytics {
            started: Instant::now(),
            modules: vec![extension, project],
            pending_reload_events: Vec::new(),
            pending_cargo_crates: HashMap::new(),
            host_current_bytes: 1024 * 1024,
            host_peak_bytes: 2 * 1024 * 1024,
            cargo_child_peak_bytes: 0,
            builds: 4,
            last_cargo_total_seconds: 0.0,
            patches: 0,
            reload_events: 0,
            patch_refusals: 0,
            patch_failures: 0,
            patch_fallbacks: Vec::new(),
        };

        // Checked as a plain lane receives it: whether `colored` emits codes
        // depends on the terminal the tests run in.
        let report = pill_core::telemetry::strip_color_codes(&render_startup_report(&collector));

        assert!(
            report.contains("BUILD / LINK / HOT-RELOAD ANALYTICS"),
            "{report}"
        );
        assert!(report.contains("    builds: 4    reloads: 0"), "{report}");
        assert!(
            !report.contains("migrate") && !report.contains("cargo  "),
            "{report}"
        );
        assert!(
            report.contains("links pill_core, std  (+2 system)"),
            "{report}"
        );
        assert!(
            report.contains("deps  glam") && !report.contains("build_script_build"),
            "{report}"
        );
        let row_of = |name: &str| {
            report
                .lines()
                .find(|line| line.trim_start().starts_with(name) && line.contains("s "))
                .unwrap_or_else(|| panic!("no row for {name}: {report}"))
                .to_string()
        };
        assert_eq!(
            row_of("pill_master_renderer_data").len(),
            row_of("project ").len(),
            "rows differ in width: {report}"
        );
        assert!(
            report.contains("9.05s"),
            "the build total is the sum: {report}"
        );
        assert!(
            report.lines().all(|line| line == line.trim_end()),
            "a line ends in padding: {report}"
        );
        // One log block: the title first.
        assert!(
            report.starts_with("BUILD / LINK / HOT-RELOAD ANALYTICS · startup\n"),
            "{report}"
        );
    }

    /// A synthetic cargo `--timings` HTML matching the format this parser
    /// targets: `DURATION = N;`, a `UNIT_DATA` JSON array whose executed units
    /// carry non-zero `duration` seconds, and a trailing `];`.
    const FIXTURE_HTML: &str = r#"<html><head><title>Cargo Build Timings</title></head>
<body><script>
DURATION = 3;
const UNIT_DATA = [
  {
    "i": 1,
    "name": "pill_core",
    "version": "0.1.0",
    "mode": "todo",
    "target": "",
    "features": [],
    "start": 0.1,
    "duration": 1.2,
    "unblocked_units": [],
    "unblocked_rmeta_units": [],
    "sections": null
  },
  {
    "i": 2,
    "name": "project",
    "version": "0.1.0",
    "mode": "todo",
    "target": "",
    "features": [],
    "start": 1.26,
    "duration": 1.09,
    "unblocked_units": [],
    "unblocked_rmeta_units": [],
    "sections": null
  },
  {
    "i": 3,
    "name": "serde",
    "version": "1.0.229",
    "mode": "todo",
    "target": "",
    "features": [],
    "start": 0.0,
    "duration": 0.0,
    "unblocked_units": [],
    "unblocked_rmeta_units": [],
    "sections": null
  }
];
const CONCURRENCY_DATA = [
  {
    "t": 0.0,
    "active": 0,
    "waiting": 0,
    "inactive": 3
  }
];
</script></body></html>"#;

    /// The two slot routes give a promise the prologue route cannot, and the
    /// benchmark harness relies on exactly that difference.
    ///
    /// A slot install is one atomic pointer store, so any call that reaches the
    /// dispatcher runs the new code. The prologue route overwrites a live
    /// function's first bytes: a caller that inlined the body still runs the old
    /// one, and the twelve-byte write is not atomic. Reporting both as `patch`
    /// would make a benchmark average a guarantee with an approximation.
    #[test]
    fn only_the_slot_routes_are_provable() {
        assert!(PatchRoute::EngineSlot.is_provable());
        assert!(PatchRoute::ArtifactSlot.is_provable());
        assert!(!PatchRoute::Prologue.is_provable());
    }

    /// The labels are the harness's vocabulary, so they are pinned rather than
    /// left free to drift. The benchmark harness's line parser matches
    /// `route=([\w+-]+)` and accepts these joined by `+`; its
    /// `PROVABLE_ROUTES` names the two slot routes among them.
    #[test]
    fn route_labels_match_what_the_harness_parses() {
        assert_eq!(PatchRoute::EngineSlot.label(), "engine-slot");
        assert_eq!(PatchRoute::ArtifactSlot.label(), "artifact-slot");
        assert_eq!(PatchRoute::Prologue.label(), "prologue");
        // Every label must survive the harness's character class, or a route
        // silently stops being parsed.
        for route in [
            PatchRoute::EngineSlot,
            PatchRoute::ArtifactSlot,
            PatchRoute::Prologue,
        ] {
            assert!(
                route
                    .label()
                    .chars()
                    .all(|character| character.is_alphanumeric() || character == '-'),
                "`{}` would not match the harness regex",
                route.label()
            );
        }
    }

    /// A reload prints no route, because it has none: it replaces the whole
    /// artifact rather than redirecting any single function.
    #[test]
    fn a_reload_carries_no_route() {
        let event = ReloadEvent {
            kind: EventKind::Reload,
            route: None,
            name: "project".to_string(),
            build_ms: 100,
            stage_ms: 1.0,
            load_ms: 1.0,
            init_ms: 1.0,
            migrate_ms: 0.0,
            artifact_bytes: 1024,
            reload_count: 1,
            cargo_crates: Vec::new(),
        };
        assert!(event.route.is_none());
        assert_eq!(event.kind.label(), "reload");
    }

    /// A managed swap records one reload event with the loader's phases: no
    /// route, no cargo crates, and no artifact size to report.
    #[test]
    fn a_managed_swap_records_a_reload_event() {
        record_managed_reload("test_managed_project", 5.0, 120.4, 2.5);
        let collector = analytics()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let event = collector
            .pending_reload_events
            .iter()
            .find(|event| event.name == "test_managed_project")
            .expect("the swap recorded an event");

        assert_eq!(event.kind, EventKind::Reload);
        assert!(event.route.is_none(), "a reload has no route");
        assert_eq!(event.build_ms, 120, "the build is rounded to milliseconds");
        assert_eq!(event.stage_ms, 5.0);
        assert_eq!(event.load_ms, 2.5);
        assert_eq!(event.init_ms, 0.0, "the loader has no activation phase");
        assert_eq!(event.migrate_ms, 0.0, "the loader has no schema phase");
        assert_eq!(event.artifact_bytes, 0, "the assembly's size is unknown");
        assert!(event.cargo_crates.is_empty(), "no cargo invocation ran");
    }

    /// A managed swap with no timed build records zeros rather than a made-up
    /// number.
    #[test]
    fn an_untimed_managed_swap_records_zeros() {
        record_managed_reload("test_untimed_managed_project", 0.0, f64::NAN, 0.0);
        let collector = analytics()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let event = collector
            .pending_reload_events
            .iter()
            .find(|event| event.name == "test_untimed_managed_project")
            .expect("the swap recorded an event");

        assert_eq!(event.build_ms, 0, "a NaN build reports zero");
    }

    /// A patch and a reload in one session take consecutive numbers from one
    /// count, so `number=N` never names two events.
    #[test]
    fn patches_and_reloads_share_one_session_count() {
        record_patch(
            "test_module::patched_function",
            1.0,
            10,
            1.0,
            1.0,
            0,
            &[],
            0,
        );
        record_reload("test_module_counted");
        let collector = analytics()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let counts: Vec<u32> = collector
            .pending_reload_events
            .iter()
            .filter(|event| {
                event.name == "test_module::patched_function" || event.name == "test_module_counted"
            })
            .map(|event| event.reload_count)
            .collect();
        assert_eq!(counts.len(), 2);
        assert!(counts[1] > counts[0], "{counts:?}");
        let module = collector
            .modules
            .iter()
            .find(|module| module.name == "test_module_counted")
            .unwrap();
        assert_eq!(module.reloads, 1);
    }

    /// Build a throwaway workspace whose `cargo-timings` directory holds the
    /// fixture, and return the workspace root.
    fn fixture_workspace() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pill_analytics_test_{}_{unique}",
            std::process::id()
        ));
        let timings = root.join(cargo_timings_directory());
        std::fs::create_dir_all(&timings).unwrap();
        std::fs::write(
            timings.join("cargo-timing-20260822T000000000Z-deadbeef.html"),
            FIXTURE_HTML,
        )
        .unwrap();
        root
    }

    /// The parser extracts per-crate durations and the total from the HTML.
    #[test]
    fn parses_cargo_timings_fixture() {
        let root = fixture_workspace();
        let timing = parse_latest_cargo_timings(&root).expect("fixture must parse");

        assert_eq!(timing.total_seconds, 3.0);
        assert_eq!(timing.crate_durations_ms.get("pill_core"), Some(&1200));
        assert_eq!(timing.crate_durations_ms.get("project"), Some(&1090));
        // A unit that did not run (duration 0.0) is left out of the map.
        assert!(!timing.crate_durations_ms.contains_key("serde"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A workspace with no `cargo-timings` directory yields `None` instead of
    /// failing.
    #[test]
    fn missing_timings_directory_returns_none() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pill_analytics_none_test_{}_{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();

        assert!(parse_latest_cargo_timings(&root).is_none());

        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The parser reads a real report from this workspace without failing on
    /// it, and every crate it names carries a duration.
    ///
    /// Two states are skipped instead of asserted on, because neither says
    /// anything about the parser. The workspace may never have produced a
    /// report (a fresh checkout), and the newest report may come from a build
    /// where nothing needed recompiling: an up-to-date build still writes a
    /// full report, but every unit in it has `"duration": 0`, and the parser
    /// keeps only units that actually ran. Asserting a non-empty result would
    /// make this test depend on what the developer happened to build last.
    #[test]
    fn parses_real_workspace_timings_when_present() {
        // Two levels up from `hosting/pill_development_host`, same as the host's own
        // `engine_workspace_root`.
        let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap();
        let Some(timing) = parse_latest_cargo_timings(workspace_root) else {
            return;
        };
        if timing.crate_durations_ms.is_empty() {
            // A no-op build: a real report, correctly parsed, with nothing in it
            // that executed.
            return;
        }
        // What the parser promises about the entries it does return.
        for (crate_name, duration_ms) in &timing.crate_durations_ms {
            assert!(!crate_name.is_empty(), "a named unit must have a name");
            assert!(
                *duration_ms > 0,
                "`{crate_name}` was kept, so it must have a non-zero duration"
            );
        }
    }

    /// The import and export walks, against a real system image.
    ///
    /// `kernel32.dll` imports named functions from `KERNELBASE.dll` and
    /// exports thousands of names; a parser that read neither table would
    /// still pass every synthetic test that only checks the DLL-name list.
    /// Skipped where the file is not there (every non-Windows test machine).
    #[test]
    fn a_real_system_dll_yields_named_imports_and_exports() {
        let path = Path::new("C:/Windows/System32/kernel32.dll");
        if !path.is_file() {
            return;
        }
        let inspection = inspect_pe(path).expect("kernel32 parses as PE");
        assert!(
            !inspection.exports.is_empty(),
            "kernel32 exports thousands of names"
        );
        assert!(
            inspection.exports.iter().any(|name| name == "LoadLibraryW"),
            "a known export must be among them"
        );
        assert!(
            !inspection.imports.is_empty(),
            "kernel32 imports from kernelbase"
        );
        assert_eq!(
            inspection.import_dlls.len(),
            inspection.imports.len(),
            "the DLL-name list and the detailed imports walk the same table"
        );
        for import in &inspection.imports {
            assert!(
                import.dll.to_ascii_lowercase().ends_with(".dll"),
                "import names are file names: {}",
                import.dll
            );
        }
        assert!(
            inspection
                .imports
                .iter()
                .any(|import| !import.symbols.is_empty()),
            "at least one import carries named symbols"
        );
    }

    /// A truncated optional header must degrade the report, not panic.
    ///
    /// The guard requires the optional header to declare at least the 60 fixed
    /// bytes the parse reads - the deepest is `SizeOfImage` at offset 56 - so a
    /// header declaring less is refused before any field is read, instead of
    /// being trusted and indexing past the buffer.
    #[test]
    fn a_truncated_optional_header_degrades_to_none() {
        // 132 bytes: enough for the COFF header and 40 optional-header bytes
        // starting at 0x58, not enough for the `SizeOfImage` field at 0x90.
        for declared_size in [0u16, 40] {
            let mut image = vec![0u8; 132];
            image[0..2].copy_from_slice(b"MZ");
            image[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
            image[0x40..0x44].copy_from_slice(b"PE\0\0");
            let coff_header_offset = 0x44usize;
            image[coff_header_offset + 2..coff_header_offset + 4]
                .copy_from_slice(&1u16.to_le_bytes());
            image[coff_header_offset + 16..coff_header_offset + 18]
                .copy_from_slice(&declared_size.to_le_bytes());
            // A valid PE32 magic, so the parse would reach the `SizeOfImage`
            // read if the guard did not stop it first.
            image[0x58..0x5a].copy_from_slice(&0x10bu16.to_le_bytes());

            let path = std::env::temp_dir().join(format!(
                "pill_pe_truncated_{}_{declared_size}.dll",
                std::process::id()
            ));
            std::fs::write(&path, &image).unwrap();
            assert!(
                inspect_pe(&path).is_none(),
                "a {declared_size}-byte optional header must degrade to None"
            );
            let _ = std::fs::remove_file(&path);
        }
    }

    /// The dependency reader extracts names from the fingerprint's
    /// array-of-arrays `deps` field.
    #[test]
    fn reads_cargo_deps_from_fingerprint() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "pill_analytics_deps_test_{}_{unique}",
            std::process::id()
        ));
        let fingerprint = root
            .join(cargo_fingerprint_directory())
            .join("pill_dummy_math-0123456789abcdef");
        assert!(
            cargo_fingerprint_directory().contains("target/hot/build"),
            "fingerprints live in the private build tree the host actually fills"
        );
        std::fs::create_dir_all(&fingerprint).unwrap();
        std::fs::write(
            fingerprint.join("lib-pill_dummy_math.json"),
            r#"{
                "rustc": 1,
                "features": "[]",
                "declared_features": "[]",
                "target": 1,
                "profile": 1,
                "path": 1,
                "deps": [
                    [11718648107452275942, "pill_engine", false, 17195598676378470079],
                    [12107043699172518021, "pill_core", false, 15008079515306957073]
                ],
                "local": [],
                "rustflags": [],
                "config": 1,
                "compile_kind": 0
            }"#,
        )
        .unwrap();

        let deps = read_cargo_deps(&root, "pill_dummy_math");
        assert_eq!(
            deps,
            vec!["pill_engine".to_string(), "pill_core".to_string()]
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
