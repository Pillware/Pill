//! A C# project as the runtime's external project backend.
//!
//! # Responsibilities
//!
//! - Implement `pill_player::ProjectBackend` for a shipped C# project, in
//!   either posture: CoreCLR through hostfxr, or a NativeAOT library.
//!
//! # Design
//!
//! The runtime never names the bridge: a C# shipping bundle builds a
//! [`CSharpBackend`] and hands it over as
//! `StaticProjectBackend::External`, and the runtime keeps the returned
//! [`CSharpRuntime`] alive beside the engine. A build that runs no C# therefore
//! links none of this.

// Standard library
use std::any::Any;
use std::path::PathBuf;

// External crates
use pill_core::error::HostError;
use pill_engine::Engine;
use pill_player::ProjectBackend;

// Current crate
use crate::mirror_calls::statically_linked_mirror_methods;
use crate::{exposed_components_from_names, CSharpModuleConfig, CSharpRuntime};

// =============================================================================
// Types
// =============================================================================

/// How the managed project is hosted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Posture {
    /// CoreCLR started through hostfxr, which loads the runtime and project
    /// assemblies. The .NET runtime must be installed on the machine.
    CoreClr,
    /// A NativeAOT library with a trimmed runtime inside it. It needs no .NET
    /// install and runs no JIT.
    NativeAot,
}

/// A shipped C# project: where its assemblies are, and how to host them.
///
/// A .NET assembly has no static form that could be linked in; it is always
/// loaded at run time. So this backend only loads it. Nothing is compiled, no
/// C# mirror sources are generated, and the assembly is not watched for
/// replacement.
#[derive(Clone, Debug)]
pub struct CSharpBackend {
    /// CoreCLR or NativeAOT.
    posture: Posture,
    /// Assembly names and output directories, in the same form a reloading
    /// build gives them. A NativeAOT build uses only the project side, whose
    /// output directory is the `dotnet publish` output.
    module_config: CSharpModuleConfig,
    /// Directory the output subdirectories in `module_config` are resolved
    /// against.
    ///
    /// A shipped bundle copies its assemblies beside the executable, and the
    /// backend looks there first. This directory is the fallback, the
    /// workspace the binary was built in.
    workspace_root: PathBuf,
}

// =============================================================================
// Impls
// =============================================================================

impl CSharpBackend {
    /// A project hosted by CoreCLR through hostfxr.
    pub fn coreclr(module_config: CSharpModuleConfig, workspace_root: PathBuf) -> Self {
        Self {
            posture: Posture::CoreClr,
            module_config,
            workspace_root,
        }
    }

    /// A project published with NativeAOT and loaded as a native library.
    pub fn native_aot(module_config: CSharpModuleConfig, workspace_root: PathBuf) -> Self {
        Self {
            posture: Posture::NativeAot,
            module_config,
            workspace_root,
        }
    }
}

impl ProjectBackend for CSharpBackend {
    /// Starts .NET (or loads the AOT library) and registers the project.
    ///
    /// Managed code gets byte-level bindings for every component the
    /// extensions registered. The mirrored Rust functions come from this
    /// binary's own inventory: every extension is linked in, so the
    /// descriptors they submitted already carry their trampolines' addresses,
    /// and no module has to be loaded.
    ///
    /// # Errors
    ///
    /// Returns [`HostError::CSharp`] when the project cannot start: .NET or
    /// the AOT library could not be started, a managed export is missing, the
    /// component manifest could not be registered, a managed startup method
    /// failed, or a reflected system access names a component that is not
    /// registered.
    fn start(
        &self,
        engine: &mut Engine,
        exposed_component_names: &[String],
    ) -> Result<Box<dyn Any>, HostError> {
        // Step 1: Resolve the byte layouts of the components the extensions
        // registered, and this binary's mirror table of its own linked
        // extensions.
        let exposed_components =
            exposed_components_from_names(engine.world(), exposed_component_names);
        let mirror_methods = statically_linked_mirror_methods();
        // Step 2: Start the runtime the posture's way. Both postures then
        // register the project through the same code.
        let runtime = match self.posture {
            // A shipped build loads no extra assemblies into the runtime, so
            // the hook does nothing.
            Posture::CoreClr => CSharpRuntime::start(
                engine,
                &self.workspace_root,
                &self.module_config,
                &exposed_components,
                &mirror_methods,
                &mut |_| {},
            )?,
            Posture::NativeAot => CSharpRuntime::start_aot(
                engine,
                &self.workspace_root,
                &self.module_config,
                &exposed_components,
                &mirror_methods,
            )?,
        };
        Ok(Box::new(runtime))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Each constructor records its posture and keeps the configuration it was
    /// given.
    #[test]
    fn constructors_select_the_posture() {
        let module_config =
            CSharpModuleConfig::new("csharp_runtime", "runtime", "project_cs", "project");
        let coreclr = CSharpBackend::coreclr(module_config.clone(), PathBuf::from("."));
        let aot = CSharpBackend::native_aot(module_config, PathBuf::from("."));
        assert_eq!(coreclr.posture, Posture::CoreClr);
        assert_eq!(aot.posture, Posture::NativeAot);
        assert_eq!(aot.module_config.project_assembly_name, "project_cs");
    }

    /// A component name the world does not know resolves to no binding, never
    /// to a guessed one.
    #[test]
    fn unknown_exposed_component_names_are_skipped() {
        let engine = Engine::new();
        let bindings =
            exposed_components_from_names(engine.world(), &["pill_spline::Spline".to_string()]);
        assert!(bindings.is_empty());
    }
}
