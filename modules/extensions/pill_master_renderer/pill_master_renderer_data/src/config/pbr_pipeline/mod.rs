//! The PBR frame: a lit geometry pass into a half-float target, and the
//! post-processing chain that brings it to the surface.
//!
//! # Responsibilities
//!
//! - Install the half of the frame that draws geometry ([`install`]): the lit
//!   pass, the shader it draws through, the offscreen target it writes, and the
//!   neutral material the chain ships as a file ([`DEFAULT_MATERIAL_PATH`]).
//! - Compose that pass with the post-processing half
//!   ([`super::post_processing`]) into one pipeline asset, with a skybox pass
//!   between them that a project turns on by giving it a sky material
//!   ([`set_skybox`]).
//!
//! # Design
//!
//! A frame written as data, exactly as a project would write it: the lit pass is
//! a `RenderPass` asset and the chain is a `RenderingPipeline` asset, so nothing
//! here needs an API a project cannot reach. The crate adds only this: the
//! assets exist without anyone declaring them, and the renderer's manager is
//! pointed at the result.
//!
//! ```text
//! pill.pbr.pass.opaque   geometry → hdr    the lit surface, in the range lighting produces
//! pill.pbr.pass.skybox   skybox   → hdr    the sky, where the lit pass drew nothing (off until set)
//! ─── the four post-processing passes, declared in `super::post_processing` ───
//! ```
//!
//! The shaders are cooked from HLSL to WGSL at build time and embedded, and
//! installing the chain still imports each one from its `.hlsl` source through
//! the store, the way a project's shader is imported. The geometry fragment
//! stage is `pbr_fragment.hlsl`, under `shader_sources/pbr_pipeline/shaders/`,
//! and includes the crate's shared header. The vertex stage is the shared
//! `common_shaders/default_vertex.wgsl`, named from the `shader_sources` root by
//! each shader's `.hlsl.meta`, because every lit pass in the crate starts from
//! the same instance layout. The four post-processing stages are under
//! `shader_sources/post_processing/shaders/`.

// Standard library
use std::collections::BTreeMap;
use std::path::Path;

// External crates
use pill_engine::{AssetGuid, AssetManager, Handle};
use pill_renderer_api::frame::MaterialParameter;

// Current crate
use crate::{
    assets::{Material, PassKind, PassTarget, RenderPass, RenderingPipeline, Shader},
    config::post_processing::{self, HDR_TARGET},
    config::skybox,
    config::{ShaderSourceRecord, DEFAULT_VERTEX},
};

// =============================================================================
// Constants
// =============================================================================

/// The shader assets [`install`] creates, and the files each is built from.
pub const SHADER_SOURCES: &[ShaderSourceRecord] = &[ShaderSourceRecord {
    shader_asset_name: SHADER_PATH,
    vertex_path: DEFAULT_VERTEX.relative_path,
    fragment_path: PBR_FRAGMENT.relative_path,
}];

/// Asset name the installed chain is stored under.
pub const PIPELINE_NAME: &str = "pill.pbr.pipeline";

/// The shader the geometry pass draws through: the path its `.hlsl` source is
/// imported under. An imported asset is named by the path it came from.
pub const SHADER_PATH: &str = "pbr_pipeline/shaders/pbr_fragment.hlsl";

/// The guid the PBR shader is stored under, so a material file can name it.
pub const SHADER_GUID: AssetGuid = AssetGuid::from_name(SHADER_GUID_NAME);

/// The material file the chain's default material is imported from, relative
/// to this module's `res`.
///
/// The chain's default material is content, not code: this file is its one
/// definition. The editor lists it read-only under the `extensions` root, and it
/// is imported like any other standalone `.material` - named by this path and
/// keyed by the guid in its own header, so a reference to it survives the file
/// being renamed.
pub const DEFAULT_MATERIAL_PATH: &str = "defaults/pill.pbr.material";

/// Asset name of the skybox pass, which [`set_skybox`] gives a material.
pub const SKYBOX_PASS: &str = "pill.pbr.pass.skybox";

config_shader_file!(
    /// The geometry pass's fragment stage, cooked from `pbr_fragment.hlsl` and
    /// embedded with its path, so the shader reload can find the file again.
    PBR_FRAGMENT, "pbr_pipeline/shaders/pbr_fragment.wgsl"
);

/// The name the PBR shader's guid is derived from.
///
/// The guid comes from this string instead of being drawn at random, so every
/// checkout and every run gives the shader the same one. The shader's
/// `.hlsl.meta` carries that guid, and the shipped default material and the
/// examples' materials name it.
const SHADER_GUID_NAME: &str = "pill.pbr.shader";

/// Asset name of the lit pass.
const OPAQUE_PASS: &str = "pill.pbr.pass.opaque";

// =============================================================================
// Free Functions
// =============================================================================

/// The PBR shader's parameters at their neutral values, by slot name.
///
/// Each factor multiplies the map bound to its slot, so these values leave a
/// bound map unchanged. A slot with no map bound falls back to the renderer's
/// default texture for the slot's type. The chain's default material and every
/// newly created `.material` start from this set, so the two start out the same.
pub fn neutral_parameters() -> BTreeMap<String, MaterialParameter> {
    BTreeMap::from([
        (
            "pbr_base".to_owned(),
            MaterialParameter::Color([1.0, 1.0, 1.0]),
        ),
        ("pbr_roughness".to_owned(), MaterialParameter::Scalar(1.0)),
        ("pbr_metallic".to_owned(), MaterialParameter::Scalar(1.0)),
        (
            "pbr_emissive".to_owned(),
            MaterialParameter::Color([1.0, 1.0, 1.0]),
        ),
    ])
}

/// Builds the sky pass: a fullscreen triangle at the far plane, into the lit
/// target, with no material.
///
/// It draws into the same target as the lit pass so it can read the depth that
/// pass left behind. A pass with no material cannot be drawn, so this one starts
/// disabled until [`set_skybox`] gives it a material.
fn default_sky_pass() -> RenderPass {
    skybox::skybox_pass(
        SKYBOX_PASS,
        Handle::INVALID,
        PassTarget::Offscreen(HDR_TARGET.to_owned()),
        0,
    )
}

/// Imports the chain's shader from its source, or returns the one the store
/// already holds.
///
/// The source lives under this module's `shader_sources` root, which
/// [`register`](crate::register) declares. The `.hlsl.meta` beside it carries
/// the guid every material file names and the slots the pass reads.
///
/// # Errors
///
/// Returns the store's import error when the source or its `.hlsl.meta` will not
/// load.
///
/// # Panics
///
/// Panics when the imported shader does not carry [`SHADER_GUID`], which happens
/// when the meta is missing or names another guid.
fn install_pbr_shader(
    assets: &mut AssetManager,
) -> Result<Handle<Shader>, Box<dyn std::error::Error>> {
    crate::config::import_shader(assets, SHADER_PATH, SHADER_GUID)
}

/// Installs the PBR chain, and returns the pipeline asset.
///
/// The shader the geometry pass draws through is stored under [`SHADER_PATH`],
/// which is how a caller that wants it finds it: the pass carries the same
/// handle, and the store is the index for the name.
///
/// Idempotent: a store that already holds the chain gets the same handle back
/// rather than a second copy of it, so this is safe to call once per generation,
/// which is what [`register`](crate::register) does.
///
/// # Errors
///
/// Returns an error when a name this chain owns is already taken by an asset of
/// another type, when a shader's source or `.hlsl.meta` will not load, or when
/// the default material's file will not parse.
///
/// # Panics
///
/// Panics when one of the chain's shaders does not carry the guid its module
/// declares; see `install_pbr_shader`.
pub fn install(
    assets: &mut AssetManager,
) -> Result<Handle<RenderingPipeline>, Box<dyn std::error::Error>> {
    // Step 1: Install the sky shaders, then hand back the chain when the store
    // already holds it. The shaders come first because a sky material file names
    // them by guid and can load before this call.
    skybox::install_shaders(assets)?;
    if let Some(pipeline) = assets.handle_by_name::<RenderingPipeline>(PIPELINE_NAME) {
        return Ok(pipeline);
    }

    // Step 2: Import the pass's shader. It is the `.hlsl` this crate cooks,
    // imported with the `.hlsl.meta` beside it; the meta declares the four maps
    // the shader reads, the engine and camera groups it binds, and the guid every
    // material file names.
    let pbr_shader = install_pbr_shader(assets)?;

    // Step 3: Import the default material, the way every other `.material` is
    // imported: named by its path and keyed by the guid in its header. Each
    // neutral factor leaves a bound map unchanged, and a slot left unbound falls
    // back to the renderer's default texture, so the material works before anyone
    // has supplied a map. A project that wants it asks the store for that path.
    assets
        .import_standalone::<Material>(Path::new(DEFAULT_MATERIAL_PATH))
        .map_err(|error| {
            format!(
                "module '{}' requires its resource '{DEFAULT_MATERIAL_PATH}', which did not import: {error}",
                crate::MODULE_NAME
            )
        })?;

    // Step 4: Build the passes and store the chain. The lit pass writes a target
    // that can hold the range the lighting produces.
    let opaque_pass = RenderPass::new(OPAQUE_PASS)
        .with_shader(pbr_shader)
        .with_kind(PassKind::Geometry)
        .with_target(PassTarget::Offscreen(HDR_TARGET.to_owned()))
        .with_order(0);
    let opaque_pass = assets.add_named_with_guid(
        OPAQUE_PASS,
        pill_engine::AssetGuid::from_name(OPAQUE_PASS),
        opaque_pass,
    )?;

    // The sky draws behind the lit surface and into the same target, so it reads
    // the depth the lit pass left. It stays off until a project gives it a
    // material.
    let sky_pass = assets.add_named(SKYBOX_PASS, default_sky_pass())?;

    // The other half of the frame reads the target this one just wrote.
    let post_processing_passes = post_processing::install(assets)?;

    let mut pipeline = RenderingPipeline::new()
        .with_pass(opaque_pass)
        .with_pass(sky_pass);
    for pass in post_processing_passes {
        pipeline.add(pass);
    }
    let pipeline = assets.add_named(PIPELINE_NAME, pipeline)?;

    Ok(pipeline)
}

/// Sets `material` as the sky drawn behind the PBR chain's lit surface, or turns
/// the sky off with `None`, and returns the skybox pass.
///
/// The material is a sky material: one of the [`skybox`] shaders with its `sky`
/// slot bound. This installs the chain when the store does not hold it, and adds
/// the skybox pass right after the lit pass when the chain does not hold it.
/// Only what differs is written, so calling this on every reload does not make
/// the renderer rebuild an unchanged chain.
///
/// # Errors
///
/// Returns an error when the chain does not install, or when the pass's name is
/// taken by an asset of another type.
///
/// # Panics
///
/// Panics when one of the chain's shaders does not carry the guid its module
/// declares; see [`install`].
#[pill_engine::pill_mirror_fn]
pub fn set_skybox(
    assets: &mut AssetManager,
    material: Option<Handle<Material>>,
) -> Result<Handle<RenderPass>, Box<dyn std::error::Error>> {
    let pipeline = install(assets)?;
    let material = material.unwrap_or(Handle::INVALID);
    let sky_pass = match assets.handle_by_name::<RenderPass>(SKYBOX_PASS) {
        Some(sky_pass) => sky_pass,
        None => assets.add_named(SKYBOX_PASS, default_sky_pass())?,
    };
    // A chain that does not hold the pass yet gets it right after the lit pass,
    // whose depth it tests against.
    let in_chain = assets
        .get(pipeline)
        .is_some_and(|chain| chain.passes.iter().any(|entry| entry.pass == sky_pass));
    if !in_chain {
        let opaque_pass_handle = assets.handle_by_name::<RenderPass>(OPAQUE_PASS);
        if let Some(chain) = assets.get_mut(pipeline) {
            let index = opaque_pass_handle
                .and_then(|opaque_pass| {
                    chain
                        .passes
                        .iter()
                        .position(|entry| entry.pass == opaque_pass)
                })
                .map_or(0, |position| position + 1);
            chain
                .passes
                .insert(index, crate::assets::PipelinePass::new(sky_pass));
        }
    }

    let enabled = material != Handle::INVALID;
    let unchanged = assets
        .get(sky_pass)
        .is_some_and(|sky| sky.material == material && sky.enabled == enabled);
    if !unchanged {
        if let Some(sky) = assets.get_mut(sky_pass) {
            sky.material = material;
            sky.enabled = enabled;
        }
    }
    Ok(sky_pass)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::MaterialDocument;
    use pill_engine::Engine;

    /// The file this crate ships is the neutral default material: its shader is
    /// the one `install` stores, its parameters are the neutral set, and nothing
    /// is bound - the same state a newly created `.material` starts from.
    #[test]
    fn the_shipped_file_is_the_neutral_default_material() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("res")
                .join(DEFAULT_MATERIAL_PATH),
        )
        .expect("the shipped file is in the crate);");
        let file_document =
            pill_engine::asset_document::parse(source.as_bytes()).expect("the shipped file parses");
        let file_document: MaterialDocument =
            serde_json::from_value(file_document["asset"].clone())
                .expect("the shipped file carries a document");
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();
        install(&mut assets).expect("a free name");

        let shader = assets
            .handle_by_name::<Shader>(SHADER_PATH)
            .expect("the chain's shader");
        let material = assets
            .get_by_name::<Material>(DEFAULT_MATERIAL_PATH)
            .expect("the chain's default material");

        assert_eq!(material.shader, shader);
        assert_eq!(material.parameters, neutral_parameters());
        assert!(material.textures.is_empty());
        assert_eq!(material.rendering_order, u8::MAX);
        assert_eq!(file_document.parameters, neutral_parameters());
        assert_eq!(
            file_document.shader,
            MaterialDocument::default().shader,
            "the file names the shader a new material names"
        );
    }

    #[test]
    fn the_installed_chain_is_the_lit_pass_the_sky_then_the_post_processing_four() {
        let mut engine = Engine::new();
        crate::declare_crate_roots();
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .expect("the engine inserts the store");

        let pipeline_handle = install(assets).expect("a free name");
        let opaque = assets
            .handle_by_name::<RenderPass>(OPAQUE_PASS)
            .expect("the lit pass");
        let sky = assets
            .handle_by_name::<RenderPass>(SKYBOX_PASS)
            .expect("the skybox pass");
        let pipeline = assets
            .get(pipeline_handle)
            .expect("the pipeline it just added");

        assert_eq!(
            pipeline.passes.len(),
            6,
            "one lit pass, the sky, and four post passes"
        );
        assert_eq!(pipeline.passes[0].pass, opaque, "the lit pass comes first");
        assert_eq!(pipeline.passes[1].pass, sky, "the sky draws behind it");
        assert!(
            !assets.get(sky).unwrap().enabled,
            "no sky until a project sets one"
        );
    }

    /// The meta beside the shader source carries the guid every material file
    /// names, so a material that names the shader resolves it.
    #[test]
    fn the_shader_file_carries_the_guid_every_material_names() {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("shader_sources")
                .join(format!("{SHADER_PATH}.meta")),
        )
        .expect("the shipped meta is in the crate");
        let file_json =
            pill_engine::asset_document::parse(source.as_bytes()).expect("the meta parses");
        assert_eq!(
            file_json["guid"].as_str(),
            Some(SHADER_GUID.to_string().as_str()),
            "`{SHADER_PATH}.meta` must carry the guid every material names"
        );
        assert_eq!(
            file_json["asset_type"].as_str(),
            Some(<Shader as pill_engine::ImportedAsset>::metadata_type_name()),
            "the meta must describe a shader"
        );
    }

    #[test]
    fn setting_a_skybox_enables_the_pass_and_clearing_it_disables_it() {
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();
        install(&mut assets).expect("a free name");
        let material = assets
            .add_named("sky", Material::builder("sky").build())
            .expect("a free name");

        let pass = set_skybox(&mut assets, Some(material)).expect("the chain is installed");
        let sky = assets.get(pass).unwrap();
        assert_eq!(sky.material, material);
        assert!(sky.enabled);

        set_skybox(&mut assets, None).expect("the chain is installed");
        assert!(!assets.get(pass).unwrap().enabled);
    }

    /// A chain that holds no skybox pass gains one right after the lit pass when
    /// a sky is set.
    #[test]
    fn setting_a_skybox_on_an_older_chain_inserts_the_pass() {
        let mut assets = AssetManager::new();
        crate::declare_crate_roots();
        let pipeline = install(&mut assets).expect("a free name");
        let sky = assets.handle_by_name::<RenderPass>(SKYBOX_PASS).unwrap();
        assets
            .get_mut(pipeline)
            .unwrap()
            .passes
            .retain(|entry| entry.pass != sky);
        let material = assets
            .add_named("sky", Material::builder("sky").build())
            .expect("a free name");

        let pass = set_skybox(&mut assets, Some(material)).expect("the chain is installed");

        let chain = &assets.get(pipeline).unwrap().passes;
        assert_eq!(chain.len(), 6);
        assert_eq!(chain[1].pass, pass);
    }
}
