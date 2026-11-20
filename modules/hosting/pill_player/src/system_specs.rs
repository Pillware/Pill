//! The start-up report of the machine the engine runs on.
//!
//! # Responsibilities
//!
//! - Reports CPU, memory, storage and OS through the telemetry stack when a
//!   [`crate::Runtime`] is created.
//!
//! # Design
//!
//! The report lives in the player, which is linked into executables, and not
//! in the engine core. `sysinfo` reaches the system through the `windows`
//! crate, whose functions are `raw-dylib` imports naming about seventy system
//! DLLs (rustc creates the import library for each DLL when it links). A
//! shared `pill_engine_core.dll` that carried those imports would make every
//! hot patch generate an import library for each DLL.

// External crates
use pill_core::info;
use pill_core::telemetry::log_block;
use pill_core::telemetry::telemetry_target;
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System};

// =============================================================================
// Constants
// =============================================================================

/// Bytes in one gibibyte, the unit every memory and storage figure is in.
const BYTES_PER_GIBIBYTE: f64 = 1024.0 * 1024.0 * 1024.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Reports the detected hardware through the telemetry stack.
///
/// The report has the CPU brand and its physical core and logical thread
/// counts, RAM and swap in GiB, one line per disk, the OS name and version
/// with the build architecture, and the system uptime. It ends with a line
/// that leaves the GPU to external tools.
///
/// It goes out as one multi-line event under `telemetry_target::SYSTEM`, so
/// it can be filtered or redirected like any other engine output, and its
/// box-drawing layout stays in one block.
///
/// [`Runtime::new`](crate::Runtime::new) calls it before it creates the
/// engine.
pub(crate) fn report_machine_specs() {
    // Step 1: Read memory, swap and CPU identity into a fresh snapshot.
    //
    // Refresh only what the report reads. `System::new_all()` would also
    // list every process on the machine, which nothing here uses, and that
    // work would run in the middle of start-up. `CpuRefreshKind::nothing()`
    // still lists the CPUs, so the brand and the counts are filled in, but it
    // skips the usage and frequency refreshes; on Windows the usage refresh is
    // the one that opens the performance-data query.
    let system = System::new_with_specifics(
        RefreshKind::nothing()
            .with_memory(MemoryRefreshKind::everything())
            .with_cpu(CpuRefreshKind::nothing()),
    );
    let disks = Disks::new_with_refreshed_list();

    let total_memory_gib = system.total_memory() as f64 / BYTES_PER_GIBIBYTE;
    let used_memory_gib =
        (system.total_memory() - system.available_memory()) as f64 / BYTES_PER_GIBIBYTE;
    let memory_usage_percent = percent(used_memory_gib, total_memory_gib);

    let total_swap_gib = system.total_swap() as f64 / BYTES_PER_GIBIBYTE;
    let used_swap_gib = system.used_swap() as f64 / BYTES_PER_GIBIBYTE;
    let swap_usage_percent = percent(used_swap_gib, total_swap_gib);

    let cpu_brand = system
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let physical_core_count = sysinfo::System::physical_core_count().unwrap_or(0);
    let logical_thread_count = system.cpus().len();

    let uptime_seconds = System::uptime();
    let uptime_text = format_uptime(uptime_seconds);

    // Step 2: Emit the hardware report as one multi-line event.
    let mut lines = vec![
        format!("├─ CPU: {cpu_brand}"),
        format!(
            "│  └─ Cores: {physical_core_count} physical, {logical_thread_count} logical threads"
        ),
        "├─ Memory".to_string(),
        format!(
            "│  ├─ RAM: {used_memory_gib:.1} / {total_memory_gib:.1} GiB used ({memory_usage_percent:.0}%)"
        ),
    ];
    if total_swap_gib > 0.0 {
        lines.push(format!(
            "│  └─ Swap: {used_swap_gib:.1} / {total_swap_gib:.1} GiB used ({swap_usage_percent:.0}%)"
        ));
    } else {
        lines.push("│  └─ Swap: none".to_string());
    }
    lines.extend(storage_report_lines(&disks));
    lines.push(format!("├─ OS: {}", os_pretty_name()));
    lines.push(format!("│  └─ Uptime: {uptime_text}"));
    lines.push("└─ GPU: use external tools (dxdiag / lspci)".to_string());
    info!(
        target: telemetry_target::SYSTEM,
        "{}",
        log_block("System specs", lines)
    );
}

/// `used` as a percentage of `total`, or `0.0` when there is nothing to divide
/// by: a machine that reports no swap, and a disk that reports no size.
fn percent(used: f64, total: f64) -> f64 {
    if total > 0.0 {
        (used / total) * 100.0
    } else {
        0.0
    }
}

/// Builds the `Storage` branch of the system report: one line per detected
/// storage device, or a single "no disks detected" line when the list is
/// empty.
///
/// Each device line shows the mount point, used and total space in GiB, the
/// usage percentage, and the disk kind: SSD, HDD, or `?` when the platform
/// does not say.
fn storage_report_lines(disks: &sysinfo::Disks) -> Vec<String> {
    if disks.is_empty() {
        return vec!["├─ Storage: no disks detected".to_string()];
    }
    let mut lines = vec!["├─ Storage".to_string()];
    let disk_count = disks.len();
    for (index, disk) in disks.iter().enumerate() {
        let total_space_gib = disk.total_space() as f64 / BYTES_PER_GIBIBYTE;
        let available_space_gib = disk.available_space() as f64 / BYTES_PER_GIBIBYTE;
        let used_space_gib = total_space_gib - available_space_gib;
        let used_percent = percent(used_space_gib, total_space_gib);
        let disk_kind = match disk.kind() {
            sysinfo::DiskKind::SSD => "SSD",
            sysinfo::DiskKind::HDD => "HDD",
            _ => "?",
        };
        // Device names can be long, so the line shows the mount point instead.
        let mount_point = disk.mount_point().to_string_lossy();
        let branch_glyph = if index == disk_count - 1 {
            "└─"
        } else {
            "├─"
        };
        lines.push(format!(
            "│  {branch_glyph} {mount_point}  {used_space_gib:.0} / {total_space_gib:.0} GiB used ({used_percent:.0}%)  [{disk_kind}]"
        ));
    }
    lines
}

/// Formats a duration in whole seconds as a compact `Nd Nh Nm` string.
///
/// The days part is dropped when it is zero, and the hours part when both
/// days and hours are zero. Zero seconds reads `"unknown"`, which is what a
/// platform that cannot report uptime returns.
fn format_uptime(seconds: u64) -> String {
    if seconds == 0 {
        return "unknown".to_string();
    }
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    if days > 0 {
        format!("{}d {}h {}m", days, hours, minutes)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

/// Returns the OS name and version with the build architecture.
///
/// A platform that reports no name gives `unknown`; one that reports no
/// version just leaves it out.
fn os_pretty_name() -> String {
    let name = sysinfo::System::name().unwrap_or_else(|| "unknown".to_string());
    let version = sysinfo::System::os_version().unwrap_or_default();
    let architecture = std::env::consts::ARCH;
    if version.is_empty() {
        format!("{name} ({architecture})")
    } else {
        format!("{name} {version} ({architecture})")
    }
}
