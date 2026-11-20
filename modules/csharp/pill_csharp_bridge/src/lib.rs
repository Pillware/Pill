//! The Rust half of the C#/Rust ABI: runs a C# project inside the engine.
//!
//! # Responsibilities
//!
//! - Starts the .NET runtime used by managed gameplay assemblies (CoreCLR
//!   through hostfxr, or a NativeAOT library).
//! - Exposes native ECS component storage to managed query iterators.
//! - Converts reflected C# query access into Rust scheduler metadata.
//! - With `hot_reload`, swaps a rebuilt project assembly into the running
//!   engine.
//!
//! # Design
//!
//! `pill_csharp_runtime` is the managed half of this boundary, and the two
//! change together. [`INTEROP_CONTRACT_VERSION`] is checked from both sides
//! when the runtime starts. [`csharp_runtime`] owns the low-level .NET hosting
//! boundary. The remaining modules split ABI layout, registration, invocation
//! scope, queries, commands and backend lifecycle. [`CSharpRuntime`] is the
//! entry point.
//!
//! Mirror generation and the in-process Roslyn compiler live in
//! `pill_development_host`. This crate reads no project settings, builds
//! nothing and watches no files. A shipped C# project reaches `pill_player`
//! as a [`CSharpBackend`] implementing `pill_player::ProjectBackend`; the
//! bridge depends on `pill_player`, never the other way round.

/// C-compatible data structures and callback table shared with the managed runtime.
mod abi;
/// Loads the native library of a C# project published with NativeAOT.
mod aot_runtime;
/// High-level C# project startup, discovery, and scheduler registration.
mod backend;
/// Native callbacks that translate C# lifecycle requests into deferred ECS commands.
mod commands;
/// C# component identities, native bindings, and manifest registration.
mod components;
/// Output locations and assembly names of a managed project.
mod config;
/// Thread-local access scope installed around one scheduled C# system.
mod context;
/// Low-level .NET hosting bootstrap used by the C# project backend.
mod csharp_runtime;
/// Read-only frame state for managed code: input and time.
mod frame_state;
/// The two-call protocol that carries every managed payload across the boundary.
mod managed_buffer;
/// C# component manifest schema, field validation, and engine type mapping.
mod manifest;
/// The single pipeline that applies every kind of manifest.
mod manifest_apply;
/// The native calls the mirror ABI needs besides its trampolines: text
/// channels, Rust-owned resources, and the asset store.
mod mirror_calls;
/// The naming rules a generated C# mirror's fields follow.
mod mirror_naming;
/// Parallel dispatch of managed callbacks onto the shared Rayon pool.
mod parallel;
/// A shipped C# project as the runtime's external project backend.
mod project_backend;
/// Native callbacks used by C# query enumerators.
mod queries;
/// Managed resource registration and the callback that serves resource bytes.
mod resources;

// =============================================================================
// Types + Impls
// =============================================================================

/// Rebuilds the mirror-method table the managed runtime copies, after an
/// extension reload changes its trampoline addresses or method set.
pub use abi::publish_mirror_methods;
/// The full type documentation lives in the `backend` module.
pub use backend::{CSharpRuntime, INTEROP_CONTRACT_VERSION};
/// The reload poll's status codes.
#[cfg(feature = "hot_reload")]
pub use backend::{POLL_MANIFEST_PENDING, POLL_NO_CHANGE, POLL_REJECTED, POLL_RELOADED};
/// Assembly names and output directories of a managed project, as the
/// backend needs them to start it.
pub use config::CSharpModuleConfig;
/// The booted CoreCLR runtime, which development tooling (the in-process
/// compiler) loads its own assemblies into.
pub use csharp_runtime::DotnetRuntimeContext;
/// The two-call protocol for managed payloads, with its error type.
pub use managed_buffer::{fetch_managed_buffer, ManagedBufferError};
/// The naming rules a generated mirror follows. The generator in
/// `pill_development_host` and the binding check here both use them.
pub use mirror_naming::{
    is_heap_owned_tag, is_opaque_container_tag, snake_to_pascal, split_array_tag,
};
/// A shipped C# project, started through `pill_player`'s external backend.
pub use project_backend::CSharpBackend;

/// Resolves registered component names into the layouts managed code binds.
pub use components::exposed_components_from_names;
/// Aggregate of the native components exposed to managed code.
pub use components::ModuleExposedComponent;

/// One mirrored Rust method, resolved to a callable address, shared by the
/// host's module loader and the C# backend. It lives here rather than in the
/// host's `hot_reload`-gated module loader because the C# backend is
/// compiled in every configuration.
#[derive(Clone, Debug)]
pub struct ResolvedMirrorMethod {
    /// Fully-qualified Rust type name the method belongs to - or, for a free
    /// function (`is_free_function`), the path of the module declaring it.
    pub type_name: String,
    /// Rust method name, snake_case.
    pub method_name: String,
    /// Return type tag; empty for a `()` return.
    pub return_tag: String,
    /// Argument type tags, in declaration order.
    pub arg_tags: Vec<String>,
    /// Argument names from the Rust source, in declaration order (`alpha`,
    /// `beta`), so the generated C# mirror names its parameters identically.
    /// Parallel to `arg_tags`.
    pub arg_names: Vec<String>,
    /// Address of the C-ABI trampoline; zero for a type row with none.
    pub address: usize,
    /// Whether the declaration is a `#[pill_mirror_fn]` free function. The
    /// codegen then emits a static method on a static class named after the
    /// declaring module, instead of an instance method on a struct mirror.
    pub is_free_function: bool,
    /// The package that declared the method, from `env!("CARGO_PKG_NAME")` at
    /// the macro's expansion site. The host keeps only the entries whose
    /// `crate_name` matches the module being generated or published, so an
    /// artifact cannot claim declarations from a crate it only links.
    pub crate_name: String,
    /// How the method takes the value it is called on: empty for none, or
    /// `"ref"`, `"mut"` or `"value"`.
    pub receiver: String,
    /// What the owning type is, when the declaring attribute knew it: `"value"`
    /// for a `#[derive(PillMirror)]` value type or a component row, `"object"`,
    /// `"enum"`, `"resource"`, or `"module"` for a free function. Empty when
    /// the codegen has to resolve the type from the type rows.
    pub owner_kind: String,
}

/// One heap-field accessor resolved to callable addresses, shared by the
/// host's module loader, the C# mirror codegen, and the managed runtime's
/// method table.
#[derive(Clone, Debug)]
#[cfg(feature = "hot_reload")]
pub struct ResolvedFieldAccessor {
    /// Fully-qualified component type name the field belongs to.
    pub type_name: String,
    /// Rust field name, snake_case.
    pub field_name: String,
    /// Container kind: `"vec"`, `"dynbuf"`, `"string"`, or `"vecstring"`.
    pub kind: String,
    /// Element type tag of a `vec` field. It is `string` for a `vecstring`
    /// field, and empty for a `string` field.
    pub element_tag: String,
    /// Address of the view trampoline. `None` for a `vecstring` field, whose
    /// elements are allocated separately and cannot be viewed as one run.
    pub view_address: Option<usize>,
    /// Address of the resize trampoline for a `vec`, `dynbuf` or `vecstring`
    /// field.
    pub resize_address: Option<usize>,
    /// Address of the replace-in-place trampoline for a `string` field.
    pub set_address: Option<usize>,
    /// Address of the per-element view trampoline for a `vecstring` field.
    pub item_address: Option<usize>,
    /// Address of the per-element replace trampoline for a `vecstring` field.
    pub set_item_address: Option<usize>,
    /// Address of the append trampoline for a `vecstring` field.
    pub push_address: Option<usize>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the name a generated `MirrorMethods.Resolve` call and the host's
/// accessor rows both use for one operation on a heap field. Writing it in
/// one place keeps the codegen (which writes it into C#) and the row builder
/// (which turns it into a table row) from drifting apart. `operation` is
/// `view`, `resize`, `set`, `item`, `set_item`, or `push`.
#[cfg(feature = "hot_reload")]
pub fn accessor_operation_name(field_name: &str, operation: &str) -> String {
    format!("{field_name}_{operation}")
}

/// Converts heap-field accessors into mirror-method rows. The managed
/// runtime then resolves them through the same table as mirrored value-type
/// methods, and refreshes that table the same way on each reload.
#[cfg(feature = "hot_reload")]
pub fn accessor_rows(accessors: &[ResolvedFieldAccessor]) -> Vec<ResolvedMirrorMethod> {
    let mut rows = Vec::with_capacity(accessors.len() * 6);
    for accessor in accessors {
        let mut push = |operation: &str, address: Option<usize>| {
            if let Some(address) = address {
                rows.push(ResolvedMirrorMethod {
                    type_name: accessor.type_name.clone(),
                    method_name: accessor_operation_name(&accessor.field_name, operation),
                    return_tag: String::new(),
                    arg_tags: Vec::new(),
                    arg_names: Vec::new(),
                    address,
                    is_free_function: false,
                    crate_name: String::new(),
                    receiver: String::new(),
                    owner_kind: String::new(),
                });
            }
        };
        push("view", accessor.view_address);
        push("resize", accessor.resize_address);
        push("set", accessor.set_address);
        push("item", accessor.item_address);
        push("set_item", accessor.set_item_address);
        push("push", accessor.push_address);
    }
    rows
}

// =============================================================================
// Tests
// =============================================================================

/// Integration-style unit tests for the native/C# ECS boundary.
///
/// The fixtures are the shared-ABI components the managed side mirrors
/// (`TransformComponent`, `MeshRendererComponent`). They come from `pill_engine`
/// and the renderer data crate (a dev-dependency), so the suite builds in
/// every configuration, headless included.
#[cfg(test)]
mod tests;
