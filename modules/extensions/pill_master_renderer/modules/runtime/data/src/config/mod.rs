//! The layout facts every renderer pipeline shares, and the two frames the
//! renderer ships with.
//!
//! # Responsibilities
//!
//! - Name the bind group each kind of shader data goes in. That is the
//!   convention the engine's shaders declare and the drawers bind against.
//! - Set how many instances the mesh drawer's instance buffer starts with and
//!   how many it adds each time it grows.
//! - Hold the pipelines the renderer defines itself - [`simple_pipeline`], and
//!   [`pbr_pipeline`] with its [`post_processing`] and [`skybox`] parts - so a
//!   project can run a frame without declaring one. Each pipeline keeps its
//!   stages under a `shaders/` directory in the crate's `shader_sources/` tree.
//! - Import every one of their shaders from its `.hlsl` source and `.hlsl.meta`
//!   ([`import_shader`]), the same way a project's own shaders are imported.
//! - Name the cooked files each shader was built from ([`all_shader_sources`]),
//!   so a runtime reload of a source reaches the assets built from it.
//!
//! # Design
//!
//! Every pipeline here is an ordinary `RenderingPipeline` asset. The renderer
//! treats these like any other pipeline; the only difference is that it
//! installs one itself.
//!
//! Each `install` is idempotent. It returns what the store already holds
//! instead of adding a second copy, which is what lets the renderer's
//! registration call it again on every generation.
//!
//! A built-in shader is built the same way as a project's shader. Its slots,
//! its bindings and the guid that material files name live beside its source in
//! a `.hlsl.meta`, and the store lists it as the file it is. The GPU module
//! builds its stand-in shader from [`DEFAULT_VERTEX`] and
//! [`simple_pipeline::DEFAULT_LIT_FRAGMENT`]. That pair is the only stage text
//! anything reads out of the crate itself, because the stand-in is built before
//! a store exists.
//!
//! [`pbr_pipeline`] is the default: [`register`](crate::register) installs it
//! and points [`RenderingManager`](crate::RenderingManager) at it. A project
//! that wants a different frame calls `set_pipeline` after registering, and one
//! that wants the renderer's own fallback chain clears the manager instead.

// External crates
use pill_engine::{AssetGuid, AssetImport, AssetManager, Handle, MetadataPolicy};

// Current crate
use crate::assets::Shader;

// =============================================================================
// Constants
// =============================================================================

/// Instances the mesh drawer's instance buffer starts with, and the step it
/// grows by.
///
/// The drawer allocates this many up front. When a frame holds more instances
/// than that, it reallocates the buffer in whole steps of this size, so one
/// growth covers a while. The value sets the buffer size, not a limit on how
/// many instances a frame may draw.
pub const INSTANCE_BATCH_SIZE: usize = 10000;

/// Initial capacity for a staging vector of instances, sized like
/// [`INSTANCE_BATCH_SIZE`].
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 10000;

/// Bind group the engine's per-frame parameters go in, in a shader that reads
/// them.
pub const ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX: u32 = 0;
/// Bind group the active camera's parameters go in, in a shader that reads
/// them.
pub const CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX: u32 = 1;
/// Bind group a material's, or a pass's, uniform parameters go in, when it has
/// any.
pub const MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX: u32 = 2;
/// Bind group a material's, or a pass's, textures go in, when it has any.
pub const MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX: u32 = 3;

// =============================================================================
// ConfigShaderFile
// =============================================================================

/// Declares a [`ConfigShaderFile`] constant for a cooked file under `shader_sources/`.
///
/// One literal feeds both halves. `include_str!` embeds the file at build time,
/// and the same path lets a runtime reload find the file again. It has to be a
/// macro because `include_str!` accepts only a literal, never a `const`.
macro_rules! config_shader_file {
    ($(#[$attribute:meta])* $visibility:vis $name:ident, $path:literal) => {
        $(#[$attribute])*
        $visibility const $name: $crate::config::ConfigShaderFile = $crate::config::ConfigShaderFile {
            relative_path: $path,
            embedded_source: include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/shader_sources/",
                $path
            )),
        };
    };
}

/// A cooked WGSL stage the renderer embeds at build time.
///
/// Each pipeline module declares one constant per stage it draws through. The
/// constant carries the stage text into the crate and remembers its path, so a
/// reload can find the file again.
///
/// The shaders themselves still live in the store. Every built-in shader is
/// imported from its `.hlsl` source like a project's, and only the pair the GPU
/// module builds its stand-in shader from is read back out of a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigShaderFile {
    /// Path relative to the crate's `shader_sources/`, e.g.
    /// `pbr_pipeline/shaders/pbr_fragment.wgsl`.
    pub relative_path: &'static str,
    /// The file's text as `include_str!` embedded it when the crate was built.
    pub embedded_source: &'static str,
}

config_shader_file!(
    /// The vertex stage every lit pass in the crate starts from.
    pub DEFAULT_VERTEX, "common_shaders/default_vertex.wgsl"
);

// =============================================================================
// ShaderSourceRecord
// =============================================================================

/// The cooked files a built-in shader asset was built from.
///
/// Each pipeline's `SHADER_SOURCES` table holds one entry per shader it
/// installs. The development shader reload uses the table to map an edited
/// cooked file back to the assets built from it.
///
/// `vertex_path` and `fragment_path` name cooked `.wgsl` files, which is what
/// the host hands over. The asset itself is named after the `.hlsl` source it
/// was imported from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShaderSourceRecord {
    /// The name the shader asset is stored under. That is the path of the
    /// `.hlsl` source it was imported from, because an imported asset is named
    /// after the path it came from.
    pub shader_asset_name: &'static str,
    /// Its vertex stage.
    pub vertex_path: &'static str,
    /// Its fragment stage.
    pub fragment_path: &'static str,
}

// =============================================================================
// Shader Sources
// =============================================================================

/// The PBR frame: the lit geometry pass and the post-processing half it
/// composes.
pub mod pbr_pipeline;
/// The post-processing passes the PBR frame composes.
pub mod post_processing;
/// The built-in lit pipeline, the fallback a project can opt into.
pub mod simple_pipeline;
/// The sky pass drawn where the lit pass wrote nothing.
pub mod skybox;

/// Imports one of the renderer's own shaders, or hands back the one the store
/// already holds.
///
/// `relative_path` is the `.hlsl` source, relative to this crate's
/// `shader_sources/`. The settings come from the `.hlsl.meta` beside it, so the
/// slots, the bindings and the guid are declared once, next to the source,
/// exactly as a project's shader declares them.
///
/// # Errors
///
/// Returns the store's import error when the source or its meta will not load.
///
/// # Panics
///
/// Panics when the shader the store yields does not carry `asset_guid`, which
/// happens when the source's `.hlsl.meta` is missing or names another guid. A
/// shader like that would be one no material file could name.
pub(crate) fn import_shader(
    assets: &mut AssetManager,
    relative_path: &str,
    asset_guid: AssetGuid,
) -> Result<Handle<Shader>, Box<dyn std::error::Error>> {
    // Step 1: Hand back the shader when the store already holds it, so calling
    // this again adds no second copy.
    if let Some(handle) = assets.handle_by_name::<Shader>(relative_path) {
        return Ok(handle);
    }
    // Step 2: Import the source and check the guid its meta declares.
    let imported_shader = assets.import(AssetImport::<Shader>::new(
        relative_path,
        MetadataPolicy::ReadIfPresent,
    ))?;
    assert_eq!(
        imported_shader.guid,
        Some(asset_guid),
        "`{relative_path}` must carry the guid its module declares"
    );
    Ok(imported_shader.handle)
}

/// Every shader asset the renderer's own pipelines install, with its files.
///
/// The development shader reload reads this table to map an edited file back
/// to the assets built from it. The pipelines' `install` functions import from
/// their own paths, so the table could fall behind them. The test at the bottom
/// of this file checks every record against the shader really installed.
pub fn all_shader_sources() -> impl Iterator<Item = &'static ShaderSourceRecord> {
    pbr_pipeline::SHADER_SOURCES
        .iter()
        .chain(post_processing::SHADER_SOURCES)
        .chain(simple_pipeline::SHADER_SOURCES)
        .chain(skybox::SHADER_SOURCES)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Shader;
    use pill_engine::{AssetManager, Engine};
    use std::path::PathBuf;

    /// The crate's own `shader_sources/` directory.
    fn sources_directory() -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("shader_sources")
    }

    /// Every record names a shader the pipelines really install, and the two
    /// cooked files it points at are the ones its stages came from.
    ///
    /// This keeps the table and the installs in step. A record whose paths
    /// moved would send an edited file nowhere, and a meta naming a different
    /// vertex stage would show up here, not as a shader that draws wrong.
    #[test]
    fn every_shader_source_record_matches_an_installed_shader() {
        let mut engine = Engine::new();
        crate::register(&mut engine);
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .expect("the engine inserts the store");
        // `register` installs the PBR chain. The simple one is opt-in.
        simple_pipeline::install(assets).expect("a free name");

        let shader_sources_directory = sources_directory();
        for record in all_shader_sources() {
            let shader = assets
                .get_by_name::<Shader>(record.shader_asset_name)
                .unwrap_or_else(|| panic!("{} is not installed", record.shader_asset_name));
            for path in [record.vertex_path, record.fragment_path] {
                let file_text = std::fs::read_to_string(shader_sources_directory.join(path))
                    .unwrap_or_else(|error| panic!("{path}: {error}"));
                let stage = if path == record.vertex_path {
                    &shader.vertex_wgsl
                } else {
                    &shader.fragment_wgsl
                };
                assert_eq!(stage, &file_text, "{}", record.shader_asset_name);
            }
        }
    }

    /// The two embedded stages are the files they name, so a path typo cannot
    /// hide behind an `include_str!` of some other file.
    #[test]
    fn every_embedded_stage_is_the_file_it_names() {
        let shader_sources_directory = sources_directory();
        for stage_file in [DEFAULT_VERTEX, simple_pipeline::DEFAULT_LIT_FRAGMENT] {
            let file_text =
                std::fs::read_to_string(shader_sources_directory.join(stage_file.relative_path))
                    .unwrap_or_else(|error| panic!("{}: {error}", stage_file.relative_path));
            assert_eq!(
                file_text, stage_file.embedded_source,
                "{}",
                stage_file.relative_path
            );
        }
    }
}
