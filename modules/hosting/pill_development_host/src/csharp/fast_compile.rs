//! In-process Roslyn compilation of the C# project, for hot reload.
//!
//! # Responsibilities
//!
//! - Build the managed compiler assembly and load it into the running runtime.
//! - Replay the compiler command line the startup build captured.
//! - Decide when the capture no longer matches the project and a full build is
//!   needed.
//!
//! # Design
//!
//! A `dotnet build` of a one-file gameplay edit takes seconds, while the C#
//! compilation inside it takes tens of milliseconds. The rest of the time goes
//! to starting a process, evaluating MSBuild, running a restore pass and walking
//! the project reference graph, all to recompute an answer that has not changed
//! since the host started. This module skips all of it. The project's startup
//! build reports the exact `csc` invocation it used, and a reload replays that
//! invocation through Roslyn inside the host process. Replaying the capture is
//! what keeps the fast path faithful: the references, defines, language version,
//! analyzers and source list are the ones MSBuild chose.
//!
//! Before a capture is replayed, it is checked against the project's source
//! files. A file added or removed under the watched source directory changes
//! the list MSBuild computed, so that case goes through a full build instead.
//! The full build also refreshes the capture, so the reload after it is fast
//! again.

// Standard library
use std::ffi::CString;
use std::path::{Path, PathBuf};

// External crates
use pill_core::error::CSharpError;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use pill_csharp_bridge::{fetch_managed_buffer, DotnetRuntimeContext};

// Current crate
use crate::config::{
    CSHARP_COMPILER_ARGUMENTS_FILE, CSHARP_COMPILER_ASSEMBLY_NAME,
    CSHARP_COMPILER_OUTPUT_SUBDIRECTORY,
};
use crate::CSharpModuleConfig;

// =============================================================================
// Constants
// =============================================================================

/// Export-contract version this host requires of the compiler assembly.
///
/// It is checked before any other export is used, as the managed runtime's
/// interop version is. A compiler assembly left over from an older checkout
/// would otherwise be called through signatures it no longer has.
const COMPILER_ABI_VERSION: u32 = 1;

/// Managed type holding the compiler's unmanaged exports.
const COMPILER_INTEROP_TYPE: &str = "TracyLive.Compiler.CompilerInterop, pill_csharp_compiler";

/// `CompilerInterop.Compile` returned: the assembly was written.
const COMPILE_STATUS_COMPILED: i32 = 0;

/// `CompilerInterop.Compile` returned: the project has errors.
const COMPILE_STATUS_FAILED: i32 = 1;

/// Upper bound for one compile's diagnostics text.
///
/// A failing build reports a few kilobytes, so no real diagnostic set reaches
/// this cap. The cap limits how much memory a corrupt length can make the host
/// reserve: the helper that fetches the text rejects a length above it before
/// allocating anything.
const MAX_DIAGNOSTICS_BYTES: u32 = 4 * 1024 * 1024;

// =============================================================================
// Managed Export Signatures
// =============================================================================

/// `uint CompilerAbiVersion()`.
type CompilerAbiVersionFn = extern "system" fn() -> u32;

/// `byte Warmup(byte* responseFilePath)`.
type WarmupFn = extern "system" fn(*const u8) -> u8;

/// `int Compile(byte* responseFilePath, byte* outputAssemblyPath)`.
type CompileFn = extern "system" fn(*const u8, *const u8) -> i32;

/// `uint DiagnosticsLength()`.
type DiagnosticsLengthFn = extern "system" fn() -> u32;

/// `byte CopyDiagnostics(byte* output, uint capacity)`.
type CopyDiagnosticsFn = extern "system" fn(*mut u8, u32) -> u8;

// =============================================================================
// FastCompileOutcome
// =============================================================================

/// What one in-process compile attempt produced.
pub(crate) enum FastCompileOutcome {
    /// The project assembly was recompiled and written.
    Compiled {
        /// Wall-clock cost of the compile, for the reload log.
        milliseconds: f64,
    },
    /// The project does not compile; the diagnostics say why.
    Failed {
        /// Compiler and analyzer errors, already formatted the way csc prints them.
        diagnostics: String,
    },
    /// The fast path cannot answer this reload; the caller must build normally.
    Unavailable {
        /// Why, so the fallback is never silent.
        reason: String,
    },
}

// =============================================================================
// FastCompiler
// =============================================================================

/// The loaded managed compiler, bound to one project's captured command line.
pub(crate) struct FastCompiler {
    /// `Compile` export.
    compile: CompileFn,
    /// `DiagnosticsLength` export.
    diagnostics_length: DiagnosticsLengthFn,
    /// `CopyDiagnostics` export.
    copy_diagnostics: CopyDiagnosticsFn,
    /// Absolute path of the file holding the captured compiler arguments.
    response_file_path: PathBuf,
    /// Absolute path the recompiled project assembly is written to.
    output_assembly_path: PathBuf,
}

impl FastCompiler {
    /// Build the compiler assembly, load it, and start its warmup.
    ///
    /// Returns `None` when the fast path cannot be set up: there is no captured
    /// command line to replay (a project shape that never hot reloads), the
    /// compiler build fails, or the assembly is missing an export or implements
    /// another ABI version. Build and load failures are each logged with one
    /// warning; either way the host stays on the ordinary `dotnet build` path,
    /// so a broken fast path costs reload speed and nothing else.
    pub(crate) fn try_new(
        runtime: &DotnetRuntimeContext,
        workspace_root: &Path,
        config: &CSharpModuleConfig,
    ) -> Option<Self> {
        // Step 1: Find the capture file and the path the output assembly is
        // written to. A shipping layout has no project `obj` directory, so it
        // has no capture file; that layout never hot reloads.
        let response_file_path = workspace_root.join(compiler_arguments_file(config)?);
        let output_assembly_path = workspace_root
            .join(&config.project_output_subdirectory)
            .join(format!("{}.dll", config.project_assembly_name));

        // Step 2: Build the compiler (MSBuild rebuilds it only when its sources
        // changed), then find the assembly it wrote. It is built here rather
        // than by the project's own build because nothing references it:
        // keeping Roslyn out of every managed project's reference graph is what
        // keeps it out of the shipping bundle.
        if let Err(error) = crate::build_runner::build_csharp_compiler(workspace_root) {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                error = %error,
                "could not build the in-process C# compiler; hot reload will run a full build each time"
            );
            return None;
        }
        let compiler_assembly_path = workspace_root
            .join(CSHARP_COMPILER_OUTPUT_SUBDIRECTORY)
            .join(format!("{CSHARP_COMPILER_ASSEMBLY_NAME}.dll"));

        // Step 3: Bind the exports, checking the contract version first so an
        // assembly from another compiler version is refused before its other
        // exports are resolved or called.
        let compiler = match Self::resolve(
            runtime,
            &compiler_assembly_path,
            response_file_path,
            output_assembly_path,
        ) {
            Ok(compiler) => compiler,
            Err(error) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    error = %error,
                    "could not load the in-process C# compiler; hot reload will run a full build each time"
                );
                return None;
            }
        };

        // Step 4: Ask the compiler to warm up. `Warmup` queues the work and
        // returns at once, so Roslyn's JIT, the reference set and the analyzers
        // are prepared in the background while the host is still starting. That
        // keeps the cost away from the developer's first edit.
        compiler.start_warmup(runtime, &compiler_assembly_path);
        info!(
            target: telemetry_target::HOT_RELOAD,
            "in-process C# compiler ready; reloads will skip MSBuild"
        );
        Some(compiler)
    }

    /// Resolve every export and check the contract version.
    ///
    /// # Errors
    ///
    /// Returns the export-resolution errors of
    /// [`DotnetRuntimeContext::get_unmanaged_fn`] when the assembly lacks one of
    /// the exports above, and [`CSharpError::InteropVersionMismatch`] when the
    /// assembly implements a different contract than this host was built
    /// against.
    fn resolve(
        runtime: &DotnetRuntimeContext,
        compiler_assembly_path: &Path,
        response_file_path: PathBuf,
        output_assembly_path: PathBuf,
    ) -> Result<Self, CSharpError> {
        let abi_version = runtime.resolve_unmanaged_function_pointer::<CompilerAbiVersionFn>(
            compiler_assembly_path,
            COMPILER_INTEROP_TYPE,
            "CompilerAbiVersion",
        )?;
        if abi_version() != COMPILER_ABI_VERSION {
            return Err(CSharpError::InteropVersionMismatch {
                expected: COMPILER_ABI_VERSION,
                actual: abi_version(),
            });
        }
        Ok(Self {
            compile: runtime.resolve_unmanaged_function_pointer::<CompileFn>(
                compiler_assembly_path,
                COMPILER_INTEROP_TYPE,
                "Compile",
            )?,
            diagnostics_length: runtime.resolve_unmanaged_function_pointer::<DiagnosticsLengthFn>(
                compiler_assembly_path,
                COMPILER_INTEROP_TYPE,
                "DiagnosticsLength",
            )?,
            copy_diagnostics: runtime.resolve_unmanaged_function_pointer::<CopyDiagnosticsFn>(
                compiler_assembly_path,
                COMPILER_INTEROP_TYPE,
                "CopyDiagnostics",
            )?,
            response_file_path,
            output_assembly_path,
        })
    }

    /// Ask the compiler to prepare its caches on a background thread.
    ///
    /// This is best effort: if the export is missing or the request cannot be
    /// made, the first reload simply pays the full Roslyn startup cost.
    fn start_warmup(&self, runtime: &DotnetRuntimeContext, compiler_assembly_path: &Path) {
        let Ok(warmup) = runtime.resolve_unmanaged_function_pointer::<WarmupFn>(
            compiler_assembly_path,
            COMPILER_INTEROP_TYPE,
            "Warmup",
        ) else {
            return;
        };
        let Some(encoded_response_file_path) = encode_path(&self.response_file_path) else {
            return;
        };
        warmup(encoded_response_file_path.as_ptr().cast());
    }

    /// Recompile the project in-process, or say why that was not possible.
    ///
    /// `watch_directory` is the project's source directory, relative to the
    /// workspace root. The captured source list is checked against it.
    pub(crate) fn compile(
        &self,
        workspace_root: &Path,
        watch_directory: &str,
    ) -> FastCompileOutcome {
        // Step 1: Reject a capture that cannot be replayed as it is. A file
        // added or removed under the watched directory changes the source list
        // MSBuild computed, so that case goes through MSBuild instead - which
        // also refreshes the capture, so the reload after it is fast again.
        if !self.response_file_path.is_file() {
            return FastCompileOutcome::Unavailable {
                reason: format!(
                    "no captured compiler arguments at {}",
                    self.response_file_path.display()
                ),
            };
        }
        if let Some(reason) =
            sources_changed_since_capture(&self.response_file_path, workspace_root, watch_directory)
        {
            return FastCompileOutcome::Unavailable { reason };
        }

        // Step 2: Encode both paths as NUL-terminated UTF-8, which is what the
        // managed entry point reads.
        let (Some(encoded_response_file_path), Some(encoded_output_assembly_path)) = (
            encode_path(&self.response_file_path),
            encode_path(&self.output_assembly_path),
        ) else {
            return FastCompileOutcome::Unavailable {
                reason: "a compiler path contains an interior NUL".to_string(),
            };
        };

        // Step 3: Run the compile and time it for the reload log.
        let started_at = Instant::now();
        let status = (self.compile)(
            encoded_response_file_path.as_ptr().cast(),
            encoded_output_assembly_path.as_ptr().cast(),
        );
        let milliseconds = started_at.elapsed().as_secs_f64() * 1000.0;

        // Step 4: Turn the status into an outcome. The managed side uses two
        // for "the fast path could not run", so any status other than
        // `COMPILE_STATUS_COMPILED` or `COMPILE_STATUS_FAILED` is a fallback
        // rather than a fault in the gameplay source.
        match status {
            COMPILE_STATUS_COMPILED => FastCompileOutcome::Compiled { milliseconds },
            COMPILE_STATUS_FAILED => FastCompileOutcome::Failed {
                diagnostics: self.diagnostics(),
            },
            _ => FastCompileOutcome::Unavailable {
                reason: self.diagnostics(),
            },
        }
    }

    /// Read back the last compile's diagnostics as UTF-8.
    ///
    /// A compile with nothing to report has no diagnostics, which is an empty
    /// string rather than a failure. Any other problem reading them is returned
    /// as text: the caller is already handling a failure and has nowhere better
    /// to put an error.
    fn diagnostics(&self) -> String {
        if (self.diagnostics_length)() == 0 {
            return String::new();
        }
        match fetch_managed_buffer(
            || (self.diagnostics_length)(),
            |pointer, length| (self.copy_diagnostics)(pointer, length),
            MAX_DIAGNOSTICS_BYTES,
        ) {
            Ok(buffer) => String::from_utf8_lossy(&buffer).into_owned(),
            Err(_) => "the compiler reported diagnostics that could not be read".to_string(),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Workspace-relative path of the captured compiler command line for `config`.
///
/// The project's build writes the capture into the project's `obj` directory,
/// so the path is worked out from the output directory instead of stored. The
/// reloading host sets that directory to
/// `<project>/bin/<configuration>/<framework>`, so the project root is the
/// directory three levels above it.
///
/// Returns `None` when the output directory has no such shape. That is the
/// shipping layout, where the assembly sits beside the executable. It never
/// hot reloads, so it never needs a capture.
fn compiler_arguments_file(config: &CSharpModuleConfig) -> Option<PathBuf> {
    let output_directory = Path::new(&config.project_output_subdirectory);
    let project_root = output_directory.ancestors().nth(3)?;
    if project_root.as_os_str().is_empty() {
        return None;
    }
    Some(
        project_root
            .join("obj")
            .join(CSHARP_COMPILER_ARGUMENTS_FILE),
    )
}

/// Encode a path as NUL-terminated UTF-8 for the managed entry points.
///
/// Returns `None` when the path contains a NUL byte inside it, because such a
/// path cannot be passed across the boundary at all.
fn encode_path(path: &Path) -> Option<CString> {
    CString::new(path.to_string_lossy().into_owned()).ok()
}

/// Report whether the captured source list still matches the project.
///
/// Returns `None` when the two agree, or a sentence saying why they do not.
/// Only files under the watched source directory are compared. The capture also
/// lists generated files from other directories, such as the C# code generated
/// for each module and the files the .NET SDK writes, and those change only as
/// part of work that rebuilds the project anyway.
fn sources_changed_since_capture(
    response_file_path: &Path,
    workspace_root: &Path,
    watch_directory: &str,
) -> Option<String> {
    let Ok(watch_root) = std::fs::canonicalize(workspace_root.join(watch_directory)) else {
        // The source directory cannot be read, so leave the decision to the
        // build: it reports the real problem.
        return Some(format!(
            "cannot read the source directory {watch_directory}"
        ));
    };
    // MSBuild wrote the capture into the project's `obj` directory and resolved
    // relative source paths against the project root, which is its parent.
    let project_root = response_file_path.parent()?.parent()?;

    let Ok(contents) = std::fs::read_to_string(response_file_path) else {
        return Some("the captured compiler arguments could not be read".to_string());
    };
    let mut captured_source_paths: Vec<PathBuf> = Vec::new();
    for line in contents.lines() {
        let line = line.trim().trim_matches('"');
        // Everything that is not a switch is a source file path.
        if line.is_empty() || line.starts_with('/') || line.starts_with('-') {
            continue;
        }
        let Ok(resolved) = std::fs::canonicalize(project_root.join(line)) else {
            continue;
        };
        if resolved.starts_with(&watch_root) {
            captured_source_paths.push(resolved);
        }
    }

    let mut present_source_paths = Vec::new();
    collect_csharp_sources(&watch_root, &mut present_source_paths);
    captured_source_paths.sort();
    present_source_paths.sort();
    if captured_source_paths == present_source_paths {
        return None;
    }
    Some(format!(
        "the project's source file set changed ({} captured, {} on disk)",
        captured_source_paths.len(),
        present_source_paths.len()
    ))
}

/// Collect every `.cs` file under one directory, recursively.
fn collect_csharp_sources(directory: &Path, found_source_paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_csharp_sources(&path, found_source_paths);
        } else if path.extension().is_some_and(|extension| extension == "cs") {
            if let Ok(resolved) = std::fs::canonicalize(&path) {
                found_source_paths.push(resolved);
            }
        }
    }
}
