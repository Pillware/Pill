//! The assembly names and output directories of a managed project.
//!
//! # Responsibilities
//!
//! - Names the runtime assembly and the project assembly, and gives the output
//!   directory of each. Together these four values are the whole configuration
//!   a managed project needs to start.
//!
//! # Design
//!
//! A development host derives these values from the project's `.csproj`. A
//! shipping bundle states them directly. Neither source is read here: this
//! crate never parses project settings.

// =============================================================================
// Types
// =============================================================================

/// Output locations and assembly names used by the managed project backend.
///
/// The runtime assembly hosts the collectible loader: the managed code that
/// loads the project assembly and can unload it again, so a reload can put a
/// new version in its place. The project assembly is loaded by the runtime, so
/// starting the managed module needs both assemblies and both output
/// directories.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct CSharpModuleConfig {
    /// Name of the runtime assembly that hosts the collectible loader.
    pub runtime_assembly_name: String,
    /// Output subdirectory for the runtime assembly, relative to the workspace root.
    pub runtime_output_subdirectory: String,
    /// Name of the project assembly loaded by the runtime.
    pub project_assembly_name: String,
    /// Output subdirectory for the project assembly, relative to the workspace root.
    pub project_output_subdirectory: String,
}

impl CSharpModuleConfig {
    /// Creates a managed project configuration from its assembly names and
    /// output directories.
    ///
    /// A development host gets these values from
    /// `pill_development_host`'s `ProjectModuleConfig::from_environment`, which
    /// reads the project's `.csproj`. A shipping build has no project path to
    /// read, so its generated bundle states the same four values directly. The
    /// constructor exists because the type is `#[non_exhaustive]`, so another
    /// crate cannot build it with a struct expression.
    ///
    /// The two subdirectories are relative to the workspace root the caller
    /// passes when it starts the project, not to any fixed location.
    pub fn new(
        runtime_assembly_name: impl Into<String>,
        runtime_output_subdirectory: impl Into<String>,
        project_assembly_name: impl Into<String>,
        project_output_subdirectory: impl Into<String>,
    ) -> Self {
        Self {
            runtime_assembly_name: runtime_assembly_name.into(),
            runtime_output_subdirectory: runtime_output_subdirectory.into(),
            project_assembly_name: project_assembly_name.into(),
            project_output_subdirectory: project_output_subdirectory.into(),
        }
    }
}
