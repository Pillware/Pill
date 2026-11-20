//! NativeAOT library loader for the managed backend.
//!
//! A C# project runs in one of two postures. The framework-dependent posture
//! starts CoreCLR through hostfxr and looks the `LoaderInterop` exports up with
//! the `load_assembly_and_get_function_pointer` delegate.
//!
//! The NativeAOT posture loads the native library produced by
//! `dotnet publish -p:PublishAot=true`, which has a trimmed runtime embedded in
//! it. This module opens that library with `libloading` and looks the same
//! exports up directly by their `pill_*` symbol names. This posture needs no
//! hostfxr, no installed .NET and no JIT.
//!
//! # Responsibilities
//!
//! - Load the AOT-published native library and resolve the `pill_*` exports.
//! - Keep the loader's call surface identical to the hostfxr path's.
//!
//! # Design
//!
//! Everything downstream (system discovery, scheduler registration, per-frame
//! dispatch) is shared with the hostfxr path. Only the way the function
//! pointers are obtained differs.

// Standard library
use std::ffi::CString;
use std::path::Path;

// External crates
use pill_core::error::CSharpError;

// =============================================================================
// Types
// =============================================================================

/// Owns a loaded NativeAOT library and resolves its unmanaged exports.
///
/// Only the shipping posture (a build without `hot_reload`) reaches this
/// type; a development build never calls it.
#[cfg_attr(feature = "hot_reload", allow(dead_code))]
pub(crate) struct AotRuntimeContext {
    /// Keeps the native library mapped for as long as the host runs, so every
    /// function pointer taken from it stays valid.
    _library: libloading::Library,
}

// =============================================================================
// Impls
// =============================================================================

impl AotRuntimeContext {
    /// Loads the AOT native library from disk.
    ///
    /// # Errors
    ///
    /// Returns [`CSharpError::AotLibraryNotFound`] when the file is absent,
    /// and [`CSharpError::AotLibraryLoadFailed`] when it cannot be mapped.
    pub(crate) fn new(library_path: &Path) -> Result<Self, CSharpError> {
        // Step 1: Refuse a path that does not name a file.
        if !library_path.is_file() {
            return Err(CSharpError::AotLibraryNotFound {
                path: library_path.display().to_string(),
            });
        }
        // Step 2: Load the library and keep it mapped.
        // SAFETY: the path names a real file, checked just above. It is either
        // the library a shipped bundle carries beside the executable or the
        // `dotnet publish` output that bundle describes. Either way the code
        // that runs on load was produced by this project's build.
        let library = unsafe {
            libloading::Library::new(library_path).map_err(|source| {
                CSharpError::AotLibraryLoadFailed {
                    path: library_path.display().to_string(),
                    source: Box::new(source),
                }
            })?
        };
        Ok(Self { _library: library })
    }

    /// Resolves one `pill_*` export into a typed function pointer.
    ///
    /// `T` must be the exact `extern "system"` signature the export declares.
    ///
    /// # Errors
    ///
    /// Returns [`CSharpError::AotExportMissing`] when the symbol is absent.
    ///
    /// # Panics
    ///
    /// Panics when `symbol_name` holds an interior NUL byte; every export name
    /// is a fixed `pill_*` string, so this never happens.
    pub(crate) fn resolve_unmanaged_function_pointer<T: Copy>(
        &self,
        symbol_name: &str,
    ) -> Result<T, CSharpError> {
        // Step 1: Encode the symbol name as a nul-terminated byte string, the
        // form `libloading` takes. Every export name is a fixed string written
        // here or in the managed forwarders, so none can hold an interior NUL
        // and the conversion never fails.
        let symbol_name_bytes = CString::new(symbol_name)
            .expect("export symbol names never contain interior NUL")
            .into_bytes_with_nul();
        // Step 2: Look the export up in the loaded library.
        // SAFETY: the caller gives `T` as the export's real signature, and the
        // doc comment above makes that the caller's contract. `_library` keeps
        // the library mapped, so the pointer stays valid while it is used.
        let loaded_symbol: libloading::Symbol<T> = unsafe {
            self._library.get(&symbol_name_bytes).map_err(|source| {
                CSharpError::AotExportMissing {
                    symbol: symbol_name.to_string(),
                    source: Box::new(source),
                }
            })?
        };
        Ok(*loaded_symbol)
    }
}
