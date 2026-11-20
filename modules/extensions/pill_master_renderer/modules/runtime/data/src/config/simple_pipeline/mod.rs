//! The simple frame: one lit geometry pass drawn onto the surface.
//!
//! # Responsibilities
//!
//! - Install the smallest chain that draws something: one geometry pass that
//!   draws through the renderer's own lit shader into the surface, and hand
//!   back the pipeline asset ([`install`]).
//!
//! # Design
//!
//! The pass names a shader, so it draws every instance whose material uses that
//! shader. It writes only the surface, so it also draws the instances no pass
//! in the chain claimed, each through its own material. The shader it installs
//! is the same lit one the renderer falls back to, with the same declared
//! slots, so a material written for the fallback works here without being
//! rebuilt.
//!
//! The renderer's built-in chain is a geometry pass that names no shader and
//! draws every instance through its own material. Install this pipeline when a
//! project wants that frame stored as an asset it can swap out.
//!
//! # Where the shaders are
//!
//! The lit fragment stage is in `shader_sources/simple_pipeline/shaders/`. It
//! is imported from its `.hlsl` source the way a project's own shader is, and
//! its slots and guid live in the `.hlsl.meta` beside that source.
//!
//! The vertex stage it pairs with lives in `shader_sources/common_shaders/`,
//! because it is shared: the renderer's fallback material, the PBR geometry
//! pass and this pipeline all build on it. Every lit pass in the crate starts
//! from the same instance layout, so the crate keeps one copy of the stage.
//!
//! The build script cooks the whole `shader_sources/` tree in one call. Every
//! directory that directly holds `.hlsl` files is a source directory, and every
//! `include/` directory in the tree is on every source's include path.
//!
//! Every stage declared with `config_shader_file!` embeds its file's text, but
//! only the fallback pair ([`DEFAULT_VERTEX`] and [`DEFAULT_LIT_FRAGMENT`]) is
//! read back out of its constant: the GPU module builds its stand-in shader
//! from it before any store exists, so the store cannot supply it.

// External crates
use pill_engine::{AssetManager, Handle};

// Current crate
use crate::{
    assets::{PassKind, PassTarget, RenderPass, RenderingPipeline},
    config::{ShaderSourceRecord, DEFAULT_VERTEX},
};

// =============================================================================
// Constants
// =============================================================================

config_shader_file!(
    /// The pass's fragment stage, cooked from `default_lit_fragment.hlsl`. It
    /// is also the fragment stage of the renderer's built-in fallback shader.
    pub DEFAULT_LIT_FRAGMENT, "simple_pipeline/shaders/default_lit_fragment.wgsl"
);

/// The shader assets [`install`] creates, and the cooked files each is built
/// from.
pub const SHADER_SOURCES: &[ShaderSourceRecord] = &[ShaderSourceRecord {
    shader_asset_name: SHADER_PATH,
    vertex_path: DEFAULT_VERTEX.relative_path,
    fragment_path: DEFAULT_LIT_FRAGMENT.relative_path,
}];

/// Asset name the installed pipeline is stored under.
pub const PIPELINE_NAME: &str = "pill.simple.pipeline";

/// The path the pass's shader source is imported under. An imported asset is
/// named by the path it came from, so this is also the shader's asset name.
pub const SHADER_PATH: &str = "simple_pipeline/shaders/default_lit_fragment.hlsl";

/// The guid the shader is stored under.
///
/// It is derived from a fixed name instead of drawn at random, so every
/// checkout and every run gives the same value. The `.hlsl.meta` beside the
/// source carries that value.
pub const SHADER_GUID: pill_engine::AssetGuid =
    pill_engine::AssetGuid::from_name("pill.simple.shader");

/// Asset name of the pass the pipeline holds.
pub const PASS_NAME: &str = "pill.simple.pass";

// =============================================================================
// Free Functions
// =============================================================================

/// Install the simple frame, and return the pipeline asset.
///
/// Idempotent: a store that already holds the pipeline gets that handle back
/// rather than a second copy, so calling it again on a later reload generation
/// is safe.
///
/// # Errors
///
/// Returns the store's import error when the shader's source, its `.hlsl.meta`
/// or the cooked `.wgsl` stages they name will not load. Returns
/// [`pill_engine::AssetBindingError::NameInUse`] when a name it stores under
/// already belongs to a live asset of that type.
///
/// # Panics
///
/// Panics when the shader's `.hlsl.meta` is missing or names a guid other than
/// [`SHADER_GUID`].
pub fn install(
    assets: &mut AssetManager,
) -> Result<Handle<RenderingPipeline>, Box<dyn std::error::Error>> {
    // Step 1: Hand back the pipeline when the store already holds it, so calling
    // this again adds no second copy.
    if let Some(pipeline) = assets.handle_by_name::<RenderingPipeline>(PIPELINE_NAME) {
        return Ok(pipeline);
    }

    // Step 2: Import the shader from its source and its meta. The import also
    // checks that the meta carries the guid this module declares.
    let shader = crate::config::import_shader(assets, SHADER_PATH, SHADER_GUID)?;

    // Step 3: Build the pass and the pipeline, and store both under their names.
    let pass = RenderPass::new(PASS_NAME)
        .with_shader(shader)
        .with_kind(PassKind::Geometry)
        .with_target(PassTarget::Surface);
    let pass = assets.add_named_with_guid(
        PASS_NAME,
        pill_engine::AssetGuid::from_name(PASS_NAME),
        pass,
    )?;

    let pipeline = assets.add_named(PIPELINE_NAME, RenderingPipeline::new().with_pass(pass))?;

    Ok(pipeline)
}
