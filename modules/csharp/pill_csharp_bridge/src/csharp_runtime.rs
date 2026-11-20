//! Loads the .NET hosting library (`hostfxr`) and drives it for the C# project
//! backend.
//!
//! # Responsibilities
//!
//! - Finds the newest installed `hostfxr` library.
//! - Starts .NET from a managed runtime configuration file.
//! - Turns methods marked `UnmanagedCallersOnly` into native function pointers.
//! - Keeps the native hosting library and the runtime context alive.
//!
//! # Design
//!
//! This module knows nothing about the ECS. It is a thin layer over the
//! `hostfxr` ABI and hands typed function pointers back to the managed backend
//! registration code.
//!
//! The file is named `csharp_runtime.rs`, but the library and export names come
//! from Microsoft (`hostfxr`), so they are not renamed.

// Standard library
use std::path::{Path, PathBuf};

// External crates
use libloading::Library;
use pill_core::error::CSharpError;

// =============================================================================
// Constants
// =============================================================================

/// The `hostfxr_delegate_type::hdt_load_assembly_and_get_function_pointer`
/// delegate kind, whose value in the `hostfxr` header is 5.
///
/// Asking for this kind gives a delegate that both loads a managed assembly
/// and returns one method from it as a native function pointer.
const LOAD_ASSEMBLY_AND_GET_FUNCTION_POINTER: i32 = 5;

// =============================================================================
// hostfxr ABI Types
// =============================================================================

/// The character type `hostfxr` expects in strings on this platform.
///
/// Windows `hostfxr` takes UTF-16 code units; Unix `hostfxr` takes the raw
/// bytes of the string. The alias follows the platform, so the exported
/// signatures below work on both.
#[cfg(windows)]
type HostChar = u16;
#[cfg(not(windows))]
type HostChar = u8;

/// An opaque handle to one initialized `hostfxr` runtime context.
///
/// `hostfxr_initialize_for_runtime_config` creates it and `hostfxr_close`
/// releases it. The Rust side only passes it back to `hostfxr`; it never looks
/// inside.
type RuntimeHandle = *mut std::ffi::c_void;

/// Signature of the `hostfxr_initialize_for_runtime_config` export.
///
/// Starts a .NET runtime from a `runtimeconfig.json` file and writes the new
/// context handle to its last argument. It returns `0` on success or a
/// negative status code.
type InitializeFn =
    unsafe extern "system" fn(*const HostChar, *const std::ffi::c_void, *mut RuntimeHandle) -> i32;

/// Signature of the `hostfxr_get_runtime_delegate` export.
///
/// Asks a live context for a runtime delegate of the requested kind and writes
/// an untyped pointer to it.
type GetDelegateFn =
    unsafe extern "system" fn(RuntimeHandle, i32, *mut *mut std::ffi::c_void) -> i32;

/// Signature of the `hostfxr_close` export.
///
/// Closes an initialized runtime context.
type CloseFn = unsafe extern "system" fn(RuntimeHandle) -> i32;

/// Signature of the `load_assembly_and_get_function_pointer` runtime delegate.
///
/// Loads a managed assembly and resolves one `UnmanagedCallersOnly` static
/// method into a native entry-point pointer.
type LoadAssemblyFn = unsafe extern "system" fn(
    *const HostChar,
    *const HostChar,
    *const HostChar,
    *const HostChar,
    *const std::ffi::c_void,
    *mut *const std::ffi::c_void,
) -> i32;

// =============================================================================
// DotnetRuntimeContext
// =============================================================================

/// Owns one initialized .NET runtime context and the delegate that loads
/// managed assemblies.
///
/// The native library stays loaded for as long as the context lives: `close`
/// and `load_assembly` point into that library's code, so unloading it first
/// would leave them dangling.
pub struct DotnetRuntimeContext {
    /// The `hostfxr` library, kept loaded so the function pointers below stay
    /// valid.
    _library: Library,
    /// The context handle that runtime initialization returned.
    runtime_handle: RuntimeHandle,
    /// The `hostfxr_close` export; `Drop` calls it to release the context.
    close: CloseFn,
    /// The `load_assembly_and_get_function_pointer` delegate that
    /// `resolve_unmanaged_function_pointer` calls.
    load_assembly: LoadAssemblyFn,
}

impl DotnetRuntimeContext {
    /// Loads `hostfxr`, starts .NET from the runtime configuration file at
    /// `runtime_config`, and gets the runtime delegate named
    /// `load_assembly_and_get_function_pointer`.
    ///
    /// `runtime_config` is the runtime configuration file of the managed
    /// runtime assembly: `hostfxr` reads the target framework and its version
    /// from it.
    ///
    /// # Errors
    ///
    /// - [`CSharpError::HostfxrOverrideNotAFile`]: `ECS_DOTNET_HOSTFXR` is set
    ///   but does not name a file.
    /// - [`CSharpError::HostfxrNotFound`]: no `host/fxr` version directory
    ///   exists under any configured .NET root.
    /// - [`CSharpError::HostfxrLoadFailed`]: the chosen library cannot be
    ///   loaded.
    /// - [`CSharpError::HostfxrExportMissing`]: the library lacks one of the
    ///   three bootstrap exports.
    /// - [`CSharpError::InteriorNul`]: the configuration path contains a NUL
    ///   byte inside it.
    /// - [`CSharpError::RuntimeInitializationFailed`]: `hostfxr` refuses to
    ///   start the runtime, or returns no context handle.
    /// - [`CSharpError::RuntimeDelegateFailed`]: `hostfxr` refuses the delegate
    ///   request, or returns no delegate.
    pub fn new(runtime_config: &Path) -> Result<Self, CSharpError> {
        // Step 1: Find and load the newest `hostfxr` library installed on this
        // machine. Looking it up at run time instead of linking against one SDK
        // installation lets the host follow the machine's configured .NET root
        // and use whichever hosting library is newest.
        let library_path = find_hostfxr_library_path()?;

        // Keep the library inside `DotnetRuntimeContext` for as long as any
        // copied export or delegate can be called. Dropping it earlier would
        // leave every function pointer resolved below pointing at unmapped
        // memory.
        // SAFETY: the path comes from the `ECS_DOTNET_HOSTFXR` override or from
        // a configured .NET installation root, never from project data, so this
        // loads the .NET host the user or the machine selected. A path that is
        // not a loadable library makes `Library::new` return an error.
        let library = unsafe {
            Library::new(&library_path).map_err(|source| CSharpError::HostfxrLoadFailed {
                path: library_path.display().to_string(),
                source: Box::new(source),
            })?
        };

        // Step 2: Copy the three bootstrap exports out of the temporary
        // `Symbol` wrappers that `libloading` returns. The pointers stay valid
        // only while `library` lives, and `library` is moved into the returned
        // context, so it outlives them.
        // SAFETY: each name is an export that `hostfxr` declares, and the type
        // alias it is read as repeats the signature `hostfxr` gives it.
        let initialize: InitializeFn = unsafe {
            *library
                .get(b"hostfxr_initialize_for_runtime_config")
                .map_err(|source| CSharpError::HostfxrExportMissing {
                    symbol: "hostfxr_initialize_for_runtime_config".to_string(),
                    source: Box::new(source),
                })?
        };
        // SAFETY: `hostfxr_get_runtime_delegate` is exported with this
        // signature.
        let get_delegate: GetDelegateFn = unsafe {
            *library
                .get(b"hostfxr_get_runtime_delegate")
                .map_err(|source| CSharpError::HostfxrExportMissing {
                    symbol: "hostfxr_get_runtime_delegate".to_string(),
                    source: Box::new(source),
                })?
        };
        // SAFETY: `hostfxr_close` is exported with this signature.
        let close: CloseFn = unsafe {
            *library
                .get(b"hostfxr_close")
                .map_err(|source| CSharpError::HostfxrExportMissing {
                    symbol: "hostfxr_close".to_string(),
                    source: Box::new(source),
                })?
        };

        // Step 3: Start a .NET runtime from the runtime configuration file,
        // which names the target framework and its version. The path has to be
        // encoded the way `hostfxr` expects on this platform: UTF-16 on
        // Windows, raw bytes on Unix.
        let encoded_runtime_config_path = host_string(runtime_config.as_os_str())?;
        let mut runtime_handle = std::ptr::null_mut();

        // Passing null parameters leaves host and dotnet-root resolution to
        // `hostfxr`'s defaults. The handle it returns owns the new context.
        // SAFETY: `encoded_runtime_config_path` ends with a nul, and
        // `runtime_handle` points at writable storage that stays valid for the
        // whole call.
        let status = unsafe {
            initialize(
                encoded_runtime_config_path.as_ptr(),
                std::ptr::null(),
                &mut runtime_handle,
            )
        };

        // `hostfxr` reports failure as a negative status code. A success that
        // still returns no context handle is also an error, because nothing can
        // be done with it.
        if status < 0 || runtime_handle.is_null() {
            return Err(CSharpError::RuntimeInitializationFailed {
                status: format!("0x{status:08X}"),
            });
        }

        // Step 4: Get the load-assembly delegate from the running runtime.
        // Delegate kind 5 (`LOAD_ASSEMBLY_AND_GET_FUNCTION_POINTER`) both loads
        // a managed assembly and returns one static method from it as a native
        // pointer.
        let mut delegate_pointer = std::ptr::null_mut();
        // SAFETY: `runtime_handle` came from a successful initialization, so it
        // is a live context.
        let status = unsafe {
            get_delegate(
                runtime_handle,
                LOAD_ASSEMBLY_AND_GET_FUNCTION_POINTER,
                &mut delegate_pointer,
            )
        };
        if status < 0 || delegate_pointer.is_null() {
            // Initialization succeeded, so this path still owns the context and
            // must release it: `Self` is never built, so `Drop` cannot run.
            // SAFETY: `runtime_handle` is the live context that initialization
            // returned.
            unsafe { close(runtime_handle) };
            return Err(CSharpError::RuntimeDelegateFailed {
                status: format!("0x{status:08X}"),
            });
        }

        // Step 5: Turn the untyped delegate pointer into a typed function
        // pointer. `hostfxr` hands back a bare pointer, and the delegate kind
        // asked for in Step 4 says which signature it has.
        // SAFETY: kind 5 is the delegate whose signature `LoadAssemblyFn`
        // repeats.
        let load_assembly = unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, LoadAssemblyFn>(delegate_pointer)
        };
        Ok(Self {
            _library: library,
            runtime_handle,
            close,
            load_assembly,
        })
    }

    /// Turns an `UnmanagedCallersOnly` static method in a managed assembly into
    /// a native function pointer.
    ///
    /// `assembly` is the managed assembly file. `type_name` is the
    /// assembly-qualified type name (`Namespace.Type, Assembly`), and
    /// `method_name` is the name of the method in it. `T` must be the exact
    /// native signature the managed method declares; the pointer is copied into
    /// a `T` without any check of the signature.
    ///
    /// # Errors
    ///
    /// - [`CSharpError::ManagedExportNotPointerSized`]: `T` is not the size of
    ///   a pointer.
    /// - [`CSharpError::InteriorNul`]: the assembly path, the type name or the
    ///   method name contains a NUL byte inside it.
    /// - [`CSharpError::ManagedEntryPointFailed`]: `hostfxr` reports failure, or
    ///   returns no function pointer.
    pub fn resolve_unmanaged_function_pointer<T: Copy>(
        &self,
        assembly: &Path,
        type_name: &str,
        method_name: &str,
    ) -> Result<T, CSharpError> {
        // Step 1: Reject any `T` that is not pointer-sized. The pointer is
        // copied into a `T` by value below, so a struct, a reference or another
        // accidental choice has to fail here rather than after the copy.
        if std::mem::size_of::<T>() != std::mem::size_of::<*const std::ffi::c_void>() {
            return Err(CSharpError::ManagedExportNotPointerSized);
        }

        // Step 2: Encode the assembly path, the type name and the method name
        // for `hostfxr`. Each buffer stays in a local variable, so its pointer
        // stays valid for the whole call.
        let encoded_assembly = host_string(assembly.as_os_str())?;
        let encoded_type_name = host_string(std::ffi::OsStr::new(type_name))?;
        let encoded_method_name = host_string(std::ffi::OsStr::new(method_name))?;
        let mut function_pointer = std::ptr::null();

        // Step 3: Ask `hostfxr` for the method's direct unmanaged entry point.
        // `usize::MAX` as the delegate-type argument is the marker `hostfxr`
        // defines for this: no delegate thunk is created, and the pointer
        // returned is the one generated for the `UnmanagedCallersOnly` method.
        let unmanaged_callers_only_marker = usize::MAX as *const HostChar;

        // The reserved-state argument stays null because the direct entry point
        // needs no extra state. The last argument is an output slot that
        // `hostfxr` fills with the function pointer.
        // SAFETY: all three strings end with a nul, and `function_pointer` is a
        // writable slot that stays valid for the whole call.
        let status = unsafe {
            (self.load_assembly)(
                encoded_assembly.as_ptr(),
                encoded_type_name.as_ptr(),
                encoded_method_name.as_ptr(),
                unmanaged_callers_only_marker,
                std::ptr::null(),
                &mut function_pointer,
            )
        };

        // A missing output pointer is an error even when `hostfxr` returned a
        // nonnegative informational code: there is no function to call.
        if status < 0 || function_pointer.is_null() {
            return Err(CSharpError::ManagedEntryPointFailed {
                method: method_name.to_string(),
                status: format!("0x{status:08X}"),
            });
        }

        // Step 4: Copy the pointer bits into the caller's `T`. Whether `T`'s
        // signature really matches the managed method is the caller's ABI
        // responsibility.
        // SAFETY: `T` is pointer-sized (checked in Step 1), so copying a
        // `*const c_void` into a `T` moves exactly one pointer's worth of bits.
        Ok(unsafe { std::mem::transmute_copy::<*const std::ffi::c_void, T>(&function_pointer) })
    }
}

impl Drop for DotnetRuntimeContext {
    /// Releases the owned `hostfxr` context before its native library is
    /// unloaded.
    fn drop(&mut self) {
        // The null check is defensive: `new` never returns a context with a
        // null handle. For a live handle, close it before `_library` is
        // dropped, so the `close` function pointer still points at loaded code.
        if !self.runtime_handle.is_null() {
            // SAFETY: this object is the only owner of the live `hostfxr`
            // context.
            unsafe { (self.close)(self.runtime_handle) };
        }
    }
}

// =============================================================================
// HostfxrVersion
// =============================================================================

/// One installed `hostfxr` candidate with its parsed version key.
struct HostfxrVersion {
    /// The numeric version parts, which are compared first when versions are
    /// ordered.
    numeric_segments: Vec<u32>,
    /// Whether the version carries no prerelease suffix.
    stable: bool,
    /// Path to this version's directory under `host/fxr`.
    version_directory_path: PathBuf,
}

// =============================================================================
// Runtime Discovery and String Conversion
// =============================================================================

/// Finds the newest installed `hostfxr` library.
///
/// If `ECS_DOTNET_HOSTFXR` is set, it wins, and its path must be a file.
/// Otherwise every configured .NET root is searched and the newest `hostfxr`
/// version is picked. A prerelease only wins when no stable release of the same
/// base version exists.
///
/// An override is checked to be a file. A path found by the search is not
/// checked, so a .NET installation without the library file only fails when the
/// caller tries to load it.
///
/// # Errors
///
/// - [`CSharpError::HostfxrOverrideNotAFile`]: `ECS_DOTNET_HOSTFXR` is set but
///   does not name a regular file.
/// - [`CSharpError::HostfxrNotFound`]: no `host/fxr` version directory exists
///   under any configured .NET root.
fn find_hostfxr_library_path() -> Result<PathBuf, CSharpError> {
    // Step 1: An explicit override pins one `hostfxr` no matter which other
    // SDKs are installed.
    if let Some(override_path) = std::env::var_os("ECS_DOTNET_HOSTFXR") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(CSharpError::HostfxrOverrideNotAFile {
            path: path.display().to_string(),
        });
    }

    // Step 2: Collect the versions found under every configured .NET root.
    let mut versions: Vec<HostfxrVersion> = Vec::new();
    for dotnet_root in dotnet_roots() {
        collect_hostfxr_versions(&dotnet_root, &mut versions);
    }

    // The newest version wins. A stable release beats a prerelease of the same
    // base version because `true` sorts after `false`.
    versions.sort_by(|left, right| {
        left.numeric_segments
            .cmp(&right.numeric_segments)
            .then(left.stable.cmp(&right.stable))
    });
    let version_directory = versions
        .last()
        .map(|version| version.version_directory_path.clone())
        .ok_or(CSharpError::HostfxrNotFound)?;

    #[cfg(windows)]
    let library_filename = "hostfxr.dll";
    #[cfg(target_os = "linux")]
    let library_filename = "libhostfxr.so";
    #[cfg(target_os = "macos")]
    let library_filename = "libhostfxr.dylib";

    // Step 3: Append this platform's shared-library file name. The version
    // directory layout is the same everywhere; only the file name differs.
    Ok(version_directory.join(library_filename))
}

/// The .NET installation roots to search, in the order they are read.
fn dotnet_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("DOTNET_ROOT").map(PathBuf::from) {
        roots.push(root);
    }
    #[cfg(windows)]
    {
        if let Some(program_files) = std::env::var_os("ProgramFiles") {
            roots.push(PathBuf::from(program_files).join("dotnet"));
        }
        if let Some(program_files_x86) = std::env::var_os("ProgramFiles(x86)") {
            roots.push(PathBuf::from(program_files_x86).join("dotnet"));
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            roots.push(
                PathBuf::from(local_app_data)
                    .join("Microsoft")
                    .join("dotnet"),
            );
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join(".dotnet"));
        }
        roots.push(PathBuf::from("/usr/share/dotnet"));
        roots.push(PathBuf::from("/usr/lib/dotnet"));
    }
    roots
}

/// Adds every version directory under one .NET root's `host/fxr` to
/// `versions`.
fn collect_hostfxr_versions(dotnet_root: &Path, versions: &mut Vec<HostfxrVersion>) {
    // A missing root is normal: not every candidate location exists on every
    // machine. An unreadable directory is skipped instead of failing the
    // search.
    let hostfxr_root_directory = dotnet_root.join("host").join("fxr");
    let Ok(entries) = std::fs::read_dir(&hostfxr_root_directory) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let (numeric_segments, stable) =
            parse_hostfxr_version(&entry.file_name().to_string_lossy());
        versions.push(HostfxrVersion {
            numeric_segments,
            stable,
            version_directory_path: entry.path(),
        });
    }
}

/// Splits a `hostfxr` directory name into a version key that can be compared.
///
/// The key holds the numeric parts that come before the first non-numeric
/// part. A name with any non-numeric part (for example `8.0.0-preview.7`) is
/// marked non-stable, so a stable release of the same base version wins.
fn parse_hostfxr_version(directory_name: &str) -> (Vec<u32>, bool) {
    let mut numeric_segments = Vec::new();
    for part in directory_name.split('.') {
        match part.find(|character: char| !character.is_ascii_digit()) {
            Some(_) => return (numeric_segments, false),
            None => numeric_segments.push(part.parse::<u32>().unwrap_or(0)),
        }
    }
    (numeric_segments, true)
}

/// Encodes an OS string as nul-terminated UTF-16 for Windows `hostfxr`.
///
/// # Errors
///
/// Returns [`CSharpError::InteriorNul`] when the string contains a NUL byte
/// inside it, because the ABI call would then stop at that byte.
#[cfg(windows)]
fn host_string(os_string: &std::ffi::OsStr) -> Result<Vec<HostChar>, CSharpError> {
    use std::os::windows::ffi::OsStrExt;

    // Windows `hostfxr` takes UTF-16 strings. The terminator is appended
    // explicitly because `OsStrExt` returns only the encoded contents.
    let mut encoded: Vec<HostChar> = os_string.encode_wide().collect();
    // An interior terminator would truncate the string at the ABI boundary.
    if encoded.contains(&0) {
        return Err(CSharpError::InteriorNul);
    }
    encoded.push(0);
    Ok(encoded)
}

/// Encodes an OS string as nul-terminated bytes for Unix `hostfxr`.
///
/// # Errors
///
/// Returns [`CSharpError::InteriorNul`] when the string contains a NUL byte
/// inside it, because the ABI call would then stop at that byte.
#[cfg(not(windows))]
fn host_string(os_string: &std::ffi::OsStr) -> Result<Vec<HostChar>, CSharpError> {
    use std::os::unix::ffi::OsStrExt;

    // Unix `hostfxr` takes the original bytes of the OS string. Those bytes are
    // not necessarily UTF-8, so the string is not converted to text first.
    let mut bytes = os_string.as_bytes().to_vec();

    // A NUL inside the string would cut it short at the ABI boundary, so the
    // call could load a different assembly or method than the one asked for.
    if bytes.contains(&0) {
        return Err(CSharpError::InteriorNul);
    }

    bytes.push(0);
    Ok(bytes)
}
